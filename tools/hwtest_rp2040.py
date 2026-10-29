#!/usr/bin/python3
"""Loader hardware test on a bare RP2040 board via picotool (docs/03-usb-link.md "Firmware update over the
link", LINK-043..045): install, trial, rollback, damaged images, interrupted copies.

    hwtest_rp2040.py [scenario ...]        all automatic scenarios when none is given
    hwtest_rp2040.py cut                   manual: unplug the board during the copies, plug it back in

The board must be in BOOTSEL. Each scenario writes the loader, a boot record and slot contents in one UF2, then
reboots. The active image is `aerolink-probe` (rp2040/hwtest/probe.c), which goes straight back to BOOTSEL: the
script then reads the boot record and the active slot to see what the loader did. Without a phone nothing
confirms a trial, so a good update is rolled back after FW_TRIAL_BOOTS boots, as it should be. Afterwards the
board is left in BOOTSEL: `make flash` puts the real firmware back.
"""

import os
import struct
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD = os.path.join(ROOT, "rp2040", "build")

# rp2040/src/fwstore.h
XIP = 0x10000000
SECTOR = 0x1000
RECORD_A, RECORD_B = 0x0E000, 0x0F000
ACTIVE, STAGING, BACKUP = 0x010000, 0x090000, 0x110000
SLOT_SIZE = 0x80000
TRIAL_BOOTS = 3
RECORD_MAGIC = 0x52424C41
NONE, STAGED, COPYING, TRIAL = range(4)
STATES = ["NONE", "STAGED", "COPYING", "TRIAL"]
FLAG_ROLLED_BACK = 1

LOADER = open(os.path.join(BUILD, "aerolink-boot.bin"), "rb").read()
PROBE = open(os.path.join(BUILD, "aerolink-probe.bin"), "rb").read()
# A second, different good image: the same program with a tail (size and CRC differ).
PROBE2 = PROBE + bytes((i * 7) & 0xFF for i in range(3000))
# Looks like a program (initial SP in RAM, Thumb reset vector in the slot) but hangs: only the loader's
# watchdog brings the board back.
HANG = struct.pack("<II", 0x20042000, XIP + ACTIVE + 0x100 + 1) + b"\xff" * (0x100 - 8) + b"\xfe\xe7"


def img(data):
    return (len(data), zlib.crc32(data))


def record(seq, state, trial=0, flags=0, active=(0, 0), staging=(0, 0), backup=(0, 0)):
    words = [RECORD_MAGIC, seq, state, trial, flags, *active, *staging, *backup]
    body = struct.pack("<11I", *words)
    return body + struct.pack("<I", zlib.crc32(body))


def parse_record(raw):
    magic, seq, state, trial, flags, *rest = struct.unpack("<11I", raw[:44])
    if magic != RECORD_MAGIC or struct.unpack("<I", raw[44:48])[0] != zlib.crc32(raw[:44]):
        return None
    return {"seq": seq, "state": state, "trial": trial, "flags": flags,
            "active": tuple(rest[0:2]), "staging": tuple(rest[2:4]), "backup": tuple(rest[4:6])}


def uf2(regions):
    """regions: {offset: bytes}, each padded to whole sectors (picotool erases what it writes)."""
    blocks = []
    for offset, data in sorted(regions.items()):
        data = data + b"\xff" * (-len(data) % SECTOR)
        for i in range(0, len(data), 256):
            blocks.append((XIP + offset + i, data[i:i + 256]))
    out = bytearray()
    for n, (addr, payload) in enumerate(blocks):
        head = struct.pack("<8I", 0x0A324655, 0x9E5D5157, 0x2000, addr, 256, n, len(blocks), 0xE48BFF56)
        out += head + payload + b"\x00" * (476 - 256) + struct.pack("<I", 0x0AB16F30)
    return bytes(out)


def picotool(*args, check=True):
    return subprocess.run(["picotool", *args], capture_output=True, text=True, check=check)


def in_bootsel():
    return picotool("info", check=False).returncode == 0


def wait_bootsel(seconds):
    end = time.time() + seconds
    while time.time() < end:
        if in_bootsel():
            return True
        time.sleep(0.3)
    return False


def write(regions):
    with tempfile.NamedTemporaryFile(suffix=".uf2") as f:
        f.write(uf2(regions))
        f.flush()
        picotool("load", "-v", f.name)


def read(offset, size):
    with tempfile.NamedTemporaryFile(suffix=".bin") as f:
        picotool("save", "-r", hex(XIP + offset), hex(XIP + offset + size), f.name, "-t", "bin")
        return open(f.name, "rb").read()


