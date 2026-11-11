# Builds the Joulescope driver as a part of a larger project. That way the
# driver leaves out its own tests and its warnings-as-errors flags, which a
# newer compiler than the vendor's would trip over.
cmake_minimum_required(VERSION 3.22)
project(benchwatt_jsdrv LANGUAGES C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_EXTENSIONS ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# The copy of the driver that build.rs prepared, with patches/ applied.
add_subdirectory(${JSDRV_SOURCE} jsdrv)
