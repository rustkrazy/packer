use anyhow::bail;
use cargo::core::compiler::{BuildConfig, CompileMode};
use cargo::core::SourceId;
use cargo::ops::CompileOptions;
use cargo::util::config::Config as CargoConfig;
use clap::Parser;
use fatfs::{FatType, FormatVolumeOptions};
use fscommon::StreamSlice;
use reqwest::Url;
use squashfs_ng::write::{
    Source as SqsSource, SourceData as SqsSourceData, SourceFile as SqsSourceFile,
    TreeProcessor as SqsTreeProcessor,
};
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, prelude::*, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

const MODE_DEVICE: u32 = 1 << 14;

#[allow(non_upper_case_globals)]
const KiB: u32 = 1024;
#[allow(non_upper_case_globals)]
const MiB: u32 = 1024 * KiB;

const KERNEL_BASE: &str = "https://github.com/rustkrazy/kernel/raw/master/";

#[derive(Debug, Parser)]
#[command(author = "The Rustkrazy Authors", version = "v0.1.0", about = "Generate a rustkrazy image.", long_about = None)]
struct Args {
    /// Output location of a full image.
    #[arg(short = 'o', long = "overwrite")]
    overwrite: String,
    /// Size of image file in bytes. Used if --overwrite is a file.
    #[arg(short = 'n', long = "size")]
    size: Option<u64>,
    /// Architecture of the device running the image. Supported: x86_64 rpi.
    #[arg(short = 'a', long = "architecture")]
    arch: String,
    /// Crates to install into the image.
    #[arg(short = 'c', long = "crates")]
    crates: Vec<String>,
    /// Crates to install from git.
    #[arg(short = 'g', long = "git")]
    git: Vec<String>,
    /// Init crate. rustkrazy_init is a reasonable default for most applications.
    #[arg(short = 'i', long = "init")]
    init: String,
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
    file.write_all(&(dev_size as u32 / 512 - 2048 - 256 * MiB / 512).to_le_bytes())?;

    // Partition 3 (unused)
    file.write_all(NOPART)?;

    // Partition 4 (unused)
    file.write_all(NOPART)?;

    file.write_all(SIGNATURE)?;

    println!("Partition table written successfully");
    Ok(())
}

fn partition(
    file: &mut File,
    dev_size: u64,
    arch: String,
    crates: Vec<String>,
    git: Vec<String>,
    init: String,
) -> anyhow::Result<()> {
    const ROOT_START: u64 = (2048 * 512 + 256 * MiB) as u64;
    let root_end = ROOT_START + (dev_size as u32 - 2048 * 512 - 256 * MiB) as u64;

    write_mbr_partition_table(file, dev_size)?;

    let mut boot_partition = StreamSlice::new(file.try_clone()?, 2048 * 512, ROOT_START - 1)?;
    let mut root_partition = StreamSlice::new(file.try_clone()?, ROOT_START, root_end)?;

    let buf = write_boot(&mut boot_partition, arch)?;
    write_mbr(file, &buf["vmlinuz"], &buf["cmdline.txt"])?;

    write_root(&mut root_partition, crates, git, init)?;

    Ok(())
}

fn partition_device(
    file: &mut File,
    overwrite: String,
    arch: String,
    crates: Vec<String>,
    git: Vec<String>,
    init: String,
) -> anyhow::Result<()> {
    let dev_size = device_size(file, overwrite)?;
    println!("Destination holds {} bytes", dev_size);

    partition(file, dev_size, arch, crates, git, init)?;

    Ok(())
}

fn write_boot(
    partition: &mut StreamSlice<File>,
    arch: String,
) -> anyhow::Result<BTreeMap<String, Vec<u8>>> {
    match arch.as_str() {
        "x86_64" => {}
        "rpi" => {}
        _ => bail!("invalid architecture (supported: x86_64 rpi)"),
    }

    let format_opts = FormatVolumeOptions::new().fat_type(FatType::Fat32);

    fatfs::format_volume(&mut *partition, format_opts)?;

    let fs = fatfs::FileSystem::new(partition, fatfs::FsOptions::new())?;
    let root_dir = fs.root_dir();

    let mut buf = BTreeMap::new();

    let mut copy = BTreeMap::new();

    copy.insert("vmlinuz", format!("vmlinuz-{}", arch));
    copy.insert("cmdline.txt", String::from("cmdline.txt"));

    for (dst, src) in copy {
        let mut file = root_dir.create_file(dst)?;

        let mut resp = reqwest::blocking::get(KERNEL_BASE.to_owned() + &src)?.error_for_status()?;

        buf.insert(dst.to_owned(), Vec::new());
        resp.copy_to(buf.get_mut(dst).unwrap())?;
        io::copy(&mut buf.get(dst).unwrap().as_slice(), &mut file)?;
    }

    println!("Boot filesystem created successfully");
    Ok(buf)
}

