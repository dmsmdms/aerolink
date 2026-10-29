#include "link.h"

#include <string.h>

#include "messages.h"

static void send(link_t *l, uint8_t type, uint8_t seq, const uint8_t *payload, size_t len) {
    if (!l->connected || !l->tx) {
        return;
    }
    uint8_t wire[PROTO_MAX_WIRE];
    size_t n = proto_encode(type, seq, payload, len, wire);
    if (n) {
        l->tx(wire, n, l->tx_ctx);
    }
}

static void send_own(link_t *l, uint8_t type, const uint8_t *payload, size_t len) {
    send(l, type, l->tx_seq++, payload, len);
}

static void send_event(link_t *l, uint8_t code, uint16_t arg) {
    uint8_t p[MSG_EVENT_LEN] = {code};
    proto_put_u16(p + 1, arg);
    send_own(l, MSG_EVENT, p, sizeof(p));
}

static void send_nack(link_t *l, uint8_t seq, uint8_t type, uint8_t reason) {
    uint8_t p[MSG_NACK_LEN] = {type, reason};
    send(l, MSG_NACK, seq, p, sizeof(p));
}

static void send_fw_ack(link_t *l, uint8_t seq, uint32_t offset, uint8_t status) {
    uint8_t p[MSG_FW_ACK_LEN];
    proto_put_u32(p, offset);
    p[4] = status;
    send(l, MSG_FW_ACK, seq, p, sizeof(p));
}

void link_init(link_t *l, control_t *ctl, fwupdate_t *fw, uint32_t caps, const uint8_t board_id[8],
               uint16_t boot_reason, link_tx_fn tx, void *tx_ctx) {
    *l = (link_t){
        .ctl = ctl,
        .fw = fw,
        .tx = tx,
        .tx_ctx = tx_ctx,
        .caps = caps,
        .boot_reason = boot_reason,
    };
    memcpy(l->board_id, board_id, sizeof(l->board_id));
    proto_decoder_reset(&l->dec);
}

void link_on_control_event(uint8_t code, uint16_t arg, void *ctx) {
    send_event((link_t *)ctx, code, arg);
}

void link_connected(link_t *l) {
    l->connected = true;
    l->hello_done = false;
    proto_decoder_reset(&l->dec);
}

void link_disconnected(link_t *l) {
    l->connected = false;
    l->hello_done = false;
    fwupdate_abort(l->fw);
    control_link_lost(l->ctl);  // SAFE-003
}

static bool length_ok(uint8_t type, uint8_t len) {
    switch (type) {
    case MSG_HELLO: return len == MSG_HELLO_LEN;
    case MSG_DRIVE: return len == MSG_DRIVE_LEN;
    case MSG_CONFIG: return len == MSG_CONFIG_LEN;
    case MSG_REBOOT: return len == MSG_REBOOT_LEN;
    case MSG_FW_BEGIN: return len == MSG_FW_BEGIN_LEN;
    case MSG_FW_DATA: return len >= MSG_FW_DATA_MIN_LEN;
    case MSG_ARM:
    case MSG_DISARM:
    case MSG_PING:
    case MSG_FW_END:
    case MSG_FW_CONFIRM: return len == 0;
    default: return false;  // unknown type
    }
}

