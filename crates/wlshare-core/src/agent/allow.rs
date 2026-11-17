//! Where an agent lets a daemon connect from: networks of IPv4 or IPv6
//! addresses, each written as a CIDR block, one address being a block of
//! one (`/32`, `/128`).
//!
//! An agent has no default here and does not start without one
//! ([`crate::agent::serve`]): who may so much as try the key is for whoever
//! runs the agent to say, and the answer is the daemon's address, not the
//! network it happens to be on.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use anyhow::Context as _;

/// A network: an address, and how many of its leading bits every address in
/// the network shares with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Net {
    address: IpAddr,
    prefix: u8,
}

/// An address's bits from the top of 128, and how many of them it has.
fn bits(address: IpAddr) -> (u128, u8) {
    match address {
        IpAddr::V4(address) => (u128::from(u32::from(address)) << 96, 32),
        IpAddr::V6(address) => (u128::from(address), 128),
    }
}

/// The top `prefix` bits of 128.
fn mask(prefix: u8) -> u128 {
    u128::MAX
        .checked_shl(128 - u32::from(prefix))
        .unwrap_or_default()
}

impl Net {
    /// Whether `address` is in the network: one of its family, sharing its
    /// prefix. An IPv4 address a dual-stack listener reports as IPv6 is to
    /// be made IPv4 again first ([`IpAddr::to_canonical`]).
    pub fn contains(&self, address: IpAddr) -> bool {
        let ((ours, width), (theirs, family)) = (bits(self.address), bits(address));
        width == family && (ours ^ theirs) & mask(self.prefix) == 0
    }
}

impl std::fmt::Display for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

impl FromStr for Net {
    type Err = anyhow::Error;

    /// `192.168.1.10/32`, `192.168.1.0/24`, `fd00:1::10/128`, `fd00:1::/64`:
    /// always with its prefix, one address having all its bits for one. A
    /// block is written as its network, with no bit set past its prefix, so
    /// that what is allowed is what was written and not a network around it.
    fn from_str(written: &str) -> anyhow::Result<Self> {
        let (address, prefix) = written.split_once('/').unwrap_or((written, ""));
        let address: IpAddr = address
            .parse()
            .ok()
            .with_context(|| format!("{address:?} is no IPv4 or IPv6 address"))?;
        // A daemon's address is compared as IPv4 where it is IPv4.
        anyhow::ensure!(
            address.to_canonical() == address,
            "{address} is an IPv4 address written as IPv6: write it as {}",
            address.to_canonical()
        );
        let (bits, width) = bits(address);
        anyhow::ensure!(
            written.contains('/'),
            "{address} has no prefix: that one address is {address}/{width}"
        );
        let prefix: u8 = prefix
            .parse()
            .ok()
            .filter(|length| prefix.bytes().all(|b| b.is_ascii_digit()) && *length <= width)
            .with_context(|| {
                format!("/{prefix} is no prefix of {address}, which has /0 to /{width}")
            })?;
        let network = bits & mask(prefix);
        anyhow::ensure!(
            network == bits,
            "{written} is an address inside {}/{prefix}: write that network, or the one address \
             as {address}/{width}",
            match address {
                IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from((network >> 96) as u32)),
                IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(network)),
            }
        );
        Ok(Self { address, prefix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(written: &str) -> Net {
        written.parse().unwrap()
    }

    fn at(address: &str) -> IpAddr {
        address.parse().unwrap()
    }

    /// A network holds the addresses that share its prefix, of its own
    /// family alone; a prefix of every bit is one address; and a prefix of
    /// no bits is every address of the family.
    #[test]
    fn a_network_is_the_addresses_of_its_prefix() {
        let lan = net("192.168.1.0/24");
        assert!(lan.contains(at("192.168.1.0")) && lan.contains(at("192.168.1.255")));
        assert!(!lan.contains(at("192.168.2.1")) && !lan.contains(at("192.168.0.255")));
        assert!(!lan.contains(at("::ffff:192.168.1.7")));
        assert!(lan.contains(at("::ffff:192.168.1.7").to_canonical()));

        let odd = net("10.1.2.128/25");
        assert!(odd.contains(at("10.1.2.129")) && !odd.contains(at("10.1.2.127")));

        let one = net("192.168.1.10/32");
        assert!(one.contains(at("192.168.1.10")) && !one.contains(at("192.168.1.11")));

        let ula = net("fd00:1::/64");
        assert!(ula.contains(at("fd00:1::10")) && ula.contains(at("fd00:1:0:0:ffff::")));
        assert!(!ula.contains(at("fd00:1:0:1::10")) && !ula.contains(at("10.0.0.1")));
        let host = net("fd00:1::10/128");
        assert!(host.contains(at("fd00:1::10")) && !host.contains(at("fd00:1::11")));

        let (every4, every6) = (net("0.0.0.0/0"), net("::/0"));
        assert!(every4.contains(at("203.0.113.9")) && !every4.contains(at("2001:db8::1")));
        assert!(every6.contains(at("2001:db8::1")) && !every6.contains(at("203.0.113.9")));

        for written in ["192.168.1.0/24", "fd00:1::/64", "10.0.0.1/32"] {
            assert_eq!(net(written).to_string(), written);
        }
    }

    /// What is no network is refused in words that say what would be one.
    #[test]
    fn what_is_no_network_is_refused() {
        let why = |written: &str| written.parse::<Net>().unwrap_err().to_string();
        assert!(why("192.168.1.20/24").contains("192.168.1.0/24"));
        assert!(why("fd00:1::10/64").contains("fd00:1::/64"));
        assert!(why("10.0.0.0/33").contains("/0 to /32"));
        assert!(why("fd00::/129").contains("/0 to /128"));
        assert!(why("::ffff:10.0.0.1/128").contains("write it as 10.0.0.1"));
        assert!(why("192.168.1.10").contains("192.168.1.10/32"));
        assert!(why("fd00:1::10").contains("fd00:1::10/128"));
        for written in [
            "",
            "lan",
            "10.0.0/8",
            "10.0.0.0/",
            "10.0.0.0/+8",
            "10.0.0.0/8/8",
        ] {
            assert!(written.parse::<Net>().is_err(), "{written:?}");
        }
    }
}
