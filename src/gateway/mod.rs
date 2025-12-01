use std::path::PathBuf;

use color_eyre::eyre::Result;

pub struct Gateway {
    file: PathBuf,

    repository: String,
    workflow: String,
}

impl Gateway {
    pub fn new(file: PathBuf, repository: String, workflow: String) -> Self {
        Self { file, repository, workflow }
    }

    pub async fn run(self) -> Result<()> {
        Ok(())
    }
}