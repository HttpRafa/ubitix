use std::{
    env, io,
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Result, eyre};
use directories::ProjectDirs;
use serde::{Serialize, de::DeserializeOwned};

use crate::common::secret::{ensure_private_dir, read_secret_file, write_secret_file};

const GATEWAY_FILE_NAME: &str = "gateway.toml";
const ACTION_FILE_NAME: &str = "ubitix.toml";

pub fn config_gateway_file() -> Result<PathBuf> {
    if let Some(directories) = ProjectDirs::from("io", "httprafa", "ubitix") {
        return Ok(directories.config_local_dir().join(GATEWAY_FILE_NAME));
    }
    Err(eyre!("Failed to find a location for the gateway.toml file"))
}

pub fn state_gateway_file() -> Result<PathBuf> {
    if let Some(directories) = ProjectDirs::from("io", "httprafa", "ubitix") {
        return Ok(directories.data_local_dir().join(GATEWAY_FILE_NAME));
    }
    Err(eyre!("Failed to find a location for the gateway.toml file"))
}

pub fn config_action_file() -> Result<PathBuf> {
    Ok(env::current_dir()?.join(ACTION_FILE_NAME))
}

pub trait SaveToTomlFile: Serialize {
    async fn save(&self, path: &Path, create_parent: bool) -> Result<()> {
        if create_parent
            && let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            match std::fs::symlink_metadata(parent) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    ensure_private_dir(parent)?;
                }
                Err(err) => return Err(err.into()),
            }
        }
        write_secret_file(path, &toml::to_string(self)?)?;
        Ok(())
    }
}

pub trait LoadFromTomlFile: DeserializeOwned {
    async fn from_file(path: &Path) -> Result<Self> {
        let data = read_secret_file(path)?;
        let config = toml::from_str(&data)?;
        Ok(config)
    }
}
