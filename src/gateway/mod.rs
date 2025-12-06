use std::{collections::HashMap, path::PathBuf, str::FromStr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use iptables::IPTables;
use log::{debug, error, info, warn};
use notify::Watcher;
use octocrab::Octocrab;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{
    common::storage::{LoadFromTomlFile, SaveToTomlFile, config_gateway_file, data_gateway_file},
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
    data: Data,

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

#[derive(Serialize, Deserialize, Default)]
struct Data {
    last_prefix: Ipv6Net,
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
            data: Data::from_file(&data_gateway_file()?)
                .await
                .unwrap_or_default(),
            configuration,
        })
    }

    async fn set_last_prefix(&mut self, prefix: Ipv6Net) -> Result<()> {
        self.data.last_prefix = prefix;
        self.data.save(&data_gateway_file()?, true).await?;
        Ok(())
    }

    async fn dispatch(&self, _mapping: &HashMap<Ipv6Net, Ipv6Net>) -> Result<()> {
        self.octocrab
            .actions()
            .create_workflow_dispatch(
                &self.configuration.owner,
                &self.configuration.repository,
                &self.configuration.workflow,
                "ref",
            )
            .send()
            .await?;
        Ok(())
    }

    pub async fn handle_line(&mut self, line: String) -> Result<()> {
        debug!("> {line}");
        if let Some(captures) = self.regex.captures(&line)
            && let Some(prefix) = captures.get(1)
        {
            let prefix = Ipv6Net::from_str(prefix.as_str())?;

            if self.data.last_prefix != prefix {
                info!(
                    "Prefix change detected: {} -> {prefix}",
                    self.data.last_prefix
                );

                let mapping = SubnetCalculator::calc(&prefix, &self.configuration.networks).await?;

                info!("Generating {} new NPTv6 rules:", mapping.len() * 2);
                for (to, from) in &mapping {
                    info!("{from} <---> {to}");
                    self.iptables
                        .append(
                            "nat",
                            "POSTROUTING",
                            &format!("-s {from} -j NETMAP --to {to}"),
                        )
                        .map_err(|error| eyre!("{error:?}"))?;
                    self.iptables
                        .append(
                            "nat",
                            "PREROUTING",
                            &format!("-d {to} -j NETMAP --to {from}"),
                        )
                        .map_err(|error| eyre!("{error:?}"))?;
                }

                if let Err(error) = self.dispatch(&mapping).await {
                    error!("Failed to dispatch Github Workflow: {error:?}");
                }

                self.set_last_prefix(prefix).await?;
            } else {
                warn!(
                    "Duplicate prefix change detected: {} -> {prefix}",
                    self.data.last_prefix
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

impl SaveToTomlFile for Data {}
impl LoadFromTomlFile for Data {}
