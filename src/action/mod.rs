use std::{
    collections::BTreeMap,
    env,
    io::{Read, Seek, SeekFrom, Write},
    net::Ipv6Addr,
    path::{Path, PathBuf},
    str::FromStr,
};

use color_eyre::eyre::{Context, Result, eyre};
use ipnet::Ipv6Net;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use walkdir_minimal::WalkDir;

use crate::common::{
    Ipv6AddrMapping, Ipv6NetMapping,
    secret::{SecretPath, classify_secret_path, open_nofollow},
    storage::{LoadFromTomlFile, SaveToTomlFile, config_action_file},
};

pub const PREFIX_ENVIRONMENT: &str = "IPV6_PREFIX";
pub const MAPPING_ENVIRONMENT: &str = "IPV6_MAPPING";

/// Record files larger than this are not rewritten. The run then fails so the
/// saved address map is not updated and a later run can try again.
const MAX_RECORD_LEN: u64 = 1024 * 1024;

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

        let file = config_action_file()?;
        if matches!(classify_secret_path(&file)?, SecretPath::Missing) {
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
        let configuration = Configuration::from_file(&file).await?;

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
        rewrite_records(&self.configuration.directory, &replacements)?;

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

impl LoadFromTomlFile for Configuration {}
impl SaveToTomlFile for Configuration {}

/// `symlink_metadata("link/")` follows a directory symlink. Drop the trailing
/// slash so the check sees the link itself.
fn path_without_trailing_slash(path: &Path) -> PathBuf {
    let stripped: PathBuf = path.components().collect();
    if stripped.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        stripped
    }
}

fn rewrite_records(directory: &Path, replacements: &[(String, String)]) -> Result<()> {
    if replacements.is_empty() {
        return Ok(());
    }

    let directory = path_without_trailing_slash(directory);
    match std::fs::symlink_metadata(&directory) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(eyre!(
                "refusing to rewrite through symlink directory {}",
                directory.display()
            ));
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(eyre!("{} is not a directory", directory.display()));
        }
        Ok(_) => {}
        Err(err) => return Err(err.into()),
    }

    for entry in WalkDir::new(&directory)?.follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                error!("Failed to read entry in provided directory {directory:?}: {error:?}");
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                error!("Failed to read file type for {:?}: {error:?}", entry.path());
                continue;
            }
        };
        if !file_type.is_file() {
            continue;
        }

        rewrite_record_file(entry.path(), replacements)
            .wrap_err_with(|| format!("failed to rewrite {}", entry.path().display()))?;
    }
    Ok(())
}

fn rewrite_record_file(path: &Path, replacements: &[(String, String)]) -> Result<()> {
    // Read first. A file that does not need a write may be mode 0444, and
    // opening it for write would abort the rest of the directory.
    let mut file = open_nofollow(path, false)?;
    if file.metadata()?.len() > MAX_RECORD_LEN {
        return Err(eyre!(
            "{} is larger than 1 MiB and was left unchanged",
            path.display()
        ));
    }

    let mut original = Vec::new();
    // `File` implements both `Read` and `Write`, so `by_ref` has to name `Read`.
    Read::by_ref(&mut file)
        .take(MAX_RECORD_LEN + 1)
        .read_to_end(&mut original)?;
    if original.len() as u64 > MAX_RECORD_LEN {
        return Err(eyre!(
            "{} is larger than 1 MiB and was left unchanged",
            path.display()
        ));
    }

    let updated = replace_bounded_addresses(&original, replacements);
    if updated == original {
        return Ok(());
    }
    drop(file);

    let mut file = open_nofollow(path, true)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&updated)?;
    file.set_len(updated.len() as u64)?;
    file.flush()?;
    Ok(())
}

/// Replace whole addresses in one pass over `input`.
///
/// A match counts only when the bytes on either side are outside `[0-9A-Fa-f:]`.
/// Longer needles win. Replacement text is not scanned again.
fn replace_bounded_addresses(input: &[u8], replacements: &[(String, String)]) -> Vec<u8> {
    let mut ranked: Vec<&(String, String)> = replacements.iter().collect();
    ranked.sort_by(|left, right| right.0.len().cmp(&left.0.len()));

    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let mut matched = false;
        for replacement in &ranked {
            let from = replacement.0.as_bytes();
            if from.is_empty() {
                continue;
            }
            let end = index + from.len();
            if end > input.len() || !input[index..end].eq_ignore_ascii_case(from) {
                continue;
            }
            let before_ok = index == 0 || !is_address_byte(input[index - 1]);
            let after_ok = end == input.len() || !is_address_byte(input[end]);
            if before_ok && after_ok {
                output.extend_from_slice(replacement.1.as_bytes());
                index = end;
                matched = true;
                break;
            }
        }
        if !matched {
            output.push(input[index]);
            index += 1;
        }
    }
    output
}

