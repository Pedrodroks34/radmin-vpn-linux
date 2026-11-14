use crate::core::constants::FW_TAG;
use crate::core::types::RvpnError;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirewallBackend {
    Nftables,
    Iptables,
    Firewalld,
    #[default]
    None,
}

#[derive(Debug, Default)]
pub struct FirewallManager {
    backend: FirewallBackend,
    device: String,
    tag: String,
    opened: bool,
    firewalld_zone_changed: bool,
}

impl FirewallManager {
    pub fn new(device: &str) -> Self {
        let backend = Self::detect_backend();
        Self {
            backend,
            device: device.to_string(),
            tag: FW_TAG.to_string(),
            opened: false,
            firewalld_zone_changed: false,
        }
    }

    pub fn with_backend(device: &str, backend: FirewallBackend) -> Self {
        Self {
            backend,
            device: device.to_string(),
            tag: FW_TAG.to_string(),
            opened: false,
            firewalld_zone_changed: false,
        }
    }

    pub fn open(&mut self) -> Result<(), RvpnError> {
        if self.opened {
            return Ok(());
        }

        self.close_leftover()?;

        match self.backend {
            FirewallBackend::Firewalld => self.open_firewalld()?,
            FirewallBackend::Nftables => self.open_nftables()?,
            FirewallBackend::Iptables => self.open_iptables()?,
            FirewallBackend::None => {}
        }

        self.opened = true;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), RvpnError> {
        if !self.opened {
            return Ok(());
        }

        match self.backend {
            FirewallBackend::Firewalld => self.close_firewalld()?,
            FirewallBackend::Nftables => self.close_nftables()?,
            FirewallBackend::Iptables => self.close_iptables()?,
            FirewallBackend::None => {}
        }

        self.opened = false;
        Ok(())
    }

    pub fn backend(&self) -> FirewallBackend {
        self.backend
    }

    pub fn is_opened(&self) -> bool {
        self.opened
    }

    fn detect_backend() -> FirewallBackend {
        if command_exists("firewall-cmd")
            && run_simple(&["firewall-cmd", "--state"])
                .map(|o| o.status.success())
                .unwrap_or(false)
        {
            return FirewallBackend::Firewalld;
        }
        if command_exists("nft") {
            return FirewallBackend::Nftables;
        }
        if command_exists("iptables") {
            return FirewallBackend::Iptables;
        }
        FirewallBackend::None
    }

    fn open_firewalld(&mut self) -> Result<(), RvpnError> {
        let out = run_simple(&[
            "firewall-cmd",
            &format!("--get-zone-of-interface={}", self.device),
        ]);
        if let Ok(out) = out {
            let zone = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if zone == "trusted" {
                return Ok(());
            }
        }

        let status = run_command(&[
            "firewall-cmd",
            "--zone=trusted",
            &format!("--change-interface={}", self.device),
        ])?;

        if status.success() {
            self.firewalld_zone_changed = true;
        }
        Ok(())
    }

    fn close_firewalld(&mut self) -> Result<(), RvpnError> {
        if self.firewalld_zone_changed {
            let _ = run_command(&[
                "firewall-cmd",
                "--zone=trusted",
                &format!("--remove-interface={}", self.device),
            ]);
            self.firewalld_zone_changed = false;
        }
        Ok(())
    }

    fn open_nftables(&mut self) -> Result<(), RvpnError> {
        let chains = self.list_nft_input_chains()?;
        for (fam, tbl, ch) in &chains {
            let status = run_command(&[
                "nft",
                "insert",
                "rule",
                fam,
                tbl,
                ch,
                "iifname",
                &self.device,
                "accept",
                "comment",
                &format!("\"{}\"", self.tag),
            ])?;

            if !status.success() {
                warn!("nft insert rule for {fam}/{tbl}/{ch} failed (non-fatal)");
            }
        }

        let _ = self.open_iptables_legacy();
        Ok(())
    }

