//! Finds the address another device would use to reach this computer.

use std::net::{IpAddr, UdpSocket};

/// Returns the local address that would be used to reach the wider network.
///
/// This connects a UDP socket to a documentation address and reads back the
/// local end. No packet is ever sent: connecting a UDP socket only fixes the
/// route, which is exactly the question being asked. Enumerating interfaces
/// instead would need a C library and would still have to guess which of
/// several addresses is the useful one.
pub fn best_local_address() -> Option<IpAddr> {
    // 192.0.2.0/24 is TEST-NET-1, reserved for documentation and never routed.
    for probe in ["192.0.2.1:9", "[2001:db8::1]:9"] {
        let bind = if probe.starts_with('[') {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        if let Ok(socket) = UdpSocket::bind(bind) {
            if socket.connect(probe).is_ok() {
                if let Ok(local) = socket.local_addr() {
                    if !local.ip().is_loopback() && !local.ip().is_unspecified() {
                        return Some(local.ip());
                    }
                }
            }
        }
    }
    None
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
