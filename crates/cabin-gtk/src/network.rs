//! Reads the host's network interfaces.
//!
//! The remote access setting offers these, so the user can publish a desktop
//! on a home network without publishing it on a hotel one at the same time.
//!
//! `getifaddrs` is the only way to get this from a process. Reading `/proc`
//! gives interface names but not the addresses on them, and an address is
//! what Podman needs. See DESIGN.md section 7.5.

use cabin_core::NetInterface;
use std::collections::{BTreeMap, HashSet};
use std::ffi::CStr;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Every interface that currently holds an address, loopback included.
///
/// The list is sorted by name so the settings dialogue does not reorder
/// itself between openings.
pub fn interfaces() -> Vec<NetInterface> {
    let mut found: BTreeMap<String, NetInterface> = BTreeMap::new();
    let unstable = unstable_v6(&std::fs::read_to_string("/proc/net/if_inet6").unwrap_or_default());

    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes one owned list to head, or returns non-zero
    // and writes nothing.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        tracing::warn!("could not read the network interfaces");
        return Vec::new();
    }

    let mut current = head;
    while !current.is_null() {
        // SAFETY: the list is walked only while the node is non-null, and
        // getifaddrs owns it until freeifaddrs below.
        let entry = unsafe { &*current };
        current = entry.ifa_next;

        if entry.ifa_name.is_null() || entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: ifa_name is a NUL terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();

        // An interface that is down holds no address another device can use.
        let up = entry.ifa_flags & libc::IFF_UP as u32 != 0;
        let running = entry.ifa_flags & libc::IFF_RUNNING as u32 != 0;
        if !up || !running {
            continue;
        }
        let loopback = entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;

        let Some(address) = read_address(entry.ifa_addr, &unstable) else {
            continue;
        };

        let interface = found.entry(name.clone()).or_insert_with(|| NetInterface {
            name,
            addresses: Vec::new(),
            loopback,
        });
        if !interface.addresses.contains(&address) {
            interface.addresses.push(address);
        }
    }

    // SAFETY: head came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(head) };
    found.into_values().collect()
}

/// Converts one `sockaddr` to a printable address, ignoring anything that is
/// not IPv4 or IPv6.
///
/// A link-local IPv6 address is skipped. It needs a scope identifier to be
/// usable, Podman will not accept one, and it is never the address a user
/// would type into another device.
///
/// Only unique-local IPv6 (fc00::/7) is kept. A global address is reachable
/// from the whole internet unless the router filters it, and that is what
/// "Every network" is for. "Chosen networks" must stay the narrower choice.
/// The tailscale0 address is unique-local, so it stays.
///
/// Temporary and deprecated addresses are skipped too, because they rotate.
/// The address is resolved when the container is created, so a rotated one
/// leaves the desktop unable to start, the same fault as a stored address.
fn read_address(addr: *const libc::sockaddr, unstable: &HashSet<Ipv6Addr>) -> Option<String> {
    // SAFETY: the caller checked that addr is non-null, and sa_family is
    // present in every sockaddr variant.
    let family = unsafe { (*addr).sa_family } as i32;
    match family {
        libc::AF_INET => {
            // SAFETY: the family says this is a sockaddr_in.
            let v4 = unsafe { &*(addr as *const libc::sockaddr_in) };
            let octets = u32::from_be(v4.sin_addr.s_addr).to_be_bytes();
            Some(Ipv4Addr::from(octets).to_string())
        }
        libc::AF_INET6 => {
            // SAFETY: the family says this is a sockaddr_in6.
            let v6 = unsafe { &*(addr as *const libc::sockaddr_in6) };
            let address = Ipv6Addr::from(v6.sin6_addr.s6_addr);
            if !publishable_v6(&address) || unstable.contains(&address) {
                return None;
            }
            Some(address.to_string())
        }
        _ => None,
    }
}

/// True for an IPv6 address that stays inside the networks it belongs to:
/// a unique-local address, fc00::/7.
fn publishable_v6(address: &Ipv6Addr) -> bool {
    address.is_unique_local()
}

/// The IPv6 addresses that the kernel marks temporary or deprecated.
///
/// `getifaddrs` does not report address flags, so they are read from
/// `/proc/net/if_inet6`. Each line holds the address as 32 hex digits, the
/// interface index, the prefix length, the scope, the flags and the name.
fn unstable_v6(if_inet6: &str) -> HashSet<Ipv6Addr> {
    const TEMPORARY: u32 = 0x01;
    const DEPRECATED: u32 = 0x20;
    if_inet6
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let hex = fields.first()?;
            let flags = u32::from_str_radix(fields.get(4)?, 16).ok()?;
            if flags & (TEMPORARY | DEPRECATED) == 0 {
                return None;
            }
            let bits = u128::from_str_radix(hex, 16).ok()?;
            (hex.len() == 32).then(|| Ipv6Addr::from(bits))
        })
        .collect()
}

/// The interfaces worth offering as a choice: everything except loopback.
///
/// Loopback is always bound, so offering it would suggest it could be turned
/// off, which it cannot.
pub fn selectable() -> Vec<NetInterface> {
    interfaces().into_iter().filter(|i| !i.loopback).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_reported_holds_an_address() {
        // A machine with no network returns an empty list, which is correct.
        // What it must never do is report an interface with nothing on it,
        // because that would publish a desktop on no address at all.
        for interface in interfaces() {
            assert!(
                !interface.addresses.is_empty(),
                "{} was reported with no address",
                interface.name
            );
        }
    }

    #[test]
    fn prints_what_this_host_has() {
        // Not an assertion. Enumeration is the one part of this that cannot
        // be checked without a real host, so it is printed for `--nocapture`.
        for interface in interfaces() {
            println!(
                "{} loopback={} {}",
                interface.name,
                interface.loopback,
                interface.addresses.join(", ")
            );
        }
    }

    #[test]
    fn only_unique_local_ipv6_is_published() {
        let ok = |a: &str| publishable_v6(&a.parse().unwrap());
        assert!(ok("fd7a:115c:a1e0::1"), "tailscale is unique-local");
        assert!(ok("fc00::1"));
        assert!(!ok("2001:db8::1"), "a global address reaches the internet");
        assert!(!ok("fe80::1"));
        assert!(!ok("::1"));
    }

    #[test]
    fn temporary_and_deprecated_addresses_are_found() {
        let table = "\
fd7a115ca1e000000000000000000001 04 80 00 80 tailscale0
fd000000000000000000000000000001 02 40 00 01 eth0
fd000000000000000000000000000002 02 40 00 20 eth0
fe800000000000000000000000000001 0b 40 20 80 veth0
";
        let unstable = unstable_v6(table);
        assert_eq!(unstable.len(), 2);
        assert!(unstable.contains(&"fd00::1".parse().unwrap()));
        assert!(unstable.contains(&"fd00::2".parse().unwrap()));
        assert!(!unstable.contains(&"fd7a:115c:a1e0::1".parse().unwrap()));
    }

    #[test]
    fn a_missing_address_table_marks_nothing() {
        assert!(unstable_v6("").is_empty());
    }

    #[test]
    fn loopback_is_not_offered_as_a_choice() {
        assert!(selectable().iter().all(|i| !i.loopback));
    }
}