def state():
    raw = read(RECORD_A, 2 * SECTOR)
    a, b = parse_record(raw[:48]), parse_record(raw[SECTOR:SECTOR + 48])
    rec = max((r for r in (a, b) if r), key=lambda r: r["seq"], default=None)
    size = rec["active"][0] if rec and rec["active"][0] else 0
    return rec, (zlib.crc32(read(ACTIVE, size)) if size else None)


def boot(wait=15):
    """Reboot into the loader and wait for the probe (or the loader's own BOOTSEL) to come back."""
    picotool("reboot", check=False)
    time.sleep(1.0)  # the old BOOTSEL device must go away first
    if not wait_bootsel(wait):
        raise RuntimeError("the board did not come back to BOOTSEL")
    return state()


def name_of(image):
    for label, data in (("probe", PROBE), ("probe2", PROBE2), ("hang", HANG)):
        if image == img(data):
            return label
    return f"{image[0]} B/{image[1]:08x}"


def describe(rec):
    if not rec:
        return "no record"
    flags = " rolled-back" if rec["flags"] & FLAG_ROLLED_BACK else ""
    return (f"seq {rec['seq']} {STATES[rec['state']] if rec['state'] < 4 else rec['state']} trial {rec['trial']}"
            f"{flags}, active {name_of(rec['active'])}, backup {name_of(rec['backup'])}")


failures = 0


def check(ok, what):
    global failures
    print(("  ✓ " if ok else "  ✗ ") + what)
    if not ok:
        failures += 1


def expect(rec, crc, what, state_, active, trial=None, rolled_back=None):
    print(f"  · {describe(rec)}")
    ok = rec is not None and rec["state"] == state_ and rec["active"] == img(active) and crc == img(active)[1]
    if ok and trial is not None:
        ok = rec["trial"] == trial
    if ok and rolled_back is not None:
        ok = bool(rec["flags"] & FLAG_ROLLED_BACK) == rolled_back
    check(ok, what)


ERASED_RECORD = b"\xff" * (2 * SECTOR)


def records(rec_a=None, rec_b=None):
    a = (rec_a or b"") + b"\xff" * (SECTOR - len(rec_a or b""))
    b = (rec_b or b"") + b"\xff" * (SECTOR - len(rec_b or b""))
    return a + b


def scenario_plain():
    """A PC flash: erased record, the application runs as it is (LINK-045)."""
    write({0: LOADER, RECORD_A: ERASED_RECORD, ACTIVE: PROBE})
    rec, crc = boot()
    check(rec is None and crc is None, "erased record stays erased, probe started")


def scenario_update():
    """STAGED → backup, install, TRIAL ×3 unconfirmed → rollback (LINK-043, LINK-044)."""
    staged = record(2, STAGED, active=img(PROBE), staging=img(PROBE2))
    write({0: LOADER, RECORD_A: records(staged), ACTIVE: PROBE, STAGING: PROBE2})
    for n in range(1, TRIAL_BOOTS + 1):
        rec, crc = boot()
        expect(rec, crc, f"boot {n}: new image on trial", TRIAL, PROBE2, trial=n)
        if n == 1:
            check(rec["backup"] == img(PROBE), "old image kept as the backup")
    rec, crc = boot()
    expect(rec, crc, f"boot {TRIAL_BOOTS + 1}: unconfirmed, rolled back to the old image", NONE, PROBE, rolled_back=True)
    rec, crc = boot()
    expect(rec, crc, "next boot: the old image runs normally", NONE, PROBE, rolled_back=True)


def scenario_corrupt():
    """A staging image that fails its CRC is not installed (LINK-043)."""
    broken = bytearray(PROBE2)
    broken[1000] ^= 0x55
    staged = record(2, STAGED, active=img(PROBE), staging=img(PROBE2))
    write({0: LOADER, RECORD_A: records(staged), ACTIVE: PROBE, STAGING: bytes(broken)})
    rec, crc = boot()
    expect(rec, crc, "damaged update dropped, old image runs", NONE, PROBE, rolled_back=False)


def scenario_hang():
    """A new image that never starts properly: the watchdog reboots, rollback after 3 boots (LINK-044)."""
    staged = record(2, STAGED, active=img(PROBE), staging=img(HANG))
    write({0: LOADER, RECORD_A: records(staged), ACTIVE: PROBE, STAGING: HANG})
    t = time.time()
    rec, crc = boot(wait=30)
    print(f"  · back after {time.time() - t:.1f} s")
    expect(rec, crc, "hanging image rolled back without a PC", NONE, PROBE, rolled_back=True)


