use std::path::PathBuf;

use color_eyre::eyre::Result;
use ipnet::Ipv6Net;

pub struct Action {
    prefix: Ipv6Net,

    directory: PathBuf,
}

impl Action {
    pub fn new(prefix: Ipv6Net, directory: PathBuf) -> Self {
        Self { prefix, directory }
    }

    pub async fn run(self) -> Result<()> {
        Ok(())
    }
}
