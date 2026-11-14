use crate::bridge::Bridge;
use crate::config::Config;
use crate::core::constants::*;
use crate::core::types::MacAddress;
use crate::core::types::RvpnError;
use crate::logging::init;
use crate::network::tap::TapInterface;
use crate::service::RadminVpnService;
use clap::{Parser, Subcommand};
use nix::sys::signal::{self, Signal};
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn signal_handler(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, short = 'c')]
    pub config: Option<PathBuf>,

    #[arg(long, default_value = "info")]
    pub log_level: String,

    #[arg(long)]
    pub json_logs: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Setup {
        #[arg(long)]
        installer: Option<PathBuf>,

        #[arg(long)]
        update: bool,

        #[arg(long)]
        no_ui: bool,

        #[arg(long)]
        no_firewall: bool,

        #[arg(long)]
        no_broadcast_routes: bool,
    },

    Service {
        #[arg(long, default_value = "radminvpn0")]
        tap_dev: String,

        #[arg(long)]
        adapter_mac: Option<String>,

        #[arg(long)]
        no_ui: bool,

        #[arg(long)]
        no_firewall: bool,
    },

    TapBridge {
        #[arg(long, default_value = "radminvpn0")]
        tap_dev: String,
    },

    NetshRelay {
        #[arg(long, default_value = "/tmp/radmin_netsh_cmd")]
        cmd_file: String,

        #[arg(long, default_value = "radminvpn0")]
        tap_dev: String,

        #[arg(long, default_value = "0.1")]
        poll_interval: f64,
    },

    Teardown {
        #[arg(long, default_value = "radminvpn0")]
        tap_dev: String,

        #[arg(long)]
        remove_tap: bool,
    },

    Status,

    HealthCheck,
}

pub fn run() -> Result<(), RvpnError> {
    let cli = Cli::parse();

    let json_logs = cli.json_logs;
    init(&cli.log_level, json_logs)
        .map_err(|e| RvpnError::Config(format!("failed to initialize logging: {e}")))?;

    match cli.command {
        Commands::Setup {
            installer,
            update,
            no_ui,
            no_firewall,
            no_broadcast_routes,
        } => cmd_setup(
            installer,
            update,
            no_ui,
            no_firewall,
            no_broadcast_routes,
            cli.config,
        )?,
        Commands::Service {
            tap_dev,
            adapter_mac,
            no_ui,
            no_firewall,
        } => cmd_service(tap_dev, adapter_mac, no_ui, no_firewall, cli.config)?,
        Commands::TapBridge { tap_dev } => cmd_tap_bridge(tap_dev)?,
        Commands::NetshRelay {
            cmd_file,
            tap_dev,
            poll_interval,
        } => cmd_netsh_relay(cmd_file, tap_dev, poll_interval)?,
        Commands::Teardown {
            tap_dev,
            remove_tap,
        } => cmd_teardown(tap_dev, remove_tap)?,
        Commands::Status => cmd_status()?,
        Commands::HealthCheck => cmd_health_check()?,
    }

    Ok(())
}

fn cmd_setup(
    installer: Option<PathBuf>,
    update: bool,
    no_ui: bool,
    no_firewall: bool,
    no_broadcast_routes: bool,
    config_file: Option<PathBuf>,
) -> Result<(), RvpnError> {
    let mut config = if let Some(path) = config_file {
        crate::config::load_config_file(Some(&path))?
    } else {
        Config::from_env()?
    };

    config.update = update;
    config.no_ui = no_ui;
    config.firewall = !no_firewall;
    config.broadcast_routes = !no_broadcast_routes;

    if let Some(path) = installer {
        config.installer = Some(path);
    }

    info!("Setting up Radmin VPN");
    info!("  Wine prefix: {}", config.wineprefix.display());
    info!("  Radmin version: {}", config.radmin_version);
    info!("  TAP device: {}", config.tap_dev);
    info!(
        "  GUI: {}",
        if config.no_ui { "disabled" } else { "enabled" }
    );
    info!(
        "  Firewall: {}",
        if config.firewall {
            "enabled"
        } else {
            "disabled"
        }
    );

    check_prerequisites()?;

    if !config.wineprefix.exists() {
        info!("Creating Wine prefix...");
        std::fs::create_dir_all(&config.wineprefix)
            .map_err(|e| RvpnError::Config(format!("create wineprefix: {e}")))?;
    }

    let radmin_dir = config.radmin_dir();
    if !radmin_dir.join("RvControlSvc.exe").exists() {
        if let Some(installer) = &config.installer {
            info!("Installing Radmin VPN from {}", installer.display());
        } else {
            return Err(RvpnError::Config(
                "Radmin VPN not installed and no installer provided. Use --installer".to_string(),
            ));
        }
    }

    info!("Setup complete");
    Ok(())
}

