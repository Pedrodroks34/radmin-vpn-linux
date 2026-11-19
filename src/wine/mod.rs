use crate::config::Config;
use crate::core::types::RvpnError;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

pub struct WineProcessManager {
    config: Config,
    wine_prefix: PathBuf,
    build_dir: PathBuf,
}

impl WineProcessManager {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            wine_prefix: config.wineprefix.clone(),
            build_dir: config.build_dir.clone(),
        }
    }

    fn wine_cmd(&self, program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.env("WINEPREFIX", &self.wine_prefix);
        if let Some(session_env) = detect_session_display_env() {
            debug!("injecting graphical session environment into wine process");
            for (key, value) in session_env {
                cmd.env(key, value);
            }
        }
        cmd
    }

    pub fn radmin_dir(&self) -> PathBuf {
        self.config.radmin_dir()
    }

    pub fn wine_prefix(&self) -> &PathBuf {
        &self.wine_prefix
    }

    pub fn kill_wine(&self) -> Result<(), RvpnError> {
        let status = self
            .wine_cmd("wineserver")
            .arg("-k")
            .status()
            .map_err(|e| RvpnError::Wine(format!("wineserver -k failed: {e}")))?;
        debug!("wineserver -k exited: {status:?}");
        Ok(())
    }

    pub fn kill_processes(&self, pattern: &str) -> Result<(), RvpnError> {
        let status = Command::new("pkill").args(["-f", pattern]).status();
        match status {
            Ok(s) => debug!("pkill -f {pattern} exited: {s:?}"),
            Err(e) => warn!("pkill -f {pattern} failed: {e}"),
        }
        Ok(())
    }

    pub fn wineboot_init(&self) -> Result<(), RvpnError> {
        let status = self
            .wine_cmd("wineboot")
            .arg("--init")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| RvpnError::Wine(format!("wineboot --init failed: {e}")))?;
        if status.success() {
            info!("Wine prefix initialized");
        } else {
            warn!("wineboot --init exited with {status:?}");
        }
        Ok(())
    }

    pub fn install_radmin(&self, installer_path: &Path) -> Result<(), RvpnError> {
        info!("Running Radmin installer: {}", installer_path.display());

        let status = self
            .wine_cmd("wine")
            .arg(installer_path)
            .arg("/VERYSILENT")
            .arg("/NORESTART")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| RvpnError::Wine(format!("wine installer failed: {e}")))?;

        if !status.success() {
            return Err(RvpnError::Wine(format!(
                "Radmin installer exited with {status:?}"
            )));
        }

        let radmin_exe = self.radmin_dir().join("RvControlSvc.exe");
        for _ in 0..60 {
            if radmin_exe.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        if !radmin_exe.exists() {
            return Err(RvpnError::Wine(
                "RvControlSvc.exe not found after installation".to_string(),
            ));
        }

        self.reg_add(
            "HKLM\\SYSTEM\\CurrentControlSet\\Services\\RvControlSvc",
            "Start",
            "REG_DWORD",
            "4",
        )?;

        info!("Radmin VPN installed successfully");
        Ok(())
    }

    pub fn install_components(&self) -> Result<(), RvpnError> {
        info!("Installing components into Wine prefix");

        let radmin_dir = self.radmin_dir();

        // The Radmin VPN binaries are proprietary and cannot be shipped; the
        // user installs them from the official Famatech MSI into this prefix.
        // Without that installation, every file copy would fail with a bare
        // "No such file or directory", so detect it up front and say what to
        // do.
        if !radmin_dir.join("RvControlSvc.exe").exists() {
            return Err(RvpnError::Wine(format!(
                "Radmin VPN is not installed in the Wine prefix ({} missing). \
                 Download it from https://www.radmin-vpn.com/download/ and \
                 install it into this prefix: WINEPREFIX={} wine RadminVPN_*.msi; \
                 then restart radmin-vpn.service.",
                radmin_dir.join("RvControlSvc.exe").display(),
                self.wine_prefix.display()
            )));
        }

        let drivers_dir = self.wine_prefix.join("drive_c/windows/system32/drivers");

        std::fs::create_dir_all(&drivers_dir)
            .map_err(|e| RvpnError::Wine(format!("create drivers dir: {e}")))?;

        let src = self.build_dir.join("rvpnnetmp.sys");
        let dst = drivers_dir.join("rvpnnetmp.sys");
        self.copy_required(&src, &dst, "rvpnnetmp.sys")?;

        let src = self.build_dir.join("adapter_hook.dll");
        let dst = radmin_dir.join("adapter_hook.dll");
        self.copy_required(&src, &dst, "adapter_hook.dll")?;

        let src = self.build_dir.join("rvpn_launcher.exe");
        let dst = radmin_dir.join("rvpn_launcher.exe");
        self.copy_required(&src, &dst, "rvpn_launcher.exe")?;

        let src = self.build_dir.join("netsh.exe");
        let dst = self.wine_prefix.join("drive_c/windows/syswow64/netsh.exe");
        std::fs::create_dir_all(dst.parent().unwrap())
            .map_err(|e| RvpnError::Wine(format!("create syswow64 dir: {e}")))?;
        self.copy_required(&src, &dst, "netsh.exe (32-bit)")?;

        let src = self.build_dir.join("netsh64.exe");
        let dst = self.wine_prefix.join("drive_c/windows/system32/netsh.exe");
        self.copy_required(&src, &dst, "netsh64.exe")?;

        let src = self.build_dir.join("drvinst.exe");
        let dst = radmin_dir.join("drvinst.exe");
        self.copy_required(&src, &dst, "drvinst.exe")?;

        let _ = Command::new("wineserver")
            .env("WINEPREFIX", &self.wine_prefix)
            .arg("-k")
            .status();

        self.scrub_ndis_driver()?;

        info!("Components installed");
        Ok(())
    }

    fn copy_required(&self, src: &Path, dst: &Path, what: &str) -> Result<(), RvpnError> {
        if !src.exists() {
            return Err(RvpnError::Wine(format!(
                "required component {what} not found at {} (build_dir = {}). \
                 Run `make install` or set RVPN_BUILD_DIR to the directory \
                 containing the Windows components.",
                src.display(),
                self.build_dir.display()
            )));
        }
        if dst.exists() && self.same_file(src, dst) {
            return Ok(());
        }
        std::fs::copy(src, dst)
            .map_err(|e| RvpnError::Wine(format!("copy {what} to {}: {e}", dst.display())))?;
        info!("Copied {} -> {}", src.display(), dst.display());
        Ok(())
    }

    fn same_file(&self, a: &Path, b: &Path) -> bool {
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.len() == mb.len() && ma.modified().ok() == mb.modified().ok(),
            _ => false,
        }
    }

    pub fn scrub_ndis_driver(&self) -> Result<(), RvpnError> {
        let system_reg = self.wine_prefix.join("system.reg");
        if !system_reg.exists() {
            return Ok(());
        }

        for driver_file in ["RvNetMP60.sys", "NetMP60_1_1_64.sys"] {
            let path = self
                .wine_prefix
                .join(format!("drive_c/windows/system32/drivers/{driver_file}"));
            let _ = std::fs::remove_file(path);
        }

        let contents = std::fs::read_to_string(&system_reg)
            .map_err(|e| RvpnError::Wine(format!("read system.reg: {e}")))?;

        let scrubbed = scrub_reg_sections(&contents, "Services\\\\RvNetMP60");
        if scrubbed != contents {
            std::fs::write(&system_reg, &scrubbed)
                .map_err(|e| RvpnError::Wine(format!("write system.reg: {e}")))?;
            info!("Scrubbed RvNetMP60 from system.reg");
        }

        for reg_file in ["user.reg", "userdef.reg"] {
            let path = self.wine_prefix.join(reg_file);
            if !path.exists() {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                let scrubbed = scrub_reg_sections(&contents, "Services\\\\RvNetMP60");
                if scrubbed != contents {
                    let _ = std::fs::write(&path, &scrubbed);
                    info!("Scrubbed RvNetMP60 from {reg_file}");
                }
            }
        }

        Ok(())
    }

    pub fn purge_hijacked_desktop_entries(&self) -> Result<(), RvpnError> {
        let apps_dir = dirs_next_home()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".local/share/applications");

        if !apps_dir.exists() {
            return Ok(());
        }

        let mut purged = 0;
        if let Ok(entries) = std::fs::read_dir(&apps_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "desktop").unwrap_or(false) {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        // Only purge entries that reference a wineprefix which no
                        // longer exists on disk (stale debris from old/test
                        // installs). Entries referencing any live prefix — the
                        // configured one or another valid one — are kept, and
                        // entries we cannot parse are always kept.
                        if Self::desktop_references_missing_prefix(&contents) {
                            let _ = std::fs::remove_file(&path);
                            purged += 1;
                        }
                    }
                }
            }
        }

        if purged > 0 {
            info!("Purged {purged} hijacked Wine desktop entries");
        }

        Ok(())
    }

    /// Return true only when the .desktop file references a wineprefix path
    /// that does not exist on disk — i.e. it is certainly stale debris.
    /// Anything referencing a live prefix (the configured one or another) is
    /// kept, as is anything we cannot parse.
    fn desktop_references_missing_prefix(contents: &str) -> bool {
        let mut found_any = false;
        for prefix in Self::extract_wineprefix_refs(contents) {
            found_any = true;
            if std::path::Path::new(&prefix).exists() {
                return false;
            }
        }
        found_any
    }

    fn extract_wineprefix_refs(contents: &str) -> Vec<String> {
        let marker = "WINEPREFIX=";
        let mut out = Vec::new();
        let mut rest = contents;
        while let Some(idx) = rest.find(marker) {
            let after = &rest[idx + marker.len()..];
            // `env "WINEPREFIX=/path" wine ...` — the value ends at the
            // closing quote; otherwise it ends at whitespace.
            let end = match after.find('"') {
                Some(0) => after.len(),
                Some(n) => n,
                None => after.find([' ', '\t', '\n']).unwrap_or(after.len()),
            };
            if end > 0 {
                out.push(after[..end].to_string());
            }
            rest = &after[end..];
        }
        out
    }

    pub fn reg_add(
        &self,
        key: &str,
        value: &str,
        value_type: &str,
        data: &str,
    ) -> Result<(), RvpnError> {
        let status = self
            .wine_cmd("wine")
            .args([
                "reg", "add", key, "/v", value, "/t", value_type, "/d", data, "/f",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| RvpnError::Wine(format!("wine reg add failed: {e}")))?;

        if status.success() {
            Ok(())
        } else {
            Err(RvpnError::Wine(format!(
                "wine reg add {key}\\{value} failed"
            )))
        }
    }

    pub fn configure_registry(&self, tap_guid: &str) -> Result<(), RvpnError> {
        info!("Configuring Wine registry for TAP adapter");

        let class_key = "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\0099".to_string();
        let net_key = format!(
            "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Network\\{{4d36e972-e325-11ce-bfc1-08002be10318}}\\{tap_guid}\\Connection"
        );

        let commands = [
            (class_key.as_str(), "NetCfgInstanceId", "REG_SZ", tap_guid),
            (
                class_key.as_str(),
                "MatchingDeviceId",
                "REG_SZ",
                &format!("{tap_guid}\\RvNetMP60"),
            ),
            (net_key.as_str(), "Name", "REG_SZ", "Radmin VPN"),
            (
                net_key.as_str(),
                "PnpInstanceID",
                "REG_SZ",
                "ROOT\\NET\\0099",
            ),
            (
                "HKLM\\Software\\Wow6432Node\\Famatech\\RadminVPN\\1.0\\Firewall",
                "AdapterId",
                "REG_SZ",
                tap_guid,
            ),
            (
                "HKLM\\SOFTWARE\\Famatech\\RadminVPN\\1.0\\Registration",
                "",
                "REG_SZ",
                "",
            ),
            (
                "HKCU\\Software\\Wine\\DllOverrides",
                "winemenubuilder.exe",
                "REG_SZ",
                "",
            ),
            (
                "HKCU\\Software\\Wine\\DllOverrides",
                "netsh.exe",
                "REG_SZ",
                "native",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "DisplayName",
                "REG_SZ",
                "Radmin VPN TAP Bridge",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "ImagePath",
                "REG_EXPAND_SZ",
                "C:\\windows\\system32\\drivers\\rvpnnetmp.sys",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "Start",
                "REG_DWORD",
                "2",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "Type",
                "REG_DWORD",
                "1",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "Group",
                "REG_SZ",
                "NDIS",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\rvpnnetmp",
                "ErrorControl",
                "REG_DWORD",
                "0",
            ),
            (
                "HKLM\\SYSTEM\\CurrentControlSet\\Services\\RvControlSvc",
                "Start",
                "REG_DWORD",
                "4",
            ),
        ];

        for (key, value, vtype, data) in &commands {
            if let Err(e) = self.reg_add(key, value, vtype, data) {
                warn!("reg add {key}\\{value} failed: {e}");
            }
        }

        info!("Registry configured");
        Ok(())
    }

    pub fn launch_service(
        &self,
        dnsfix_so: Option<&Path>,
    ) -> Result<std::process::Child, RvpnError> {
        let mut cmd = self.wine_cmd("wine");

        cmd.current_dir(self.radmin_dir());
        cmd.arg("rvpn_launcher.exe");
        cmd.arg("/run");

        cmd.env("WINEDEBUG", "-all");
        cmd.env("WINE_LARGE_ADDRESS_AWARE", "1");

        if let Some(so_path) = dnsfix_so {
            let mut ld_preload = so_path.to_string_lossy().to_string();
            if let Ok(existing) = std::env::var("LD_PRELOAD") {
                if !existing.is_empty() {
                    ld_preload = format!("{ld_preload}:{existing}");
                }
            }
            cmd.env("LD_PRELOAD", ld_preload);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| RvpnError::Wine(format!("failed to launch service: {e}")))?;

        info!("Radmin service launched (pid={})", child.id());
        Ok(child)
    }

    pub fn launch_gui(&self, winedbg: bool) -> Result<std::process::Child, RvpnError> {
        let mut cmd = self.wine_cmd("wine");

        cmd.current_dir(self.radmin_dir());

        if winedbg {
            cmd.arg("winedbg");
            cmd.arg("--auto");
            cmd.arg("RvRvpnGui.exe");
        } else {
            cmd.arg("RvRvpnGui.exe");
            cmd.env("LIBGL_ALWAYS_SOFTWARE", "1");
            cmd.env("GALLIUM_DRIVER", "llvmpipe");
        }

        cmd.env("WINEDEBUG", "-all");
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| RvpnError::Wine(format!("failed to launch GUI: {e}")))?;

        info!("Radmin GUI launched (pid={})", child.id());
        Ok(child)
    }

    pub fn boot_wineserver_with_reuseport(
        &self,
        reuseport_so: Option<&Path>,
    ) -> Result<(), RvpnError> {
        let mut cmd = self.wine_cmd("wineserver");
        cmd.arg("-p");

        if let Some(so_path) = reuseport_so {
            cmd.env("LD_PRELOAD", so_path.to_string_lossy().to_string());
        }

        let status = cmd
            .status()
            .map_err(|e| RvpnError::Wine(format!("wineserver -p failed: {e}")))?;

        if status.success() {
            info!("wineserver booted with persistence");
        } else {
            warn!("wineserver -p exited with {status:?}");
        }

        Ok(())
    }
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::home_dir()
}

