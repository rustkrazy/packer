use anyhow::bail;
use clap::Parser;
use std::fs::File;
use std::io::Write;
use std::os::unix::io::AsRawFd;

#[derive(Debug, Parser)]
#[command(author = "The Rustkrazy Authors", version = "v0.1.0", about = "Generate a rustkrazy image.", long_about = None)]
struct Args {
    /// Output location of a full image.
    #[arg(short = 'o', long = "overwrite")]
    overwrite: String,
}

#[cfg(target_os = "linux")]
fn device_size(file: &std::fs::File, path: String) -> anyhow::Result<u64> {
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

fn write_mbr_partition_table(file: &mut std::fs::File, dev_size: u64) -> anyhow::Result<()> {
    const NOPART: &[u8] = &[0; 16];
    const BOOTABLE: &[u8] = &[0x80];
    const INVALID_CHS: &[u8] = &[0xFF, 0xFF, 0xFE]; // Causes sector values to be used
    const FAT: &[u8] = &[0xc];
    const SIGNATURE: &[u8] = &[0x55, 0xAA];

    #[allow(non_upper_case_globals)]
    const KiB: u32 = 1024;
    #[allow(non_upper_case_globals)]
    const MiB: u32 = 1024 * KiB;

    file.write_all(&[0; 446])?; // Boot code

    // Partition 1
    file.write_all(BOOTABLE)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(FAT)?;
    file.write_all(INVALID_CHS)?;
    file.write_all(&2048_u32.to_le_bytes())?; // Start at sector 2048
    file.write_all(&(256 * MiB / 512).to_le_bytes())?; // 256 MiB in size

    // Partition 2 (unused)
    file.write_all(NOPART)?;

    // Partition 3 (unused)
    file.write_all(NOPART)?;

    // Partition 4 (unused)
    file.write_all(NOPART)?;

    file.write_all(SIGNATURE)?;

    Ok(())
}

fn partition(file: &mut std::fs::File, dev_size: u64) -> anyhow::Result<()> {
    write_mbr_partition_table(file, dev_size)?;

    Ok(())
}

fn partition_device(overwrite: String) -> anyhow::Result<()> {
    let mut file = File::create(overwrite.clone())?;

    let dev_size = device_size(&file, overwrite)?;
    println!("Destination holds {} bytes", dev_size);

    partition(&mut file, dev_size)?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    partition_device(args.overwrite)?;

    Ok(())
}
