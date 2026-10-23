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
        let (postrouting, prerouting) = netmap_rules(public, private)?;
        iptables.append("nat", "POSTROUTING", &postrouting)?;
        iptables.append("nat", "PREROUTING", &prerouting)?;
        Ok(())
    }

    async fn delete_rules(
        iptables: &IPTables,
        public: &Ipv6Net,
        private: &Ipv6Net,
    ) -> Result<(), Box<dyn Error>> {
        let (postrouting, prerouting) = netmap_rules(public, private)?;
        iptables.delete("nat", "POSTROUTING", &postrouting)?;
        iptables.delete("nat", "PREROUTING", &prerouting)?;
        Ok(())
    }
}

fn netmap_rules(public: &Ipv6Net, private: &Ipv6Net) -> Result<(String, String), Box<dyn Error>> {
    if !network_token_is_safe(&public.to_string()) || !network_token_is_safe(&private.to_string()) {
        return Err("refusing to pass an unexpected prefix to ip6tables".into());
    }
    Ok((
        format!("-s {private} -j NETMAP --to {public}"),
        format!("-d {public} -j NETMAP --to {private}"),
    ))
}

/// `ip6tables` receives the prefix as an argument. Only the characters that
/// can appear in an IPv6 prefix are allowed, so a corrupted state file cannot
/// smuggle extra options.
pub fn network_token_is_safe(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'/' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iptables_tokens_reject_extra_arguments() {
        assert!(network_token_is_safe("2001:db8::/56"));
        assert!(network_token_is_safe("::ffff:192.0.2.1/128"));
        assert!(!network_token_is_safe("2001:db8::/56 -j ACCEPT"));
        assert!(!network_token_is_safe(""));
        assert!(!network_token_is_safe("2001:db8::/56;reboot"));
    }
}
