use crate::core::types::RvpnError;
use std::path::Path;

pub fn launch_with_hooks(
    radmin_dir: &Path,
    dnsfix_so: Option<&Path>,
) -> Result<std::process::Child, RvpnError> {
    let mut cmd = std::process::Command::new("wine");
    cmd.current_dir(radmin_dir);
    cmd.arg("rvpn_launcher.exe");
    cmd.arg("/run");

    if let Some(so_path) = dnsfix_so {
        cmd.env("LD_PRELOAD", so_path);
    }

    cmd.env("WINEDEBUG", "-all");
    cmd.env("WINE_LARGE_ADDRESS_AWARE", "1");

    let child = cmd
        .spawn()
        .map_err(|e| RvpnError::Service(format!("failed to launch service: {e}")))?;

    Ok(child)
}

pub fn launch_service_direct(
    radmin_dir: &Path,
    dnsfix_so: Option<&Path>,
) -> Result<std::process::Child, RvpnError> {
    let mut cmd = std::process::Command::new("wine");
    cmd.current_dir(radmin_dir);
    cmd.arg("RvControlSvc.exe");

    if let Some(so_path) = dnsfix_so {
        cmd.env("LD_PRELOAD", so_path);
    }

    cmd.env("WINEDEBUG", "-all");

    let child = cmd
        .spawn()
        .map_err(|e| RvpnError::Service(format!("failed to launch RvControlSvc: {e}")))?;

    Ok(child)
}

#[cfg(test)]
mod tests {
    #[test]
    fn launcher_module_exists() {
        let _ = std::marker::PhantomData::<fn()>;
    }
}