fn is_address_byte(byte: u8) -> bool {
    matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F' | b':')
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_RECORD_LEN, path_without_trailing_slash, replace_bounded_addresses, rewrite_records,
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn pair(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }

    #[test]
    fn replaces_whole_addresses_without_extending_neighbors() {
        let replacements = [
            pair("2001:db8::1", "2001:db8::9"),
            pair("2001:db8::10", "2001:db8::20"),
        ];
        let input = b"2001:db8::10 2001:db8::1 2001:db8::1a 2001:DB8::1";
        let output = replace_bounded_addresses(input, &replacements);
        assert_eq!(output, b"2001:db8::20 2001:db8::9 2001:db8::1a 2001:db8::9");
    }

    #[test]
    fn several_replacements_in_one_file_all_survive() {
        let replacements = [
            pair("2001:db8::1", "2001:db8::ff"),
            pair("fd00::1", "fd00::2"),
        ];
        let output = replace_bounded_addresses(b"a=2001:db8::1;b=fd00::1;", &replacements);
        assert_eq!(output, b"a=2001:db8::ff;b=fd00::2;");
    }

    #[test]
    fn does_not_rescan_replacement_text() {
        let replacements = [
            pair("2001:db8::1", "2001:db8::2"),
            pair("2001:db8::2", "2001:db8::3"),
        ];
        let output = replace_bounded_addresses(b"2001:db8::1", &replacements);
        assert_eq!(output, b"2001:db8::2");
    }

    #[test]
    fn colon_and_hex_keep_the_address_intact() {
        let replacements = [pair("2001:db8::1", "2001:db8::9")];
        let output = replace_bounded_addresses(b"2001:db8::1:ffff 2001:db8::1.", &replacements);
        assert_eq!(output, b"2001:db8::1:ffff 2001:db8::9.");
    }

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ubitix-records-{}-{nanos}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn trailing_slash_does_not_follow_a_directory_symlink() {
        let root = temp_dir();
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("zone"), "2001:db8::1").unwrap();
        let link = root.join("records");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let configured = PathBuf::from(format!("{}/", link.display()));
        assert!(configured.to_string_lossy().ends_with('/'));
        assert_ne!(
            path_without_trailing_slash(&configured).as_os_str(),
            configured.as_os_str()
        );

        let error = rewrite_records(&configured, &[pair("2001:db8::1", "2001:db8::9")]);
        assert!(error.is_err());
        assert_eq!(
            fs::read_to_string(real.join("zone")).unwrap(),
            "2001:db8::1"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_only_unchanged_file_does_not_abort_the_rewrite() {
        let root = temp_dir();
        let note = root.join("note");
        fs::write(&note, "nothing").unwrap();
        let mut permissions = fs::metadata(&note).unwrap().permissions();
        permissions.set_readonly(true);
        permissions.set_mode(0o444);
        fs::set_permissions(&note, permissions).unwrap();
        fs::write(root.join("zone"), "2001:db8::1\n").unwrap();

        rewrite_records(&root, &[pair("2001:db8::1", "2001:db8::9")]).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("zone")).unwrap(),
            "2001:db8::9\n"
        );
        assert_eq!(fs::read_to_string(&note).unwrap(), "nothing");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_record_fails_without_changing_the_file() {
        let root = temp_dir();
        let big = root.join("big");
        fs::write(&big, vec![b'x'; MAX_RECORD_LEN as usize + 1]).unwrap();
        let error = rewrite_records(&root, &[pair("2001:db8::1", "2001:db8::9")]);
        assert!(error.is_err());
        assert_eq!(fs::metadata(&big).unwrap().len(), MAX_RECORD_LEN + 1);
        let _ = fs::remove_dir_all(&root);
    }
}
