//! Reads the host's network interfaces.
//!
//! The remote access setting offers these, so the user can publish a desktop
//! on a home network without publishing it on a hotel one at the same time.
//!
//! `getifaddrs` is the only way to get this from a process. Reading `/proc`
//! gives interface names but not the addresses on them, and an address is
//! what Podman needs. See DESIGN.md section 7.5.

use cabin_core::NetInterface;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Every interface that currently holds an address, loopback included.
///
/// The list is sorted by name so the settings dialogue does not reorder
/// itself between openings.
pub fn interfaces() -> Vec<NetInterface> {
    let mut found: BTreeMap<String, NetInterface> = BTreeMap::new();

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

        let Some(address) = read_address(entry.ifa_addr) else {
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
fn read_address(addr: *const libc::sockaddr) -> Option<String> {
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
            if address.is_unicast_link_local() {
                return None;
            }
            Some(address.to_string())
        }
        _ => None,
    }
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
    fn loopback_is_not_offered_as_a_choice() {
        assert!(selectable().iter().all(|i| !i.loopback));
    }
}
