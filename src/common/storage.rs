use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, eyre};
use directories::ProjectDirs;
use serde::{Serialize, de::DeserializeOwned};
use tokio::fs;

const GATEWAY_FILE_NAME: &str = "gateway.toml";

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

pub trait SaveToTomlFile: Serialize {
    async fn save(&self, path: &Path, create_parent: bool) -> Result<()> {
        if create_parent && let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, toml::to_string(self)?).await?;
        Ok(())
    }
}

pub trait LoadFromTomlFile: DeserializeOwned {
    async fn from_file(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path).await?;
        let config = toml::from_str(&data)?;
        Ok(config)
    }
}
