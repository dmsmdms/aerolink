// AeroLink RP2040 firmware: bridge between the phone (USB, Android Open Accessory)
// and the hoverboard motor controller.

#include <stdio.h>

#include "hardware/watchdog.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "pico/unique_id.h"

#include "config.h"
#include "control.h"
#include "link.h"
#include "messages.h"
#include "motor.h"

#define LOOP_PERIOD_MS      5
#define LED_ARMED_PERIOD_MS 100
#define LED_IDLE_PERIOD_MS  1000

static control_t control;
static link_t link;

// Placeholder transport until the AOA host is in place: frames are dropped.
static bool tx_drop(const uint8_t *data, size_t len, void *ctx) {
    (void)data;
    (void)len;
    (void)ctx;
    return false;
}

static void led_update(uint32_t now_ms) {
#ifdef PICO_DEFAULT_LED_PIN
    uint32_t period = control_active(&control) ? LED_ARMED_PERIOD_MS : LED_IDLE_PERIOD_MS;
    gpio_put(PICO_DEFAULT_LED_PIN, (now_ms / (period / 2)) & 1);
#else
    (void)now_ms;
#endif
}

int main(void) {
    stdio_init_all();

    uint16_t boot_reason = watchdog_caused_reboot() ? BOOT_WATCHDOG : BOOT_POWER_ON;
    printf("aerolink: fw %d.%d.%d starting, boot reason %u\n", AEROLINK_FW_VERSION_MAJOR,
           AEROLINK_FW_VERSION_MINOR, AEROLINK_FW_VERSION_PATCH, boot_reason);

#ifdef PICO_DEFAULT_LED_PIN
    gpio_init(PICO_DEFAULT_LED_PIN);
    gpio_set_dir(PICO_DEFAULT_LED_PIN, GPIO_OUT);
#endif

    pico_unique_board_id_t board_id;
    pico_get_unique_board_id(&board_id);

    motor_init();
    uint32_t now = to_ms_since_boot(get_absolute_time());
    link_init(&link, &control, motor_caps(), board_id.id, boot_reason, tx_drop, NULL);
    control_init(&control, now, link_on_control_event, &link);

    watchdog_enable(CFG_WATCHDOG_MS, true);

    absolute_time_t next = get_absolute_time();
    while (true) {
        watchdog_update();
        now = to_ms_since_boot(get_absolute_time());

        motor_feedback_t fb;
        motor_feedback(&fb);
        control_set_faults(&control, fb.faults);
        control_tick(&control, now);
        motor_set(control_out_left(&control), control_out_right(&control), now);
        link_tick(&link, now, &fb);
        led_update(now);

        if (link.bootsel_requested) {
            printf("aerolink: rebooting to BOOTSEL\n");
            reset_usb_boot(0, 0);
        }

        // Low-power wait (WFE on a timer alarm) until the next control tick.
        next = delayed_by_ms(next, LOOP_PERIOD_MS);
        sleep_until(next);
    }
}
