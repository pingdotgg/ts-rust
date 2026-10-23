/* memfast.c from lust-lang (https://github.com/t3dotgg/lust-lang), the file
   src/runtime/memfast.c at commit 6ced2b86fd66280cf262b6c8d6b0ff61c625d26b. The text
   after this header is that file without change; its comment describes lust's build.

   Copyright (c) 2026 Theo Browne
   SPDX-License-Identifier: MIT
   License: licenses/memfast-LICENSE.txt (NOTICE.md, "memfast").

   ts-rust links it into the linux-x64 tsc (static musl) in place of musl's memcpy,
   memmove, memcmp, bcmp and memchr: crates/ts_goport/scripts/memfast.sh. */
/* Fast memcpy, memmove, memcmp, bcmp and memchr for musl release builds (src/cc.rs
   `memfast_unit`; the release driver links this unit next to mimalloc). musl's memcmp
   and memchr are byte loops, and zig's compiler_rt memcpy and memmove (which a zig musl
   link uses) move a word at a time: a release tsgo spent 2.3 G instructions in them on
   the TypeORM check, where glibc's vector forms do the same work in about a tenth.
   These use 16-byte vectors on x86_64 (SSE2, which every x86_64 has) with unaligned head
   and tail blocks, and 8-byte words elsewhere. Results are libc's (memcmp returns the
   difference of the first two bytes that differ), and no function reads a byte outside
   the ranges it is given. The C compiler must not turn a loop here back into a call of
   the same function: `MF_FN` asks clang for no builtins and gcc for no loop patterns.
   The unit has no includes (a release unit names its own types). */
typedef __SIZE_TYPE__ mf_size;
typedef __UINTPTR_TYPE__ mf_uptr;
typedef __UINT64_TYPE__ mf_u64;
typedef __UINT32_TYPE__ mf_u32;
typedef unsigned char mf_u8;
/* Unaligned word access through types of alignment 1 (units build with
   -fno-strict-aliasing). */
typedef mf_u64 mf_u64u __attribute__((aligned(1)));
typedef mf_u32 mf_u32u __attribute__((aligned(1)));
#define MF_LD8(p) (*(const mf_u64u *)(const void *)(p))
#define MF_ST8(p, v) (*(mf_u64u *)(void *)(p) = (v))
#define MF_LD4(p) (*(const mf_u32u *)(const void *)(p))
#define MF_ST4(p, v) (*(mf_u32u *)(void *)(p) = (v))

#if defined(__clang__)
#define MF_FN __attribute__((no_builtin))
#elif defined(__GNUC__)
#define MF_FN __attribute__((optimize("no-tree-loop-distribute-patterns")))
#else
#define MF_FN
#endif

#if defined(__x86_64__) && defined(__SSE2__) && (defined(__clang__) || defined(__GNUC__)) && !defined(MF_NO_V16)
#define MF_V16 1
typedef char mf_v16 __attribute__((vector_size(16)));
typedef char mf_v16u __attribute__((vector_size(16), aligned(1)));
#define MF_LDV(p) (*(const mf_v16u *)(const void *)(p))
#define MF_STV(p, v) (*(mf_v16u *)(void *)(p) = (v))
/* Bit i set when byte i of `v` (a comparison result, bytes 0 or -1) is set. */
#define MF_MASK(v) ((unsigned)__builtin_ia32_pmovmskb128((v)))
#endif

#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define MF_LE 1
#endif

