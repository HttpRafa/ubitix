use std::path::PathBuf;

use color_eyre::eyre::Result;
use ipnet::Ipv6Net;
use serde::{Deserialize, Serialize};

pub struct Action {
    prefix: Ipv6Net,

    directory: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Configuration {
    dictionary: PathBuf,
}

impl Action {
    pub fn new(prefix: Ipv6Net, directory: PathBuf) -> Self {
        Self { prefix, directory }
    }

    pub async fn run(self) -> Result<()> {
        Ok(())
    }
}