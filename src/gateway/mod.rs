use std::{collections::HashMap, error::Error, path::PathBuf, str::FromStr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use iptables::IPTables;
use log::{debug, error, info, warn};
use octocrab::Octocrab;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::fs;

use crate::{
    common::{
        State,
        storage::{LoadFromTomlFile, SaveToTomlFile, config_gateway_file, state_gateway_file},
    },
    gateway::{subnet::SubnetCalculator, watcher::FileWatcher},
};

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

#[derive(Serialize, Deserialize)]
struct Configuration {
    file: PathBuf,
    regex: String,
    networks: Vec<Ipv6Net>,
    token: String,
    owner: String,
    repository: String,
    workflow: String,
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

    async fn update_state(
        &mut self,
        prefix: Ipv6Net,
        mapping: HashMap<Ipv6Net, Ipv6Net>,
    ) -> Result<()> {
        self.state.prefix = prefix;
        self.state.mapping = mapping;
        self.state.save(&state_gateway_file()?, true).await?;
        Ok(())
    }

    async fn dispatch_workflow(
        &self,
        prefix: &Ipv6Net,
        mapping: &HashMap<Ipv6Net, Ipv6Net>,
    ) -> Result<()> {
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

    async fn append_rules(
        &self,
        public: &Ipv6Net,
        private: &Ipv6Net,
    ) -> Result<(), Box<dyn Error>> {
        self.iptables.append(
            "nat",
            "POSTROUTING",
            &format!("-s {private} -j NETMAP --to {public}"),
        )?;
        self.iptables.append(
            "nat",
            "PREROUTING",
            &format!("-d {public} -j NETMAP --to {private}"),
        )?;
        Ok(())
    }

    async fn delete_rules(
        &self,
        public: &Ipv6Net,
        private: &Ipv6Net,
    ) -> Result<(), Box<dyn Error>> {
        self.iptables.delete(
            "nat",
            "POSTROUTING",
            &format!("-s {private} -j NETMAP --to {public}"),
        )?;
        self.iptables.delete(
            "nat",
            "PREROUTING",
            &format!("-d {public} -j NETMAP --to {private}"),
        )?;
        Ok(())
    }

    pub async fn handle_line(&mut self, line: String) -> Result<()> {
        debug!("> {line}");
        if let Some(captures) = self.regex.captures(&line)
            && let Some(prefix) = captures.get(1)
        {
            let prefix = Ipv6Net::from_str(prefix.as_str())?;

            if self.state.prefix != prefix {
                info!("Prefix change detected: {} -> {prefix}", self.state.prefix);

                info!("Deleting {} old NPTv6 rules:", self.state.mapping.len());
                for (public, private) in &self.state.mapping {
                    info!("-: {public} <---> {private}");
                    if let Err(error) = self.delete_rules(public, private).await {
                        error!("Failed to delete iptables rules: {:?}", eyre!("{error}"));
                    }
                }

                let mapping = SubnetCalculator::calc(&prefix, &self.configuration.networks).await?;
                info!("Appending {} new NPTv6 rules:", mapping.len() * 2);
                for (public, private) in &mapping {
                    info!("+: {public} <---> {private}");
                    if let Err(error) = self.append_rules(public, private).await {
                        error!("Failed to append iptables rules: {:?}", eyre!("{error}"));
                    }
                }

                info!("Dispatching Github Workflow...");
                if let Err(error) = self.dispatch_workflow(&prefix, &mapping).await {
                    error!("Failed to dispatch Github Workflow: {error:?}");
                }

                self.update_state(prefix, mapping).await?;
            } else {
                warn!(
                    "Duplicate prefix change detected: {} -> {prefix}",
                    self.state.prefix
                );
            }
        }
        Ok(())
    }

    pub async fn run(mut self) -> Result<()> {
        let file = self.configuration.file.clone();

        FileWatcher::watch(&file, &mut self, async |gateway, line| {
            gateway.handle_line(line).await?;
            Ok(())
        })
        .await?;

        Ok(())
    }
}

impl SaveToTomlFile for Configuration {}
impl LoadFromTomlFile for Configuration {}

impl SaveToTomlFile for State {}
impl LoadFromTomlFile for State {}
