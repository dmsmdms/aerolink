// AeroLink RP2040 firmware: bridge between the phone (USB, Android Open Accessory)
// and the hoverboard motor controller. Runs from the active slot behind the loader (boot/loader.c),
// which installs the images the phone sends (docs/03-usb-link.md "Firmware update over the link").

#include <stdio.h>

#include "hardware/watchdog.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "pico/unique_id.h"
#include "tusb.h"

#include "config.h"
#include "control.h"
#include "flash_rp2040.h"
#include "fwupdate.h"
#include "link.h"
#include "messages.h"
#include "motor.h"
#include "usb_aoa.h"

#define LOOP_PERIOD_MS      5
#define LED_ARMED_PERIOD_MS 100
#define LED_IDLE_PERIOD_MS  1000

// Watchdog scratch 0 (kept across the reboot, not used by the SDK or the loader): REBOOT from the phone.
#define REBOOT_MARK 0x52424F54u  // "TOBR"

static control_t control;
static link_t link;
static motor_feedback_t feedback;
static fw_flash_t flash;
static fwupdate_t fwupdate;

extern char __flash_binary_end;

static uint32_t now_ms(void) { return to_ms_since_boot(get_absolute_time()); }

// LED: blinking 1 s = no phone, solid = phone connected and disarmed, fast blink = armed.
static void led_update(uint32_t now) {
#ifdef PICO_DEFAULT_LED_PIN
    bool on;
    if (control_active(&control)) {
        on = (now / (LED_ARMED_PERIOD_MS / 2)) & 1;
    } else if (usb_aoa_connected()) {
        on = true;
    } else {
        on = (now / (LED_IDLE_PERIOD_MS / 2)) & 1;
    }
    gpio_put(PICO_DEFAULT_LED_PIN, on);
#else
    (void)now;
#endif
}

// Everything that keeps the robot safe; must keep running even while the USB stack blocks.
static void safety_tick(uint32_t now) {
    watchdog_update();
    motor_feedback(&feedback);
    control_set_faults(&control, feedback.faults);
    control_tick(&control, now);
    motor_set(control_out_left(&control), control_out_right(&control), now);
    led_update(now);
}

// TinyUSB host enumeration busy-waits (port reset 50 ms, debounce 450 ms, retries). Without RTOS it
// calls this hook, so the control loop and the watchdog keep running meanwhile (SAFE-002/004).
// link_tick is not called here: no USB transfers from inside the USB stack.
void tusb_time_delay_ms_api(uint32_t ms) {
    absolute_time_t end = make_timeout_time_ms(ms);
    while (!time_reached(end)) {
        safety_tick(now_ms());
        absolute_time_t step = make_timeout_time_ms(LOOP_PERIOD_MS);
        sleep_until(absolute_time_diff_us(step, end) < 0 ? end : step);
    }
}

uint32_t tusb_time_millis_api(void) { return now_ms(); }

static void on_usb_connected(void *ctx) { link_connected((link_t *)ctx); }

static void on_usb_disconnected(void *ctx) { link_disconnected((link_t *)ctx); }

static void on_usb_rx(const uint8_t *data, size_t len, void *ctx) {
    link_rx((link_t *)ctx, data, len, now_ms());
}

static const usb_aoa_callbacks_t usb_callbacks = {
    .connected = on_usb_connected,
    .disconnected = on_usb_disconnected,
    .rx = on_usb_rx,
    .ctx = &link,
};

int main(void) {
    stdio_init_all();

    uint16_t boot_reason = watchdog_hw->scratch[0] == REBOOT_MARK ? BOOT_REBOOT
                           : watchdog_caused_reboot()               ? BOOT_WATCHDOG
                                                                    : BOOT_POWER_ON;
    watchdog_hw->scratch[0] = 0;

    // This image (the active slot up to the end of the binary): its CRC tells the phone which firmware runs.
    flash_rp2040_init(&flash, CFG_WATCHDOG_MS);
    fw_image_t running = {.size = (uint32_t)((uintptr_t)&__flash_binary_end - (FW_XIP_BASE + FW_ACTIVE))};
    running.crc = fw_crc(&flash, FW_ACTIVE, running.size);
    fwupdate_init(&fwupdate, &flash, running);
    printf("aerolink: fw %d.%d.%d (%lu bytes, crc %08lx, boot %u) starting, boot reason %u\n",
           AEROLINK_FW_VERSION_MAJOR, AEROLINK_FW_VERSION_MINOR, AEROLINK_FW_VERSION_PATCH,
           (unsigned long)running.size, (unsigned long)running.crc, fwupdate.boot, boot_reason);

#ifdef PICO_DEFAULT_LED_PIN
    gpio_init(PICO_DEFAULT_LED_PIN);
    gpio_set_dir(PICO_DEFAULT_LED_PIN, GPIO_OUT);
#endif

    pico_unique_board_id_t board_id;
    pico_get_unique_board_id(&board_id);
    static char serial[2 * PICO_UNIQUE_BOARD_ID_SIZE_BYTES + 1];
    pico_get_unique_board_id_string(serial, sizeof(serial));

    motor_init();
    link_init(&link, &control, &fwupdate, motor_caps(), board_id.id, boot_reason, usb_aoa_send, NULL);
    control_init(&control, now_ms(), link_on_control_event, &link);

    watchdog_enable(CFG_WATCHDOG_MS, true);
    usb_aoa_init(&usb_callbacks, serial);

    absolute_time_t next = get_absolute_time();
    while (true) {
        tuh_task();
        uint32_t now = now_ms();
        safety_tick(now);
        link_tick(&link, now, &feedback);

        if (link.reboot_requested) {  // DISARMED, checked by the link (SAFE-007)
            motor_set(0, 0, now);
            if (link.reboot_mode == REBOOT_BOOTSEL) {
                printf("aerolink: rebooting to BOOTSEL\n");
                reset_usb_boot(0, 0);
            }
            printf("aerolink: rebooting%s\n", fwupdate.staged ? " to install the new firmware" : "");
            watchdog_hw->scratch[0] = REBOOT_MARK;
            watchdog_reboot(0, 0, 0);
            while (true) {
                tight_loop_contents();
            }
        }

        // Sleep (WFE) until the next control tick, but serve USB events as soon as they arrive
        // so commands are not delayed by up to a full tick.
        next = delayed_by_ms(next, LOOP_PERIOD_MS);
        if (time_reached(next)) {
            next = get_absolute_time();  // overrun (e.g. enumeration): do not try to catch up
        }
        while (!time_reached(next) && !tuh_task_event_ready()) {
            best_effort_wfe_or_timeout(next);
        }
    }
}
