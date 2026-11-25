use crate::config::Config;
use crate::core::constants::*;
use crate::core::types::RvpnError;
use std::path::PathBuf;

pub struct InstallerManager {
    config: Config,
}

impl InstallerManager {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }

    pub fn installer_url(&self) -> String {
        RADMIN_INSTALLER_URL_TEMPLATE.replace("{version}", &self.config.radmin_version)
    }

    pub fn installer_name(&self) -> String {
        format!("Radmin_VPN_{}.exe", self.config.radmin_version)
    }

    pub fn download_installer(&self) -> Result<PathBuf, RvpnError> {
        let download_dir = self.config.data_dir.join("installers");
        std::fs::create_dir_all(&download_dir)
            .map_err(|e| RvpnError::Config(format!("create download dir: {e}")))?;

        let installer_name = self.installer_name();
        let installer_path = download_dir.join(&installer_name);

        if installer_path.exists() {
            if self.verify_installer(&installer_path)? {
                return Ok(installer_path);
            }

            let _ = std::fs::remove_file(&installer_path);
        }

        let url = self.installer_url();
        info_log(&format!("Downloading Radmin installer from {url}"));
        let installer_path_str = installer_path.to_string_lossy().into_owned();

        let status = std::process::Command::new("curl")
            .args(["-fL", "-o", installer_path_str.as_str(), &url])
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                return Err(RvpnError::Config(format!(
                    "download failed (curl exited {s:?})"
                )));
            }
            Err(_) => {
                let status = std::process::Command::new("wget")
                    .args(["-q", "-O", installer_path_str.as_str(), &url])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    _ => {
                        return Err(RvpnError::Config(
                            "download failed (install curl or wget)".to_string(),
                        ));
                    }
                }
            }
        }

        if !self.verify_installer(&installer_path)? {
            let _ = std::fs::remove_file(&installer_path);
            return Err(RvpnError::Config(
                "installer hash verification failed".to_string(),
            ));
        }

        Ok(installer_path)
    }

    pub fn verify_installer(&self, path: &std::path::Path) -> Result<bool, RvpnError> {
        let Some(expected_hash) = &self.config.radmin_sha256 else {
            warn_log(&format!(
                "No pinned SHA256 for Radmin {} — installer NOT verified",
                self.config.radmin_version
            ));
            return Ok(true);
        };

        let expected_name = self.installer_name();
        if path.file_name().and_then(|n| n.to_str()) != Some(expected_name.as_str()) {
            warn_log(&format!(
                "{} is not the validated build — installer NOT verified",
                path.display()
            ));
            return Ok(true);
        }

        let actual_hash = compute_sha256(path)?;
        if actual_hash != *expected_hash {
            warn_log("Installer SHA256 MISMATCH");
            warn_log(&format!("  file: {}", path.display()));
            warn_log(&format!("  expected: {expected_hash}"));
            warn_log(&format!("  actual:   {actual_hash}"));
            return Ok(false);
        }

        info_log(&format!(
            "Installer SHA256 verified ({})",
            self.config.radmin_version
        ));
        Ok(true)
    }
}

fn compute_sha256(path: &std::path::Path) -> Result<String, RvpnError> {
    let path_str = path.to_string_lossy().into_owned();

    let output = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| RvpnError::Config(format!("sha256sum failed: {e}")))?;

    if !output.status.success() {
        let output = std::process::Command::new("shasum")
            .args(["-a", "256", path_str.as_str()])
            .output()
            .map_err(|e| RvpnError::Config(format!("shasum failed: {e}")))?;
        if !output.status.success() {
            return Ok(String::new());
        }
        let output_str = String::from_utf8_lossy(&output.stdout);
        return Ok(output_str
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string());
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(output_str
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string())
}

fn info_log(msg: &str) {
    println!("[*] {msg}");
}

fn warn_log(msg: &str) {
    println!("[!] {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_name() {
        let config = crate::config::Config::default();
        let mgr = InstallerManager::new(&config);
        assert_eq!(mgr.installer_name(), "Radmin_VPN_2.1.4952.2.exe");
    }

    #[test]
    fn installer_url() {
        let config = crate::config::Config::default();
        let mgr = InstallerManager::new(&config);
        let url = mgr.installer_url();
        assert!(url.contains("2.1.4952.2"));
        assert!(url.contains("download.radmin-vpn.com"));
    }
}
