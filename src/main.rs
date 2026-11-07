#![forbid(unsafe_code)]
use clap::{Args, Parser, Subcommand};
use gsk::{
    Error, Result,
    client::{Client, Config},
    wire,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tempfile::NamedTempFile;

#[derive(Parser)]
#[command(
    version,
    about = "GSK CNC file transfer over UDP — native Rust, no GSKComm DLL",
    after_help = "Compatibility: gsk <IP> <LOCAL-FILE> <O1234.CNC> [options] is an alias for send.\nUploads replace an existing remote file unless --no-clobber is specified."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Upload a saved NC program. Existing remote file is replaced.
    #[command(alias = "put")]
    Send {
        #[command(flatten)]
        network: Network,
        file: PathBuf,
        name: String,
        /// Read the file back and compare SHA-256.
        #[arg(long)]
        verify: bool,
        /// Refuse an existing remote name (preflight check, not an atomic lock).
        #[arg(long)]
        no_clobber: bool,
    },
    /// Download a program. Publish the destination only after successful completion.
    #[command(alias = "get")]
    Receive {
        #[command(flatten)]
        network: Network,
        name: String,
        file: PathBuf,
        /// Replace an existing local destination.
        #[arg(long)]
        force: bool,
    },
    /// Compare an existing remote program with a local file without uploading.
    Verify {
        #[command(flatten)]
        network: Network,
        file: PathBuf,
        name: String,
    },
    /// List the program directory.
    #[command(alias = "ls")]
    List {
        #[command(flatten)]
        network: Network,
    },
    /// Read program metadata (no upload).
    Stat {
        #[command(flatten)]
        network: Network,
        name: String,
    },
}

#[derive(Args)]
struct Network {
    /// Controller IPv4 address.
    ip: Ipv4Addr,
    #[arg(long, default_value_t=6000, value_parser=clap::value_parser!(u16).range(1..))]
    port: u16,
    /// Local UDP port. 0 asks the OS to choose an available port.
    #[arg(long, default_value_t = 0)]
    local_port: u16,
    #[arg(long, default_value_t=3000, value_parser=clap::value_parser!(u64).range(100..=60000))]
    timeout_ms: u64,
    /// Additional attempts for offset I/O, stat and mount; never replay open or directory reads.
    #[arg(long, default_value_t=2, value_parser=clap::value_parser!(u8).range(0..=10))]
    retries: u8,
    #[arg(long, default_value = "/user/NCPROG")]
    remote_dir: String,
    /// Validate local inputs without opening a network socket.
    #[arg(long)]
    dry_run: bool,
    /// Print packet operation, sequence and length (not file contents).
    #[arg(long, short)]
    verbose: bool,
}

impl Network {
    fn config(&self, cancelled: Arc<AtomicBool>) -> Config {
        let mut cfg = Config::new(self.ip);
        cfg.port = self.port;
        cfg.local_port = self.local_port;
        cfg.timeout = Duration::from_millis(self.timeout_ms);
        cfg.retries = self.retries;
        cfg.verbose = self.verbose;
        cfg.cancelled = cancelled;
        cfg
    }
    fn directory(&self) -> Result<&str> {
        let dir = self.remote_dir.trim_end_matches('/');
        wire::validate_path(dir)?;
        if self.ip.octets()[0] == 0 || self.ip.is_multicast() || self.ip.is_broadcast() {
            return Err(Error::Invalid("Expected a unicast IPv4 address".into()));
        }
        Ok(dir)
    }
    fn path(&self, name: &str) -> Result<String> {
        wire::program_path(self.directory()?, name)
    }
}

fn snapshot(path: &Path) -> Result<(NamedTempFile, u32, String)> {
    let mut source = File::open(path)
        .map_err(|e| Error::Invalid(format!("Cannot read {}: {e}", path.display())))?;
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > u32::MAX as u64 {
        return Err(Error::Invalid(
            "Source must be a regular file, 1..4294967295 bytes".into(),
        ));
    }
    let mut staged = NamedTempFile::new()?;
    let copied = io::copy(&mut source, staged.as_file_mut())?;
    if copied != metadata.len() {
        return Err(Error::Invalid("Source size changed while reading".into()));
    }
    let hash = hash_file(staged.as_file_mut())?;
    staged.as_file_mut().rewind()?;
    Ok((staged, copied as u32, hash))
}

