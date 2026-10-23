use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    net::Ipv6Addr,
    path::{Path, PathBuf},
    str::FromStr,
};

use color_eyre::eyre::{Context, Result};
use ipnet::Ipv6Net;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use walkdir_minimal::WalkDir;

use crate::common::{
    Ipv6AddrMapping, Ipv6NetMapping,
    storage::{LoadFromTomlFile, SaveToTomlFile, config_action_file},
};

pub const PREFIX_ENVIRONMENT: &str = "IPV6_PREFIX";
pub const MAPPING_ENVIRONMENT: &str = "IPV6_MAPPING";

pub struct Action {
    /* Environment */
    _prefix: Ipv6Net,
    mapping: Ipv6NetMapping,

    /* Configuration */
    configuration: Configuration,
}

#[derive(Serialize, Deserialize)]
struct Configuration {
    directory: PathBuf,
    devices: Ipv6AddrMapping,
}

impl Action {
    pub async fn load() -> Result<Self> {
        let _prefix = env::var(PREFIX_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Prefix using the environment variable: {PREFIX_ENVIRONMENT}"))?.parse()?;
        let mapping = serde_json::from_str(&env::var(MAPPING_ENVIRONMENT).wrap_err_with(|| format!("Please provide the IPv6 Mappings using the environment variable: {MAPPING_ENVIRONMENT}"))?)?;

        let configuration = Configuration::from_file(&{
            let file = config_action_file()?;
            if !file.exists() {
                Configuration {
                    directory: PathBuf::from("records/"),
                    devices: BTreeMap::from([(
                        Ipv6Addr::from_str("fd0a::1")?,
                        Ipv6Addr::from_str("fd0a::1")?,
                    )]),
                }
                .save(&file, true)
                .await?;
            }
            file
        })
        .await?;

        Ok(Self {
            _prefix,
            mapping,
            configuration,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let mut skipped = 0;
        let mut changed = vec![];

        let valid_mappings: Vec<_> = self
            .mapping
            .iter()
            .filter(|(public, private)| {
                if private.prefix_len() != public.prefix_len() {
                    warn!(
                        "Prefix length mismatch. Skipping rule: Private {} != Public {}",
                        private, public
                    );
                    return false;
                }
                true
            })
            .collect();

        for (device_address, current_mapped_ip) in self.configuration.devices.iter_mut() {
            if let Some((public_net, private_net)) = valid_mappings
                .iter()
                .find(|(_, private)| private.contains(device_address))
            {
                let new_public_ip =
                    Self::calculate_target_ip(device_address, private_net, public_net);

                if *current_mapped_ip != new_public_ip {
                    debug!(
                        "Updating device {device_address}: {current_mapped_ip} -> {new_public_ip}"
                    );
                    changed.push((*current_mapped_ip, new_public_ip));
                    *current_mapped_ip = new_public_ip;
                } else {
                    skipped += 1;
                }
            }
        }

        info!("Replacing: {} addresses:", changed.len());
        for (current, new) in &changed {
            info!("- {current} with {new}");
        }

        let replacements: Vec<(String, String)> = changed
            .iter()
            .map(|(current, new)| (current.to_string(), new.to_string()))
            .collect();

        for entry in WalkDir::new(&self.configuration.directory)?.follow_links(false) {
            match entry {
                Ok(entry) => {
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(error) => {
                            error!("Failed to stat {}: {error}", entry.path().display());
                            continue;
                        }
                    };
                    // `is_file()` is false for a symlink. Combined with the
                    // no-follow open below, a link in the records tree cannot
                    // redirect the write outside the directory.
                    if !file_type.is_file() {
                        continue;
                    }
                    rewrite_record(entry.path(), &replacements, MAX_RECORD_BYTES)?;
                }
                Err(error) => {
                    error!(
                        "Failed to read entry in provided directory {}: {error:?}",
                        self.configuration.directory.display()
                    );
                }
            }
        }

        info!(
            "Run complete: {} addresses updated, {skipped} skipped.",
            changed.len()
        );

        self.configuration
            .save(&config_action_file()?, true)
            .await?;

        Ok(())
    }

    fn calculate_target_ip(
        device_address: &Ipv6Addr,
        private_net: &Ipv6Net,
        public_net: &Ipv6Net,
    ) -> Ipv6Addr {
        (device_address & private_net.hostmask()) | (public_net.addr() & public_net.netmask())
    }
}

const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;

fn rewrite_record(path: &Path, replacements: &[(String, String)], max_bytes: u64) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Ok(());
    }
    if meta.len() > max_bytes {
        warn!("Skipping {}: larger than {max_bytes} bytes", path.display());
        return Ok(());
    }

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    if !same_file(&meta, &file.metadata()?) {
        warn!("Skipping {}: path changed or is a symlink", path.display());
        return Ok(());
    }

