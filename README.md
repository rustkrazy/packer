# packer

The rustkrazy image generator.

# Supported hardware

The images created by this program should work on the following machines:

* Raspberry Pi 3 (any model)
* Raspberry Pi 4 (any model)
* x86_64 hardware supporting legacy boot

UEFI support (for x86_64) is not planned, but contributions are welcome.

# System package requirements

For the packer to work you need to install the following packages:

* nasm
* squashfs-tools-ng
* musl
* (optional) clang
* (optional) kernel-headers-musl

If you want to build images for the Raspberry Pi, the aarch64 versions
of the musl related packages (musl itself and optionally clang and the kernel headers)
are required as well.

Clang and the kernel headers are needed for some small sys crates to compile.
Many crates will compile without them but there are a few that won't.

Then, add the musl target:

`rustup target add x86_64-unknown-linux-musl`

# Supported crates

Most Rust crates should compile if the packer is set up correctly.
Larger sys crates that dynamically link to system libraries
cannot easily be supported. Most notably OpenSSL is not supported.
Check if your dependencies offer Cargo features to disable OpenSSL
and replace it with rustls.

In general simple crates are more likely to work
but more complex crates can be built if the correct dependencies are installed.

# Building the packer

Make sure you have `cargo-make` installed:

```
cargo install cargo-make
```

Since the packer requires the compiled MBR for the x86_64 architecture
it needs to be built by running:

```
cargo make
```

if you want to build x86_64 images.

This assembles the MBR and builds the packer.

You may skip this step if you're only interested in building images
for the Raspberry Pi.