fn write_mbr(file: &mut File, kernel_buf: &[u8], cmdline_buf: &[u8]) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let kernel_offset: u32 = (buf
        .windows(kernel_buf.len())
        .position(|window| window == kernel_buf)
        .expect("can't find kernel (/vmlinuz) on boot partition")
        / 512
        + 1)
    .try_into()?;
    let cmdline_offset: u32 = (buf
        .windows(cmdline_buf.len())
        .position(|window| window == cmdline_buf)
        .expect("can't find cmdline (/cmdline.txt) on boot partition")
        / 512
        + 1)
    .try_into()?;

    let kernel_lba = kernel_offset + 2048;
    let cmdline_lba = cmdline_offset + 2048;

    let mut bootloader_params = Vec::new();
    bootloader_params.extend_from_slice(&kernel_lba.to_le_bytes());
    bootloader_params.extend_from_slice(&cmdline_lba.to_le_bytes());

    let mut bootloader_file = File::open("boot.bin")?;
    let mut bootloader_buf = Vec::new();
    bootloader_file.read_to_end(&mut bootloader_buf)?;
    bootloader_buf.resize(432, 0);

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&bootloader_buf[..432])?;
    file.write_all(&bootloader_params)?;

    println!("MBR written successfully");
    println!("MBR summary:");
    println!("  LBA: vmlinuz={}, cmdline.txt={}", kernel_lba, cmdline_lba);

    Ok(())
}

