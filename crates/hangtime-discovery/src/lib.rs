//! LAN discovery with mDNS/DNS-SD (`_hangtime._udp.local.`).
//!
//! Each node advertises its port and, in TXT records, the protocol major
//! version, radio ID, and identity fingerprint. Browsing reports other
//! nodes speaking the same major version. mDNS is link-local: it does not
//! cross VLANs, routed subnets, or Tailscale, so static peers remain the
//! fallback.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

/// DNS-SD service type.
pub const SERVICE_TYPE: &str = "_hangtime._udp.local.";

/// Protocol major version advertised in the `v` TXT record.
pub const PROTOCOL_MAJOR: &str = "2";

/// What this node advertises.
pub struct Advert<'a> {
    pub radio_id: u32,
    /// Identity fingerprint as lowercase hex, no separators.
    pub fingerprint: &'a str,
    pub name: &'a str,
    pub port: u16,
}

/// Another node found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Advertised fingerprint (lowercase hex). Unverified until connected.
    pub fingerprint: String,
    pub addrs: Vec<SocketAddr>,
}

/// Advertises this node and reports others until dropped.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    events: mdns_sd::Receiver<ServiceEvent>,
    own_fingerprint: String,
}

impl Discovery {
    pub fn start(advert: &Advert<'_>) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        let instance = format!("{} {:08x}", advert.name, advert.radio_id);
        let host = format!("hangtime-{:08x}.local.", advert.radio_id);
        let radio_id = format!("{:08x}", advert.radio_id);
        let properties = [
            ("v", PROTOCOL_MAJOR),
            ("id", radio_id.as_str()),
            ("fp", advert.fingerprint),
        ];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &host,
            "",
            advert.port,
            &properties[..],
        )?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        let events = daemon.browse(SERVICE_TYPE)?;
        Ok(Self {
            daemon,
            fullname,
            events,
            own_fingerprint: advert.fingerprint.to_string(),
        })
    }

    /// Waits for the next node found. Nodes are reported again when they
    /// re-announce, so callers should ignore ones they are connected to.
    pub async fn next(&self) -> Option<Found> {
        loop {
            if let ServiceEvent::ServiceResolved(service) = self.events.recv_async().await.ok()?
                && let Some(found) = self.accept(&service)
            {
                return Some(found);
            }
        }
    }

    fn accept(&self, service: &ResolvedService) -> Option<Found> {
        let addrs = service
            .get_addresses()
            .iter()
            .map(|ip| ip.to_ip_addr())
            .collect::<Vec<_>>();
        filter(
            service.txt_properties.get_property_val_str("v"),
            service.txt_properties.get_property_val_str("fp"),
            &self.own_fingerprint,
            &addrs,
            service.get_port(),
        )
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Keeps nodes that speak our major version and are not us, with their
/// IPv4 addresses (the endpoint binds IPv4) in a stable order.
fn filter(
    version: Option<&str>,
    fingerprint: Option<&str>,
    own_fingerprint: &str,
    addrs: &[IpAddr],
    port: u16,
) -> Option<Found> {
    if version != Some(PROTOCOL_MAJOR) {
        return None;
    }
    let fingerprint = fingerprint?;
    if fingerprint == own_fingerprint {
        return None;
    }
    let mut addrs: Vec<SocketAddr> = addrs
        .iter()
        .filter(|ip| ip.is_ipv4())
        .map(|&ip| SocketAddr::new(ip, port))
        .collect();
    addrs.sort();
    (!addrs.is_empty()).then(|| Found {
        fingerprint: fingerprint.to_string(),
        addrs,
    })
}

#[derive(Debug)]
pub struct DiscoveryError(mdns_sd::Error);

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mDNS: {}", self.0)
    }
}

impl std::error::Error for DiscoveryError {}

impl From<mdns_sd::Error> for DiscoveryError {
    fn from(e: mdns_sd::Error) -> Self {
        Self(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn keeps_peers_on_our_version() {
        let found = filter(
            Some("2"),
            Some("bb"),
            "aa",
            &[ip("192.168.1.9"), ip("10.0.0.2")],
            4747,
        );
        assert_eq!(
            found,
            Some(Found {
                fingerprint: "bb".into(),
                addrs: vec![
                    "10.0.0.2:4747".parse().unwrap(),
                    "192.168.1.9:4747".parse().unwrap()
                ],
            })
        );
    }

    #[test]
    fn skips_self_other_versions_and_ipv6_only() {
        assert_eq!(
            filter(Some("2"), Some("aa"), "aa", &[ip("10.0.0.2")], 1),
            None
        );
        assert_eq!(
            filter(Some("1"), Some("bb"), "aa", &[ip("10.0.0.2")], 1),
            None
        );
        assert_eq!(filter(None, Some("bb"), "aa", &[ip("10.0.0.2")], 1), None);
        assert_eq!(filter(Some("2"), None, "aa", &[ip("10.0.0.2")], 1), None);
        assert_eq!(
            filter(Some("2"), Some("bb"), "aa", &[ip("fe80::1")], 1),
            None
        );
    }
}

/// Uses real multicast on this machine, so it needs a network interface
/// and mDNS allowed through the firewall.
#[cfg(test)]
mod lan_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn two_nodes_find_each_other() {
        let a = Discovery::start(&Advert {
            radio_id: 0xaaaa_aaaa,
            fingerprint: "aa",
            name: "test-a",
            port: 40001,
        })
        .unwrap();
        let b = Discovery::start(&Advert {
            radio_id: 0xbbbb_bbbb,
            fingerprint: "bb",
            name: "test-b",
            port: 40002,
        })
        .unwrap();
        let wait = Duration::from_secs(10);
        let seen_by_a = tokio::time::timeout(wait, a.next()).await.unwrap().unwrap();
        let seen_by_b = tokio::time::timeout(wait, b.next()).await.unwrap().unwrap();
        assert_eq!(seen_by_a.fingerprint, "bb");
        assert!(seen_by_a.addrs.iter().all(|addr| addr.port() == 40002));
        assert_eq!(seen_by_b.fingerprint, "aa");
    }
}