/* Up to 64 bytes: every load before any store, so memmove takes it for any overlap. */
MF_FN static inline __attribute__((always_inline)) void mf_copy_small(mf_u8 *d, const mf_u8 *s, mf_size n) {
    if (n <= 16) {
        if (n >= 8) {
            mf_u64 a = MF_LD8(s), b = MF_LD8(s + n - 8);
            MF_ST8(d, a);
            MF_ST8(d + n - 8, b);
        } else if (n >= 4) {
            mf_u32 a = MF_LD4(s), b = MF_LD4(s + n - 4);
            MF_ST4(d, a);
            MF_ST4(d + n - 4, b);
        } else if (n) {
            mf_u8 a = s[0], b = s[n >> 1], c = s[n - 1];
            d[0] = a;
            d[n >> 1] = b;
            d[n - 1] = c;
        }
        return;
    }
#ifdef MF_V16
    if (n <= 32) {
        mf_v16 a = MF_LDV(s), b = MF_LDV(s + n - 16);
        MF_STV(d, a);
        MF_STV(d + n - 16, b);
        return;
    }
    mf_v16 a = MF_LDV(s), b = MF_LDV(s + 16), c = MF_LDV(s + n - 32), e = MF_LDV(s + n - 16);
    MF_STV(d, a);
    MF_STV(d + 16, b);
    MF_STV(d + n - 32, c);
    MF_STV(d + n - 16, e);
#else
    mf_u64 w[8];
    mf_size k = 0;
    for (; k + 8 <= n; k += 8) w[k >> 3] = MF_LD8(s + k);
    mf_u64 t = MF_LD8(s + n - 8);
    for (mf_size j = 0; j < k; j += 8) MF_ST8(d + j, w[j >> 3]);
    MF_ST8(d + n - 8, t);
#endif
}

/* More than 64 bytes, front to back: right for disjoint ranges and for d below s. The
   last 64 bytes are read first, as the loop may overwrite them when d is below s. */
MF_FN static inline __attribute__((always_inline)) void mf_copy_fwd(mf_u8 *d, const mf_u8 *s, mf_size n) {
#ifdef MF_V16
    mf_v16 t0 = MF_LDV(s + n - 64), t1 = MF_LDV(s + n - 48), t2 = MF_LDV(s + n - 32), t3 = MF_LDV(s + n - 16);
    for (mf_size i = 0; n - i > 64; i += 64) {
        mf_v16 a = MF_LDV(s + i), b = MF_LDV(s + i + 16), c = MF_LDV(s + i + 32), e = MF_LDV(s + i + 48);
        MF_STV(d + i, a);
        MF_STV(d + i + 16, b);
        MF_STV(d + i + 32, c);
        MF_STV(d + i + 48, e);
    }
    MF_STV(d + n - 64, t0);
    MF_STV(d + n - 48, t1);
    MF_STV(d + n - 32, t2);
    MF_STV(d + n - 16, t3);
#else
    mf_u64 t = MF_LD8(s + n - 8);
    for (mf_size i = 0; n - i > 8; i += 8) MF_ST8(d + i, MF_LD8(s + i));
    MF_ST8(d + n - 8, t);
#endif
}

/* More than 64 bytes, back to front, for d above s: the first 64 bytes are read first. */
MF_FN static inline __attribute__((always_inline)) void mf_copy_bwd(mf_u8 *d, const mf_u8 *s, mf_size n) {
#ifdef MF_V16
    mf_v16 h0 = MF_LDV(s), h1 = MF_LDV(s + 16), h2 = MF_LDV(s + 32), h3 = MF_LDV(s + 48);
    for (mf_size i = n; i > 64; i -= 64) {
        mf_v16 a = MF_LDV(s + i - 64), b = MF_LDV(s + i - 48), c = MF_LDV(s + i - 32), e = MF_LDV(s + i - 16);
        MF_STV(d + i - 64, a);
        MF_STV(d + i - 48, b);
        MF_STV(d + i - 32, c);
        MF_STV(d + i - 16, e);
    }
    MF_STV(d, h0);
    MF_STV(d + 16, h1);
    MF_STV(d + 32, h2);
    MF_STV(d + 48, h3);
#else
    mf_u64 h = MF_LD8(s);
    for (mf_size i = n; i > 8; i -= 8) MF_ST8(d + i - 8, MF_LD8(s + i - 8));
    MF_ST8(d, h);
#endif
}

MF_FN void *memcpy(void *d, const void *s, mf_size n) {
    if (n <= 64) mf_copy_small((mf_u8 *)d, (const mf_u8 *)s, n);
    else mf_copy_fwd((mf_u8 *)d, (const mf_u8 *)s, n);
    return d;
}

