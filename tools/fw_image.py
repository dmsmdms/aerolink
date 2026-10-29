#!/usr/bin/python3
"""Joins the loader and the application into one flash image for PC flashing (`make flash`).

    fw_image.py <loader.bin> <app.bin> <out.bin>

Layout (rp2040/src/fwstore.h): loader at 0x0, boot record at 0xE000..0x10000 (written as erased, so a PC flash
also resets any update state left on the board), application at 0x10000 (the active slot).
"""

import sys

LOADER_MAX = 0xE000
ACTIVE = 0x10000
SLOT_SIZE = 0x80000


def main():
    loader_path, app_path, out_path = sys.argv[1:4]
    loader = open(loader_path, "rb").read()
    app = open(app_path, "rb").read()
    if len(loader) > LOADER_MAX:
        sys.exit(f"loader is {len(loader)} bytes, at most {LOADER_MAX} fit")
    if not 0 < len(app) <= SLOT_SIZE:
        sys.exit(f"application is {len(app)} bytes, at most {SLOT_SIZE} fit")
    image = loader + b"\xff" * (ACTIVE - len(loader)) + app
    open(out_path, "wb").write(image)


if __name__ == "__main__":
    main()
