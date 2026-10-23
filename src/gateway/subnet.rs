use std::{collections::HashMap, net::Ipv6Addr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use log::warn;

use crate::common::Ipv6NetMapping;

pub struct SubnetCalculator;

impl SubnetCalculator {
    pub fn calc(prefix: &Ipv6Net, private_networks: &[Ipv6Net]) -> Result<Ipv6NetMapping> {
        if prefix.prefix_len() > 64 {
            return Err(eyre!(
                "The detected prefix must be /64 or shorter to be split into /64 subnets."
            ));
        }

        let required = private_networks.len();
        // 2^(64 - prefix_len) /64s fit in the delegation. Compute that in constant
        // time. Materializing the iterator is unbounded for a short prefix: a
        // forged "/0" or "/32" log line would otherwise exhaust memory.
        let shift = u32::from(64 - prefix.prefix_len());
        let available = 1u128 << shift;

        if required as u128 > available {
            return Err(eyre!(
                "The prefix {} ({}) does not have enough /64 networks for your setup ({} required, only {} available)",
                prefix,
                prefix.prefix_len(),
                required,
                available
            ));
        }

        let base = u128::from(prefix.network());
        let public_prefixes = (0..required)
            .map(|offset| {
                let offset = u128::try_from(offset)
                    .map_err(|_| eyre!("too many private networks to address"))?;
                let index = available - 1 - offset;
                let bits = index
                    .checked_shl(64)
                    .ok_or_else(|| eyre!("IPv6 subnet index overflow"))?;
                let addr = base
                    .checked_add(bits)
                    .ok_or_else(|| eyre!("IPv6 subnet address overflow"))?;
                Ok(Ipv6Net::new(Ipv6Addr::from(addr), 64)
                    .map_err(|_| eyre!("failed to build a /64 subnet"))?
                    .trunc())
            })
            .collect::<Result<Vec<_>>>()?;

        let mapping = public_prefixes
            .into_iter()
            .zip(private_networks.iter().cloned())
            .filter_map(|(public, private)| {
                if private.prefix_len() != 64 {
                    warn!("Assigned subnet {private} is not a /64. Skipping this mapping.");
                    None
                } else {
                    Some((public, private))
                }
            })
            .collect::<HashMap<_, _>>();

        Ok(mapping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(text: &str) -> Ipv6Net {
        text.parse().unwrap()
    }

    fn legacy_last_subnets(prefix: Ipv6Net, required: usize) -> Vec<Ipv6Net> {
        prefix
            .subnets(64)
            .unwrap()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .take(required)
            .map(|prefix| prefix.trunc())
            .collect()
    }

    #[test]
    fn last_subnets_match_the_previous_iterator_order() {
        let prefix = net("2001:db8::/60");
        let private = [net("fd00:1::/64"), net("fd00:2::/64"), net("fd00:3::/64")];
        let mapping = SubnetCalculator::calc(&prefix, &private).unwrap();
        let expected = legacy_last_subnets(prefix, private.len());

        assert_eq!(mapping.len(), 3);
        for (public, private) in expected.into_iter().zip(private) {
            assert_eq!(mapping.get(&public), Some(&private));
        }
    }

    #[test]
    fn short_prefix_uses_only_the_needed_tail_subnets() {
        let prefix = net("2001:db8::/32");
        let private = [net("fd00::/64")];
        let mapping = SubnetCalculator::calc(&prefix, &private).unwrap();
        let public = net("2001:db8:ffff:ffff::/64");
        assert_eq!(mapping.get(&public), Some(&private[0]));
    }

    #[test]
    fn skips_private_networks_that_are_not_slash_64() {
        let prefix = net("2001:db8::/62");
        let private = [net("fd00:1::/64"), net("fd00:2::/48"), net("fd00:3::/64")];
        let mapping = SubnetCalculator::calc(&prefix, &private).unwrap();
        let publics = legacy_last_subnets(prefix, private.len());

        assert_eq!(mapping.get(&publics[0]), Some(&private[0]));
        assert!(
            !mapping
                .values()
                .any(|private| *private == net("fd00:2::/48"))
        );
        assert_eq!(mapping.get(&publics[2]), Some(&private[2]));
        assert_eq!(mapping.len(), 2);
    }

    #[test]
    fn rejects_prefixes_longer_than_64_and_too_many_networks() {
        let err = SubnetCalculator::calc(&net("2001:db8::/65"), &[net("fd00::/64")]).unwrap_err();
        assert!(err.to_string().contains("/64 or shorter"));

        let err = SubnetCalculator::calc(
            &net("2001:db8::/64"),
            &[net("fd00::/64"), net("fd00:1::/64")],
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not have enough"));
    }
}
