#![allow(non_upper_case_globals)]

use libc::{c_char, c_int, sockaddr, sockaddr_in, sockaddr_in6, socklen_t};
use std::ffi::CStr;
use std::sync::OnceLock;

type RealGetnameinfo = unsafe extern "C" fn(
    *const sockaddr,
    socklen_t,
    *mut c_char,
    socklen_t,
    *mut c_char,
    socklen_t,
    c_int,
) -> c_int;

type RealGethostbyaddr =
    unsafe extern "C" fn(*const std::ffi::c_void, socklen_t, c_int) -> *mut libc::hostent;

static REAL_GETNAMEINFO: OnceLock<RealGetnameinfo> = OnceLock::new();
static REAL_GETHOSTBYADDR: OnceLock<RealGethostbyaddr> = OnceLock::new();

fn dnsfix_is_private_v4(h: u32) -> bool {
    if (h & 0xFF000000u32) == 0x7F000000u32 {
        return true;
    }
    if (h & 0xFF000000u32) == 0x0A000000u32 {
        return true;
    }
    if (h & 0xFFF00000u32) == 0xAC100000u32 {
        return true;
    }
    if (h & 0xFFFF0000u32) == 0xC0A80000u32 {
        return true;
    }
    if (h & 0xFFFF0000u32) == 0xA9FE0000u32 {
        return true;
    }
    if (h & 0xFFC00000u32) == 0x64400000u32 {
        return true;
    }
    false
}

fn dnsfix_is_private_v6(a: &libc::in6_addr) -> bool {
    let s = &a.s6_addr;

    if s[..15].iter().all(|b| *b == 0) && s[15] == 1 {
        return true;
    }

    if (s[0] == 0xFE) && (s[1] & 0xC0) == 0x80 {
        return true;
    }

    if (s[0] == 0xFE) && (s[1] & 0xC0) == 0xC0 {
        return true;
    }

    if (s[0] & 0xFE) == 0xFC {
        return true;
    }
    false
}

#[no_mangle]
pub unsafe extern "C" fn getnameinfo(
    sa: *const sockaddr,
    salen: socklen_t,
    host: *mut c_char,
    hostlen: socklen_t,
    serv: *mut c_char,
    servlen: socklen_t,
    flags: c_int,
) -> c_int {
    let mut numeric = [0u8; 256];
    let mut port: u16 = 0;
    let mut hit = false;

    numeric[0] = 0;

    if !sa.is_null() {
        match (*sa).sa_family as i32 {
            libc::AF_INET => {
                if salen >= std::mem::size_of::<sockaddr_in>() as socklen_t {
                    let si = &*(sa as *const sockaddr_in);
                    let h = u32::from_le(si.sin_addr.s_addr);
                    if dnsfix_is_private_v4(h) {
                        let octets = [(h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8, h as u8];
                        let ip =
                            std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]);
                        let s = ip.to_string();
                        let bytes = s.as_bytes();
                        let n = bytes.len().min(numeric.len() - 1);
                        numeric[..n].copy_from_slice(&bytes[..n]);
                        numeric[n] = 0;
                        port = u16::from_be(si.sin_port);
                        hit = true;
                    }
                }
            }
            libc::AF_INET6 => {
                if salen >= std::mem::size_of::<sockaddr_in6>() as socklen_t {
                    let si6 = &*(sa as *const sockaddr_in6);
                    if dnsfix_is_private_v6(&si6.sin6_addr) {
                        let ip = std::net::Ipv6Addr::from(si6.sin6_addr.s6_addr);
                        let mut s = ip.to_string();
                        if si6.sin6_scope_id != 0 {
                            s.push('%');
                            s.push_str(&si6.sin6_scope_id.to_string());
                        }
                        let bytes = s.as_bytes();
                        let n = bytes.len().min(numeric.len() - 1);
                        numeric[..n].copy_from_slice(&bytes[..n]);
                        numeric[n] = 0;
                        port = u16::from_be(si6.sin6_port);
                        hit = true;
                    }
                }
            }
            _ => {}
        }
    }

    if hit {
        if !host.is_null() && hostlen > 0 {
            let host_len = CStr::from_ptr(numeric.as_ptr() as *const c_char)
                .to_bytes()
                .len();
            if host_len >= hostlen as usize {
                return libc::EAI_OVERFLOW;
            }
            std::ptr::copy_nonoverlapping(numeric.as_ptr(), host as *mut u8, host_len);
            *host.add(host_len) = 0;
        }
        if !serv.is_null() && servlen > 0 {
            let sbuf = port.to_string();
            let sbuf_bytes = sbuf.as_bytes();
            if sbuf_bytes.len() >= servlen as usize {
                return libc::EAI_OVERFLOW;
            }
            std::ptr::copy_nonoverlapping(sbuf_bytes.as_ptr(), serv as *mut u8, sbuf_bytes.len());
            *serv.add(sbuf_bytes.len()) = 0;
        }
        return 0;
    }

    let real = REAL_GETNAMEINFO.get_or_init(|| {
        let sym = libc::dlsym(libc::RTLD_NEXT, b"getnameinfo\0".as_ptr() as *const c_char);
        if sym.is_null() {
            real_getnameinfo_fallback
        } else {
            std::mem::transmute(sym)
        }
    });

    unsafe { real(sa, salen, host, hostlen, serv, servlen, flags) }
}

extern "C" fn real_getnameinfo_fallback(
    _sa: *const sockaddr,
    _salen: socklen_t,
    _host: *mut c_char,
    _hostlen: socklen_t,
    _serv: *mut c_char,
    _servlen: socklen_t,
    _flags: c_int,
) -> c_int {
    libc::EAI_FAIL
}

const HOST_NOT_FOUND: c_int = 1;

extern "C" {
    fn __h_errno_location() -> *mut c_int;
}

#[no_mangle]
pub unsafe extern "C" fn gethostbyaddr(
    addr: *const std::ffi::c_void,
    len: socklen_t,
    type_: c_int,
) -> *mut libc::hostent {
    if !addr.is_null() && type_ == libc::AF_INET as c_int && len == 4 {
        let mut net: u32 = 0;
        std::ptr::copy_nonoverlapping(addr as *const u8, &mut net as *mut u32 as *mut u8, 4);
        let h = u32::from_le(net);
        if dnsfix_is_private_v4(h) {
            unsafe { *__h_errno_location() = HOST_NOT_FOUND };
            return std::ptr::null_mut();
        }
    } else if !addr.is_null()
        && type_ == libc::AF_INET6 as c_int
        && len == std::mem::size_of::<libc::in6_addr>() as socklen_t
    {
        if dnsfix_is_private_v6(&*(addr as *const libc::in6_addr)) {
            unsafe { *__h_errno_location() = HOST_NOT_FOUND };
            return std::ptr::null_mut();
        }
    }

    let real = REAL_GETHOSTBYADDR.get_or_init(|| {
        let sym = libc::dlsym(
            libc::RTLD_NEXT,
            b"gethostbyaddr\0".as_ptr() as *const c_char,
        );
        if sym.is_null() {
            real_gethostbyaddr_fallback
        } else {
            std::mem::transmute(sym)
        }
    });

    unsafe { real(addr, len, type_) }
}

extern "C" fn real_gethostbyaddr_fallback(
    _addr: *const std::ffi::c_void,
    _len: socklen_t,
    _type: c_int,
) -> *mut libc::hostent {
    std::ptr::null_mut()
}
