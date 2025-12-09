use std::{collections::BTreeMap, env, net::Ipv6Addr, path::PathBuf, str::FromStr};

use color_eyre::eyre::{Context, Result};
use ipnet::Ipv6Net;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::common::{
    Ipv6AddrMapping, Ipv6NetMapping,
    storage::{LoadFromTomlFile, SaveToTomlFile, config_action_file},
};

pub const PREFIX_ENVIRONMENT: &str = "IPV6_PREFIX";
pub const MAPPING_ENVIRONMENT: &str = "IPV6_MAPPING";

pub struct Action {
    /* Environment */
    _prefix: Ipv6Net,
    mapping: Ipv6NetMapping,

    /* Configuration */
    configuration: Configuration,
}

#[derive(Serialize, Deserialize)]
struct Configuration {
    directory: PathBuf,
    devices: Ipv6AddrMapping,
}

impl Action {
    pub async fn load() -> Result<Self> {
        let _prefix = env::var(PREFIX_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Prefix using the environment variable: {PREFIX_ENVIRONMENT}"))?.parse()?;
        let mapping = serde_json::from_str(&env::var(MAPPING_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Mappings using the environment variable: {MAPPING_ENVIRONMENT}"))?)?;

        let configuration = Configuration::from_file(&{
            let file = config_action_file()?;
            if !file.exists() {
                Configuration {
                    directory: PathBuf::from("records/"),
                    devices: BTreeMap::from([(
                        Ipv6Addr::from_str("fd0a::1")?,
                        Ipv6Addr::from_str("fd0a::1")?,
                    )]),
                }
                .save(&file, true)
                .await?;
            }
            file
        })
        .await?;

        Ok(Self {
            _prefix,
            mapping,
            configuration,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let mut skipped = 0;
        let mut changed = 0;

        let valid_mappings: Vec<_> = self
            .mapping
            .iter()
            .filter(|(public, private)| {
                if private.prefix_len() != public.prefix_len() {
                    warn!(
                        "Prefix length mismatch. Skipping rule: Private {} != Public {}",
                        private, public
                    );
                    return false;
                }
                true
            })
            .collect();

        for (device_address, current_mapped_ip) in self.configuration.devices.iter_mut() {
            if let Some((public_net, private_net)) = valid_mappings
                .iter()
                .find(|(_, private)| private.contains(device_address))
            {
                let new_public_ip =
                    Self::calculate_target_ip(device_address, private_net, public_net);

                if *current_mapped_ip != new_public_ip {
                    debug!(
                        "Updating device {}: {} -> {}",
                        device_address, current_mapped_ip, new_public_ip
                    );
                    *current_mapped_ip = new_public_ip;
                    changed += 1;
                } else {
                    skipped += 1;
                }
            }
        }

        info!(
            "Run complete: {} addresses updated, {} skipped.",
            changed, skipped
        );

        self.configuration
            .save(&config_action_file()?, true)
            .await?;

        Ok(())
    }

    fn calculate_target_ip(
        device_address: &Ipv6Addr,
        private_net: &Ipv6Net,
        public_net: &Ipv6Net,
    ) -> Ipv6Addr {
        (device_address & private_net.hostmask()) | (public_net.addr() & public_net.netmask())
    }
}

impl LoadFromTomlFile for Configuration {}
impl SaveToTomlFile for Configuration {}
