// Hardware test image for the loader (tools/hwtest_rp2040.py, docs/03-usb-link.md "Firmware update over the
// link"): linked into the active slot like the application, it goes straight to BOOTSEL, so after every boot the
// PC sees the board again and reads with picotool which image the loader started and what it wrote to the boot
// record. Never shipped.

#include "pico/bootrom.h"

int main(void) {
    reset_usb_boot(0, 0);
}