fn hash_file(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify(client: &mut Client, remote: &str, expected: &str) -> Result<()> {
    let mut received = NamedTempFile::new()?;
    client.receive(remote, received.as_file_mut())?;
    let actual = hash_file(received.as_file_mut())?;
    if actual != expected {
        return Err(Error::Verification(format!(
            "{remote}: expected {expected}, received {actual}"
        )));
    }
    Ok(())
}

fn execute(cli: Cli, cancelled: Arc<AtomicBool>) -> Result<()> {
    match cli.command {
        Command::Send {
            network,
            file,
            name,
            verify: check,
            no_clobber,
        } => {
            let remote = network.path(&name)?;
            let (mut staged, length, hash) = snapshot(&file)?;
            println!(
                "{} -> {}:{} {remote} ({length} bytes)",
                file.display(),
                network.ip,
                network.port
            );
            println!("SHA-256: {hash}");
            if network.dry_run {
                println!("Dry run OK; no network traffic.");
                return Ok(());
            }
            let mut client = Client::connect(network.config(cancelled))?;
            if let Err(e) = client.send(&remote, staged.as_file_mut(), length, no_clobber) {
                if !matches!(e, Error::Invalid(_)) {
                    eprintln!(
                        "Upload was not confirmed; if writing had begun, the remote file may be partial."
                    );
                }
                return Err(e);
            }
            if check {
                println!("Reading back for verification...");
                if let Err(e) = verify(&mut client, &remote, &hash) {
                    eprintln!("Upload completed, but verification did not succeed.");
                    return Err(e);
                }
                println!("OK: uploaded and verified byte-for-byte (SHA-256).");
            } else {
                println!("OK: uploaded; controller acknowledged file close.");
            }
        }
        Command::Receive {
            network,
            name,
            file,
            force,
        } => {
            let remote = network.path(&name)?;
            if file.exists() && !force {
                return Err(Error::Invalid(format!(
                    "Destination exists: {} (use --force)",
                    file.display()
                )));
            }
            if file.is_dir() {
                return Err(Error::Invalid("Destination is a directory".into()));
            }
            let parent = file
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if !parent.is_dir() {
                return Err(Error::Invalid(format!(
                    "Destination directory does not exist: {}",
                    parent.display()
                )));
            }
            println!(
                "{}:{} {remote} -> {}",
                network.ip,
                network.port,
                file.display()
            );
            if network.dry_run {
                println!("Dry run OK; no network traffic.");
                return Ok(());
            }
            // Same-filesystem staging preserves any existing destination on transfer failure.
            let mut received = NamedTempFile::new_in(parent)?;
            let mut client = Client::connect(network.config(cancelled))?;
            let size = client.receive(&remote, received.as_file_mut())?;
            received.as_file_mut().flush()?;
            received.as_file().sync_all()?;
            let hash = hash_file(received.as_file_mut())?;
            let result = if force {
                received.persist(&file)
            } else {
                received.persist_noclobber(&file)
            };
            result.map_err(|e| Error::Io(e.error))?;
            println!("OK: received {size} bytes. SHA-256: {hash}");
        }
        Command::Verify {
            network,
            file,
            name,
        } => {
            let remote = network.path(&name)?;
            let (_snapshot, size, hash) = snapshot(&file)?;
            if network.dry_run {
                println!("Dry run OK: {size} bytes; SHA-256: {hash}; no network traffic.");
                return Ok(());
            }
            let mut client = Client::connect(network.config(cancelled))?;
            verify(&mut client, &remote, &hash)?;
            println!("OK: {remote} matches {}. SHA-256: {hash}", file.display());
        }
        Command::List { network } => {
            let directory = network.directory()?.to_owned();
            if network.dry_run {
                println!(
                    "Dry run OK: {} {directory}; no network traffic.",
                    network.ip
                );
                return Ok(());
            }
            let mut client = Client::connect(network.config(cancelled))?;
            let mut entries = client.list(&directory)?;
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            for entry in entries {
                println!(
                    "{:>10}  {} {}",
                    entry.size,
                    if entry.is_directory() { "DIR " } else { "FILE" },
                    entry.name.escape_default()
                );
            }
        }
        Command::Stat { network, name } => {
            let remote = network.path(&name)?;
            if network.dry_run {
                println!("Dry run OK: {} {remote}; no network traffic.", network.ip);
                return Ok(());
            }
            let mut client = Client::connect(network.config(cancelled))?;
            let stat = client.stat(&remote)?;
            println!("{remote}: {} bytes (mode 0x{:08x})", stat.size, stat.mode);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut arguments: Vec<_> = std::env::args_os().collect();
    if arguments
        .get(1)
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<Ipv4Addr>().ok())
        .is_some()
    {
        arguments.insert(1, "send".into());
    }
    let cli = Cli::parse_from(arguments);
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&cancelled);
    if let Err(e) = ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)) {
        eprintln!("Cannot install interrupt handler: {e}");
        return ExitCode::from(4);
    }
    match execute(cli, cancelled) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(e.exit_code())
        }
    }
}
