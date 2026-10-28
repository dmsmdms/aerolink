#include "proto.h"

#include <string.h>

uint16_t proto_crc16(const uint8_t *data, size_t len) {
    uint16_t crc = 0xFFFF;
    for (size_t i = 0; i < len; i++) {
        crc ^= (uint16_t)data[i] << 8;
        for (int bit = 0; bit < 8; bit++) {
            crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021) : (uint16_t)(crc << 1);
        }
    }
    return crc;
}

// Raw frames are <= PROTO_MAX_FRAME (< 254) bytes, so a single COBS block never hits the 0xFF limit
// except at the end; the generic loop still handles it.
static size_t cobs_encode(const uint8_t *in, size_t len, uint8_t *out) {
    size_t code_idx = 0;
    size_t o = 1;
    uint8_t code = 1;
    for (size_t i = 0; i < len; i++) {
        if (in[i] == 0) {
            out[code_idx] = code;
            code_idx = o++;
            code = 1;
        } else {
            out[o++] = in[i];
            if (++code == 0xFF) {
                out[code_idx] = code;
                code_idx = o++;
                code = 1;
            }
        }
    }
    out[code_idx] = code;
    return o;
}

// Returns the decoded length, or -1 if the input is not valid COBS or does not fit.
static int cobs_decode(const uint8_t *in, size_t len, uint8_t *out, size_t cap) {
    size_t i = 0;
    size_t o = 0;
    while (i < len) {
        uint8_t code = in[i++];
        if (code == 0 || i + code - 1 > len) {
            return -1;
        }
        for (uint8_t k = 1; k < code; k++) {
            if (o >= cap) {
                return -1;
            }
            out[o++] = in[i++];
        }
        if (code != 0xFF && i < len) {
            if (o >= cap) {
                return -1;
            }
            out[o++] = 0;
        }
    }
    return (int)o;
}

size_t proto_encode(uint8_t type, uint8_t seq, const uint8_t *payload, size_t len,
                    uint8_t out[PROTO_MAX_WIRE]) {
    if (len > PROTO_MAX_PAYLOAD) {
        return 0;
    }
    uint8_t raw[PROTO_MAX_FRAME];
    raw[0] = type;
    raw[1] = seq;
    if (len) {
        memcpy(raw + 2, payload, len);
    }
    proto_put_u16(raw + 2 + len, proto_crc16(raw, 2 + len));
    size_t n = cobs_encode(raw, len + 4, out);
    out[n++] = 0;
    return n;
}

void proto_decoder_reset(proto_decoder_t *d) {
    d->len = 0;
    d->overflow = false;
}

proto_result_t proto_decoder_push(proto_decoder_t *d, uint8_t byte, proto_frame_t *out) {
    if (byte != 0) {
        if (d->len < sizeof(d->buf)) {
            d->buf[d->len++] = byte;
        } else {
            d->overflow = true;
        }
        return PROTO_NONE;
    }

    // Delimiter: decode whatever was collected.
    bool overflow = d->overflow;
    size_t enc_len = d->len;
    proto_decoder_reset(d);
    if (overflow || enc_len == 0) {
        return PROTO_ERROR;
    }

    uint8_t raw[PROTO_MAX_FRAME];
    int n = cobs_decode(d->buf, enc_len, raw, sizeof(raw));
    if (n < 4) {
        return PROTO_ERROR;
    }
    size_t body = (size_t)n - 2;
    if (proto_crc16(raw, body) != proto_get_u16(raw + body)) {
        return PROTO_ERROR;
    }
    out->type = raw[0];
    out->seq = raw[1];
    out->len = (uint8_t)(body - 2);
    memcpy(out->payload, raw + 2, out->len);
    return PROTO_FRAME;
}
