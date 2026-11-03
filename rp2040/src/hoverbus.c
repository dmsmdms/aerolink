#include "hoverbus.h"

#include <string.h>

#define START_CMD   '/'
#define START_REPLY 0xABCD

static void put_u16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

static uint16_t get_u16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

static bool reached(uint32_t now, uint32_t deadline) { return (int32_t)(now - deadline) >= 0; }

uint16_t hover_crc16(const uint8_t *data, size_t len) {
    uint16_t crc = 0;
    for (size_t i = 0; i < len; i++) {
        crc ^= (uint16_t)(data[i] << 8);
        for (int b = 0; b < 8; b++) {
            crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021) : (uint16_t)(crc << 1);
        }
    }
    return crc;
}

void hover_command(uint8_t out[HOVER_CMD_LEN], uint8_t slave, int16_t speed, uint8_t state) {
    out[0] = START_CMD;
    out[1] = 0;  // data type 0: SerialServer2Hover
    out[2] = slave;
    put_u16(out + 3, (uint16_t)speed);
    out[5] = state;
    put_u16(out + 6, hover_crc16(out, 6));
}

void hover_parser_init(hover_parser_t *p) { p->len = 0; }

// Drops bytes up to the next possible start of a reply.
static void resync(hover_parser_t *p) {
    size_t i = 1;
    while (i < p->len && p->buf[i] != (START_REPLY & 0xFF)) {
        i++;
    }
    memmove(p->buf, p->buf + i, p->len - i);
    p->len -= i;
}

bool hover_parse(hover_parser_t *p, uint8_t byte, hover_reply_t *out) {
    p->buf[p->len++] = byte;
    for (;;) {
        if (p->len >= 1 && p->buf[0] != (START_REPLY & 0xFF)) {
            resync(p);
            continue;
        }
        if (p->len >= 2 && p->buf[1] != (START_REPLY >> 8)) {
            resync(p);
            continue;
        }
        if (p->len < HOVER_REPLY_LEN) {
            return false;
        }
        if (get_u16(p->buf + 13) != hover_crc16(p->buf, 13)) {
            resync(p);
            continue;
        }
        out->slave = p->buf[2];
        out->speed = (int16_t)get_u16(p->buf + 3);
        out->volt = get_u16(p->buf + 5);
        out->amp = (int16_t)get_u16(p->buf + 7);
        out->odom = (int32_t)((uint32_t)get_u16(p->buf + 9) | (uint32_t)get_u16(p->buf + 11) << 16);
        p->len = 0;
        return true;
    }
}

void hover_side_init(hover_side_t *s, uint8_t slave, bool invert, uint32_t now_ms) {
    memset(s, 0, sizeof(*s));
    s->slave = slave;
    s->invert = invert;
    hover_parser_init(&s->parser);
    s->next_tx_ms = now_ms;
    s->grace_until_ms = now_ms + HOVER_BOOT_GRACE_MS;
    s->alive = true;  // until the grace ends without a reply
}

bool hover_side_tx(hover_side_t *s, int16_t speed, uint32_t now_ms, uint8_t out[HOVER_CMD_LEN]) {
    if (!reached(now_ms, s->next_tx_ms)) {
        return false;
    }
    // A late loop does not send a burst to catch up.
    s->next_tx_ms = reached(now_ms, s->next_tx_ms + HOVER_TX_PERIOD_MS) ? now_ms + HOVER_TX_PERIOD_MS
                                                                        : s->next_tx_ms + HOVER_TX_PERIOD_MS;
    int32_t v = speed < -HOVER_SPEED_MAX ? -HOVER_SPEED_MAX : speed > HOVER_SPEED_MAX ? HOVER_SPEED_MAX : speed;
    hover_command(out, s->slave, (int16_t)(s->invert ? -v : v), HOVER_STATE_LED_BATTERY);
    return true;
}

void hover_side_rx(hover_side_t *s, uint8_t byte, uint32_t now_ms) {
    hover_reply_t r;
    if (hover_parse(&s->parser, byte, &r) && r.slave == s->slave) {
        s->last = r;
        s->last_rx_ms = now_ms;
        s->seen = true;
    }
}

bool hover_side_check(hover_side_t *s, uint32_t now_ms) {
    if (s->seen) {
        s->alive = now_ms - s->last_rx_ms < HOVER_LINK_TIMEOUT_MS;
    } else {
        s->alive = !reached(now_ms, s->grace_until_ms);
    }
    return s->alive;
}

int16_t hover_side_speed(const hover_side_t *s) {
    int32_t v = (int32_t)s->last.speed * 1000 / HOVER_FULL_SPEED_KMH100;
    v = s->invert ? -v : v;
    return (int16_t)(v < INT16_MIN ? INT16_MIN : v > INT16_MAX ? INT16_MAX : v);
}
