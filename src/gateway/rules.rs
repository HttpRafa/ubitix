use color_eyre::eyre::eyre;
use ipnet::Ipv6Net;
use iptables::IPTables;
use log::{error, info};
use std::error::Error;

use crate::common::Ipv6NetMapping;

pub struct IPTableRules;

impl IPTableRules {
    pub async fn append_all_rules(iptables: &IPTables, mapping: &Ipv6NetMapping) {
        info!("Appending {} new NPTv6 rules:", mapping.len() * 2);
        for (public, private) in mapping {
            info!("+: {public} <---> {private}");
            if let Err(error) = Self::append_rules(iptables, public, private).await {
                error!("Failed to append iptables rules: {:?}", eyre!("{error}"));
            }
        }
    }

    pub async fn delete_all_rules(iptables: &IPTables, mapping: &Ipv6NetMapping) {
        info!("Deleting {} old NPTv6 rules:", mapping.len() * 2);
        for (public, private) in mapping {
            info!("-: {public} <---> {private}");
            if let Err(error) = Self::delete_rules(iptables, public, private).await {
                error!("Failed to delete iptables rules: {:?}", eyre!("{error}"));
            }
        }
    }

    async fn append_rules(
        iptables: &IPTables,
        public: &Ipv6Net,
        private: &Ipv6Net,
    ) -> Result<(), Box<dyn Error>> {
        iptables.append(
            "nat",
            "POSTROUTING",
            &format!("-s {private} -j NETMAP --to {public}"),
        )?;
        iptables.append(
            "nat",
            "PREROUTING",
            &format!("-d {public} -j NETMAP --to {private}"),
        )?;
        Ok(())
    }

    async fn delete_rules(
        iptables: &IPTables,
        public: &Ipv6Net,
        private: &Ipv6Net,
    ) -> Result<(), Box<dyn Error>> {
        iptables.delete(
            "nat",
            "POSTROUTING",
            &format!("-s {private} -j NETMAP --to {public}"),
        )?;
        iptables.delete(
            "nat",
            "PREROUTING",
            &format!("-d {public} -j NETMAP --to {private}"),
        )?;
        Ok(())
    }
}
