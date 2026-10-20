//! Finds the address another device would use to reach this computer.

use std::net::{IpAddr, UdpSocket};

/// Returns the local IPv4 address that would be used to reach the wider
/// network.
///
/// This connects a UDP socket to a documentation address and reads back the
/// local end. No packet is ever sent: connecting a UDP socket only fixes the
/// route, which is exactly the question being asked. Enumerating interfaces
/// instead would need a C library and would still have to guess which of
/// several addresses is the useful one.
///
/// Only IPv4 is asked about. "Every network" publishes the port on 0.0.0.0,
/// and pasta binds that as a dual-stack socket, so the desktop also answers
/// on IPv6. But the dialogue shows one address, and an IPv4 address on the
/// local network is the one most likely to work from a nearby device.
pub fn best_local_address() -> Option<IpAddr> {
    // 192.0.2.0/24 is TEST-NET-1, reserved for documentation and never routed.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let local = socket.local_addr().ok()?.ip();
    if local.is_loopback() || local.is_unspecified() {
        return None;
    }
    Some(local)
}

/// How the user should describe the address to themselves.
pub fn describe(address: Option<IpAddr>) -> String {
    match address {
        Some(ip) => ip.to_string(),
        None => "this computer's IP address".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_returns_loopback_or_unspecified() {
        // On a machine with no network this returns None, which is correct.
        // What it must never do is offer an address another device cannot use.
        if let Some(ip) = best_local_address() {
            assert!(ip.is_ipv4(), "every network is published on IPv4 only");
            assert!(!ip.is_loopback(), "loopback is useless to another device");
            assert!(
                !ip.is_unspecified(),
                "the unspecified address is not reachable"
            );
        }
    }

    #[test]
    fn describes_a_missing_address_in_words() {
        assert_eq!(describe(None), "this computer's IP address");
        assert_eq!(
            describe(Some("192.168.1.5".parse().unwrap())),
            "192.168.1.5"
        );
    }
}
