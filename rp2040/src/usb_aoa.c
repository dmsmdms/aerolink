#include "usb_aoa.h"

#include <stdio.h>
#include <string.h>

#include "tusb.h"
// Application class driver API; needs tusb.h first.
#include "host/usbh_pvt.h"

// Android Open Accessory 1.0 (https://source.android.com/docs/core/interaction/accessories/aoa)
#define AOA_VID               0x18D1
#define AOA_PID_ACCESSORY     0x2D00
#define AOA_PID_ACCESSORY_ADB 0x2D01
#define AOA_GET_PROTOCOL      51
#define AOA_SEND_STRING       52
#define AOA_START             53
#define AOA_STRING_COUNT      6
#define ADB_SUBCLASS          0x42

// Identification strings, must match android/app/src/main/res/xml/accessory_filter.xml (LINK-002/003).
#define AOA_MANUFACTURER "AeroLink"
#define AOA_MODEL        "RobotBase"
#define AOA_DESCRIPTION  "AeroLink robot base"
#define AOA_URI          "https://aerolink.local/robot"

#define STR_(x) #x
#define STR(x)  STR_(x)
#define AOA_VERSION                                                                          \
    STR(AEROLINK_FW_VERSION_MAJOR) "." STR(AEROLINK_FW_VERSION_MINOR) "." STR(               \
        AEROLINK_FW_VERSION_PATCH)

// Android's accessory gadget completes a read only on a short packet or when its 16 KiB request
// is full, so a transfer that ends on a full 64-byte packet would sit in the phone's buffer.
// Chunks of 63 bytes always end with a short packet.
#define TX_CHUNK      63
#define TX_QUEUE_SIZE 512
#define RX_BUF_SIZE   64

// Start sequence steps (user_data of the control transfers): GET_PROTOCOL, 6 strings, START.
#define STEP_GET_PROTOCOL 0
#define STEP_START        (1 + AOA_STRING_COUNT)

static const usb_aoa_callbacks_t *cb;
static const char *strings[AOA_STRING_COUNT];

static struct {
    uint8_t daddr;
    uint8_t ep_in;
    uint8_t ep_out;
    bool opened;   // accessory interface claimed during enumeration
    bool mounted;  // enumeration finished, transfers running
    bool tx_busy;
} acc;

static uint8_t txq[TX_QUEUE_SIZE];
static uint16_t txq_head;
static uint16_t txq_len;

CFG_TUH_MEM_SECTION CFG_TUH_MEM_ALIGN static uint8_t tx_buf[TX_CHUNK];
CFG_TUH_MEM_SECTION CFG_TUH_MEM_ALIGN static uint8_t rx_buf[RX_BUF_SIZE];
CFG_TUH_MEM_SECTION CFG_TUH_MEM_ALIGN static uint8_t ctrl_buf[2];

static bool is_accessory(uint8_t daddr) {
    uint16_t vid, pid;
    return tuh_vid_pid_get(daddr, &vid, &pid) && vid == AOA_VID &&
           (pid == AOA_PID_ACCESSORY || pid == AOA_PID_ACCESSORY_ADB);
}

// --- Accessory start sequence (LINK-002) ---------------------------------------------------

static void start_step_done(tuh_xfer_t *xfer);

static bool start_step(uint8_t daddr, uintptr_t step) {
    tusb_control_request_t req = {
        .bmRequestType_bit = {.recipient = TUSB_REQ_RCPT_DEVICE,
                              .type = TUSB_REQ_TYPE_VENDOR,
                              .direction = TUSB_DIR_OUT},
    };
    uint8_t *buf = NULL;
    if (step == STEP_GET_PROTOCOL) {
        req.bmRequestType_bit.direction = TUSB_DIR_IN;
        req.bRequest = AOA_GET_PROTOCOL;
        req.wLength = sizeof(ctrl_buf);
        buf = ctrl_buf;
    } else if (step < STEP_START) {
        const char *s = strings[step - 1];
        req.bRequest = AOA_SEND_STRING;
        req.wIndex = (uint16_t)(step - 1);
        req.wLength = (uint16_t)(strlen(s) + 1);
        buf = (uint8_t *)(uintptr_t)s;  // OUT data, only read by the stack
    } else {
        req.bRequest = AOA_START;
    }
    tuh_xfer_t xfer = {
        .daddr = daddr,
        .ep_addr = 0,
        .setup = &req,
        .buffer = buf,
        .complete_cb = start_step_done,
        .user_data = step,
    };
    return tuh_control_xfer(&xfer);
}

