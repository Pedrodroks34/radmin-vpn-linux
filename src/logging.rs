use once_cell::sync::Lazy;
use std::os::unix::net::UnixDatagram;
use std::sync::Mutex;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub static STRUCTURED_LOG: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

pub fn init(filter: &str, json: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    if json {
        *STRUCTURED_LOG.lock().unwrap() = true;
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_target(false))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(false))
            .init();
    }

    Ok(())
}

pub fn notify_systemd(ready: bool, stopping: bool) {
    if std::env::var("NOTIFY_SOCKET").is_err() {
        return;
    }

    let msg = if ready {
        "READY=1\n"
    } else if stopping {
        "STOPPING=1\n"
    } else {
        ""
    };

    if msg.is_empty() {
        return;
    }

    let _ = UnixDatagram::unbound()
        .and_then(|sock| sock.send_to(msg.as_bytes(), std::env::var("NOTIFY_SOCKET").unwrap()));
}

pub fn watchdog_ping() {
    if std::env::var("WATCHDOG_PID").is_ok() || std::env::var("NOTIFY_SOCKET").is_ok() {
        let _ = UnixDatagram::unbound()
            .and_then(|sock| sock.send_to(b"WATCHDOG=1", std::env::var("NOTIFY_SOCKET").unwrap()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_systemd_no_socket() {
        std::env::remove_var("NOTIFY_SOCKET");
        notify_systemd(true, false);
        notify_systemd(false, true);
    }

    #[test]
    fn watchdog_ping_no_socket() {
        std::env::remove_var("NOTIFY_SOCKET");
        watchdog_ping();
    }
}
