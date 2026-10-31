vcpkg_check_linkage(ONLY_STATIC_LIBRARY)
vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO CarVac/librtprocess
    REF 9a858270acb2096e2e403d932760ee688fcac425
    SHA512 8d8276e01639b66ae5c3665ca1f10b078c7ed073b237526637a9f4b9172a440f01c7acaf3671bfa98a87ab25d2a662ffd55fd626ee6f7ffb270e1df895cc6131
    PATCHES
        0001-cancellable-demosaic.patch
        0002-rcd-parallel-correctness.patch
        0003-cancellable-deterministic-ca.patch
)
# This build recipe deliberately replaces upstream's library-wide OpenMP switch.
include("${CURRENT_PORT_DIR}/verify-sources.cmake")
file(COPY "${CURRENT_PORT_DIR}/CMakeLists.txt" "${CURRENT_PORT_DIR}/phaos-rtprocess.pc.in" DESTINATION "${SOURCE_PATH}")
vcpkg_cmake_configure(SOURCE_PATH "${SOURCE_PATH}")
vcpkg_cmake_install()
vcpkg_fixup_pkgconfig()
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE.txt")
