// TinyUSB configuration: native USB port in host mode, no class drivers except the
// application AOA driver in usb_aoa.c (docs/03-usb-link.md LINK-001).

#pragma once

#ifndef CFG_TUSB_MCU
#error CFG_TUSB_MCU must be defined (set by the Pico SDK)
#endif

#ifndef CFG_TUSB_OS
#define CFG_TUSB_OS OPT_OS_NONE
#endif

#define CFG_TUSB_DEBUG 0

#define CFG_TUH_MEM_SECTION
#define CFG_TUH_MEM_ALIGN __attribute__((aligned(4)))

#define CFG_TUH_ENABLED   1
#define CFG_TUH_MAX_SPEED OPT_MODE_FULL_SPEED

// The phone is attached directly, never through a hub.
#define CFG_TUH_HUB        0
#define CFG_TUH_DEVICE_MAX 1

// Phones expose large composite descriptors (MTP + adb + RNDIS...); the whole configuration
// descriptor must fit or enumeration aborts before AOA can start.
#define CFG_TUH_ENUMERATION_BUFSIZE 512
#define CFG_TUH_INTERFACE_MAX       16
#define CFG_TUH_ENDPOINT_MAX        8
