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
find_program(PYTHON3_BIN python3 REQUIRED)

pico_sdk_init()

# Motor driver selection (docs/02-architecture.md "Motor driver"). Only "sim" exists for now.
set(AEROLINK_MOTOR_DRIVER "sim" CACHE STRING "Motor driver implementation (src/motor_<name>.c)")

# Flash layout: loader | boot record | active | staging | backup (src/fwstore.h,
# docs/03-usb-link.md "Firmware update over the link").

# --- aerolink-boot: the loader at the start of flash, flashed with a PC only ---
add_executable(aerolink-boot
    boot/loader.c
    src/fwstore.c
    src/crc32.c
    src/flash_rp2040.c
)
target_compile_options(aerolink-boot PRIVATE -Wall -Wextra)
target_include_directories(aerolink-boot PRIVATE ${CMAKE_CURRENT_LIST_DIR}/src)
target_link_libraries(aerolink-boot
    pico_stdlib
    hardware_flash
    hardware_watchdog
)
pico_enable_stdio_uart(aerolink-boot 0)
pico_enable_stdio_usb(aerolink-boot 0)
pico_add_linker_script_override_path(aerolink-boot ${CMAKE_CURRENT_LIST_DIR}/ld/loader FILES memory_flash.incl)
pico_add_bin_output(aerolink-boot)

# --- aerolink: the application, linked to run from the active slot ---
add_executable(aerolink
    src/main.c
    src/proto.c
    src/control.c
    src/link.c
    src/crc32.c
    src/fwstore.c
    src/fwupdate.c
    src/flash_rp2040.c
    src/usb_aoa.c
    src/motor_${AEROLINK_MOTOR_DRIVER}.c
)

target_compile_options(aerolink PRIVATE -Wall -Wextra)
# tusb_config.h lives in src/
target_include_directories(aerolink PRIVATE ${CMAKE_CURRENT_LIST_DIR}/src)
target_compile_definitions(aerolink PRIVATE
    AEROLINK_FW_VERSION_MAJOR=${PROJECT_VERSION_MAJOR}
    AEROLINK_FW_VERSION_MINOR=${PROJECT_VERSION_MINOR}
    AEROLINK_FW_VERSION_PATCH=${PROJECT_VERSION_PATCH}
)

target_link_libraries(aerolink
    pico_stdlib
    pico_unique_id
    hardware_flash
    hardware_watchdog
    tinyusb_host
)

# The native USB port belongs to the phone link (AOA host), debug log goes to UART0 (GP0/GP1)
pico_enable_stdio_uart(aerolink 1)
pico_enable_stdio_usb(aerolink 0)
pico_add_linker_script_override_path(aerolink ${CMAKE_CURRENT_LIST_DIR}/ld/app
    FILES memory_flash.incl section_boot2.incl)

# aerolink.bin is the image the phone installs (LINK-040..046; shipped in the APK).
pico_add_extra_outputs(aerolink)

# --- aerolink-probe: loader hardware tests only (tools/hwtest_rp2040.py), linked into the active slot ---
add_executable(aerolink-probe hwtest/probe.c)
target_link_libraries(aerolink-probe pico_stdlib)
pico_enable_stdio_uart(aerolink-probe 0)
pico_enable_stdio_usb(aerolink-probe 0)
pico_add_linker_script_override_path(aerolink-probe ${CMAKE_CURRENT_LIST_DIR}/ld/app
    FILES memory_flash.incl section_boot2.incl)
pico_add_bin_output(aerolink-probe)

# aerolink.uf2 for PC flashing: loader + erased boot record + application.
add_custom_command(TARGET aerolink POST_BUILD
    COMMAND ${PYTHON3_BIN} ${CMAKE_CURRENT_LIST_DIR}/../tools/fw_image.py
        $<TARGET_FILE_DIR:aerolink-boot>/aerolink-boot.bin aerolink.bin aerolink-flash.bin
    COMMAND ${PICOTOOL_BIN} uf2 convert --quiet aerolink-flash.bin -t bin aerolink.uf2 -o 0x10000000 --family rp2040
    BYPRODUCTS aerolink-flash.bin aerolink.uf2
    VERBATIM
)
add_dependencies(aerolink aerolink-boot)
