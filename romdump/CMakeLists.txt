# Configure with [fxsdk build-fx], which provides the toolchain file and module path of the fxSDK

cmake_minimum_required(VERSION 3.15)
project(RomDump)

include(GenerateG1A)
include(Fxconv)
find_package(Gint 2.9 REQUIRED)

add_executable(romdump src/main.c)
target_compile_options(romdump PRIVATE -Wall -Wextra -Os)
target_link_libraries(romdump Gint::Gint)

generate_g1a(TARGET romdump OUTPUT "ROMDUMP.g1a" NAME "RomDump" ICON assets-fx/icon.png)