static void start_step_done(tuh_xfer_t *xfer) {
    uintptr_t step = xfer->user_data;
    if (xfer->result != XFER_RESULT_SUCCESS) {
        printf("aoa: dev %u: step %u failed (not an Android phone?)\n", xfer->daddr, (unsigned)step);
        return;
    }
    if (step == STEP_GET_PROTOCOL) {
        uint16_t protocol = (uint16_t)(ctrl_buf[0] | ctrl_buf[1] << 8);
        if (xfer->actual_len < 2 || protocol < 1) {
            printf("aoa: dev %u: no accessory support\n", xfer->daddr);
            return;
        }
        printf("aoa: dev %u: protocol %u, starting accessory mode\n", xfer->daddr, protocol);
    }
    if (step == STEP_START) {
        printf("aoa: dev %u: waiting for re-enumeration\n", xfer->daddr);
        return;
    }
    if (!start_step(xfer->daddr, step + 1)) {
        printf("aoa: dev %u: step %u not queued\n", xfer->daddr, (unsigned)step + 1);
    }
}

// --- Data transfers ------------------------------------------------------------------------

static void rx_start(void) {
    if (!usbh_edpt_claim(acc.daddr, acc.ep_in)) {
        return;
    }
    if (!usbh_edpt_xfer(acc.daddr, acc.ep_in, rx_buf, sizeof(rx_buf))) {
        usbh_edpt_release(acc.daddr, acc.ep_in);
        printf("aoa: rx not queued\n");
    }
}

static void tx_kick(void) {
    if (!acc.mounted || acc.tx_busy || txq_len == 0) {
        return;
    }
    uint16_t n = txq_len < TX_CHUNK ? txq_len : TX_CHUNK;
    for (uint16_t i = 0; i < n; i++) {
        tx_buf[i] = txq[(txq_head + i) % TX_QUEUE_SIZE];
    }
    if (!usbh_edpt_claim(acc.daddr, acc.ep_out)) {
        return;
    }
    if (!usbh_edpt_xfer(acc.daddr, acc.ep_out, tx_buf, n)) {
        usbh_edpt_release(acc.daddr, acc.ep_out);
        return;
    }
    txq_head = (uint16_t)((txq_head + n) % TX_QUEUE_SIZE);
    txq_len = (uint16_t)(txq_len - n);
    acc.tx_busy = true;
}

bool usb_aoa_send(const uint8_t *data, size_t len, void *ctx) {
    (void)ctx;
    if (!acc.mounted || len > (size_t)(TX_QUEUE_SIZE - txq_len)) {
        return false;
    }
    for (size_t i = 0; i < len; i++) {
        txq[(txq_head + txq_len++) % TX_QUEUE_SIZE] = data[i];
    }
    tx_kick();
    return true;
}

bool usb_aoa_connected(void) { return acc.mounted; }

// --- Application class driver: claims the accessory interface ------------------------------

static bool drv_init(void) { return true; }

static bool drv_deinit(void) { return true; }

static bool drv_open(uint8_t rhport, uint8_t daddr, tusb_desc_interface_t const *itf,
                     uint16_t max_len) {
    (void)rhport;
    // Interface 0 of an accessory is the AOA one; 0x2D01 adds adb (subclass 0x42), left alone.
    if (acc.opened || !is_accessory(daddr) || itf->bInterfaceClass != TUSB_CLASS_VENDOR_SPECIFIC ||
        itf->bInterfaceSubClass == ADB_SUBCLASS || itf->bNumEndpoints != 2) {
        return false;
    }
    tusb_desc_endpoint_t const *ep_in = NULL;
    tusb_desc_endpoint_t const *ep_out = NULL;
    uint8_t const *p = tu_desc_next(itf);
    uint8_t const *end = (uint8_t const *)itf + max_len;
    for (; p < end && tu_desc_type(p) != TUSB_DESC_INTERFACE; p = tu_desc_next(p)) {
        if (tu_desc_type(p) != TUSB_DESC_ENDPOINT) {
            continue;
        }
        tusb_desc_endpoint_t const *ep = (tusb_desc_endpoint_t const *)p;
        if (ep->bmAttributes.xfer != TUSB_XFER_BULK) {
            return false;
        }
        if (tu_edpt_dir(ep->bEndpointAddress) == TUSB_DIR_IN) {
            ep_in = ep;
        } else {
            ep_out = ep;
        }
    }
    if (!ep_in || !ep_out || !tuh_edpt_open(daddr, ep_in) || !tuh_edpt_open(daddr, ep_out)) {
        return false;
    }
    acc = (typeof(acc)){
        .daddr = daddr,
        .ep_in = ep_in->bEndpointAddress,
        .ep_out = ep_out->bEndpointAddress,
        .opened = true,
    };
    return true;
}