MF_FN void *memmove(void *d, const void *s, mf_size n) {
    if (n <= 64) mf_copy_small((mf_u8 *)d, (const mf_u8 *)s, n);
    else if ((mf_uptr)d - (mf_uptr)s >= (mf_uptr)n) mf_copy_fwd((mf_u8 *)d, (const mf_u8 *)s, n);
    else mf_copy_bwd((mf_u8 *)d, (const mf_u8 *)s, n);
    return d;
}

/* The first index below n where a and b differ, from index i on, or n. */
MF_FN static inline __attribute__((always_inline)) mf_size mf_diff(const mf_u8 *a, const mf_u8 *b, mf_size i, mf_size n) {
#ifdef MF_V16
    for (; n - i >= 16; i += 16) {
        unsigned m = MF_MASK(MF_LDV(a + i) == MF_LDV(b + i)) ^ 0xFFFFu;
        if (m) return i + (mf_size)__builtin_ctz(m);
    }
    if (i < n && n >= 16) {
        /* The last 16 bytes; the ones below i are equal. */
        mf_size t = n - 16;
        unsigned m = MF_MASK(MF_LDV(a + t) == MF_LDV(b + t)) ^ 0xFFFFu;
        return m ? t + (mf_size)__builtin_ctz(m) : n;
    }
#endif
#ifdef MF_LE
    for (; n - i >= 8; i += 8) {
        mf_u64 x = MF_LD8(a + i) ^ MF_LD8(b + i);
        if (x) return i + (mf_size)(__builtin_ctzll(x) >> 3);
    }
#endif
    for (; i < n; i++)
        if (a[i] != b[i]) return i;
    return n;
}

MF_FN int memcmp(const void *a, const void *b, mf_size n) {
    const mf_u8 *x = (const mf_u8 *)a, *y = (const mf_u8 *)b;
    mf_size i = mf_diff(x, y, 0, n);
    return i < n ? (int)x[i] - (int)y[i] : 0;
}

/* bcmp(a, b, n): 0 when equal, else not 0 (the C compiler emits it for memcmp() == 0). */
MF_FN int bcmp(const void *a, const void *b, mf_size n) {
    return mf_diff((const mf_u8 *)a, (const mf_u8 *)b, 0, n) < n;
}

MF_FN void *memchr(const void *s, int c, mf_size n) {
    const mf_u8 *p = (const mf_u8 *)s;
    mf_u8 b = (mf_u8)c;
    mf_size i = 0;
#ifdef MF_V16
    if (n >= 16) {
        mf_v16 v = (mf_v16){0} + (char)b;
        for (; n - i >= 64; i += 64) {
            mf_v16 e0 = MF_LDV(p + i) == v, e1 = MF_LDV(p + i + 16) == v, e2 = MF_LDV(p + i + 32) == v, e3 = MF_LDV(p + i + 48) == v;
            if (MF_MASK(e0 | e1 | e2 | e3)) {
                mf_u64 m = (mf_u64)MF_MASK(e0) | (mf_u64)MF_MASK(e1) << 16 | (mf_u64)MF_MASK(e2) << 32 | (mf_u64)MF_MASK(e3) << 48;
                return (void *)(p + i + (mf_size)__builtin_ctzll(m));
            }
        }
        for (; n - i >= 16; i += 16) {
            unsigned m = MF_MASK(MF_LDV(p + i) == v);
            if (m) return (void *)(p + i + (mf_size)__builtin_ctz(m));
        }
        if (i < n) {
            /* The last 16 bytes, without the ones below i (already checked). */
            mf_size t = n - 16;
            unsigned m = MF_MASK(MF_LDV(p + t) == v) & (0xFFFFu << (unsigned)(i - t));
            if (m) return (void *)(p + t + (mf_size)__builtin_ctz(m));
        }
        return 0;
    }
#endif
    for (; i < n; i++)
        if (p[i] == b) return (void *)(p + i);
    return 0;
}
