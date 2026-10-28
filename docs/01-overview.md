# 01. Overview

AeroLink lets a remote operator drive a delivery robot from a web browser over cellular internet.
Autonomy (lane keeping, traffic-light detection, …) is planned later; the MVP is operator-only.

## Robot hardware

| Part | Role |
|------|------|
| Android phone (Android 6.0+, first model Samsung SM-J710F; SIM card) | Camera video, microphone, speaker (talk to people), flashlight as headlight, cellular uplink, robot "brain" |
| RP2040 board | Motor controller bridge: USB to the phone, GPIO/UART to the hoverboard |
| Hoverboard (mainboard + 2 hub motors + battery) | Propulsion and main battery |

The phone and the RP2040 are connected by USB. The RP2040 is the **USB host** and the phone is an
**Android Open Accessory** device, so the robot battery powers the USB bus and charges the phone
(see [03](03-usb-link.md), [04](04-power.md)).

## MVP scope

1. Reliable, low-power RP2040 ↔ phone link with motor control and telemetry (stage 1).
2. Remote driving from a web console: live video + audio, drive commands, telemetry (stage 2).
3. Field readiness: headlight, two-way audio, reconnects, adaptive bitrate (stage 3).

One operator supervises **several robots**. Later robots run semi-autonomously and stop and request
the operator when they cannot decide; the MVP data model and console are built for that from day one.

Out of scope for the MVP: autonomy, fleet dispatch, customer-facing apps, cargo compartment and other
peripherals. Long-term target is a full delivery rover (Yandex Rover class): the USB protocol reserves
message ranges and capability bits for peripherals.