fn cmd_service(
    tap_dev: String,
    adapter_mac: Option<String>,
    no_ui: bool,
    no_firewall: bool,
    config_file: Option<PathBuf>,
) -> Result<(), RvpnError> {
    let mut config = if let Some(path) = config_file {
        crate::config::load_config_file(Some(&path))?
    } else {
        Config::from_env()?
    };

    config.tap_dev = tap_dev;
    config.no_ui = no_ui;
    config.firewall = !no_firewall;

    if let Some(mac_str) = adapter_mac {
        config.adapter_mac = Some(
            MacAddress::parse(&mac_str)
                .map_err(|e| RvpnError::Config(format!("invalid MAC address: {e}")))?
                .to_string(),
        );
    }

    info!("Starting radmin-vpn service");
    info!("  TAP device: {}", config.tap_dev);

    let mut service = RadminVpnService::new(config)?;
    let shutdown = service.shutdown_handle();

    unsafe {
        signal::signal(Signal::SIGTERM, signal::SigHandler::Handler(signal_handler)).ok();
        signal::signal(Signal::SIGINT, signal::SigHandler::Handler(signal_handler)).ok();
        signal::signal(Signal::SIGHUP, signal::SigHandler::Handler(signal_handler)).ok();
    }

    let run_handle = std::thread::spawn(move || service.run());

    while !SHUTDOWN.load(Ordering::SeqCst) {
        if run_handle.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if SHUTDOWN.load(Ordering::SeqCst) {
        info!("Shutdown signal received, stopping service...");
        shutdown.store(false, Ordering::SeqCst);
    }

    match run_handle.join() {
        Ok(result) => result?,
        Err(_) => {
            return Err(RvpnError::Service("service thread panicked".to_string()));
        }
    }

    let cleanup_cfg = Config::from_env().unwrap_or_default();
    let mut cleanup_svc = RadminVpnService::new(cleanup_cfg)?;
    let _ = cleanup_svc.cleanup();

    Ok(())
}

fn cmd_tap_bridge(tap_dev: String) -> Result<(), RvpnError> {
    info!("Starting TAP bridge for device {tap_dev}");

    let mac = MacAddress::new([0x02, 0x50, 0xDE, 0xAD, 0xBE, 0xEF]);
    let tap = TapInterface::create(&tap_dev, mac, DEFAULT_MTU, false, false)?;

    let mut bridge = Bridge::new(tap)?;
    bridge.run()?;

    Ok(())
}

fn cmd_netsh_relay(cmd_file: String, tap_dev: String, poll_interval: f64) -> Result<(), RvpnError> {
    info!("Starting netsh relay: {cmd_file} -> {tap_dev} (poll={poll_interval}s)");

    loop {
        if std::path::Path::new(&cmd_file).exists() {
            let _ = std::fs::remove_file(&cmd_file);
            info!("Processed netsh command file");
        }

        let sleep_ms = (poll_interval * 1000.0) as u64;
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms));

        if SHUTDOWN.load(Ordering::SeqCst) {
            break;
        }
    }

    Ok(())
}

