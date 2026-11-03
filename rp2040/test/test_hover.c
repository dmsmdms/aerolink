// Host tests for hoverbus (Gen2.x REMOTE_UARTBUS). Reference frames come from tools/hoverboard_uart.py, the
// encoder that drove the real boards on the bench (docs/11-hoverboard.md).

#include <string.h>

#include "check.h"
#include "hoverbus.h"

static const uint8_t CMD_0_100[] = {0x2f, 0x00, 0x00, 0x64, 0x00, 0x20, 0xc2, 0x93};
static const uint8_t CMD_1_M300[] = {0x2f, 0x00, 0x01, 0xd4, 0xfe, 0x20, 0x47, 0x2b};
// slave 1, -0.58 km/h, 29.56 V, -0.12 A, odometer -67
static const uint8_t REPLY_1[] = {0xcd, 0xab, 0x01, 0xc6, 0xff, 0x8c, 0x0b, 0xf4,
                                  0xff, 0xbd, 0xff, 0xff, 0xff, 0xa7, 0xbf};

static int feed(hover_parser_t *p, const uint8_t *data, size_t len, hover_reply_t *out) {
    int n = 0;
    for (size_t i = 0; i < len; i++) {
        n += hover_parse(p, data[i], out);
    }
    return n;
}

static void test_frames(void) {
    CHECK_EQ(hover_crc16((const uint8_t *)"123456789", 9), 0x31C3);
    uint8_t cmd[HOVER_CMD_LEN];
    hover_command(cmd, 0, 100, HOVER_STATE_LED_BATTERY);
    CHECK(memcmp(cmd, CMD_0_100, sizeof(cmd)) == 0);
    hover_command(cmd, 1, -300, HOVER_STATE_LED_BATTERY);
    CHECK(memcmp(cmd, CMD_1_M300, sizeof(cmd)) == 0);

    hover_parser_t p;
    hover_reply_t r;
    hover_parser_init(&p);
    CHECK_EQ(feed(&p, REPLY_1, sizeof(REPLY_1), &r), 1);
    CHECK_EQ(r.slave, 1);
    CHECK_EQ(r.speed, -58);
    CHECK_EQ(r.volt, 2956);
    CHECK_EQ(r.amp, -12);
    CHECK_EQ(r.odom, -67);
}

static void test_parser_resync(void) {
    hover_parser_t p;
    hover_reply_t r;
    hover_parser_init(&p);
    // Garbage, a lone start byte, a truncated reply, then a good one.
    static const uint8_t junk[] = {0x00, 0xcd, 0x12, 0xab, 0xcd, 0xab, 0x01, 0x02};
    CHECK_EQ(feed(&p, junk, sizeof(junk), &r), 0);
    CHECK_EQ(feed(&p, REPLY_1, sizeof(REPLY_1), &r), 1);
    CHECK_EQ(r.odom, -67);

    // A corrupted reply is dropped, the next one is found.
    uint8_t bad[sizeof(REPLY_1)];
    memcpy(bad, REPLY_1, sizeof(bad));
    bad[7] ^= 1;
    CHECK_EQ(feed(&p, bad, sizeof(bad), &r), 0);
    CHECK_EQ(feed(&p, REPLY_1, sizeof(REPLY_1), &r), 1);
    // Back to back.
    CHECK_EQ(feed(&p, REPLY_1, sizeof(REPLY_1), &r) + feed(&p, REPLY_1, sizeof(REPLY_1), &r), 2);
}

static void rx_reply(hover_side_t *s, const uint8_t *frame, uint32_t now) {
    for (size_t i = 0; i < HOVER_REPLY_LEN; i++) {
        hover_side_rx(s, frame[i], now);
    }
}

static void test_side_tx(void) {
    hover_side_t s;
    uint8_t cmd[HOVER_CMD_LEN];
    hover_side_init(&s, 1, true, 1000);
    CHECK(hover_side_tx(&s, 300, 1000, cmd));
    CHECK(memcmp(cmd, CMD_1_M300, sizeof(cmd)) == 0);  // inverted side: +300 goes out as -300
    CHECK(!hover_side_tx(&s, 300, 1000 + HOVER_TX_PERIOD_MS - 1, cmd));
    CHECK(hover_side_tx(&s, 300, 1000 + HOVER_TX_PERIOD_MS, cmd));
    // A long stall sends one command, not a burst.
    CHECK(hover_side_tx(&s, 0, 5000, cmd));
    CHECK(!hover_side_tx(&s, 0, 5001, cmd));
    // Out-of-range commands are clamped to the firmware's range.
    CHECK(hover_side_tx(&s, -5000, 6000, cmd));
    CHECK_EQ((int16_t)(cmd[3] | cmd[4] << 8), HOVER_SPEED_MAX);
}

static void test_side_link(void) {
    hover_side_t s;
    hover_side_init(&s, 1, true, 0);
    CHECK(hover_side_check(&s, HOVER_BOOT_GRACE_MS - 1));  // boot grace
    CHECK(!hover_side_check(&s, HOVER_BOOT_GRACE_MS));     // never answered

    rx_reply(&s, REPLY_1, 3000);
    CHECK(hover_side_check(&s, 3000));
    CHECK_EQ(hover_side_speed(&s), 58 * 1000 / HOVER_FULL_SPEED_KMH100);  // inverted back to + forward
    CHECK(hover_side_check(&s, 3000 + HOVER_LINK_TIMEOUT_MS - 1));
    CHECK(!hover_side_check(&s, 3000 + HOVER_LINK_TIMEOUT_MS));

    // A reply from the other board (swapped cables) does not count.
    hover_side_init(&s, 0, false, 0);
    rx_reply(&s, REPLY_1, 10);
    CHECK(!s.seen);
    CHECK(!hover_side_check(&s, HOVER_BOOT_GRACE_MS));

    // Time wrap.
    hover_side_init(&s, 1, false, 0xFFFFFF00u);
    rx_reply(&s, REPLY_1, 0xFFFFFFF0u);
    CHECK(hover_side_check(&s, 0x10));
    CHECK_EQ(hover_side_speed(&s), -58 * 1000 / HOVER_FULL_SPEED_KMH100);
}

void test_hover(void) {
    test_frames();
    test_parser_resync();
    test_side_tx();
    test_side_link();
}
