use std::{
    ffi::OsStr,
    fs::{DirBuilder, File, OpenOptions, Permissions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use color_eyre::eyre::{Result, eyre};

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;

pub(crate) enum SecretPath {
    Missing,
    File,
}

/// Classify the final path without following a symlink.
pub(crate) fn classify_secret_path(path: &Path) -> Result<SecretPath> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(eyre!("refusing to use symlink {}", path.display()))
        }
        Ok(meta) if meta.is_file() => Ok(SecretPath::File),
        Ok(_) => Err(eyre!("{} is not a regular file", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(SecretPath::Missing),
        Err(err) => Err(err.into()),
    }
}

/// Open `path` without following a trailing symlink.
pub(crate) fn open_nofollow(path: &Path, write: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    if write {
        options.write(true);
    }
    options.open(path)
}

/// Read a secret file through an `O_NOFOLLOW` fd and tighten it to `0600`.
pub(crate) fn read_secret_file(path: &Path) -> Result<String> {
    let mut file = open_nofollow(path, false).map_err(|err| nofollow_error(path, err))?;
    file.set_permissions(Permissions::from_mode(FILE_MODE))?;
    let mut data = String::new();
    file.read_to_string(&mut data)?;
    Ok(data)
}

/// Create or replace a secret file without following a trailing symlink.
///
/// An existing symlink is left in place. The contents are written to a new `0600`
/// file in the same directory and renamed over `path`, so a crash cannot leave
/// an empty file where the previous contents were truncated.
pub(crate) fn write_secret_file(path: &Path, contents: &str) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(eyre!("refusing to use symlink {}", path.display()));
        }
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(eyre!("{} is not a regular file", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    let temporary = temporary_secret_path(path);
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|err| nofollow_error(&temporary, err))?;
        file.set_permissions(Permissions::from_mode(FILE_MODE))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn temporary_secret_path(path: &Path) -> PathBuf {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_else(|| OsStr::new("secret"));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(".{}.{nanos}.tmp", std::process::id()));
    parent.join(temporary_name)
}

/// Create `path` at mode `0700` and refuse a symlink at that directory.
///
/// Ancestors are created as needed. An existing real directory is tightened to `0700`
/// through an `O_NOFOLLOW` fd. A symlink is left untouched.
pub(crate) fn ensure_private_dir(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(eyre!("refusing to use an empty directory path"));
    }

    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(eyre!(
            "refusing to use symlink directory {}",
            path.display()
        )),
        Ok(meta) if meta.is_dir() => chmod_dir_nofollow(path),
        Ok(_) => Err(eyre!("{} is not a directory", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            match DirBuilder::new().mode(DIR_MODE).create(path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err.into()),
            }
            chmod_dir_nofollow(path)
        }
        Err(err) => Err(err.into()),
    }
}

fn chmod_dir_nofollow(path: &Path) -> Result<()> {
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map_err(|err| nofollow_error(path, err))?;
    dir.set_permissions(Permissions::from_mode(DIR_MODE))?;
    Ok(())
}

fn nofollow_error(path: &Path, err: io::Error) -> color_eyre::eyre::Report {
    if is_loop(&err) {
        eyre!("refusing to follow symlink {}", path.display())
    } else {
        err.into()
    }
}

fn is_loop(err: &io::Error) -> bool {
    // `ErrorKind::FilesystemLoop` is still unstable. `O_NOFOLLOW` reports `ELOOP`.
    err.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(test)]
mod tests {
    use super::{
        SecretPath, classify_secret_path, ensure_private_dir, read_secret_file, write_secret_file,
    };
    use std::{
        fs::{self, OpenOptions, Permissions},
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("ubitix-secret-{}-{nanos}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode_bits(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn secret_file_nofollow_and_mode() {
        let dir = TempDir::new();
        ensure_private_dir(dir.path()).unwrap();

        let file = dir.path().join("gateway.toml");
        write_secret_file(&file, "token = \"secret\"\n").unwrap();
        assert!(matches!(
            classify_secret_path(&file).unwrap(),
            SecretPath::File
        ));
        assert_eq!(read_secret_file(&file).unwrap(), "token = \"secret\"\n");

        let file_mode = mode_bits(&file);
        let dir_mode = mode_bits(dir.path());
        // Some temp filesystems ignore chmod and report 0777 for every inode.
        if file_mode == 0o777 && dir_mode == 0o777 {
            eprintln!("filesystem does not store unix permissions; skipping mode asserts");
        } else {
            assert_eq!(file_mode, 0o600);
            assert_eq!(dir_mode, 0o700);

            let wide = dir.path().join("wide.toml");
            let mut opened = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&wide)
                .unwrap();
            opened
                .set_permissions(Permissions::from_mode(0o644))
                .unwrap();
            opened.write_all(b"loose\n").unwrap();
            drop(opened);
            assert_eq!(mode_bits(&wide), 0o644);
            assert_eq!(read_secret_file(&wide).unwrap(), "loose\n");
            assert_eq!(mode_bits(&wide), 0o600);
        }

        write_secret_file(&file, "token = \"replaced\"\n").unwrap();
        assert_eq!(read_secret_file(&file).unwrap(), "token = \"replaced\"\n");
        assert!(dir.path().read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));

        let target = dir.path().join("outside");
        fs::write(&target, "original").unwrap();
        let target_mode = mode_bits(&target);
        let link = dir.path().join("link.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(write_secret_file(&link, "rewritten").is_err());
        assert!(read_secret_file(&link).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert_eq!(mode_bits(&target), target_mode);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let missing = dir.path().join("created-through-link");
        let dangling = dir.path().join("dangling.toml");
        std::os::unix::fs::symlink(&missing, &dangling).unwrap();
        assert!(write_secret_file(&dangling, "nope").is_err());
        assert!(!missing.exists());
        assert!(
            fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let _ = fs::set_permissions(&real, Permissions::from_mode(0o755));
        fs::write(real.join("marker"), "keep").unwrap();
        let real_mode = mode_bits(&real);
        let dir_link = dir.path().join("dir-link");
        std::os::unix::fs::symlink(&real, &dir_link).unwrap();
        assert!(ensure_private_dir(&dir_link).is_err());
        assert_eq!(fs::read_to_string(real.join("marker")).unwrap(), "keep");
        assert_eq!(mode_bits(&real), real_mode);
        assert!(
            fs::symlink_metadata(&dir_link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
