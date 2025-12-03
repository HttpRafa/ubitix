use std::{io::SeekFrom, path::PathBuf, str::FromStr};

use color_eyre::eyre::Result;
use ipnet::Ipv6Net;
use log::error;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use octocrab::Octocrab;
use regex::Regex;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::mpsc::channel,
};

const CHANNEL_BUFFER_SIZE: usize = 32;

pub struct Gateway {
    /* Configuration */
    file: PathBuf,
    regex: Regex,
    owner: String,
    repository: String,
    workflow: String,

    /* Runtime */
    octocrab: Octocrab,
    current: Ipv6Net,
}

impl Gateway {
    pub fn new(file: PathBuf, regex: String, token: String, owner: String, repository: String, workflow: String) -> Result<Self> {
        Ok(Self {
            file,
            regex: Regex::new(&regex)?,
            owner,
            repository,
            workflow,
            octocrab: Octocrab::builder().personal_token(token).build()?,
            current: Ipv6Net::from_str("2001::/64")?,
        })
    }

    async fn dispatch(&self, _prefix: Ipv6Net) -> Result<()> {
        self.octocrab.actions().create_workflow_dispatch(&self.owner, &self.repository, &self.workflow, "ref").send().await?;
        Ok(())   
    }

    async fn handle_line(&self, line: String) -> Result<()> {
        if let Some(captures) = self.regex.captures(&line)
            && let Some(prefix) = captures.get(1)
        {
            let prefix = Ipv6Net::from_str(prefix.as_str())?;
            if self.current != prefix {
                self.dispatch(prefix).await?;
                // TODO: Change iptable rules
            }
        }
        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
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
