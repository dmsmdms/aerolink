cmake_minimum_required(VERSION 3.13)

# PICO_SDK_PATH comes from the environment (default /opt/pico-sdk, see tools/build_rp2040.sh)
include(pico_sdk_import.cmake)

project(aerolink VERSION 0.1.0 LANGUAGES C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

# Pico SDK 2.3 insists on picotool >= 2.3.0 (and passes it 2.3-only flags), Debian ships 2.1.1.
# On RP2040 the SDK's picotool post-processing (signing/hashing) is a no-op, so disable it and
# produce the UF2 ourselves with the system picotool (see the end of this file).
set(PICO_NO_PICOTOOL 1)
find_program(PICOTOOL_BIN picotool REQUIRED)

pico_sdk_init()

# Motor driver selection (docs/02-architecture.md "Motor driver"). Only "sim" exists for now.
set(AEROLINK_MOTOR_DRIVER "sim" CACHE STRING "Motor driver implementation (src/motor_<name>.c)")

add_executable(aerolink
    src/main.c
    src/proto.c
    src/control.c
    src/link.c
    src/motor_${AEROLINK_MOTOR_DRIVER}.c
)

target_compile_options(aerolink PRIVATE -Wall -Wextra)
target_compile_definitions(aerolink PRIVATE
    AEROLINK_FW_VERSION_MAJOR=${PROJECT_VERSION_MAJOR}
    AEROLINK_FW_VERSION_MINOR=${PROJECT_VERSION_MINOR}
    AEROLINK_FW_VERSION_PATCH=${PROJECT_VERSION_PATCH}
)

target_link_libraries(aerolink
    pico_stdlib
    pico_unique_id
    hardware_watchdog
)

# The native USB port belongs to the phone link (AOA host), debug log goes to UART0 (GP0/GP1)
pico_enable_stdio_uart(aerolink 1)
pico_enable_stdio_usb(aerolink 0)

pico_add_extra_outputs(aerolink)

add_custom_command(TARGET aerolink POST_BUILD
    COMMAND ${PICOTOOL_BIN} uf2 convert --quiet $<TARGET_FILE:aerolink> aerolink.uf2 --family rp2040
    BYPRODUCTS aerolink.uf2
    VERBATIM
)
