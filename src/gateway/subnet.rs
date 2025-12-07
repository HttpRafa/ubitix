use std::collections::HashMap;

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use log::warn;

pub struct SubnetCalculator;

impl SubnetCalculator {
    pub async fn calc(
        prefix: &Ipv6Net,
        private_networks: &[Ipv6Net],
    ) -> Result<HashMap<Ipv6Net, Ipv6Net>> {
        if prefix.prefix_len() > 64 {
            return Err(eyre!(
                "The detected prefix must be /64 or shorter to be split into /64 subnets."
            ));
        }

        let required = private_networks.len();
        let subnets = prefix.subnets(64)?;
        let available = subnets.count();

        if required > available {
            return Err(eyre!(
                "The prefix {} ({}) does not have enough /64 networks for your setup ({} required, only {} available)",
                prefix,
                prefix.prefix_len(),
                required,
                available
            ));
        }

        let mapping = subnets
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .take(required)
            .zip(private_networks.iter().cloned())
            .filter_map(|(public, private)| {
                if public.prefix_len() != 64 {
                    warn!("Assigned subnet {public} is not a /64. Skipping this mapping.");
                    None
                } else if private.prefix_len() != 64 {
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
