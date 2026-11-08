# Recent MSVC emits UTF-8 diagnostics when piped. CMake may decode its
# localized /showIncludes prefix with the system ANSI code page instead,
# leaving Ninja unable to record header dependencies. Probe explicitly as UTF-8.
set(aigle_prefix_dir "${CMAKE_BINARY_DIR}/CMakeFiles/aigle-msvc-prefix")
file(MAKE_DIRECTORY "${aigle_prefix_dir}")
file(WRITE "${aigle_prefix_dir}/header.hpp" "")
file(WRITE "${aigle_prefix_dir}/probe.cpp" "#include \"header.hpp\"\n")
execute_process(
  COMMAND "${CMAKE_CXX_COMPILER}" /nologo /Zs /showIncludes "${aigle_prefix_dir}/probe.cpp"
  OUTPUT_VARIABLE aigle_prefix_output RESULT_VARIABLE aigle_prefix_result
  ERROR_QUIET ENCODING UTF-8)
if(aigle_prefix_result EQUAL 0)
  file(TO_NATIVE_PATH "${aigle_prefix_dir}/header.hpp" aigle_prefix_header)
  string(REGEX MATCH "[^\r\n]*header.hpp" aigle_prefix_line "${aigle_prefix_output}")
  string(FIND "${aigle_prefix_line}" "${aigle_prefix_header}" aigle_prefix_length)
  if(aigle_prefix_length GREATER 0)
    string(SUBSTRING "${aigle_prefix_line}" 0 ${aigle_prefix_length} aigle_prefix)
    string(FIND "${aigle_prefix}" "�" aigle_invalid_utf8)
    if(aigle_invalid_utf8 EQUAL -1)
      set(CMAKE_CL_SHOWINCLUDES_PREFIX "${aigle_prefix}")
    endif()
  endif()
endif()
