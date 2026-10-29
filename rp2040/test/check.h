// Minimal check macros shared by the host test files.

#pragma once

#include <stdio.h>

extern int failures;
extern int checks;

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
