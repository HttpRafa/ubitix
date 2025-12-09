use std::{env, path::PathBuf};

use color_eyre::eyre::{Context, Result, eyre};
use ipnet::Ipv6Net;
use serde::Deserialize;
use tokio::fs;

use crate::common::{
    Ipv6Mapping,
    storage::{LoadFromTomlFile, config_action_file},
};

const DEFAULT_CONFIG: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/configs/ubitix.toml"));

pub const PREFIX_ENVIRONMENT: &str = "IPV6_PREFIX";
pub const MAPPING_ENVIRONMENT: &str = "IPV6_MAPPING";

pub struct Action {
    /* Environment */
    prefix: Ipv6Net,
    mapping: Ipv6Mapping,

    /* Configuration */
    configuration: Configuration,
}

#[derive(Deserialize)]
struct Configuration {
    directory: PathBuf,
}

impl Action {
    pub async fn load() -> Result<Self> {
        let prefix = env::var(PREFIX_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Prefix using the environment variable: {PREFIX_ENVIRONMENT}"))?.parse::<Ipv6Net>()?;
        let mapping = serde_json::from_str::<Ipv6Mapping>(&env::var(PREFIX_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Mappings using the environment variable: {MAPPING_ENVIRONMENT}"))?)?;

        let configuration = Configuration::from_file(&{
            let file = config_action_file()?;
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
            prefix,
            mapping,
            configuration,
        })
    }

    pub async fn run(self) -> Result<()> {
        Ok(())
    }
}

impl LoadFromTomlFile for Configuration {}
