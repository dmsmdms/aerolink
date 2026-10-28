# 07. Open questions

| # | Question | Impacts |
|---|----------|---------|
| Q1 | Hoverboard mainboard and firmware (stock / EFeru FOC), interface | Real motor driver, TELEMETRY fields. Until then: `motor_sim` |
| Q4 | Final phone models (first: Samsung SM-J710F, Android 6) | AOA quirks, sysfs charge node, H.264 encoder, Camera2 level |
| Q5 | Peripherals (cargo lock, lid sensor, lights, range sensors) | Messages `0x20–0x3F` / `0xA0–0xBF`, `caps` |
| Q6 | Target stick-to-wheel latency, server location | Backend, TURN placement |

## Resolved

| # | Decision |
|---|----------|
| Q2 | USB topology: Android Open Accessory, RP2040 = host, phone charged from the robot battery |
| Q3 | RP2040 board: Pico-compatible clone with USB-C, `PICO_BOARD=pico` |
| — | Phone battery window 45–60 %, stop charging > 40 °C; needs root, disabled without it (`PWR-020..023`) |
| — | One operator — many robots; semi-autonomous robots stop and request help (`ARCH-001..004`) |
