# `x86_64-unknown-dragonfly`

**Tier: 3**

[DragonFly BSD] 4.4BSD-based UNIX-like operating system for the x86-64 architecture.

[DragonFly BSD]: https://www.dragonflybsd.org/

## Requirements

The target can be natively or cross compiled with full support for `std`.  To compile tools like `cargo` that require SSL support, OpenSSL should be installed from DPorts. 

## Building the target

The target can be built natively or cross compiled by enabling it for a `rustc` build in `config.toml`.  For example:

```toml
[build]
target = ["x86_64-unknown-dragonfly"]
```

When cross compiling the C and C++ compilers must be specified in `config.toml`.  For example:

```toml
[target.x86_64-unknown-dragonfly]
cc = "x86_64-unknown-gcc"
cxx = "x86_64-unknown-g++"
```

To build `cargo` and any other utilities that require SSL support, location of the SSL libraries can be specified via the `OPENSSL_DIR` environment variable e.g. `OPENSSL_DIR=${SYSROOT}/usr/local`.

## Building Rust programs

Rust does not ship pre-compiled artifacts for this target. To compile for
this target, you will either need to build Rust with the target enabled (see
"Building the target" above), or install a pre-built binary from DPorts.

Due to #114376, rust versions 1.70 through 1.72 will not compile for this target with the default linker, `ld.gold`.  `ld.bfd` is required and can be specified by setting `RUSTFLAGS` to include `-C linker-arg=-fuse-ld=bfd` at build time.

## Testing

Does the target support running binaries, or do binaries have varying
expectations that prevent having a standard way to run them? If users can run
binaries, can they do so in some common emulator, or do they need native
hardware? Does the target support running the Rust testsuite?

## Cross-compilation toolchains and C code

The target can be cross-compiled with either GCC or LLVM (although the latter is mostly untested). 
