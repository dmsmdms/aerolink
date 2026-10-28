// USB link framing (docs/03-usb-link.md, LINK-010..013): COBS + CRC-16/CCITT-FALSE.
// No SDK dependencies: also built for the host unit tests.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define PROTO_VERSION 1

#define PROTO_MAX_FRAME   64                       // decoded: type + seq + payload + crc
#define PROTO_MAX_PAYLOAD (PROTO_MAX_FRAME - 4)
#define PROTO_MAX_ENCODED (PROTO_MAX_FRAME + 1)    // COBS overhead for <= 254 bytes, no delimiter
#define PROTO_MAX_WIRE    (PROTO_MAX_ENCODED + 1)  // + 0x00 delimiter

typedef struct {
    uint8_t type;
    uint8_t seq;
    uint8_t len;
    uint8_t payload[PROTO_MAX_PAYLOAD];
} proto_frame_t;

typedef struct {
    uint8_t buf[PROTO_MAX_ENCODED];
    size_t len;
    bool overflow;
} proto_decoder_t;

typedef enum {
    PROTO_NONE,   // need more bytes
    PROTO_FRAME,  // *out holds a CRC-valid frame
    PROTO_ERROR,  // malformed frame dropped (LINK-012)
} proto_result_t;

uint16_t proto_crc16(const uint8_t *data, size_t len);

// Encodes a frame for the wire (COBS + trailing 0x00). Returns the wire length, 0 if payload is too long.
size_t proto_encode(uint8_t type, uint8_t seq, const uint8_t *payload, size_t len,
                    uint8_t out[PROTO_MAX_WIRE]);

void proto_decoder_reset(proto_decoder_t *d);

// Feeds one received byte. On PROTO_FRAME the frame is written to *out.
proto_result_t proto_decoder_push(proto_decoder_t *d, uint8_t byte, proto_frame_t *out);

// Little-endian field helpers.
static inline uint16_t proto_get_u16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }
static inline int16_t proto_get_i16(const uint8_t *p) { return (int16_t)proto_get_u16(p); }
static inline uint32_t proto_get_u32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static inline void proto_put_u16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}
static inline void proto_put_u32(uint8_t *p, uint32_t v) {
    proto_put_u16(p, (uint16_t)v);
    proto_put_u16(p + 2, (uint16_t)(v >> 16));
}
