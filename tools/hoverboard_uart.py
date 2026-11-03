#!/usr/bin/env python3
"""Bench test of a hoverboard board running Gen2.x REMOTE_UARTBUS firmware (docs/11-hoverboard.md).

Talks to the board through any 3.3 V USB-UART (the debugprobe's own UART: GP4 = TX -> board RX, GP5 = RX <- board
TX). Sends speed commands at 20 Hz for a few seconds, prints every reply, then goes silent and keeps listening so
you can see the board stop by itself (HOV-001: soft stop after 0.5 s without commands, PWM off after 2 s).

  hoverboard_uart.py                      speed 0: only telemetry, the wheel must not move
  hoverboard_uart.py --speed 150 --time 3 slow forward for 3 s, then silence
  hoverboard_uart.py --speed -150         backwards

Speed is the firmware's raw unit (DRIVING_MODE 0: PWM, -1000..1000); --max caps it for bench safety.
"""

import argparse
import struct
import sys
import time

try:
    import serial
except ImportError:
    sys.exit("❌ Error: pyserial not found (Debian package: python3-serial).")

BAUD = 19200  # Gen2.x REMOTE_BAUD


def crc16(data: bytes) -> int:
    """Gen2.x CalcCRC: CRC-16/XMODEM (poly 0x1021, init 0, no reflection)."""
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else crc << 1
            crc &= 0xFFFF
    return crc


def command(slave: int, speed: int, state: int = 0) -> bytes:
    """SerialServer2Hover, data type 0: '/', type, slave id, int16 speed, uint8 wState, uint16 crc (all LE)."""
    body = struct.pack('<cBBhB', b'/', 0, slave, speed, state)
    return body + struct.pack('<H', crc16(body))


# SerialHover2Server without SEND_IMU_DATA: uint16 0xABCD, slave, int16 100*km/h, uint16 100*V, int16 100*A,
# int32 hall steps, uint16 crc.
REPLY = struct.Struct('<HBhHhiH')


def replies(buf: bytearray):
    """Pops complete, CRC-valid replies off the front of buf."""
    while True:
        start = buf.find(b'\xcd\xab')
        if start < 0:
            del buf[:max(0, len(buf) - 1)]
            return
        del buf[:start]
        if len(buf) < REPLY.size:
            return
        frame = bytes(buf[:REPLY.size])
        if REPLY.unpack(frame)[-1] != crc16(frame[:-2]):
            del buf[:2]
            continue
        del buf[:REPLY.size]
        yield REPLY.unpack(frame)


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--port', default='/dev/ttyACM0')
    p.add_argument('--slave', type=int, default=0, help='SLAVE_ID of the board (default 0)')
    p.add_argument('--speed', type=int, default=0)
    p.add_argument('--time', type=float, default=3.0, help='seconds of commands (default 3)')
    p.add_argument('--listen', type=float, default=3.0, help='seconds of silence afterwards (default 3)')
    p.add_argument('--max', type=int, default=300, help='bench cap for |speed| (default 300)')
    a = p.parse_args()
    if abs(a.speed) > a.max:
        sys.exit(f'❌ |speed| {abs(a.speed)} > --max {a.max}')

    port = serial.Serial(a.port, BAUD, timeout=0.01)
    port.reset_input_buffer()
    buf = bytearray()
    sent = got = 0
    t0 = time.monotonic()
    next_tx = t0
    print(f'→ slave {a.slave}, speed {a.speed} for {a.time} s, then {a.listen} s silent')
    while (now := time.monotonic()) - t0 < a.time + a.listen:
        if now - t0 < a.time and now >= next_tx:
            port.write(command(a.slave, a.speed))
            sent += 1
            next_tx += 0.05
        buf += port.read(256)
        for _, slave, speed, volt, amp, odom, _ in replies(buf):
            got += 1
            phase = 'cmd ' if now - t0 < a.time else 'idle'
            print(f'{now - t0:6.2f}s {phase} slave {slave}  {volt / 100:5.2f} V  {amp / 100:5.2f} A  '
                  f'{speed / 100:6.2f} km/h  odom {odom}')
    # One more frame with speed 0 is not sent on purpose: the board must stop without it (HOV-001).
    print(f'sent {sent}, replies {got}, unparsed bytes {len(buf)}')
    if got == 0:
        print('❌ no replies: check TX/RX crossing, GND, the slave id, and that the board runs REMOTE_UARTBUS firmware')


if __name__ == '__main__':
    main()
