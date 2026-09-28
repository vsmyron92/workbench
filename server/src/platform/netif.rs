//! Local network interface addresses (for "reach Workbench from your phone at …").

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub addr: IpAddr,
}

/// Addresses of interfaces that are up (loopback included; see `classify`).
pub fn list() -> Vec<Iface> {
    let mut out = vec![];
    // SAFETY: getifaddrs allocates a linked list that we only read and then free
    // with freeifaddrs; every pointer is checked for null before it is dereferenced,
    // and sockaddr casts follow the address family reported by the kernel.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() || ifa.ifa_flags & (libc::IFF_UP as libc::c_uint) == 0 {
                continue;
            }
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            let addr = match i32::from((*ifa.ifa_addr).sa_family) {
                libc::AF_INET => {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
                }
                libc::AF_INET6 => {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            out.push(Iface { name, addr });
        }
        libc::freeifaddrs(head);
    }
    out
}

/// `lan`, `tailscale` or `virtual` (containers, VMs); `None` for addresses no
/// other device can use (loopback, IPv6 link-local).
pub fn classify(name: &str, addr: &IpAddr) -> Option<&'static str> {
    if addr.is_loopback() || addr.is_unspecified() {
        return None;
    }
    let tailscale = match addr {
        // 100.64.0.0/10 (CGNAT range Tailscale assigns from)
        IpAddr::V4(v4) => v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64,
        // fd7a:115c:a1e0::/48
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    };
    if let IpAddr::V6(v6) = addr {
        if (v6.segments()[0] & 0xffc0) == 0xfe80 {
            return None;
        }
    }
    if tailscale || name.starts_with("tailscale") {
        return Some("tailscale");
    }
    const VIRTUAL: &[&str] = &["docker", "br-", "veth", "virbr", "lxc", "lxd", "cni", "flannel", "podman", "vmnet", "vboxnet", "zt"];
    if VIRTUAL.iter().any(|p| name.starts_with(p)) {
        return Some("virtual");
    }
    Some("lan")
}

/// `host[:port]` form of an address for URLs (IPv6 in brackets).
pub fn url_host(addr: &IpAddr, port: u16) -> String {
    match addr {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(classify("lo", &ip("127.0.0.1")), None);
        assert_eq!(classify("wlp3s0", &ip("192.168.1.20")), Some("lan"));
        assert_eq!(classify("tailscale0", &ip("100.101.102.103")), Some("tailscale"));
        assert_eq!(classify("eth0", &ip("100.64.0.1")), Some("tailscale"));
        assert_eq!(classify("eth0", &ip("100.128.0.1")), Some("lan"));
        assert_eq!(classify("tailscale0", &ip("fd7a:115c:a1e0::1")), Some("tailscale"));
        assert_eq!(classify("docker0", &ip("172.17.0.1")), Some("virtual"));
        assert_eq!(classify("wlp3s0", &ip("fe80::1")), None);
        assert_eq!(url_host(&ip("::1"), 7777), "[::1]:7777");
    }

    #[test]
    fn lists_at_least_loopback() {
        let all = list();
        assert!(all.iter().any(|i| i.addr.is_loopback()), "{all:?}");
    }
}
