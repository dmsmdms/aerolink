// Host unit tests for the SDK-free firmware modules (proto, control, link).
// Usage: test_main <path to protocol/vectors.txt>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "control.h"
#include "link.h"
#include "messages.h"
#include "proto.h"

static int failures;
static int checks;

#define CHECK(cond)                                                      \
    do {                                                                 \
        checks++;                                                        \
        if (!(cond)) {                                                   \
            failures++;                                                  \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__, __LINE__, #cond); \
        }                                                                \
    } while (0)

#define CHECK_EQ(a, b)                                                                   \
    do {                                                                                 \
        checks++;                                                                        \
        long _a = (long)(a), _b = (long)(b);                                             \
        if (_a != _b) {                                                                  \
            failures++;                                                                  \
            fprintf(stderr, "%s:%d: %s == %ld, expected %s == %ld\n", __FILE__, __LINE__, #a, _a, #b, _b); \
        }                                                                                \
    } while (0)

// ---------------------------------------------------------------------------------------------
// proto

static size_t unhex(const char *s, uint8_t *out, size_t cap) {
    if (strcmp(s, "-") == 0) {
        return 0;
    }
    size_t n = strlen(s) / 2;
    if (n > cap) {
        fprintf(stderr, "hex too long: %s\n", s);
        exit(2);
    }
    for (size_t i = 0; i < n; i++) {
        unsigned v;
        sscanf(s + 2 * i, "%2x", &v);
        out[i] = (uint8_t)v;
    }
    return n;
}

typedef struct {
    int frames;
    int errors;
    proto_frame_t last;
} decode_result_t;

static decode_result_t decode_all(const uint8_t *wire, size_t len) {
    proto_decoder_t d;
    proto_decoder_reset(&d);
    decode_result_t r = {0};
    for (size_t i = 0; i < len; i++) {
        switch (proto_decoder_push(&d, wire[i], &r.last)) {
        case PROTO_FRAME: r.frames++; break;
        case PROTO_ERROR: r.errors++; break;
        case PROTO_NONE: break;
        }
    }
    return r;
}

static void test_crc(void) {
    CHECK_EQ(proto_crc16((const uint8_t *)"123456789", 9), 0x29B1);  // CRC-16/CCITT-FALSE check value
}

static void test_vectors(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) {
        perror(path);
        exit(2);
    }
    char line[512];
    int ok_count = 0;
    int bad_count = 0;
    uint8_t stream[4096];
    size_t stream_len = 0;

    while (fgets(line, sizeof(line), f)) {
        char kind[8], name[64], a[256], b[256], c[256], d[256];
        if (line[0] == '#' || line[0] == '\n') {
            continue;
        }
        int n = sscanf(line, "%7s %63s %255s %255s %255s %255s", kind, name, a, b, c, d);
        if (strcmp(kind, "ok") == 0 && n == 6) {
            unsigned type = (unsigned)strtoul(a, NULL, 16);
            unsigned seq = (unsigned)strtoul(b, NULL, 10);
            uint8_t payload[PROTO_MAX_PAYLOAD + 8], wire[PROTO_MAX_WIRE + 8], out[PROTO_MAX_WIRE];
            size_t plen = unhex(c, payload, sizeof(payload));
            size_t wlen = unhex(d, wire, sizeof(wire));

            size_t olen = proto_encode((uint8_t)type, (uint8_t)seq, payload, plen, out);
            CHECK_EQ(olen, wlen);
            CHECK(olen == wlen && memcmp(out, wire, wlen) == 0);

            decode_result_t r = decode_all(wire, wlen);
            CHECK_EQ(r.frames, 1);
            CHECK_EQ(r.errors, 0);
            CHECK_EQ(r.last.type, type);
            CHECK_EQ(r.last.seq, seq);
            CHECK_EQ(r.last.len, plen);
            CHECK(memcmp(r.last.payload, payload, plen) == 0);
            if (r.frames != 1) {
                fprintf(stderr, "  vector: %s\n", name);
            }

            memcpy(stream + stream_len, wire, wlen);
            stream_len += wlen;
            ok_count++;
        } else if (strcmp(kind, "bad") == 0 && n >= 3) {
            uint8_t wire[256];
            size_t wlen = unhex(a, wire, sizeof(wire));
            decode_result_t r = decode_all(wire, wlen);
            CHECK_EQ(r.frames, 0);
            CHECK_EQ(r.errors, 1);
            if (r.frames || r.errors != 1) {
                fprintf(stderr, "  vector: %s\n", name);
            }
            bad_count++;
        }
    }
    fclose(f);
    CHECK(ok_count > 0);
    CHECK(bad_count > 0);

    // All good frames back to back decode in order, regardless of chunking.
    decode_result_t r = decode_all(stream, stream_len);
    CHECK_EQ(r.frames, ok_count);
    CHECK_EQ(r.errors, 0);
}

