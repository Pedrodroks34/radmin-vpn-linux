use crate::core::types::RvpnError;

pub fn init_hooks() -> Result<(), RvpnError> {
    Ok(())
}

pub fn service_ld_preloads(build_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut libs = Vec::new();

    let dnsfix = build_dir.join("rvpn_dnsfix.so");
    if dnsfix.exists() {
        libs.push(dnsfix);
    }

    let reuseport = build_dir.join("rvpn_reuseport.so");
    if reuseport.exists() {
        libs.push(reuseport);
    }

    libs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_init() {
        assert!(init_hooks().is_ok());
    }
}
