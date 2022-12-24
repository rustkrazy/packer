use anyhow::bail;
use clap::Parser;
use fatfs::{FatType, FormatVolumeOptions};
use fscommon::StreamSlice;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

const MODE_DEVICE: u32 = 1 << 14;

#[allow(non_upper_case_globals)]
const KiB: u32 = 1024;
#[allow(non_upper_case_globals)]
const MiB: u32 = 1024 * KiB;

#[derive(Debug, Parser)]
#[command(author = "The Rustkrazy Authors", version = "v0.1.0", about = "Generate a rustkrazy image.", long_about = None)]
struct Args {
    /// Output location of a full image.
    #[arg(short = 'o', long = "overwrite")]
    overwrite: String,
    /// Size of image file in bytes. Used if --overwrite is a file.
    #[arg(short = 'n', long = "size")]
    size: Option<u64>,
}

#[cfg(target_os = "linux")]
fn device_size(file: &File, path: String) -> anyhow::Result<u64> {
    use nix::ioctl_read;

    const BLKGETSIZE64_CODE: u8 = 0x12;
    const BLKGETSIZE64_SEQ: u8 = 114;
    ioctl_read!(ioctl_blkgetsize64, BLKGETSIZE64_CODE, BLKGETSIZE64_SEQ, u64);

    let fd = file.as_raw_fd();

    let mut dev_size = 0;
    let dev_size_ptr = &mut dev_size as *mut u64;

    unsafe {
        match ioctl_blkgetsize64(fd, dev_size_ptr) {
            Ok(_) => {}
            Err(_) => bail!("{} does not seem to be a device", path),
        }
    }

    Ok(dev_size)
}

fn write_mbr_partition_table(file: &mut File, dev_size: u64) -> anyhow::Result<()> {
    const NOPART: &[u8] = &[0; 16];
    const INACTIVE: &[u8] = &[0x00];
    const ACTIVE: &[u8] = &[0x80];
    const INVALID_CHS: &[u8] = &[0xFF, 0xFF, 0xFE]; // Causes sector values to be used
    const FAT: &[u8] = &[0xc];
    const LINUX: &[u8] = &[0x83];
    const SQUASHFS: &[u8] = LINUX;
    const SIGNATURE: &[u8] = &[0x55, 0xAA];

    file.write_all(&[0; 446])?; // Boot code

    // Partition 1: boot
    file.write_all(ACTIVE)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(FAT)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(&2048_u32.to_le_bytes())?; // Start at sector 2048
    file.write_all(&(256 * MiB / 512).to_le_bytes())?; // 256 MiB in size

    // Partition 2 rootfs
    file.write_all(INACTIVE)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(SQUASHFS)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(&(2048 + 256 * MiB / 512).to_le_bytes())?;
    file.write_all(&(dev_size as u32 / 512 - 8192 - 256 * MiB / 512).to_le_bytes())?;

    // Partition 3 (unused)
    file.write_all(NOPART)?;

    // Partition 4 (unused)
    file.write_all(NOPART)?;

    file.write_all(SIGNATURE)?;

    println!("Partition table written successfully");
    Ok(())
}

fn partition(file: &mut File, dev_size: u64) -> anyhow::Result<()> {
    write_mbr_partition_table(file, dev_size)?;

    Ok(())
}

fn partition_device(file: &mut File, overwrite: String) -> anyhow::Result<()> {
    let dev_size = device_size(file, overwrite)?;
    println!("Destination holds {} bytes", dev_size);

    partition(file, dev_size)?;

    Ok(())
}

fn copy_file(dst: &mut fatfs::File<&mut StreamSlice<File>>, src: &mut File) -> anyhow::Result<()> {
    let mut buf = Vec::new();

    src.read_to_end(&mut buf)?;
    dst.write_all(&buf)?;

    Ok(())
}

fn write_boot(partition: &mut StreamSlice<File>) -> anyhow::Result<()> {
    let format_opts = FormatVolumeOptions::new().fat_type(FatType::Fat32);

    fatfs::format_volume(&mut *partition, format_opts)?;

    let kernel_dir = Path::new(".");

    let fs = fatfs::FileSystem::new(partition, fatfs::FsOptions::new())?;
    let root_dir = fs.root_dir();

    let copy = ["vmlinuz", "cmdline.txt"];
    for path in copy {
        let mut file = root_dir.create_file(path)?;
        copy_file(&mut file, &mut File::open(kernel_dir.join(path))?)?;
    }

    println!("Boot filesystem created successfully");
    Ok(())
}

fn write_mbr(file: &mut File) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let kernel_dir = Path::new(".");

    let mut kernel_file = File::open(kernel_dir.join("vmlinuz"))?;
    let mut kernel_buf = Vec::new();
    kernel_file.read_to_end(&mut kernel_buf)?;

    let mut cmdline_file = File::open(kernel_dir.join("cmdline.txt"))?;
    let mut cmdline_buf = Vec::new();
    cmdline_file.read_to_end(&mut cmdline_buf)?;

    let kernel_offset: u32 = buf
        .windows(kernel_buf.len())
        .position(|window| window == kernel_buf)
        .expect("can't find kernel (/vmlinuz) on boot partition")
        .try_into()?;
    let cmdline_offset: u32 = buf
        .windows(cmdline_buf.len())
        .position(|window| window == cmdline_buf)
        .expect("can't find cmdline (/cmdline.txt) on boot partition")
        .try_into()?;

    let kernel_lba = kernel_offset + 2048;
    let cmdline_lba = cmdline_offset + 2048;

    let mut bootloader_params = Vec::new();
    bootloader_params.extend_from_slice(&kernel_lba.to_le_bytes());
    bootloader_params.extend_from_slice(&cmdline_lba.to_le_bytes());
    eprintln!("{}", bootloader_params.len());

    let mut bootloader_file = File::open("boot.bin")?;
    let mut bootloader_buf = Vec::new();
    bootloader_file.read_to_end(&mut bootloader_buf)?;
    bootloader_buf.resize(432, 0);

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bootloader_buf[..432])?;
    file.write_all(&bootloader_params)?;

    Ok(())
}

fn overwrite_device(file: &mut File, overwrite: String) -> anyhow::Result<()> {
    partition_device(file, overwrite)?;

    let mut boot_partition = StreamSlice::new(
        file.try_clone()?,
        2048 * 512,
        (2048 * 512 + 256 * MiB - 1).into(),
    )?;

    write_boot(&mut boot_partition)?;
    write_mbr(file)?;

    Ok(())
}

fn overwrite_file(file: &mut File, file_size: u64) -> anyhow::Result<()> {
    partition(file, file_size)?;

    let mut boot_partition = StreamSlice::new(
        file.try_clone()?,
        2048 * 512,
        (2048 * 512 + 256 * MiB - 1).into(),
    )?;

    write_boot(&mut boot_partition)?;
    write_mbr(file)?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(args.overwrite.clone())?;

    if file.metadata()?.permissions().mode() & MODE_DEVICE != 0 {
        overwrite_device(&mut file, args.overwrite)
    } else {
        match args.size {
            Some(v) => overwrite_file(&mut file, v),
            None => bail!("Files require --size to be specified"),
        }
    }
}