    fn close_nftables(&mut self) -> Result<(), RvpnError> {
        let chains = self.list_nft_input_chains()?;
        for (fam, tbl, ch) in &chains {
            let out = run_simple(&["nft", "-a", "list", "chain", fam, tbl, ch]);
            if let Ok(out) = out {
                let output = String::from_utf8_lossy(&out.stdout);
                for line in output.lines() {
                    if let Some(handle) = parse_nft_handle(line, &self.tag) {
                        let _ = run_command(&[
                            "nft", "delete", "rule", fam, tbl, ch, "handle", &handle,
                        ]);
                    }
                }
            }
        }

        let _ = self.close_iptables_legacy();
        Ok(())
    }

    fn list_nft_input_chains(&self) -> Result<Vec<(String, String, String)>, RvpnError> {
        let out = run_simple(&["nft", "list", "chains"])?;
        let output = String::from_utf8_lossy(&out.stdout);
        let mut chains = Vec::new();
        let mut fam = String::new();
        let mut tbl = String::new();

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("table ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 3 {
                    fam = parts[1].to_string();
                    tbl = parts[2].to_string();
                }
            } else if trimmed.starts_with("chain ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let ch = parts[1].to_string();
                    if matches!(fam.as_str(), "ip" | "ip6" | "inet") {
                        chains.push((fam.clone(), tbl.clone(), ch));
                    }
                }
            }
        }

        Ok(chains)
    }

    fn open_iptables_legacy(&self) -> Result<(), RvpnError> {
        let _ = run_command(&[
            "iptables",
            "-I",
            "INPUT",
            "-i",
            &self.device,
            "-m",
            "comment",
            "--comment",
            &self.tag,
            "-j",
            "ACCEPT",
        ]);
        let _ = run_command(&[
            "ip6tables",
            "-I",
            "INPUT",
            "-i",
            &self.device,
            "-m",
            "comment",
            "--comment",
            &self.tag,
            "-j",
            "ACCEPT",
        ]);
        Ok(())
    }

    fn close_iptables_legacy(&self) -> Result<(), RvpnError> {
        loop {
            let status = run_command(&[
                "iptables",
                "-D",
                "INPUT",
                "-i",
                &self.device,
                "-m",
                "comment",
                "--comment",
                &self.tag,
                "-j",
                "ACCEPT",
            ]);
            match status {
                Ok(s) if s.success() => continue,
                _ => break,
            }
        }
        loop {
            let status = run_command(&[
                "ip6tables",
                "-D",
                "INPUT",
                "-i",
                &self.device,
                "-m",
                "comment",
                "--comment",
                &self.tag,
                "-j",
                "ACCEPT",
            ]);
            match status {
                Ok(s) if s.success() => continue,
                _ => break,
            }
        }
        Ok(())
    }

    fn open_iptables(&self) -> Result<(), RvpnError> {
        self.open_iptables_legacy()
    }

    fn close_iptables(&self) -> Result<(), RvpnError> {
        self.close_iptables_legacy()
    }

    fn close_leftover(&mut self) -> Result<(), RvpnError> {
        match self.backend {
            FirewallBackend::Nftables => {
                let _ = self.close_nftables();
            }
            FirewallBackend::Iptables => {
                let _ = self.close_iptables();
            }
            FirewallBackend::Firewalld => {
                let _ = self.close_firewalld();
            }
            FirewallBackend::None => {}
        }
        self.opened = false;
        self.firewalld_zone_changed = false;
        Ok(())
    }
}

fn parse_nft_handle(line: &str, tag: &str) -> Option<String> {
    if !line.contains(tag) {
        return None;
    }

    let mut found = false;
    for token in line.split_whitespace() {
        if found {
            return Some(token.to_string());
        }
        if token == "handle" {
            found = true;
        }
    }
    None
}

fn command_exists(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .is_ok()
        || std::process::Command::new(cmd)
            .arg("--help")
            .output()
            .is_ok()
}

fn run_simple(args: &[&str]) -> Result<std::process::Output, RvpnError> {
    std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .map_err(|e| RvpnError::Firewall(format!("command {args:?} failed: {e}")))
}

fn run_command(args: &[&str]) -> Result<std::process::ExitStatus, RvpnError> {
    std::process::Command::new(args[0])
        .args(&args[1..])
        .status()
        .map_err(|e| RvpnError::Firewall(format!("command {args:?} failed: {e}")))
}

impl Drop for FirewallManager {
    fn drop(&mut self) {
        if self.opened {
            let _ = self.close();
        }
    }
}
