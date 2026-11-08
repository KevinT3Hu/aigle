cmake_minimum_required(VERSION 3.22)
project(phaos_rtprocess VERSION 0.11.0 LANGUAGES CXX)
include(GNUInstallDirs)
add_library(rtprocess STATIC
    src/demosaic/amaze.cc src/demosaic/border.cc src/demosaic/markesteijn.cc
    src/demosaic/rcd.cc src/preprocess/CA_correct.cc src/postprocess/hilite_recon.cc)
set_target_properties(rtprocess PROPERTIES CXX_STANDARD 11 CXX_STANDARD_REQUIRED ON CXX_EXTENSIONS OFF)
target_compile_definitions(rtprocess PRIVATE LIBRTPROCESS_STATIC)
target_include_directories(rtprocess PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/src/include>)
set(OMP_LIBS "")
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
    target_compile_options(rtprocess PRIVATE -O3 -fno-fast-math -ffp-contract=off)
endif()
# Do not link the OpenMP imported target: that would enable it for every source.
if(CMAKE_CXX_COMPILER_ID STREQUAL "GNU")
    find_package(OpenMP REQUIRED COMPONENTS CXX)
    set_source_files_properties(src/demosaic/rcd.cc src/preprocess/CA_correct.cc
        PROPERTIES COMPILE_OPTIONS "${OpenMP_CXX_FLAGS}")
    target_link_libraries(rtprocess PRIVATE ${OpenMP_CXX_LIBRARIES})
    set(OMP_LIBS "-lgomp")
endif()
install(TARGETS rtprocess ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR})
install(FILES src/include/librtprocess.h DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})

if(MSVC)
    set(CXX_RUNTIME_LIBS "")
else()
    set(CXX_RUNTIME_LIBS "-lstdc++ -lm")
endif()

configure_file(${CMAKE_CURRENT_SOURCE_DIR}/phaos-rtprocess.pc.in phaos-rtprocess.pc @ONLY)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/phaos-rtprocess.pc DESTINATION ${CMAKE_INSTALL_LIBDIR}/pkgconfig)
