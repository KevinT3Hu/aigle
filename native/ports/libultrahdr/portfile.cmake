vcpkg_from_github(
    OUT_SOURCE_PATH SOURCE_PATH
    REPO google/libultrahdr
    REF v${VERSION}
    SHA512 a5df720e8c9af6f2fa19a015040b3a572a764145079d3a843520407a5f5dc1dc2a80c91b6000248cc99e27f30b7d18a7d7e22bc0c3d24122eea33bcc1eb4b1b9
)
vcpkg_cmake_configure(SOURCE_PATH "${SOURCE_PATH}" OPTIONS
    -DUHDR_BUILD_DEPS=OFF -DUHDR_BUILD_EXAMPLES=OFF -DUHDR_BUILD_TESTS=OFF
    -DUHDR_ENABLE_HEIF=OFF -DUHDR_WRITE_XMP=ON -DUHDR_WRITE_ISO=ON
    -DCMAKE_REQUIRE_FIND_PACKAGE_JPEG=ON)
vcpkg_cmake_install()
vcpkg_fixup_pkgconfig()
# Upstream also installs an unused static archive in shared builds.
if(VCPKG_LIBRARY_LINKAGE STREQUAL "dynamic")
    file(REMOVE "${CURRENT_PACKAGES_DIR}/lib/libuhdr.a")
endif()
file(REMOVE_RECURSE "${CURRENT_PACKAGES_DIR}/debug/include")
vcpkg_install_copyright(FILE_LIST "${SOURCE_PATH}/LICENSE")
