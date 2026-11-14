#![allow(non_upper_case_globals)]

use libc::{c_char, c_int, c_void, socklen_t};
use std::sync::OnceLock;

type RealSocket = unsafe extern "C" fn(c_int, c_int, c_int) -> c_int;

static REAL_SOCKET: OnceLock<RealSocket> = OnceLock::new();

#[no_mangle]
pub unsafe extern "C" fn socket(domain: c_int, type_: c_int, protocol: c_int) -> c_int {
    let real = REAL_SOCKET.get_or_init(|| {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"socket\0".as_ptr() as *const c_char);
        if sym.is_null() {
            real_socket_fallback
        } else {
            std::mem::transmute(sym)
        }
    });

    let fd = real(domain, type_, protocol);
    if fd < 0 {
        return fd;
    }

    let base = type_ & !(libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC);
    if (domain == libc::AF_INET || domain == libc::AF_INET6)
        && base == libc::SOCK_STREAM
        && (protocol == 0 || protocol == libc::IPPROTO_TCP)
    {
        let on: c_int = 1;
        let saved = *libc::__errno_location();
        let _ = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &on as *const c_int as *const c_void,
            std::mem::size_of::<c_int>() as socklen_t,
        );
        *libc::__errno_location() = saved;
    }

    fd
}

extern "C" fn real_socket_fallback(_domain: c_int, _type: c_int, _protocol: c_int) -> c_int {
    -1
}
