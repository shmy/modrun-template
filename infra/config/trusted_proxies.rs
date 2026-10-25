use std::net::IpAddr;
use std::str::FromStr as _;

use ipnet::IpNet;

#[derive(Clone, Debug, Default)]
pub struct TrustedProxies {
    networks: Vec<IpNet>,
}

impl TrustedProxies {
    pub fn parse(raw: &str) -> Self {
        let networks = raw
            .split(',')
            .filter_map(|entry| {
                let entry = entry.trim();
                if entry.is_empty() {
                    return None;
                }
                IpNet::from_str(entry)
                    .ok()
                    .or_else(|| entry.parse::<IpAddr>().ok().map(IpNet::from))
            })
            .collect();
        Self { networks }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.networks.iter().any(|network| network.contains(&ip))
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn parses_exact_ip_and_cidr() {
        let trusted = TrustedProxies::parse("127.0.0.1,10.0.0.0/8");
        let localhost = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        let internal = IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3));
        let external = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        assert!(trusted.contains(localhost));
        assert!(trusted.contains(internal));
        assert!(!trusted.contains(external));
    }
}
