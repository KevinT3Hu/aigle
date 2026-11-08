if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  set(QT_DEPLOY_USE_PATCHELF ON CACHE BOOL "Use patchelf for relocatable Qt plugins")
elseif(WIN32)
  # TODO(windows): native dependencies, bridge DLL deployment and installer layout.
endif()
find_package(Qt6 6.11 REQUIRED COMPONENTS Widgets Gui GuiPrivate ShaderTools Svg Test LinguistTools)
find_package(Vulkan REQUIRED)
qt_standard_project_setup()
include(GNUInstallDirs)

set(AIGLE_ROOT "${PROJECT_SOURCE_DIR}")
# Use the workspace pin as Corrosion's default toolchain; callers can override
# it with -DRust_TOOLCHAIN. Corrosion resolves matching Cargo and rustc binaries.
file(STRINGS "${AIGLE_ROOT}/rust-toolchain.toml" aigle_toolchain_channel
  REGEX "^channel[ \t]*=")
string(REGEX REPLACE "^[^=]*=[ \t]*\"([^\"]+)\".*$" "\\1"
  aigle_toolchain_channel "${aigle_toolchain_channel}")
set(Rust_TOOLCHAIN "${aigle_toolchain_channel}" CACHE STRING "Rust toolchain for the Qt engine bridge")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${AIGLE_ROOT}/rust-toolchain.toml")
set(AIGLE_RUST_PROFILE "dev" CACHE STRING "Cargo profile (dev or release)")

include(FetchContent)
# Corrosion v0.6.1, pinned to an immutable commit and verified archive.
FetchContent_Declare(Corrosion
  URL https://codeload.github.com/corrosion-rs/corrosion/tar.gz/1499b14e4906a2890f5cee1547c8848db261753d
  URL_HASH SHA256=68556a48c0b00264565e158935dc3f1e7656f06e9a2837fd49612f41dbac7ab8
  DOWNLOAD_EXTRACT_TIMESTAMP FALSE)
FetchContent_MakeAvailable(Corrosion)
corrosion_import_crate(
  MANIFEST_PATH "${AIGLE_ROOT}/crates/aigle/qt-bridge/Cargo.toml"
  CRATES aigle-qt-bridge CRATE_TYPES cdylib
  PROFILE "${AIGLE_RUST_PROFILE}" LOCKED NO_LINKER_OVERRIDE)
# RUSTC takes precedence over Cargo's build.rustc setting. Keep an inherited
# RUSTC from an editor/build environment from overriding Corrosion's selected toolchain.
corrosion_set_env_vars(aigle_qt_bridge "RUSTC=$<TARGET_FILE:Rust::Rustc>")
target_include_directories(aigle_qt_bridge INTERFACE
  "${AIGLE_ROOT}/crates/aigle/qt-bridge/include")

qt_add_executable(aigle-gui src/main.cpp src/engine.cpp src/engine.hpp
  src/window.cpp src/window_library.cpp src/window_preferences.cpp src/window_shortcuts.cpp src/window_exports.cpp src/comparison.cpp src/comparison.hpp src/window.hpp src/canvas.cpp src/canvas.hpp
  src/tool_panel.cpp src/tool_panel.hpp src/retouch_panel.cpp src/retouch_panel.hpp src/photo_delegate.hpp src/export_dialog.cpp src/export_dialog.hpp
  src/adjustments.cpp src/adjustments.hpp src/viewport.hpp src/value_editor.cpp src/value_editor.hpp
  src/curve.cpp src/curve.hpp src/agent_panel.cpp src/agent_panel.hpp src/scopes.cpp src/scopes.hpp src/theme.cpp src/theme.hpp)
