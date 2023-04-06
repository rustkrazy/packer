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

# Usage

Invoke the packer with `-h` or `--help` to view a list of all available options.

## --overwrite

The overwrite argument specifies where to write the image to.
It can be a device file (loop or real) as long as you have permission
to write to it.

To grant yourself write access to a device, use the following command (as root):

```
setfacl -m u:<USERNAME>:rw <DEVICE_FILE>
```

If the target file is a device this is sufficient. If it's an image file
you have to pass `-n` or `--size`. You can use a tool like `fdisk`
to get the size of your image file in bytes and use that number with the packer.
This is needed because the packer needs to know the size of the image
for partitioning but can't call ioctl on regular files to get it.

Alternatively you can create a loop device for your image
and write to it instead.

## --crates

This is a list of crate names to install from the crates.io registry.
To install more than one crate you can pass this argument multiple times.
Example:

```
rustkrazy_packer -o /dev/some_device -c crate1 -c crate2
```

The name is assumed to be the same as the name of the resulting binary.
As a result you may need to swap hyphens with underscores or vice versa.

## --git

This is similar to `--crates`, but allows you to install crates
from any git repository. Just like `--crates` it can be passed
multiple times. It expects the argument to be in the following format:

```
<REPO_URL>%<CRATE_NAME>
```

where `REPO_URL` is the location of the repository
and `CRATE_NAME` is the name of the crate defined in its Cargo.toml.

Like `--crates` this argument expects `CRATE_NAME` to match the binary file name.

Example:

```
rustkrazy_packer -o /dev/some_device -g https://github.com/rustkrazy/init%rustkrazy_init
```

## --init

Use this flag to tell the packer which one of the crates is the init system.
It will be moved to `/bin/init` in the image and start on boot.
It is responsible for starting the other binaries as needed.

The argument is the name of one of the crates listed in `--crates`
or `--git`.

The rustkrazy project comes with its own small init system
that oneshots all services and logs startup information
to the primary display. It also mounts essential file systems
like /boot or /proc.

You can however use your own init system if it's better for your use case.

If you're only installing a single crate an init may not be needed,
allowing you to set the init argument to that crate.
However this is not guruanteed to work since the program can fail
if it tries to access pseudo file systems that have not been mounted.

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
