# `sparcv9-sun-solaris` and `x86_64-pc-solaris`

**Tier: 2**

Oracle Solaris 11.4 and illumos SVR4 based operating systems for 64-bit SPARC and x86 architectures.

## Requirements

The target can be cross or natively compiled with full support for `std`.  All host tools should build, but this is not tested.

## Building the target

The target can be built natively or cross compiled by enabling it for a `rustc` build in `config.toml`.  For example:

```toml
[build]
target = ["x86_64-pc-solaris"]
```

or:

```toml
[build]
target = ["sparcv9-sun-solaris"]
```

The target has been verified to build with the following Solaris packages:

```
# system headers
system/header@11.4,5.11-11.4.42.0.0.111.1:20211203T213657Z

# Core system libraries
system/library@11.4,5.11-11.4.42.0.0.111.1:20211203T214210Z

# ld.so, crt*
system/linker@11.4-11.4.0.0.1.15.0:20180817T003258Z

# libc
system/library/libc@11.4-11.4.42.0.0.111.1:20211203T214042Z

# libm
system/library/math@11.4-11.4.0.1.0.17.0:20200221T005425Z

# libucrypto
system/library/security/crypto@11.4,5.11-11.4.42.0.0.111.1:20211203T214125Z

# libssl v3
library/security/openssl-3@3,11.4-11.4.42.0.0.111.0:20211203T203706Z

# libz
library/zlib@1.2.11,11.4-11.4.42.0.0.111.0:20211203T203816Z
```

To link against OpenSSL, the location of the headers and libraries should be specified separately at build time as `OPENSSL_INCLUDE_DIR="${SYSROOT}/usr/openssl/3/include"` and `OPENSSL_LIB_DIR="${SYSROOT}/usr/openssl/3/lib/64"` respectively.

## Building Rust programs

Rust does not ship pre-compiled artifacts for this target. To compile for
this target, you will need to build Rust with the target enabled (see
"Building the target" above).

## Testing

Does the target support running binaries, or do binaries have varying
expectations that prevent having a standard way to run them? If users can run
binaries, can they do so in some common emulator, or do they need native
hardware? Does the target support running the Rust testsuite?

## Cross-compilation toolchains and C code

Does the target support C code? If so, what toolchain target should users use
to build compatible C code? (This may match the target triple, or it may be a
toolchain for a different target triple, potentially with specific options or
caveats.)