target_compile_features(aigle-gui PRIVATE cxx_std_23)
set_target_properties(aigle-gui PROPERTIES CXX_EXTENSIONS OFF)
target_compile_options(aigle-gui PRIVATE $<$<CXX_COMPILER_ID:GNU,Clang>:-Wall;-Wextra;-Wpedantic;-Wconversion;-Wshadow>)
target_link_libraries(aigle-gui PRIVATE Qt6::Widgets Qt6::Svg Qt6::GuiPrivate Vulkan::Vulkan aigle_qt_bridge)
file(GLOB aigle_ui_icons CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/assets/icons/*.svg")
qt_add_resources(aigle-gui editor-style PREFIX "/ui" BASE "${CMAKE_CURRENT_SOURCE_DIR}"
  FILES assets/theme.qss assets/develop-layout.json ${aigle_ui_icons})
if(UNIX AND NOT APPLE)
  set_target_properties(aigle-gui PROPERTIES INSTALL_RPATH "$ORIGIN/../${CMAKE_INSTALL_LIBDIR}")
endif()
qt_add_shaders(aigle-gui canvas-shaders PREFIX "/shaders" BASE shaders
  FILES shaders/canvas.vert shaders/canvas.frag shaders/retouch.vert shaders/retouch.frag)
install(TARGETS aigle-gui RUNTIME DESTINATION bin)
corrosion_install(TARGETS aigle_qt_bridge LIBRARY DESTINATION "${CMAKE_INSTALL_LIBDIR}")
install(DIRECTORY "${AIGLE_ROOT}/data/" DESTINATION share/aigle)
install(FILES "${AIGLE_ROOT}/crates/aigle/xtask/assets/aigle.desktop" DESTINATION share/applications)
install(FILES "${AIGLE_ROOT}/crates/aigle/xtask/assets/aigle.svg" DESTINATION share/icons/hicolor/scalable/apps)
option(AIGLE_DEPLOY_QT "Bundle Qt libraries and plugins in the install tree" OFF)
if(AIGLE_DEPLOY_QT)
  set(aigle_platform_plugins)
  if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    find_package(Qt6 6.11 REQUIRED COMPONENTS WaylandClient)
    # The deployment API takes plugin file-name stems, rather than CMake targets.
    list(APPEND aigle_platform_plugins qwayland xdg-shell bradient qxdgdesktopportal)
  endif()
  string(JOIN " " aigle_platform_plugins ${aigle_platform_plugins})
  qt_generate_deploy_script(TARGET aigle-gui OUTPUT_SCRIPT aigle_deploy_script CONTENT "
    qt_deploy_runtime_dependencies(
      EXECUTABLE \"$<TARGET_FILE:aigle-gui>\"
      GENERATE_QT_CONF NO_TRANSLATIONS
      INCLUDE_PLUGINS ${aigle_platform_plugins}
      EXCLUDE_PLUGIN_TYPES imageformats iconengines styles)
  ")
  install(SCRIPT "${aigle_deploy_script}")
endif()

if(BUILD_TESTING)
  qt_add_executable(aigle-scopes-tests tests/scopes_test.cpp src/scopes.cpp src/scopes.hpp)
  target_compile_features(aigle-scopes-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-scopes-tests PRIVATE src)
  target_link_libraries(aigle-scopes-tests PRIVATE Qt6::Test Qt6::Widgets)
  add_test(NAME qt-scopes COMMAND aigle-scopes-tests)
  set_tests_properties(qt-scopes PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
  qt_add_executable(aigle-comparison-tests tests/comparison_test.cpp src/comparison.hpp)
  target_compile_features(aigle-comparison-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-comparison-tests PRIVATE src)
  target_link_libraries(aigle-comparison-tests PRIVATE Qt6::Test Qt6::Widgets)
  add_test(NAME qt-comparison COMMAND aigle-comparison-tests)
  qt_add_executable(aigle-export-queue-tests tests/export_queue_test.cpp src/export_queue.hpp)
  target_compile_features(aigle-export-queue-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-export-queue-tests PRIVATE src)
  target_link_libraries(aigle-export-queue-tests PRIVATE Qt6::Test Qt6::Core)
  add_test(NAME qt-export-queue COMMAND aigle-export-queue-tests)
  qt_add_executable(aigle-library-tests tests/library_query_test.cpp src/library_query.hpp)
  target_compile_features(aigle-library-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-library-tests PRIVATE src)
  target_link_libraries(aigle-library-tests PRIVATE Qt6::Test Qt6::Core)
  add_test(NAME qt-library-query COMMAND aigle-library-tests)
  qt_add_executable(aigle-qt-tests tests/viewport_test.cpp src/viewport.hpp)
  target_compile_features(aigle-qt-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-qt-tests PRIVATE src)
  target_link_libraries(aigle-qt-tests PRIVATE Qt6::Test Qt6::Gui)
  add_test(NAME qt-viewport COMMAND aigle-qt-tests)
  set_tests_properties(qt-viewport PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
  qt_add_executable(aigle-value-tests tests/value_editor_test.cpp src/engine.cpp src/engine.hpp src/agent_panel.cpp src/agent_panel.hpp src/value_editor.cpp
    src/value_editor.hpp src/adjustments.cpp src/adjustments.hpp src/curve.cpp src/curve.hpp src/theme.cpp src/theme.hpp
    src/tool_panel.cpp src/tool_panel.hpp src/retouch_panel.cpp src/retouch_panel.hpp src/export_dialog.cpp src/export_dialog.hpp
    src/canvas.cpp src/canvas.hpp src/window.cpp src/window.hpp src/window_library.cpp
    src/window_preferences.cpp src/window_shortcuts.cpp src/window_exports.cpp
    src/comparison.cpp src/comparison.hpp src/scopes.cpp src/scopes.hpp)
  target_compile_features(aigle-value-tests PRIVATE cxx_std_23)
  target_include_directories(aigle-value-tests PRIVATE src)
  target_link_libraries(aigle-value-tests PRIVATE Qt6::Test Qt6::Widgets Qt6::Svg Qt6::GuiPrivate Vulkan::Vulkan aigle_qt_bridge)
  qt_add_resources(aigle-value-tests test-editor-style PREFIX "/ui" BASE "${CMAKE_CURRENT_SOURCE_DIR}"
    FILES assets/theme.qss assets/develop-layout.json ${aigle_ui_icons})
  qt_add_shaders(aigle-value-tests test-canvas-shaders PREFIX "/shaders" BASE shaders
    FILES shaders/canvas.vert shaders/canvas.frag shaders/retouch.vert shaders/retouch.frag)
  add_test(NAME qt-value-editors COMMAND aigle-value-tests)
  set_tests_properties(qt-value-editors PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endif()

qt_add_lrelease(TS_FILES translations/aigle_zh_CN.ts QM_FILES_OUTPUT_VARIABLE aigle_qm
  MERGE_QT_TRANSLATIONS QT_TRANSLATION_CATALOGS qtbase)
qt_add_resources(aigle-gui editor-translations PREFIX "/i18n"
  BASE "${CMAKE_CURRENT_BINARY_DIR}" FILES ${aigle_qm})
if(TARGET aigle-value-tests)
  qt_add_resources(aigle-value-tests test-translations PREFIX "/i18n"
    BASE "${CMAKE_CURRENT_BINARY_DIR}" FILES ${aigle_qm})
endif()
