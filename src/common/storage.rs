use std::{
    env,
    path::{Path, PathBuf},
};

use color_eyre::eyre::{Result, eyre};
use directories::ProjectDirs;
use log::warn;
use serde::{Serialize, de::DeserializeOwned};
use tokio::fs;

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

/// The gateway config contains a GitHub token. Do not follow a symlink to it,
/// and do not leave it readable by group or other.
pub fn restrict_secret_file(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(eyre!(
            "refusing to use {} because it is a symlink",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let mut permissions = meta.permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions).map_err(|error| {
                eyre!(
                    "refusing to use {}: it is accessible to other users ({mode:04o}) and could not be restricted: {error}",
                    path.display()
                )
            })?;
            warn!(
                "Restricted permissions on {} from {mode:04o} to 0600",
                path.display()
            );
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn secret_file_is_forced_to_owner_read_write() {
        let path = std::env::temp_dir().join(format!("ubitix-secret-{}", std::process::id()));
        std::fs::write(&path, "token = \"secret\"\n").unwrap();
        let mut permissions = std::fs::symlink_metadata(&path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&path, permissions).unwrap();

        restrict_secret_file(&path).unwrap();
        let mode = std::fs::symlink_metadata(&path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        std::os::unix::fs::symlink(&path, path.with_extension("link")).unwrap();
        assert!(restrict_secret_file(&path.with_extension("link")).is_err());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("link"));
    }
}