def scenario_damaged_active():
    """The active slot no longer matches the record: restored from the backup (LINK-045)."""
    broken = bytearray(PROBE2)
    broken[2000] ^= 0x01
    rec_ = record(4, NONE, active=img(PROBE2), backup=img(PROBE))
    write({0: LOADER, RECORD_A: records(rec_), ACTIVE: bytes(broken), BACKUP: PROBE})
    rec, crc = boot()
    expect(rec, crc, "damaged active image restored from the backup", NONE, PROBE, rolled_back=True)


def scenario_torn_copy():
    """Power cut while copying staging → active: the copy is repeated (LINK-043)."""
    half = PROBE2[:len(PROBE2) // 2] + b"\x00" * (len(PROBE2) // 2)
    copying = record(6, COPYING, active=img(PROBE), staging=img(PROBE2), backup=img(PROBE))
    write({0: LOADER, RECORD_A: records(copying), ACTIVE: half, STAGING: PROBE2, BACKUP: PROBE})
    rec, crc = boot()
    expect(rec, crc, "interrupted install finished, new image on trial", TRIAL, PROBE2, trial=1)


def scenario_torn_record():
    """A torn record write (bad CRC in the newer sector) leaves the previous record in force."""
    good = record(8, TRIAL, trial=1, active=img(PROBE2), backup=img(PROBE))
    torn = bytearray(record(9, NONE, active=img(PROBE)))
    torn[20] ^= 0xFF
    write({0: LOADER, RECORD_A: records(good, bytes(torn)), ACTIVE: PROBE2, BACKUP: PROBE})
    rec, crc = boot()
    expect(rec, crc, "older record used: trial goes on", TRIAL, PROBE2, trial=2)


def scenario_no_app():
    """Nothing runnable and no backup: the loader enters BOOTSEL itself (LINK-045)."""
    write({0: LOADER, RECORD_A: ERASED_RECORD, ACTIVE: b"\xff" * SECTOR})
    rec, crc = boot()
    check(rec is None, "loader went to BOOTSEL with an empty active slot")


def scenario_cut():
    """Manual power cuts: big images (each copy takes seconds), the board is unplugged during the copies."""
    big1 = PROBE + bytes((i * 13) & 0xFF for i in range(480 * 1024 - len(PROBE)))
    big2 = PROBE + bytes((i * 31) & 0xFF for i in range(480 * 1024 - len(PROBE)))
    staged = record(2, STAGED, active=img(big1), staging=img(big2))
    print("  writing 1 MB, takes a while…")
    write({0: LOADER, RECORD_A: records(staged), ACTIVE: big1, STAGING: big2})
    print("  ready: unplug the board, plug it in (no button) and pull it out again after 2-5 s, as often as you")
    print("  like; finally leave it plugged in. Each plug-in resumes the copy where the record says.")
    end = time.time() + 600
    while in_bootsel() and time.time() < end:
        time.sleep(0.3)
    t = time.time()
    # Back in BOOTSEL only when the probe runs, i.e. the install is complete.
    if not wait_bootsel(600):
        raise RuntimeError("the board did not come back to BOOTSEL")
    rec, crc = state()
    print(f"  · back after {time.time() - t:.1f} s: {describe(rec)}")
    ok = rec is not None and rec["state"] == TRIAL and rec["active"] == img(big2) and crc == img(big2)[1] \
        and rec["backup"] == img(big1) and zlib.crc32(read(BACKUP, len(big1))) == img(big1)[1]
    check(ok, "install completed after the power cut, both images intact")


SCENARIOS = {
    "plain": scenario_plain,
    "update": scenario_update,
    "corrupt": scenario_corrupt,
    "hang": scenario_hang,
    "damaged": scenario_damaged_active,
    "torn-copy": scenario_torn_copy,
    "torn-record": scenario_torn_record,
    "no-app": scenario_no_app,
}


def main():
    sys.stdout.reconfigure(line_buffering=True)  # progress shows up even when the output goes to a file
    names = sys.argv[1:] or list(SCENARIOS)
    if not in_bootsel():
        sys.exit("❌ No RP2040 in BOOTSEL: hold the BOOTSEL button while plugging the board in.")
    for name in names:
        fn = scenario_cut if name == "cut" else SCENARIOS.get(name)
        if not fn:
            sys.exit(f"unknown scenario {name}; known: {', '.join(SCENARIOS)}, cut")
        print(f"[{name}] {fn.__doc__.splitlines()[0]}")
        try:
            fn()
        except (RuntimeError, subprocess.CalledProcessError) as e:
            check(False, f"{name}: {getattr(e, 'stderr', None) or e}")
            if not wait_bootsel(5):
                break
    print("✅ all loader checks passed" if not failures else f"❌ {failures} check(s) failed")
    print("The board is in BOOTSEL with test images: `make flash` puts the real firmware back.")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
