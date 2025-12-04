use std::{collections::HashMap, io::SeekFrom, path::PathBuf, str::FromStr};

use color_eyre::eyre::{Result, eyre};
use ipnet::Ipv6Net;
use log::{debug, error, info, warn};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use octocrab::Octocrab;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::mpsc::channel,
};

use crate::common::storage::{LoadFromTomlFile, SaveToTomlFile, gateway_file};

const CHANNEL_BUFFER_SIZE: usize = 32;

pub struct Gateway {
    /* Configuration */
    file: PathBuf,
    regex: Regex,
    networks: Vec<Ipv6Net>,
    owner: String,
    repository: String,
    workflow: String,

    /* Runtime */
    octocrab: Octocrab,
    data: Data,
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    current: Ipv6Net,
}

impl Gateway {
    pub async fn new(
        file: PathBuf,
        regex: Regex,
        networks: Vec<Ipv6Net>,
        token: String,
        owner: String,
        repository: String,
        workflow: String,
    ) -> Result<Self> {
        Ok(Self {
            file,
            regex,
            networks,
            owner,
            repository,
            workflow,
            octocrab: Octocrab::builder().personal_token(token).build()?,
            data: Data::from_file(&gateway_file()?).await.unwrap_or_default(),
        })
    }

    async fn set_current(&mut self, prefix: Ipv6Net) -> Result<()> {
        self.data.current = prefix;
        self.data.save(&gateway_file()?, true).await?;
        Ok(())
    }

    async fn dispatch(&self, mapping: &HashMap<Ipv6Net, Ipv6Net>) -> Result<()> {
        self.octocrab
            .actions()
            .create_workflow_dispatch(&self.owner, &self.repository, &self.workflow, "ref")
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

            if self.data.current != prefix {
                info!("Prefix change detected: {} -> {prefix}", self.data.current);
                self.set_current(prefix).await?;

                if prefix.prefix_len() > 64 {
                    return Err(eyre!(
                        "The detected prefix must be /64 or shorter to be split into /64 subnets."
                    ));
                }

                let required = self.networks.len();
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
                    .zip(self.networks.iter().cloned())
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

                for (to, from) in &mapping {
                    info!("{from} - [NPTv6] -> {to}")
                }
                
                info!("Updating iptables...");

                if let Err(error) = self.dispatch(&mapping).await {
                    error!("Failed to dispatch Github Workflow: {error:?}");
                }
            } else {
                warn!("Useless change detected: {} -> {prefix}", self.data.current);
            }
        }
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut file = File::open(&self.file).await?;
        let mut position = self.file.metadata()?.len();

        let (sender, mut receiver) = channel(CHANNEL_BUFFER_SIZE);
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                sender
                    .blocking_send(result)
                    .expect("Failed to send watch event to sender")
            },
            Config::default(),
        )?;
        watcher.watch(&self.file, RecursiveMode::NonRecursive)?;

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

impl SaveToTomlFile for Data {}
impl LoadFromTomlFile for Data {}
