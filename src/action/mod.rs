use std::{net::Ipv6Addr, path::PathBuf};

use color_eyre::eyre::Result;

pub struct Action {
    prefix: Ipv6Addr,
    subnet: u8,

    directory: PathBuf,
}

impl Action {
    pub fn new(prefix: Ipv6Addr, subnet: u8, directory: PathBuf) -> Self {
        Self { prefix, subnet, directory }
    }

    pub async fn run(self) -> Result<()> {
        Ok(())
    }
}