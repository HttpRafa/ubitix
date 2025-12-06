use std::{io::SeekFrom, path::PathBuf};

use color_eyre::eyre::Result;
use log::error;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncSeekExt, BufReader},
    sync::mpsc::channel,
};

const CHANNEL_BUFFER_SIZE: usize = 32;

pub struct FileWatcher;

impl FileWatcher {
    pub async fn watch<F, D>(path: &PathBuf, d: &mut D, f: F) -> Result<()>
    where
        F: AsyncFn(&mut D, String) -> Result<()>,
    {
        let mut file = File::open(path).await?;
        let mut position = path.metadata()?.len();

        let (sender, mut receiver) = channel(CHANNEL_BUFFER_SIZE);
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                sender
                    .blocking_send(result)
                    .expect("Failed to send watch event to sender")
            },
            Config::default(),
        )?;
        watcher.watch(path, RecursiveMode::NonRecursive)?;

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
                        if let Err(error) = f(d, line).await {
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