fn write_root(
    partition: &mut StreamSlice<File>,
    crates: Vec<String>,
    git: Vec<String>,
    init: String,
) -> anyhow::Result<()> {
    println!("Installing crates: {:?}", crates);
    println!("Installing git: {:?}", git);

    let tmp_dir = tempfile::tempdir()?;

    let mut cargo_opts = CargoConfig::default()?;
    let mut compile_opts = CompileOptions::new(&CargoConfig::default()?, CompileMode::Build)?;

    cargo_opts.configure(0, false, None, false, false, false, &None, &[], &[])?;
    compile_opts.build_config = BuildConfig::new(
        &CargoConfig::default()?,
        None,
        false,
        &[String::from("x86_64-unknown-linux-musl")],
        CompileMode::Build,
    )?;

    if !crates.is_empty() {
        cargo::ops::install(
            &cargo_opts,
            Some(tmp_dir.path().to_str().unwrap()), // root (output dir)
            crates.iter().map(|pkg| (pkg.as_str(), None)).collect(),
            SourceId::crates_io(&CargoConfig::default()?)?,
            false, // from_cwd
            &compile_opts,
            false, // force
            true,  // no_track
        )?;
    }

    for location in &git {
        let url = Url::parse(location)?;
        let pkg = url
            .path_segments()
            .unwrap()
            .next_back()
            .unwrap()
            .trim_end_matches(".git");

        cargo::ops::install(
            &cargo_opts,
            Some(tmp_dir.path().to_str().unwrap()), // root (output dir)
            vec![(pkg, None)],
            SourceId::from_url(&("git+".to_owned() + url.as_str()))?,
            false, // from_cwd
            &compile_opts,
            false, // force
            true,  // no_track
        )?;
    }

    let mut partition_buf = Vec::new();
    partition.read_to_end(&mut partition_buf)?;

    let mut tmp_file = tempfile::NamedTempFile::new()?;
    tmp_file.write_all(&partition_buf)?;

    let tree = SqsTreeProcessor::new(tmp_file.path())?;

    let mut crate_inodes = Vec::new();

    for pkg in &crates {
        let crate_path = tmp_dir.path().join("bin/".to_owned() + pkg);
        let crate_file = File::open(crate_path)?;

        crate_inodes.push(tree.add(SqsSourceFile {
            path: Path::new("/bin").join(if *pkg == init { "init" } else { pkg }),
            content: SqsSource {
                data: SqsSourceData::File(Box::new(crate_file)),
                uid: 0,
                gid: 0,
                mode: 0o755,
                modified: 0,
                xattrs: HashMap::new(),
                flags: 0,
            },
        })?);
    }

    for location in &git {
        let url = Url::parse(location)?;
        let pkg = url
            .path_segments()
            .unwrap()
            .next_back()
            .unwrap()
            .trim_end_matches(".git");

        let crate_path = tmp_dir.path().join("bin/".to_owned() + pkg);
        let crate_file = File::open(crate_path)?;

        crate_inodes.push(tree.add(SqsSourceFile {
            path: Path::new("/bin").join(if *pkg == init { "init" } else { pkg }),
            content: SqsSource {
                data: SqsSourceData::File(Box::new(crate_file)),
                uid: 0,
                gid: 0,
                mode: 0o755,
                modified: 0,
                xattrs: HashMap::new(),
                flags: 0,
            },
        })?);
    }

    let bin_inode = tree.add(SqsSourceFile {
        path: PathBuf::from("/bin"),
        content: SqsSource {
            data: SqsSourceData::Dir(Box::new(
                crates
                    .into_iter()
                    .map(move |pkg| {
                        if pkg == init {
                            String::from("init")
                        } else {
                            pkg
                        }
                    })
                    .map(OsString::from)
                    .zip(crate_inodes.into_iter()),
            )),
            uid: 0,
            gid: 0,
            mode: 0o755,
            modified: 0,
            xattrs: HashMap::new(),
            flags: 0,
        },
    })?;

    tree.add(SqsSourceFile {
        path: PathBuf::from("/"),
        content: SqsSource {
            data: SqsSourceData::Dir(Box::new(
                vec![(OsString::from("bin"), bin_inode)].into_iter(),
            )),
            uid: 0,
            gid: 0,
            mode: 0o755,
            modified: 0,
            xattrs: HashMap::new(),
            flags: 0,
        },
    })?;

    tree.finish()?;

    tmp_file.seek(SeekFrom::Start(0))?;
    partition.seek(SeekFrom::Start(0))?;
    io::copy(&mut tmp_file, partition)?;

    println!("Root filesystem created successfully");
    Ok(())
}

fn overwrite_device(
    file: &mut File,
    overwrite: String,
    arch: String,
    crates: Vec<String>,
    git: Vec<String>,
    init: String,
) -> anyhow::Result<()> {
    partition_device(file, overwrite, arch, crates, git, init)?;
    Ok(())
}

fn overwrite_file(
    file: &mut File,
    file_size: u64,
    arch: String,
    crates: Vec<String>,
    git: Vec<String>,
    init: String,
) -> anyhow::Result<()> {
    partition(file, file_size, arch, crates, git, init)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.arch.as_str() {
        "x86_64" => {}
        "rpi" => {}
        _ => bail!("invalid architecture (supported: x86_64 rpi)"),
    }

    let init_in_crates = args.crates.iter().any(|pkg| *pkg == args.init);
    let init_in_git = args.git.iter().any(|location| {
        let url = match Url::parse(location) {
            Ok(url) => url,
            Err(e) => {
                println!("Invalid git crate {}: {}", location, e);
                return false;
            }
        };

        let pkg = url
            .path_segments()
            .unwrap()
            .next_back()
            .unwrap()
            .trim_end_matches(".git");

        pkg == args.init
    });

    if !init_in_crates && !init_in_git {
        bail!("Init must be listed in crates to install");
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(args.overwrite.clone())?;

    if file.metadata()?.permissions().mode() & MODE_DEVICE != 0 {
        overwrite_device(
            &mut file,
            args.overwrite,
            args.arch,
            args.crates,
            args.git,
            args.init,
        )
    } else {
        match args.size {
            Some(v) => overwrite_file(&mut file, v, args.arch, args.crates, args.git, args.init),
            None => bail!("Files require --size to be specified"),
        }
    }
}
