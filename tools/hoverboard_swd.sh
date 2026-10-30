#!/bin/bash
# Hoverboard mainboard over SWD (docs/11-hoverboard.md): an RP2040 running debugprobe (CMSIS-DAP) + openocd.
#   info              chip ID, flash size, read protection; changes nothing
#   backup FILE       save the flash to FILE (only works while read protection is off)
#   unlock --erase    remove read protection = MASS ERASE, the stock firmware is gone for good
#   flash FILE.bin    write FILE.bin at the start of flash, verify, start it
#   rtt               print the firmware's RTT log until Ctrl-C (Gen2.x with RTT_REMOTE)
# OPENOCD_TARGET picks the openocd target script (default target/stm32f1x.cfg: GD32F130/GD32F103/STM32F103).
# info, backup and rtt never halt the CPU: a halted MCU leaves the motor PWM frozen in its last state. unlock and
# flash do halt it, so run them with the battery disconnected and the board on 3.3 V from the probe.
set -e

TARGET_CFG="${OPENOCD_TARGET:-target/stm32f1x.cfg}"
RTT_PORT=19021

if ! command -v openocd > /dev/null; then
    echo "❌ Error: openocd not found (Debian package: openocd)."
    exit 1
fi

# GD32 chips report their own Cortex-M TAP ID, CPUTAPID 0 accepts any.
ocd() {
    openocd -f interface/cmsis-dap.cfg -c "transport select swd; adapter speed 1000; set CPUTAPID 0" \
        -f "$TARGET_CFG" -c init "$@" -c shutdown 2>&1
}

# Prints "name: value" lines; a failed read prints "name: ERROR" instead of aborting.
read_words() {
    local tcl=""
    while [ $# -gt 0 ]; do
        tcl+="if {[catch {set v [read_memory $2 32 $3]}]} {echo \"$1: ERROR\"} else {echo \"$1: \$v\"};"
        shift 3
    done
    ocd -c "$tcl" | grep -E '^[a-z]+: '
}

case "$1" in
info)
    out=$(read_words idcode 0xE0042000 1 size 0x1FFFF7E0 1 options 0x1FFFF800 1 flash 0x08000000 2) || true
    echo "$out"
    id=$(echo "$out" | awk '/^idcode:/ {print $2}')
    size=$(echo "$out" | awk '/^size:/ {print $2}')
    opt=$(echo "$out" | awk '/^options:/ {print $2}')
    if [ -z "$id" ] || [ "$id" = ERROR ]; then
        echo "❌ No chip: check the wiring (SWDIO, SWCLK, GND) and that the board has 3.3 V."
        exit 1
    fi
    printf '   device 0x%03x rev 0x%04x, flash %d KB, RAM %d KB\n' $((id & 0xFFF)) $((id >> 16)) \
        $((size & 0xFFFF)) $((size >> 16))
    # Low byte of the first option word is the read protection byte: 0xA5 = off, anything else = on.
    if [ $((opt & 0xFF)) -eq $((0xA5)) ]; then
        echo "   read protection: off (backup possible)"
    else
        printf '   read protection: ON (0x%02x), the stock firmware cannot be saved; unlock erases it\n' $((opt & 0xFF))
    fi
    ;;
backup)
    [ -n "$2" ] || { echo "usage: $0 backup FILE"; exit 1; }
    ocd -c "flash read_bank 0 $(realpath -m "$2")" | grep -E 'wrote|Error'
    ;;
unlock)
    if [ "$2" != "--erase" ]; then
        echo "unlock mass-erases the chip, the stock firmware is lost for good. Run: $0 unlock --erase"
        exit 1
    fi
    ocd -c "reset halt" -c "stm32f1x unlock 0" -c "reset halt" | grep -E 'unlocked|Error'
    "$0" info
    ;;
flash)
    [ -f "$2" ] || { echo "usage: $0 flash FILE.bin"; exit 1; }
    bin=$(realpath "$2")
    sha256sum "$bin"
    ocd -c "reset halt" -c "flash write_image erase $bin 0x08000000 bin" \
        -c "verify_image $bin 0x08000000 bin" -c "reset run" | grep -E 'wrote|verified|Error|mismatch'
    ;;
rtt)
    # openocd finds the SEGGER RTT control block in RAM and serves channel 0 on a local TCP port.
    openocd -f interface/cmsis-dap.cfg -c "transport select swd; adapter speed 1000; set CPUTAPID 0" \
        -f "$TARGET_CFG" -c init -c "rtt setup 0x20000000 8192 {SEGGER RTT}" -c "rtt start" \
        -c "rtt server start $RTT_PORT 0" > /tmp/hoverboard_rtt.log 2>&1 &
    trap 'kill $! 2> /dev/null' EXIT
    sleep 3
    python3 -I -c "
import socket, sys
s = socket.create_connection(('localhost', $RTT_PORT))
while data := s.recv(4096):
    sys.stdout.write(data.decode(errors='replace').replace('\r', '')); sys.stdout.flush()
"
    ;;
*)
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
    ;;
esac
