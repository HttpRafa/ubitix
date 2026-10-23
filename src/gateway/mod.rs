use std::{path::PathBuf, str::FromStr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use iptables::IPTables;
use log::{error, info, warn};
use octocrab::Octocrab;
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use tokio::{fs, select, signal};

use crate::{
    common::{
        Ipv6NetMapping, State,
        storage::{LoadFromTomlFile, SaveToTomlFile, config_gateway_file, state_gateway_file},
    },
    gateway::{rules::IPTableRules, subnet::SubnetCalculator, watcher::FileWatcher},
};

pub mod prefix;
pub mod rules;
pub mod subnet;
pub mod watcher;

const DEFAULT_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/configs/gateway.toml"));

pub struct Gateway {
    /* Configuration */
    configuration: Configuration,

    /* State */
    state: State,

    /* Runtime */
    iptables: IPTables,
    regex: Regex,
    octocrab: Octocrab,
}

#[derive(Deserialize)]
struct Configuration {
    file: PathBuf,
    regex: String,
    interface: String,
    networks: Vec<Ipv6Net>,
    token: String,
    owner: String,
    repository: String,
    workflow: String,
    /// When true, a prefix from the log is applied only if the kernel already
    /// has that exact prefix on the named interface (or as a route on `lo`).
    #[serde(default = "default_verify_prefix")]
    verify_prefix: bool,
}

fn default_verify_prefix() -> bool {
    true
}

impl Gateway {
    pub async fn load() -> Result<Self> {
        let configuration = Configuration::from_file(&{
            let file = config_gateway_file()?;
            if !file.exists() {
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::write(&file, DEFAULT_CONFIG).await?;
            }
            // The file holds a GitHub token. Refuse symlinks and drop group/other access.
            crate::common::storage::restrict_secret_file(&file)?;
            file
        })
        .await?;

        Ok(Self {
            iptables: iptables::new(true).map_err(|error| eyre!("{error:?}"))?,
            regex: Regex::new(&configuration.regex)?,
            octocrab: Octocrab::builder()
                .personal_token(configuration.token.clone())
                .build()?,
            state: State::from_file(&state_gateway_file()?)
                .await
                .unwrap_or_default(),
            configuration,
        })
    }

    async fn update_state(&mut self, prefix: Ipv6Net, mapping: Ipv6NetMapping) -> Result<()> {
        self.state.prefix = prefix;
        self.state.mapping = mapping;
        self.state.save(&state_gateway_file()?, true).await?;
        Ok(())
    }

    async fn dispatch_workflow(&self, prefix: &Ipv6Net, mapping: &Ipv6NetMapping) -> Result<()> {
        self.octocrab
            .actions()
            .create_workflow_dispatch(
                &self.configuration.owner,
                &self.configuration.repository,
                &self.configuration.workflow,
                "main",
            )
            .inputs(json!({
                "prefix": prefix,
                "mapping": format!("{}", json!(mapping)),
            }))
            .send()
            .await?;
        Ok(())
    }

    pub async fn handle_line(&mut self, line: String) -> Result<()> {
        if let Some(captures) = self.regex.captures(&line)
            && let Some(prefix) = captures.get(1)
            && let Some(interface) = captures.get(2)
        {
            let prefix = Ipv6Net::from_str(prefix.as_str())?;
            let interface = interface.as_str();
            if self.configuration.interface != interface {
                info!(
                    "Prefix change detected on a different interface ({interface}): Unknown -> {prefix} (no action taken)"
                );
                return Ok(());
            }

            if self.state.prefix != prefix {
                if !prefix::acceptable_delegated_prefix(&prefix) {
                    warn!(
                        "Ignoring prefix {prefix} on {interface}: refusing link-local, multicast, loopback, or longer-than-/64 prefixes"
                    );
                    return Ok(());
                }
                if self.configuration.verify_prefix {
                    match prefix::delegated_prefix_is_installed(&prefix, interface).await {
                        Ok(true) => {}
                        Ok(false) => {
                            warn!(
                                "Ignoring prefix {prefix} on {interface}: the kernel has no matching route or address. Set verify_prefix = false only when this host does not install delegated prefixes locally"
                            );
                            return Ok(());
                        }
                        Err(error) => {
                            error!(
                                "Refusing prefix {prefix} on {interface}: could not verify it against the kernel: {error}"
                            );
                            return Ok(());
                        }
                    }
                }

                info!(
                    "Prefix updated on target interface {interface}: {} -> {prefix}",
                    self.state.prefix
                );

                IPTableRules::delete_all_rules(&self.iptables, &self.state.mapping).await;

                let mapping = SubnetCalculator::calc(&prefix, &self.configuration.networks)?;
                IPTableRules::append_all_rules(&self.iptables, &mapping).await;

                info!("Dispatching Github Workflow...");
                if let Err(error) = self.dispatch_workflow(&prefix, &mapping).await {
                    error!("Failed to dispatch Github Workflow: {error:?}");
                }

                self.update_state(prefix, mapping).await?;
            } else {
                warn!(
                    "Received redundant prefix change for interface {interface}: {} -> {prefix} (no action taken)",
                    self.state.prefix
                );
            }
        }
        Ok(())
    }

    pub async fn run(mut self) -> Result<()> {
        IPTableRules::append_all_rules(&self.iptables, &self.state.mapping).await;

        let file = self.configuration.file.clone();

        info!("Starting file watcher...");
        select! {
            _ = signal::ctrl_c() => {

            },
            result = FileWatcher::watch(&file, &mut self, async |gateway, line| {
                gateway.handle_line(line).await?;
                Ok(())
            }) => {
                result?
            }
        }

        info!("Starting cleanup...");
        IPTableRules::delete_all_rules(&self.iptables, &self.state.mapping).await;
        Ok(())
    }
}

impl LoadFromTomlFile for Configuration {}

impl SaveToTomlFile for State {}
impl LoadFromTomlFile for State {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_gateway_config_enables_prefix_verification() {
        let configuration: Configuration = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert!(configuration.verify_prefix);
        assert_eq!(configuration.interface, "eth9");
        assert_eq!(configuration.file, PathBuf::from("/var/log/messages"));
    }

    #[test]
    fn existing_configs_verify_prefixes_by_default() {
        let configuration: Configuration = toml::from_str(
            r#"
            file = "/var/log/messages"
            regex = "prefix"
            interface = "ppp1"
            networks = []
            token = "some-token"
            owner = "owner"
            repository = "repo"
            workflow = "records.yml"
            "#,
        )
        .unwrap();
        assert!(configuration.verify_prefix);
    }
}