static void test_resync_after_garbage(void) {
    uint8_t wire[200];
    size_t n = 0;
    memset(wire, 0x55, 80);  // longer than any frame, no delimiter
    n += 80;
    n += proto_encode(MSG_PING, 9, NULL, 0, wire + n);
    decode_result_t r = decode_all(wire, n);
    CHECK_EQ(r.errors, 1);
    CHECK_EQ(r.frames, 0);  // garbage merged with the next frame up to its delimiter

    n = proto_encode(MSG_PING, 10, NULL, 0, wire);
    proto_decoder_t d;
    proto_decoder_reset(&d);
    proto_frame_t f;
    for (int i = 0; i < 80; i++) {
        proto_decoder_push(&d, 0x55, &f);
    }
    CHECK_EQ(proto_decoder_push(&d, 0, &f), PROTO_ERROR);
    proto_result_t last = PROTO_NONE;
    for (size_t i = 0; i < n; i++) {
        last = proto_decoder_push(&d, wire[i], &f);
    }
    CHECK_EQ(last, PROTO_FRAME);
    CHECK_EQ(f.seq, 10);
}

// ---------------------------------------------------------------------------------------------
// control

typedef struct {
    int count;
    uint8_t code[64];
    uint16_t arg[64];
} events_t;

static void record_event(uint8_t code, uint16_t arg, void *ctx) {
    events_t *e = ctx;
    if (e->count < 64) {
        e->code[e->count] = code;
        e->arg[e->count] = arg;
        e->count++;
    }
}

static int count_events(const events_t *e, uint8_t code) {
    int n = 0;
    for (int i = 0; i < e->count; i++) {
        n += e->code[i] == code;
    }
    return n;
}

// Runs control in 5 ms ticks, refreshing the command every 50 ms (20 Hz) while `drive` is set.
static uint32_t run(control_t *c, uint32_t now, uint32_t ms, int drive, int16_t speed, int16_t steer) {
    for (uint32_t t = 0; t < ms; t += 5) {
        now += 5;
        if (drive && t % 50 == 0) {
            control_drive(c, speed, steer, 250, now);
        }
        control_tick(c, now);
    }
    return now;
}

static void test_control_arm_drive_timeout(void) {
    control_t c;
    events_t ev = {0};
    uint32_t now = 1000;
    control_init(&c, now, record_event, &ev);
    control_config(&c, 1000, 1000, 200, 250);

    CHECK_EQ(c.state, STATE_DISARMED);
    control_drive(&c, 1000, 0, 250, now);  // ignored while disarmed
    now = run(&c, now, 100, 0, 0, 0);
    CHECK_EQ(control_out_left(&c), 0);

    CHECK_EQ(control_arm(&c, now), 0);
    CHECK_EQ(c.state, STATE_ARMED);
    now = run(&c, now, 500, 1, 1000, 0);
    // accel 1000 ‰/s for 0.5 s
    CHECK(control_out_left(&c) >= 480 && control_out_left(&c) <= 500);
    CHECK_EQ(control_out_left(&c), control_out_right(&c));
    now = run(&c, now, 1000, 1, 1000, 0);
    CHECK_EQ(control_out_left(&c), 1000);

    // Commands stop: timeout within 250 ms, stop at CFG_STOP_DECEL (3000 ‰/s -> ~334 ms), then DISARMED.
    ev.count = 0;
    now = run(&c, now, 250, 0, 0, 0);
    CHECK_EQ(count_events(&ev, EVENT_CMD_TIMEOUT), 1);
    CHECK(c.state == STATE_STOPPING || c.state == STATE_DISARMED);
    now = run(&c, now, 400, 0, 0, 0);
    CHECK_EQ(c.state, STATE_DISARMED);
    CHECK_EQ(control_out_left(&c), 0);
    CHECK_EQ(control_out_right(&c), 0);
}

