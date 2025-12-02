use std::{io::SeekFrom, path::PathBuf};

use color_eyre::eyre::Result;
use log::{error, info};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;
use tokio::{
    fs::{File, metadata},
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::mpsc::channel,
};

pub struct Gateway {
    file: PathBuf,

    regex: Regex,

    repository: String,
    workflow: String,
}

impl Gateway {
    pub fn new(file: PathBuf, regex: String, repository: String, workflow: String) -> Result<Self> {
        Ok(Self {
            file,
            regex: Regex::new(&regex)?,
            repository,
            workflow,
        })
    }

    pub async fn run(&self) -> Result<()> {
        let mut file = File::open(&self.file).await?;
        let mut position = metadata(&self.file).await?.len();

        let (sender, mut receiver) = channel(128);
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
                        self.handle_line(line).await?;
                    }
                }
                Err(error) => error!("{error:?}"),
            }
        }

        Ok(())
    }

    async fn handle_line(&self, line: String) -> Result<()> {
        if let Some(captures) = self.regex.captures(&line)
            && let Some(prefix) = captures.get(1)
            && let Some(subnet) = captures.get(2)
        {
            info!("--- Match Found ---");
            info!("Extracted Prefix: {}", prefix.as_str());
            info!("Extracted Length: {}", subnet.as_str());
            info!("-------------------");
        }
        Ok(())
    }
}