static void handle(link_t *l, const proto_frame_t *f, uint32_t now_ms) {
    if (!length_ok(f->type, f->len)) {
        l->rx_errors++;
        return;
    }
    const uint8_t *p = f->payload;
    switch (f->type) {
    case MSG_HELLO: {
        fwupdate_abort(l->fw);  // a new handshake: whatever transfer ran is gone
        uint8_t a[MSG_HELLO_ACK_LEN] = {PROTO_VERSION, AEROLINK_FW_VERSION_MAJOR,
                                        AEROLINK_FW_VERSION_MINOR, AEROLINK_FW_VERSION_PATCH};
        proto_put_u32(a + 4, l->caps);
        memcpy(a + 8, l->board_id, sizeof(l->board_id));
        proto_put_u32(a + 16, l->fw->running.crc);
        a[20] = l->fw->boot;
        send(l, MSG_HELLO_ACK, f->seq, a, sizeof(a));
        l->hello_done = true;
        if (!l->boot_reported) {
            l->boot_reported = true;
            send_event(l, EVENT_BOOT, l->boot_reason);
        }
        // Let the phone learn the current state/faults right after the handshake.
        send_event(l, EVENT_STATE, l->ctl->state);
        if (l->ctl->faults) {
            send_event(l, EVENT_FAULTS, l->ctl->faults);
        }
        break;
    }
    case MSG_ARM: {
        // SAFE-007: no driving while an image is being received.
        uint8_t reason = l->hello_done && !l->fw->active ? control_arm(l->ctl, now_ms) : NACK_BAD_STATE;
        if (reason) {
            send_nack(l, f->seq, f->type, reason);
        } else {
            l->next_telemetry = now_ms;
        }
        break;
    }
    case MSG_DISARM:
        control_disarm(l->ctl);
        break;
    case MSG_DRIVE:
        control_drive(l->ctl, proto_get_i16(p), proto_get_i16(p + 2), proto_get_u16(p + 5), now_ms);
        break;
    case MSG_PING:
        control_ping(l->ctl, now_ms);
        send(l, MSG_PONG, f->seq, NULL, 0);
        break;
    case MSG_CONFIG:
        control_config(l->ctl, proto_get_u16(p), proto_get_u16(p + 2), proto_get_u16(p + 4),
                       proto_get_u16(p + 6));
        break;
    case MSG_REBOOT:
        if (proto_get_u32(p) != REBOOT_MAGIC || p[4] > REBOOT_BOOTSEL) {
            send_nack(l, f->seq, f->type, NACK_BAD_ARG);
        } else if (l->ctl->state != STATE_DISARMED) {
            send_nack(l, f->seq, f->type, NACK_BAD_STATE);
        } else {
            l->reboot_requested = true;
            l->reboot_mode = p[4];
        }
        break;
    case MSG_FW_BEGIN: {
        // LINK-040 / SAFE-007: only while DISARMED; ARM is refused from here until FW_END.
        uint8_t status = l->ctl->state == STATE_DISARMED
                             ? fwupdate_begin(l->fw, proto_get_u32(p), proto_get_u32(p + 4))
                             : FW_BAD_STATE;
        send_fw_ack(l, f->seq, 0, status);
        break;
    }
    case MSG_FW_DATA: {
        bool ack;
        uint8_t status = fwupdate_data(l->fw, proto_get_u32(p), p + 4, (uint8_t)(f->len - 4), &ack);
        if (status != FW_OK || ack) {
            send_fw_ack(l, f->seq, l->fw->written, status);  // LINK-041
        }
        break;
    }
    case MSG_FW_END: {
        uint8_t status = fwupdate_end(l->fw);
        send_fw_ack(l, f->seq, l->fw->written, status);
        break;
    }
    case MSG_FW_CONFIRM:
        send_fw_ack(l, f->seq, 0, fwupdate_confirm(l->fw));
        break;
    }
}

void link_rx(link_t *l, const uint8_t *data, size_t len, uint32_t now_ms) {
    proto_frame_t f;
    for (size_t i = 0; i < len; i++) {
        switch (proto_decoder_push(&l->dec, data[i], &f)) {
        case PROTO_FRAME: handle(l, &f, now_ms); break;
        case PROTO_ERROR: l->rx_errors++; break;
        case PROTO_NONE: break;
        }
    }
}

void link_tick(link_t *l, uint32_t now_ms, const motor_feedback_t *fb) {
    const control_t *c = l->ctl;
    if (!l->connected || !l->hello_done || !control_active(c) || c->telemetry_period_ms == 0) {
        return;  // LINK-020: no periodic traffic when idle
    }
    if ((int32_t)(now_ms - l->next_telemetry) < 0) {
        return;
    }
    l->next_telemetry = now_ms + c->telemetry_period_ms;

    uint8_t t[MSG_TELEMETRY_LEN] = {c->state};
    proto_put_u16(t + 1, c->faults);
    proto_put_u16(t + 3, fb->battery_mv);
    proto_put_u16(t + 5, (uint16_t)fb->speed_l);
    proto_put_u16(t + 7, (uint16_t)fb->speed_r);
    proto_put_u16(t + 9, (uint16_t)fb->temp_dc);
    proto_put_u16(t + 11, l->rx_errors);
    send_own(l, MSG_TELEMETRY, t, sizeof(t));
}