static void test_control_ttl_keeps_armed(void) {
    control_t c;
    uint32_t now = 0;
    control_init(&c, now, NULL, NULL);
    control_config(&c, 1000, 2000, 200, 250);
    control_arm(&c, now);
    control_drive(&c, 500, 0, 50, now);
    now = run(&c, now, 40, 0, 0, 0);
    CHECK(control_out_left(&c) > 0);
    // ttl expired, PINGs keep the link alive: target drops to zero, still ARMED.
    for (int i = 0; i < 10; i++) {
        control_ping(&c, now);
        now = run(&c, now, 50, 0, 0, 0);
    }
    CHECK_EQ(c.state, STATE_ARMED);
    CHECK_EQ(control_out_left(&c), 0);
}

static void test_control_mixing_and_limits(void) {
    control_t c;
    uint32_t now = 0;
    control_init(&c, now, NULL, NULL);
    control_config(&c, 60000, 60000, 1, 60000);  // all above the caps
    CHECK_EQ(c.max_speed, 1000);
    CHECK_EQ(c.accel, 2000);
    CHECK_EQ(c.cmd_timeout_ms, 250);
    CHECK_EQ(c.telemetry_period_ms, 20);

    control_config(&c, 500, 2000, 200, 250);
    control_arm(&c, now);
    now = run(&c, now, 2000, 1, 1000, 1000);  // full right turn: left forward, right... clamped
    CHECK_EQ(control_out_left(&c), 500);       // (1000 + 1000) clamped to 1000, * 0.5
    CHECK_EQ(control_out_right(&c), 0);
    now = run(&c, now, 2000, 1, 0, -1000);    // spin left in place
    CHECK_EQ(control_out_left(&c), -500);
    CHECK_EQ(control_out_right(&c), 500);
    now = run(&c, now, 2000, 1, -32768, 0);   // out-of-range input clamped
    CHECK_EQ(control_out_left(&c), -500);
    CHECK_EQ(control_out_right(&c), -500);

    // Lowering max_speed applies to the running command.
    control_config(&c, 100, 2000, 200, 250);
    now = run(&c, now, 500, 0, 0, 0);
    CHECK(control_out_left(&c) >= -100);
}

static void test_control_reverse_brakes_with_stop_rate(void) {
    control_t c;
    uint32_t now = 0;
    control_init(&c, now, NULL, NULL);
    control_config(&c, 1000, 500, 200, 250);  // slow accel
    control_arm(&c, now);
    now = run(&c, now, 3000, 1, 1000, 0);
    CHECK_EQ(control_out_left(&c), 1000);
    now = run(&c, now, 340, 1, -1000, 0);  // 1000 ‰ at 3000 ‰/s ≈ 333 ms to zero
    CHECK(control_out_left(&c) <= 0);
    CHECK(control_out_left(&c) > -50);     // then accelerates backwards at the slow rate
}

