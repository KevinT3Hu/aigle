# Expected hashes of the tested project source after applying the patch series.
file(READ "${SOURCE_PATH}/src/decoders/decoders_dcraw.cpp" content)
string(REPLACE "\r\n" "\n" content "${content}")
string(SHA256 actual "${content}")
if(NOT actual STREQUAL "665d8f6649a4ef09bfd0c2b9d71aed03f503edd8d2710544857579ee9105d792")
    message(FATAL_ERROR "Patched source mismatch: src/decoders/decoders_dcraw.cpp")
endif()
