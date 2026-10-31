vcpkg_check_linkage(ONLY_STATIC_LIBRARY)
vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO LibRaw/LibRaw
    REF 24fa7e5463cbf8b8615dbd2b16c933a294d52400
    SHA512 39a27d9a6338ca8f8a8b2c523cca342f845776d31288df78a85a70fe3a7f8353cfdb4d46b929a93b62f3733df548a92bdf66d8c8bf4a720913c0516fa104052c
    PATCHES 0001-nikon-buffered-entropy.patch
)
include("${CURRENT_PORT_DIR}/verify-sources.cmake")
file(COPY "${CURRENT_PORT_DIR}/CMakeLists.txt" "${CURRENT_PORT_DIR}/phaos-libraw.pc.in" DESTINATION "${SOURCE_PATH}")
vcpkg_cmake_configure(SOURCE_PATH "${SOURCE_PATH}")
vcpkg_cmake_install()
vcpkg_fixup_pkgconfig()
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE.LGPL" "${SOURCE_PATH}/LICENSE.CDDL")