static void test_control_link_lost_and_fault(void) {
    control_t c;
    events_t ev = {0};
    uint32_t now = 0;
    control_init(&c, now, record_event, &ev);
    control_config(&c, 1000, 2000, 200, 250);
    control_arm(&c, now);
    now = run(&c, now, 1000, 1, 1000, 0);
    control_link_lost(&c);
    CHECK_EQ(c.state, STATE_STOPPING);
    now = run(&c, now, 300, 0, 0, 0);  // SAFE-003 / acceptance: stopped within 300 ms from full speed...
    now = run(&c, now, 50, 0, 0, 0);   // ...plus one tick margin at 3000 ‰/s from 1000 ‰
    CHECK_EQ(c.state, STATE_DISARMED);

    // Critical fault: immediate zero, ARM refused until cleared, FAULT left only via DISARM.
    control_arm(&c, now);
    now = run(&c, now, 500, 1, 1000, 0);
    control_set_faults(&c, FAULT_MOTOR_LINK_LOST);
    CHECK_EQ(c.state, STATE_FAULT);
    CHECK_EQ(control_out_left(&c), 0);
    CHECK_EQ(control_arm(&c, now), NACK_FAULT_ACTIVE);
    control_set_faults(&c, 0);
    CHECK_EQ(c.state, STATE_FAULT);
    CHECK_EQ(control_arm(&c, now), NACK_BAD_STATE);
    control_disarm(&c);
    CHECK_EQ(c.state, STATE_DISARMED);
    CHECK_EQ(control_arm(&c, now), 0);

    // LOW_BATTERY is only a warning.
    control_set_faults(&c, FAULT_LOW_BATTERY);
    CHECK_EQ(c.state, STATE_ARMED);
    CHECK(count_events(&ev, EVENT_FAULTS) >= 3);
}

static void test_control_time_wrap(void) {
    control_t c;
    uint32_t now = 0xFFFFFF00u;
    control_init(&c, now, NULL, NULL);
    control_arm(&c, now);
    now = run(&c, now, 400, 1, 500, 0);  // crosses the 32-bit wrap
    CHECK_EQ(c.state, STATE_ARMED);
    CHECK(control_out_left(&c) > 0);
}

// ---------------------------------------------------------------------------------------------
// link

typedef struct {
    proto_decoder_t dec;
    int count;
    proto_frame_t frames[32];
} sink_t;

static bool sink_tx(const uint8_t *data, size_t len, void *ctx) {
    sink_t *s = ctx;
    for (size_t i = 0; i < len; i++) {
        proto_frame_t f;
        if (proto_decoder_push(&s->dec, data[i], &f) == PROTO_FRAME && s->count < 32) {
            s->frames[s->count++] = f;
        }
    }
    return true;
}

static int sink_find(const sink_t *s, uint8_t type) {
    for (int i = 0; i < s->count; i++) {
        if (s->frames[i].type == type) {
            return i;
        }
    }
    return -1;
}

static void phone_send(link_t *l, uint8_t type, uint8_t seq, const uint8_t *p, size_t len, uint32_t now) {
    uint8_t wire[PROTO_MAX_WIRE];
    size_t n = proto_encode(type, seq, p, len, wire);
    link_rx(l, wire, n, now);
}

