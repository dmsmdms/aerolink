# 07. Open questions

| # | Question | Impacts |
|---|----------|---------|
| Q1 | Hoverboard mainboard and firmware (stock / EFeru FOC), interface | Real motor driver, TELEMETRY fields. Until then: `motor_sim` |
| Q4 | Final phone models (first: Samsung SM-J710F, Android 6) | AOA quirks, sysfs charge node, H.264 encoder, Camera2 level |
| Q5 | Peripherals (cargo lock, lid sensor, lights, range sensors) | Messages `0x20–0x3F` / `0xA0–0xBF`, `caps` |
| Q6 | Target stick-to-wheel latency, server location | Backend, TURN placement |
| Q7 | Domain name and server for production | nginx `server_name`, certificates, `AEROLINK_ORIGIN` (placeholder `aerolink.example.com`) |
| Q8 | Operator registration, robot rental, ownership/fleets | `users.role`, robot access table, landing pages |

## Resolved

| # | Decision |
|---|----------|
| Q2 | USB topology: Android Open Accessory, RP2040 = host, phone charged from the robot battery |
| Q3 | RP2040 board: Pico-compatible clone with USB-C, `PICO_BOARD=pico` |
| Q9 | Admin map: public OpenStreetMap tiles for now (attribution shown, light use within the OSMF tile policy); a paid provider or self-hosted tiles later if the fleet grows (`ADM-011`) |
| Q10 | Robot RP2040 clone: 16 MB flash (W25Q-compatible, `boot2_w25q080`); the 1.6 MB slot layout fits with room to spare (`LINK-040..046`) |
| Q11 | Admins see the fleets on their list; some admins see all fleets (`ADM-005`). Owners/rental (Q8) build on the same fleets |
| — | Phone battery window 45–60 %, stop charging > 40 °C; needs root, disabled without it (`PWR-020..023`) |
| — | One operator — many robots; semi-autonomous robots stop and request help (`ARCH-001..004`) |
