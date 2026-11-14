use crate::core::types::MacAddress;
use crate::core::types::RvpnError;
use crate::driver::UserspaceDriver;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub fn run(
    adapter_mac: MacAddress,
    control_socket_path: &str,
    running: Arc<AtomicBool>,
) -> Result<(), RvpnError> {
    let mut driver = UserspaceDriver::new(adapter_mac);

    driver.open_fifos()?;

    driver.setup_control_socket(control_socket_path)?;

    info!("Userspace driver shim running");
    info!("  adapter MAC: {}", adapter_mac);
    info!("  control socket: {}", control_socket_path);

    while running.load(Ordering::Relaxed) {
        if let Some(listener) = &driver.control_socket {
            listener.set_nonblocking(true).ok();
            match listener.accept() {
                Ok((stream, _addr)) => {
                    if let Err(e) = handle_control_connection(stream, &mut driver) {
                        warn!("control connection error: {e}");
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    warn!("control socket accept error: {e}");
                }
            }
        }

        let mut frame_buf = [0u8; 2048];
        match driver.read_frame_from_bridge(&mut frame_buf) {
            Ok(0) => {}
            Ok(n) => {
                debug!("read {} bytes from bridge", n);

                if n >= 2 {
                    let frame_len = u16::from_le_bytes([frame_buf[0], frame_buf[1]]) as usize;
                    if frame_len > 0 && frame_len + 2 <= n {
                        let frame_data = &frame_buf[2..2 + frame_len];
                        let frame = crate::core::types::EthernetFrame::new(frame_data.to_vec());

                        let _ = frame;
                    }
                }
            }
            Err(_) => {}
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = std::fs::remove_file(control_socket_path);
    info!("Userspace driver shim stopped");
    Ok(())
}

fn handle_control_connection(
    mut stream: UnixStream,
    driver: &mut UserspaceDriver,
) -> Result<(), RvpnError> {
    stream.set_nonblocking(true).ok();

    let mut header = [0u8; 8];
    match read_exact_timeout(&mut stream, &mut header, Duration::from_secs(5)) {
        Ok(()) => {}
        Err(_) => return Ok(()),
    }

    let cmd = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let input_len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

    let mut input = vec![0u8; input_len];
    if input_len > 0 {
        read_exact_timeout(&mut stream, &mut input, Duration::from_secs(5))?;
    }

    let mut output = [0u8; 4096];
    match driver.process_command(cmd, &input, &mut output) {
        Ok(info_len) => {
            let mut response = Vec::with_capacity(4 + info_len as usize);
            response.extend_from_slice(&info_len.to_le_bytes());
            response.extend_from_slice(&output[..info_len as usize]);
            let _ = stream.write_all(&response);
        }
        Err(e) => {
            error!("command processing error: {e}");
        }
    }

    Ok(())
}

fn read_exact_timeout(
    stream: &mut UnixStream,
    buf: &mut [u8],
    timeout: Duration,
) -> Result<(), std::io::Error> {
    stream.set_read_timeout(Some(timeout)).ok();
    let mut done = 0;
    while done < buf.len() {
        match stream.read(&mut buf[done..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF",
                ))
            }
            Ok(n) => done += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