static bool drv_set_config(uint8_t daddr, uint8_t itf_num) {
    usbh_driver_set_config_complete(daddr, itf_num);
    return true;
}

static bool drv_xfer_cb(uint8_t daddr, uint8_t ep_addr, xfer_result_t result, uint32_t len) {
    if (!acc.mounted || daddr != acc.daddr) {
        return false;
    }
    if (ep_addr == acc.ep_in) {
        if (result != XFER_RESULT_SUCCESS) {
            // Not re-queued: a stalled endpoint would complete instantly forever. The phone's
            // command timeout stops the robot (SAFE-002); a re-attach recovers the link.
            printf("aoa: rx failed (%d), reception stopped\n", result);
            return true;
        }
        if (len) {
            cb->rx(rx_buf, len, cb->ctx);
        }
        rx_start();
    } else if (ep_addr == acc.ep_out) {
        acc.tx_busy = false;
        if (result != XFER_RESULT_SUCCESS) {
            printf("aoa: tx failed (%d), chunk dropped\n", result);
        }
        tx_kick();
    }
    return true;
}

static void drv_close(uint8_t daddr) {
    if (!acc.opened || daddr != acc.daddr) {
        return;
    }
    bool was_mounted = acc.mounted;
    acc = (typeof(acc)){0};
    txq_head = 0;
    txq_len = 0;
    printf("aoa: accessory detached\n");
    if (was_mounted) {
        cb->disconnected(cb->ctx);
    }
}

static const usbh_class_driver_t aoa_driver = {
    .name = "AOA",
    .init = drv_init,
    .deinit = drv_deinit,
    .open = drv_open,
    .set_config = drv_set_config,
    .xfer_cb = drv_xfer_cb,
    .close = drv_close,
};

usbh_class_driver_t const *usbh_app_driver_get_cb(uint8_t *driver_count) {
    *driver_count = 1;
    return &aoa_driver;
}

// --- TinyUSB host callbacks ----------------------------------------------------------------

void tuh_mount_cb(uint8_t daddr) {
    uint16_t vid = 0, pid = 0;
    tuh_vid_pid_get(daddr, &vid, &pid);
    printf("usb: dev %u attached (%04x:%04x)\n", daddr, vid, pid);

    if (acc.opened && daddr == acc.daddr) {
        acc.mounted = true;
        printf("aoa: accessory connected\n");
        rx_start();
        cb->connected(cb->ctx);
        return;
    }
    if (is_accessory(daddr)) {
        printf("aoa: dev %u: accessory interface not found\n", daddr);
        return;
    }
    if (!start_step(daddr, STEP_GET_PROTOCOL)) {
        printf("aoa: dev %u: GET_PROTOCOL not queued\n", daddr);
    }
}

void tuh_umount_cb(uint8_t daddr) { printf("usb: dev %u detached\n", daddr); }

void usb_aoa_init(const usb_aoa_callbacks_t *callbacks, const char *serial) {
    cb = callbacks;
    strings[0] = AOA_MANUFACTURER;
    strings[1] = AOA_MODEL;
    strings[2] = AOA_DESCRIPTION;
    strings[3] = AOA_VERSION;
    strings[4] = AOA_URI;
    strings[5] = serial;

    const tusb_rhport_init_t host = {.role = TUSB_ROLE_HOST, .speed = TUSB_SPEED_FULL};
    tusb_init(0, &host);
}
