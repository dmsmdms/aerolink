# AeroLink documentation

AeroLink is a teleoperation system for sidewalk delivery robots. These documents are the spec;
requirement IDs (e.g. `LINK-010`) are referenced from code comments and commits.

| # | Document | Contents |
|---|----------|----------|
| 01 | [Overview](01-overview.md) | Product, robot hardware, scope of the MVP |
| 02 | [Architecture](02-architecture.md) | Components, data flows, technology choices |
| 03 | [USB link](03-usb-link.md) | RP2040 ↔ phone: Android Open Accessory, framing, messages |
| 04 | [Power](04-power.md) | Power topology and phone energy-saving rules |
| 05 | [Safety](05-safety.md) | Failsafes, limits, command arbitration |
| 06 | [Roadmap](06-roadmap.md) | MVP stages and acceptance criteria |
| 07 | [Open questions](07-open-questions.md) | Unresolved decisions |
| 08 | [Backend and signaling](08-backend.md) | Server process, API, signaling and console ↔ robot control protocol |
| 09 | [Deployment](09-deployment.md) | Debian package, nginx (pre-compressed static, abuse filtering), systemd, firewall guard, coturn |
| 10 | [Packages](10-packages.md) | Debian packages for building and for running the server |
| 11 | [Hoverboard drive](11-hoverboard.md) | Reflashing hoverboard mainboards over SWD, per board model |