fn parse_environ(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    bytes
        .split(|b| *b == 0)
        .filter(|kv| !kv.is_empty())
        .filter_map(|kv| {
            let eq = kv.iter().position(|b| *b == b'=')?;
            Some((
                String::from_utf8_lossy(&kv[..eq]).into_owned(),
                kv[eq + 1..].to_vec(),
            ))
        })
        .collect()
}

/// Detect the graphical session environment by scanning same-uid processes'
/// environ (plasmashell, kwin, Xwayland, ...). No caching: at service start
/// the graphical session may not be up yet, and a stale "not found" must not
/// be remembered forever.
fn detect_session_display_env() -> Option<Vec<(&'static str, std::ffi::OsString)>> {
    compute_session_display_env()
}

/// Wait until a graphical session environment is detectable, polling every
/// 500ms up to `timeout`. Returns true if a display was found. Without a
/// display the Wine session cannot create GUI windows, so the caller should
/// wait for one on machines that have a desktop.
pub fn wait_for_display(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if detect_session_display_env().is_some() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn compute_session_display_env() -> Option<Vec<(&'static str, std::ffi::OsString)>> {
    use std::os::unix::ffi::OsStringExt;

    let mut wayland_only: Option<Vec<(&'static str, std::ffi::OsString)>> = None;
    let proc = std::fs::read_dir("/proc").ok()?;

    for entry in proc.flatten() {
        let pid: u32 = match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
            continue;
        };

        let mut display = None;
        let mut xauth = None;
        let mut wayland = None;
        let mut xdg_runtime = None;
        for (key, value) in parse_environ(&environ) {
            match key.as_str() {
                "DISPLAY" => display = Some(value),
                "XAUTHORITY" => xauth = Some(value),
                "WAYLAND_DISPLAY" => wayland = Some(value),
                "XDG_RUNTIME_DIR" => xdg_runtime = Some(value),
                _ => {}
            }
        }

        if let Some(d) = display {
            let mut out = vec![("DISPLAY", std::ffi::OsString::from_vec(d))];
            if let Some(x) = xauth {
                out.push(("XAUTHORITY", std::ffi::OsString::from_vec(x)));
            }
            if let Some(w) = wayland {
                out.push(("WAYLAND_DISPLAY", std::ffi::OsString::from_vec(w)));
            }
            if let Some(x) = xdg_runtime {
                out.push(("XDG_RUNTIME_DIR", std::ffi::OsString::from_vec(x)));
            }
            return Some(out);
        }

        if wayland_only.is_none() {
            if let (Some(w), Some(x)) = (wayland, xdg_runtime) {
                wayland_only = Some(vec![
                    ("WAYLAND_DISPLAY", std::ffi::OsString::from_vec(w)),
                    ("XDG_RUNTIME_DIR", std::ffi::OsString::from_vec(x)),
                ]);
            }
        }
    }

    wayland_only
}

fn scrub_reg_sections(contents: &str, needle: &str) -> String {
    let mut out = String::with_capacity(contents.len());
    let mut skipping = false;
    let mut dropped = 0usize;

    for line in contents.lines() {
        let is_header = line.starts_with('[');
        if is_header {
            skipping = line.contains(needle);
            if skipping {
                dropped += 1;
                continue;
            }
        }
        if skipping {
            if line.trim().is_empty() {
                skipping = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    if dropped > 0 {
        info!("scrub_reg_sections: dropped {dropped} section(s) matching {needle}");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_environ_roundtrip() {
        let mut blob = Vec::new();
        blob.extend_from_slice(b"DISPLAY=:0\0XAUTHORITY=/run/xauth\0EMPTY=\0BROKEN\0");
        let parsed = parse_environ(&blob);
        let map: std::collections::HashMap<String, Vec<u8>> = parsed.into_iter().collect();
        assert_eq!(map.get("DISPLAY").unwrap(), b":0");
        assert_eq!(map.get("XAUTHORITY").unwrap(), b"/run/xauth");
        assert_eq!(map.get("EMPTY").unwrap(), b"");
        assert_eq!(map.get("BROKEN"), None);
    }

    #[test]
    fn wine_process_manager_creation() {
        let config = Config::default();
        let mgr = WineProcessManager::new(&config);
        assert!(mgr.wine_prefix().ends_with("wineprefix"));
    }

    #[test]
    fn scrub_reg_sections_drops_whole_blocks() {
        let input = "[System\\\\CurrentControlSet\\\\Services\\\\rvpnnetmp] 1\n#time=abc\n\"DisplayName\"=\"ours\"\n\n[System\\\\CurrentControlSet\\\\Services\\\\RvNetMP60] 2\n#time=def\n\"DisplayName\"=\"drop me\"\n\"Start\"=dword:00000003\n\n[System\\\\CurrentControlSet\\\\Services\\\\RvNetMP60\\\\Enum] 3\n\"0\"=\"enum\"\n\n[System\\\\CurrentControlSet\\\\Services\\\\Wd] 4\n\"DisplayName\"=\"keep\"\n";
        let out = scrub_reg_sections(input, "Services\\\\RvNetMP60");
        assert!(out.contains("rvpnnetmp"));
        assert!(out.contains("Services\\\\Wd"));
        assert!(!out.contains("RvNetMP60"));
        assert!(!out.contains("drop me"));
        assert!(!out.contains("enum"));
    }
}