static void test_link(void) {
    static const uint8_t board_id[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    control_t c;
    link_t l;
    sink_t s = {0};
    proto_decoder_reset(&s.dec);
    motor_feedback_t fb = {.battery_mv = 36000, .speed_l = 10, .speed_r = 20, .temp_dc = TEMP_UNKNOWN};
    uint32_t now = 0;

    link_init(&l, &c, CAP_MOTOR_SIMULATED, board_id, BOOT_WATCHDOG, sink_tx, &s);
    control_init(&c, now, link_on_control_event, &l);
    link_connected(&l);

    // ARM before HELLO is refused.
    phone_send(&l, MSG_ARM, 1, NULL, 0, now);
    CHECK_EQ(s.count, 1);
    CHECK_EQ(s.frames[0].type, MSG_NACK);
    CHECK_EQ(s.frames[0].seq, 1);
    CHECK_EQ(s.frames[0].payload[1], NACK_BAD_STATE);

    s.count = 0;
    uint8_t hello[1] = {PROTO_VERSION};
    phone_send(&l, MSG_HELLO, 2, hello, 1, now);
    int i = sink_find(&s, MSG_HELLO_ACK);
    CHECK(i >= 0);
    if (i >= 0) {
        CHECK_EQ(s.frames[i].seq, 2);
        CHECK_EQ(s.frames[i].len, MSG_HELLO_ACK_LEN);
        CHECK_EQ(s.frames[i].payload[0], PROTO_VERSION);
        CHECK_EQ(proto_get_u32(s.frames[i].payload + 4), CAP_MOTOR_SIMULATED);
        CHECK(memcmp(s.frames[i].payload + 8, board_id, 8) == 0);
    }
    i = sink_find(&s, MSG_EVENT);
    CHECK(i >= 0 && s.frames[i].payload[0] == EVENT_BOOT && proto_get_u16(s.frames[i].payload + 1) == BOOT_WATCHDOG);

    // Idle: no periodic traffic (LINK-020).
    s.count = 0;
    for (int k = 0; k < 100; k++) {
        now += 10;
        control_tick(&c, now);
        link_tick(&l, now, &fb);
    }
    CHECK_EQ(s.count, 0);

    // Wrong length and unknown type are counted, not acted upon.
    phone_send(&l, MSG_ARM, 3, hello, 1, now);
    phone_send(&l, 0x1F, 4, NULL, 0, now);
    CHECK_EQ(c.state, STATE_DISARMED);
    CHECK_EQ(l.rx_errors, 2);

    // ARM -> STATE event, telemetry starts at the default period.
    phone_send(&l, MSG_ARM, 5, NULL, 0, now);
    CHECK_EQ(c.state, STATE_ARMED);
    CHECK(sink_find(&s, MSG_EVENT) >= 0);
    s.count = 0;
    uint8_t drive[MSG_DRIVE_LEN];
    proto_put_u16(drive, 300);
    proto_put_u16(drive + 2, 0);
    drive[4] = SOURCE_LOCAL;
    proto_put_u16(drive + 5, 200);
    for (int k = 0; k < 100; k++) {  // 1 s
        now += 10;
        if (k % 5 == 0) {
            phone_send(&l, MSG_DRIVE, (uint8_t)k, drive, sizeof(drive), now);
        }
        control_tick(&c, now);
        link_tick(&l, now, &fb);
    }
    int telemetry = 0;
    for (int k = 0; k < s.count; k++) {
        if (s.frames[k].type == MSG_TELEMETRY) {
            telemetry++;
            CHECK_EQ(s.frames[k].len, MSG_TELEMETRY_LEN);
            CHECK_EQ(proto_get_u16(s.frames[k].payload + 3), 36000);
            CHECK_EQ(proto_get_u16(s.frames[k].payload + 11), 2);
        }
    }
    CHECK(telemetry >= 4 && telemetry <= 6);
    CHECK(control_out_left(&c) > 0);

    // PING -> PONG with the same seq.
    s.count = 0;
    phone_send(&l, MSG_PING, 77, NULL, 0, now);
    i = sink_find(&s, MSG_PONG);
    CHECK(i >= 0 && s.frames[i].seq == 77);

    // REBOOT_BOOTSEL refused while armed, accepted when disarmed with the right magic.
    uint8_t magic[4];
    proto_put_u32(magic, REBOOT_BOOTSEL_MAGIC);
    s.count = 0;
    phone_send(&l, MSG_REBOOT_BOOTSEL, 80, magic, 4, now);
    CHECK(sink_find(&s, MSG_NACK) >= 0);
    CHECK(!l.bootsel_requested);

    // USB detach stops the robot (SAFE-003).
    link_disconnected(&l);
    for (int k = 0; k < 40; k++) {
        now += 10;
        control_tick(&c, now);
    }
    CHECK_EQ(c.state, STATE_DISARMED);

    link_connected(&l);
    phone_send(&l, MSG_HELLO, 81, hello, 1, now);
    proto_put_u32(magic, 0x12345678);
    phone_send(&l, MSG_REBOOT_BOOTSEL, 82, magic, 4, now);
    CHECK(!l.bootsel_requested);
    proto_put_u32(magic, REBOOT_BOOTSEL_MAGIC);
    phone_send(&l, MSG_REBOOT_BOOTSEL, 83, magic, 4, now);
    CHECK(l.bootsel_requested);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s <protocol/vectors.txt>\n", argv[0]);
        return 2;
    }
    test_crc();
    test_vectors(argv[1]);
    test_resync_after_garbage();
    test_control_arm_drive_timeout();
    test_control_ttl_keeps_armed();
    test_control_mixing_and_limits();
    test_control_reverse_brakes_with_stop_rate();
    test_control_link_lost_and_fault();
    test_control_time_wrap();
    test_link();

    if (failures) {
        fprintf(stderr, "❌ %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("✅ %d checks passed\n", checks);
    return 0;
}
