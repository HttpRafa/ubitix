use std::{collections::HashMap, io::SeekFrom, path::PathBuf, str::FromStr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use log::{debug, error, info, warn};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use octocrab::Octocrab;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File},
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::mpsc::channel,
};

use crate::common::storage::{
    LoadFromTomlFile, SaveToTomlFile, config_gateway_file, data_gateway_file,
};

const DEFAULT_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/configs/gateway.toml"));
const CHANNEL_BUFFER_SIZE: usize = 32;

pub struct Gateway {
    /* Configuration */
    configuration: Configuration,

    /* State */
    data: Data,

    /* Runtime */
    regex: Regex,
    octocrab: Octocrab,
}

#[derive(Serialize, Deserialize)]
struct Configuration {
    file: PathBuf,
    regex: String,
    networks: Vec<Ipv6Net>,
    token: Option<String>,
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
        let mut configuration = Configuration::from_file(&{
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
            regex: Regex::new(&configuration.regex)?,
            octocrab: Octocrab::builder()
                .personal_token(configuration.token.take().unwrap_or_default())
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

    async fn handle_line(&mut self, line: String) -> Result<()> {
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
                self.set_last_prefix(prefix).await?;

                if prefix.prefix_len() > 64 {
                    return Err(eyre!(
                        "The detected prefix must be /64 or shorter to be split into /64 subnets."
                    ));
                }

                let required = self.configuration.networks.len();
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
                    .zip(self.configuration.networks.iter().cloned())
                    .filter_map(|(to, from)| {
                        if from.prefix_len() != 64 {
                            warn!("Assigned subnet {from} is not a /64. Skipping this mapping.");
                            None
                        } else if to.prefix_len() != 64 {
                            warn!("Assigned subnet {to} is not a /64. Skipping this mapping.");
                            None
                        } else {
                            Some((from, to))
                        }
                    })
                    .collect::<HashMap<_, _>>();

                info!("Computed {} new subnets:", mapping.len());
                for (to, from) in &mapping {
                    info!("{from} - [NPTv6] -> {to}")
                }

                info!("Updating iptables...");

                if let Err(error) = self.dispatch(&mapping).await {
                    error!("Failed to dispatch Github Workflow: {error:?}");
                }
            } else {
                warn!(
                    "Useless change detected: {} -> {prefix}",
                    self.data.last_prefix
                );
            }
        }
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut file = File::open(&self.configuration.file).await?;
        let mut position = self.configuration.file.metadata()?.len();

        let (sender, mut receiver) = channel(CHANNEL_BUFFER_SIZE);
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                sender
                    .blocking_send(result)
                    .expect("Failed to send watch event to sender")
            },
            Config::default(),
        )?;
        watcher.watch(&self.configuration.file, RecursiveMode::NonRecursive)?;

        while let Some(result) = receiver.recv().await {
            match result {
                Ok(_event) => {
                    if file.metadata().await?.len() == position {
                        continue;
                    }

                    file.seek(SeekFrom::Start(position)).await?;
                    position = file.metadata().await?.len();

                    let reader = BufReader::new(&mut file);
                    let mut lines = reader.lines();
                    while let Some(line) = lines.next_line().await? {
                        if let Err(error) = self.handle_line(line).await {
                            error!("Failed to handle read line: {error:?}");
                        }
                    }
                }
                Err(error) => error!("{error:?}"),
            }
        }

        Ok(())
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            file: PathBuf::from("/var/log/messages"),
            regex: String::from("Adding PD prefix ([\\da-fA-F:]+/\\d{1,3})"),
            networks: Vec::new(),
            token: Some(String::from("some-token")),
            owner: String::from("some-username"),
            repository: String::from("some-repository"),
            workflow: String::from("some-workflow.yml"),
        }
    }
}

impl SaveToTomlFile for Configuration {}
impl LoadFromTomlFile for Configuration {}

impl SaveToTomlFile for Data {}
impl LoadFromTomlFile for Data {}