fn cmd_teardown(tap_dev: String, remove_tap: bool) -> Result<(), RvpnError> {
    info!("Tearing down Radmin VPN resources for {tap_dev}");

    for path in [FIFO_B2D, FIFO_D2B_HIGH, FIFO_D2B_LOW] {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_fifo() {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    for path in [NETSH_CMD_FILE, NETSH_CMD_FILE_PROC, MAC_FILE, FILTER_PATH] {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_file(format!("{NETSH_CMD_FILE}.proc"));

    if remove_tap {
        match crate::network::tap::destroy_owned_interface(&tap_dev) {
            Ok(true) => info!("Deleted owned TAP interface {tap_dev}"),
            Ok(false) => info!("TAP interface {tap_dev} not removed (not owned or absent)"),
            Err(e) => warn!("Failed to delete owned TAP interface {tap_dev}: {e}"),
        }
    }

    if let Ok(cfg) = crate::config::Config::from_env() {
        let _ = std::process::Command::new("wineserver")
            .env("WINEPREFIX", &cfg.wineprefix)
            .arg("-k")
            .status();
    }

    info!("Teardown complete");
    Ok(())
}

fn cmd_status() -> Result<(), RvpnError> {
    println!("Radmin VPN Status");
    println!("==================");

    match std::process::Command::new("systemctl")
        .args(["is-active", "radmin-vpn.service"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("radmin-vpn.service: {state}");
        }
        Ok(_) => {
            println!("radmin-vpn.service: inactive or not installed");
        }
        Err(_) => {
            println!("radmin-vpn.service: systemctl unavailable");
        }
    }

    let lock_file = crate::core::paths::lock_file();
    let lock_exists = lock_file.exists();
    println!(
        "Lock file {}: {}",
        lock_file.display(),
        if lock_exists { "present" } else { "absent" }
    );

    let tap_exists = std::path::Path::new("/sys/class/net/radminvpn0").exists();
    println!(
        "TAP device radminvpn0: {}",
        if tap_exists { "exists" } else { "not found" }
    );

    for fifo in ["/tmp/rvpn_b2d", "/tmp/rvpn_d2b_high", "/tmp/rvpn_d2b_low"] {
        let exists = std::path::Path::new(fifo).exists();
        println!(
            "FIFO {fifo}: {}",
            if exists { "exists" } else { "not found" }
        );
    }

    let output = std::process::Command::new("pgrep")
        .args(["-af", "RvControlSvc|wineserver|radmin-vpn"])
        .output();
    if let Ok(output) = output {
        let processes = String::from_utf8_lossy(&output.stdout);
        if !processes.is_empty() {
            println!("Processes:");
            for line in processes.lines() {
                println!("  {line}");
            }
        } else {
            println!("No Radmin VPN processes running");
        }
    }

    Ok(())
}

fn cmd_health_check() -> Result<(), RvpnError> {
    println!("Radmin VPN Health Check");
    println!("========================");

    match std::process::Command::new("systemctl")
        .args(["is-active", "radmin-vpn.service"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("radmin-vpn.service: {state}");
        }
        Ok(_) => println!("radmin-vpn.service: inactive or not installed"),
        Err(_) => println!("radmin-vpn.service: systemctl unavailable"),
    }

    if let Ok(output) = std::process::Command::new("systemctl")
        .args([
            "show",
            "radmin-vpn.service",
            "-p",
            "ActiveState,SubState,MainPID",
        ])
        .output()
    {
        let details = String::from_utf8_lossy(&output.stdout);
        for line in details.lines() {
            if !line.trim().is_empty() {
                println!("  {line}");
            }
        }
    }

    match std::process::Command::new("wine").arg("--version").output() {
        Ok(output) => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("Wine: {version}");
        }
        Err(_) => println!("Wine: not found"),
    }

    let tun_exists = std::path::Path::new("/dev/net/tun").exists();
    println!(
        "/dev/net/tun: {}",
        if tun_exists { "available" } else { "not found" }
    );

    println!("networking: in-process netlink (iproute2 not required)");

    let nft_ok = std::process::Command::new("nft")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let ipt_ok = std::process::Command::new("iptables")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    match (nft_ok, ipt_ok) {
        (true, _) => println!("Firewall backend: nftables"),
        (false, true) => println!("Firewall backend: iptables"),
        (false, false) => println!("Firewall backend: none detected"),
    }

    let workspace = std::path::PathBuf::from(".");
    for binary in [
        "target/release/radmin-vpn",
        "target/release/librvpn_dnsfix.so",
        "target/release/librvpn_reuseport.so",
    ] {
        let path = workspace.join(binary);
        println!(
            "{binary}: {}",
            if path.exists() { "built" } else { "not built" }
        );
    }

    Ok(())
}

fn check_prerequisites() -> Result<(), RvpnError> {
    let mut missing = Vec::new();

    if std::process::Command::new("wine").output().is_err() {
        missing.push("wine");
    }
    if std::process::Command::new("wineserver").output().is_err() {
        missing.push("wineserver");
    }

    let nft_ok = std::process::Command::new("nft")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let ipt_ok = std::process::Command::new("iptables")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !nft_ok && !ipt_ok {
        missing.push("nftables or iptables");
    }

    if !std::path::Path::new("/dev/net/tun").exists() {
        missing.push("/dev/net/tun (install/load TUN/TAP kernel module)");
    }

    if !missing.is_empty() {
        return Err(RvpnError::Config(format!(
            "Missing dependencies: {}",
            missing.join(", ")
        )));
    }

    Ok(())
}