    let mut contents = String::new();
    if let Err(error) = file.read_to_string(&mut contents) {
        // Binary records are not rewritten. Other read failures abort the run.
        if error.kind() == std::io::ErrorKind::InvalidData {
            return Ok(());
        }
        return Err(error.into());
    }

    let updated = apply_replacements(&contents, replacements);
    if updated == contents {
        return Ok(());
    }

    file.seek(SeekFrom::Start(0))?;
    file.write_all(updated.as_bytes())?;
    file.set_len(updated.len() as u64)?;
    Ok(())
}

/// Replace each old address once, and only on IPv6 token boundaries.
///
/// Replacements are planned against the original text. A plain `str::replace`
/// would both cascade (`A -> B` then `B -> C`) and corrupt a longer address
/// such as `2001:db8::10` when replacing `2001:db8::1`.
fn apply_replacements(haystack: &str, replacements: &[(String, String)]) -> String {
    let hay = haystack.as_bytes();
    let mut out = String::with_capacity(haystack.len());
    let mut index = 0;
    while index < hay.len() {
        let boundary = index == 0 || !is_ipv6_token_byte(hay[index - 1]);
        let matched = boundary.then(|| {
            replacements
                .iter()
                .filter(|pair| !pair.0.is_empty())
                .find(|pair| {
                    let from = pair.0.as_bytes();
                    let end = index + from.len();
                    hay[index..].starts_with(from)
                        && (end >= hay.len() || !is_ipv6_token_byte(hay[end]))
                })
        });
        if let Some(pair) = matched.flatten() {
            out.push_str(&pair.1);
            index += pair.0.len();
            continue;
        }
        let ch = haystack[index..]
            .chars()
            .next()
            .expect("index is inside the string");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn is_ipv6_token_byte(byte: u8) -> bool {
    // '.' is punctuation as often as it is part of an IPv4-mapped address.
    // A longer address still continues with a hex digit (`::1` vs `::10`).
    byte.is_ascii_hexdigit() || byte == b':'
}

fn same_file(before: &fs::Metadata, opened: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == opened.dev() && before.ino() == opened.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (before, opened);
        true
    }
}

impl LoadFromTomlFile for Configuration {}
impl SaveToTomlFile for Configuration {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ubitix-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn replacement_does_not_eat_a_longer_address_or_cascade() {
        let pairs = [
            ("2001:db8::1".to_string(), "2001:db8::2".to_string()),
            ("2001:db8::2".to_string(), "2001:db8::9".to_string()),
        ];
        let updated = apply_replacements(
            "keep 2001:db8::10 and 2001:db8::1:2, change 2001:db8::1 and 2001:db8::2.",
            &pairs,
        );
        assert_eq!(
            updated,
            "keep 2001:db8::10 and 2001:db8::1:2, change 2001:db8::2 and 2001:db8::9."
        );
    }

    #[test]
    fn rewrite_skips_symlinks_and_updates_regular_files() {
        let dir = scratch("records");
        let outside = scratch("outside");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "2001:db8::1\n").unwrap();
        symlink(&secret, dir.join("link.txt")).unwrap();
        fs::write(dir.join("ok.txt"), "ip 2001:db8::1;\n").unwrap();
        fs::write(dir.join("big.txt"), "2001:db8::1\n").unwrap();

        let replacements = [("2001:db8::1".to_string(), "2001:db8::2".to_string())];
        rewrite_record(
            dir.join("ok.txt").as_path(),
            &replacements,
            MAX_RECORD_BYTES,
        )
        .unwrap();
        rewrite_record(
            dir.join("link.txt").as_path(),
            &replacements,
            MAX_RECORD_BYTES,
        )
        .unwrap();
        rewrite_record(dir.join("big.txt").as_path(), &replacements, 4).unwrap();

        assert_eq!(fs::read_to_string(&secret).unwrap(), "2001:db8::1\n");
        assert_eq!(
            fs::read_to_string(dir.join("ok.txt")).unwrap(),
            "ip 2001:db8::2;\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("big.txt")).unwrap(),
            "2001:db8::1\n"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }
}
