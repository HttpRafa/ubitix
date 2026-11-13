use std::{collections::HashMap, net::Ipv6Addr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use log::warn;

use crate::common::Ipv6NetMapping;

pub struct SubnetCalculator;

impl SubnetCalculator {
    /// Pair each private network with a `/64` taken from the end of `prefix`.
    ///
    /// Only `/56` through `/64` are accepted. The last `/64` pairs with
    /// `private_networks[0]`, the one before that with `[1]`, and so on.
    /// A private network that is not a `/64` still consumes its public slot.
    pub fn calc(prefix: &Ipv6Net, private_networks: &[Ipv6Net]) -> Result<Ipv6NetMapping> {
        let prefix_len = prefix.prefix_len();
        if !(56..=64).contains(&prefix_len) {
            return Err(eyre!(
                "The detected prefix /{prefix_len} must be between /56 and /64 to be split into /64 subnets."
            ));
        }

        let required = private_networks.len();
        // `prefix_len` is in 56..=64, so this shift is 0..=8 and never walks the prefix.
        let available = 1usize << u32::from(64 - prefix_len);
        if required > available {
            return Err(eyre!(
                "The prefix {} ({}) does not have enough /64 networks for your setup ({} required, only {} available)",
                prefix,
                prefix_len,
                required,
                available
            ));
        }

        let base = u128::from(prefix.network());
        let mut mapping = HashMap::with_capacity(required);
        for (offset, private) in private_networks.iter().enumerate() {
            if private.prefix_len() != 64 {
                warn!("Assigned subnet {private} is not a /64. Skipping this mapping.");
                continue;
            }

            // The /64 index occupies the address bits immediately above the 64-bit host.
            let index = (available - 1 - offset) as u128;
            let public = Ipv6Net::new(Ipv6Addr::from(base | (index << 64)), 64)?;
            mapping.insert(public, *private);
        }

        Ok(mapping)
    }
}

#[cfg(test)]
mod tests {
    use super::SubnetCalculator;
    use ipnet::Ipv6Net;

    fn net(value: &str) -> Ipv6Net {
        value.parse().unwrap()
    }

    #[test]
    fn slash_56_assigns_from_the_end() {
        let mapping = SubnetCalculator::calc(
            &net("2001:db8:1::/56"),
            &[net("fd00:a::/64"), net("fd00:b::/64")],
        )
        .unwrap();

        assert_eq!(mapping.len(), 2);
        assert_eq!(
            mapping.get(&net("2001:db8:1:ff::/64")),
            Some(&net("fd00:a::/64"))
        );
        assert_eq!(
            mapping.get(&net("2001:db8:1:fe::/64")),
            Some(&net("fd00:b::/64"))
        );
        assert!(!mapping.contains_key(&net("2001:db8:1::/64")));
    }

    #[test]
    fn slash_60_assigns_from_the_end() {
        let mapping = SubnetCalculator::calc(
            &net("2001:db8:1::/60"),
            &[net("fd00:a::/64"), net("fd00:b::/64")],
        )
        .unwrap();

        assert_eq!(mapping.len(), 2);
        assert_eq!(
            mapping.get(&net("2001:db8:1:f::/64")),
            Some(&net("fd00:a::/64"))
        );
        assert_eq!(
            mapping.get(&net("2001:db8:1:e::/64")),
            Some(&net("fd00:b::/64"))
        );
    }

    #[test]
    fn slash_64_maps_the_prefix_itself() {
        let mapping =
            SubnetCalculator::calc(&net("2001:db8:1:ff::/64"), &[net("fd00:a::/64")]).unwrap();

        assert_eq!(mapping.len(), 1);
        assert_eq!(
            mapping.get(&net("2001:db8:1:ff::/64")),
            Some(&net("fd00:a::/64"))
        );
    }

    #[test]
    fn rejects_prefixes_outside_56_through_64() {
        let private = [net("fd00:a::/64")];
        assert!(SubnetCalculator::calc(&net("2001:db8::/55"), &private).is_err());
        assert!(SubnetCalculator::calc(&net("2001:db8::/32"), &private).is_err());
    }

    #[test]
    fn non_64_private_network_consumes_its_slot() {
        let mapping = SubnetCalculator::calc(
            &net("2001:db8:1::/56"),
            &[net("fd00:a::/48"), net("fd00:b::/64")],
        )
        .unwrap();

        assert_eq!(mapping.len(), 1);
        assert!(!mapping.contains_key(&net("2001:db8:1:ff::/64")));
        assert_eq!(
            mapping.get(&net("2001:db8:1:fe::/64")),
            Some(&net("fd00:b::/64"))
        );
    }
}
