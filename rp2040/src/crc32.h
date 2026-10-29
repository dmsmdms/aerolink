// CRC-32 (IEEE 802.3, reflected, the zlib / java.util.zip.CRC32 one) for firmware images (LINK-042).
// SDK-free: also built for the host unit tests.

#pragma once

#include <stddef.h>
#include <stdint.h>

// Continues `crc` (0 for a fresh one) over `data`; crc32_update(crc32_update(0, a), b) == CRC of a + b.
uint32_t crc32_update(uint32_t crc, const uint8_t *data, size_t len);
