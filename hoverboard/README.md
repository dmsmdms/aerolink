# Hoverboard mainboard firmware

One directory per board model ([docs/11-hoverboard.md](../docs/11-hoverboard.md)): `config.h` (the upstream
config with our changes), `build.conf` (pinned upstream sources), the built images `firmware-id<N>.bin` (one per
`SLAVE_ID`, 0 = left, 1 = right) and their `SHA256SUMS`. Flash: `tools/hoverboard_swd.sh flash <image>`;
rebuild after changing `config.h`: `tools/hoverboard_build.sh <model>` (byte-identical for the same sources and
`gcc-arm-none-eabi`).

The images are built from third-party GPL-3.0 firmware (license text next to each model's images, e.g.
`gen2x-tt-sd2.2/LICENSE-Gen2x`). Corresponding source: the repository and commit in `build.conf` with the
`config.h` next to it, nothing else changed. They run on the hoverboard's own MCU and are not part of the
AeroLink code.
