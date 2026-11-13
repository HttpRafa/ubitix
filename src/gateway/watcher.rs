use std::{
    io::{ErrorKind, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use color_eyre::eyre::Result;
use log::error;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc::channel,
};

const CHANNEL_BUFFER_SIZE: usize = 32;
/// rsyslog's usual message cap. Longer records are discarded through the next newline.
const MAX_LINE_LEN: usize = 8 * 1024;

pub struct FileWatcher;

/// Unprocessed tail of the byte stream. At most [`MAX_LINE_LEN`] bytes are retained.
#[derive(Debug, Default, PartialEq, Eq)]
struct LineCarry {
    pending: Vec<u8>,
    discarding: bool,
}

/// Split `chunk` into lines no longer than [`MAX_LINE_LEN`].
///
/// A line is returned only after `\n`. Bytes after the last newline stay in the
/// carry and are not returned. A record that exceeds the cap is omitted; `chunk`
/// is still fully consumed, and a later chunk continues the discard until `\n`.
fn split_capped_lines(mut carry: LineCarry, mut chunk: &[u8]) -> (Vec<String>, LineCarry) {
    let mut lines = Vec::new();

    while !chunk.is_empty() {
        if carry.discarding {
            match chunk.iter().position(|&byte| byte == b'\n') {
                Some(index) => {
                    carry.discarding = false;
                    chunk = &chunk[index + 1..];
                }
                None => break,
            }
            continue;
        }

        match chunk.iter().position(|&byte| byte == b'\n') {
            Some(index) => {
                let room = MAX_LINE_LEN - carry.pending.len();
                if index > room {
                    carry.pending.clear();
                    chunk = &chunk[index + 1..];
                    continue;
                }
                carry.pending.extend_from_slice(&chunk[..index]);
                lines.push(decode_line(&carry.pending));
                carry.pending.clear();
                chunk = &chunk[index + 1..];
            }
            None => {
                let room = MAX_LINE_LEN - carry.pending.len();
                if chunk.len() > room {
                    carry.pending.clear();
                    carry.discarding = true;
                } else {
                    carry.pending.extend_from_slice(chunk);
                }
                break;
            }
        }
    }

    (lines, carry)
}

fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Reopen `path` when logrotate replaces or truncates it.
///
/// Returns false when the path is missing and this event should be skipped.
/// After a reopen the cursor is zero, so a prefix already written to the new
/// file is read by the caller. The first open is unchanged and still starts at EOF.
async fn reopen_if_rotated(
    file: &mut File,
    path: &Path,
    position: &mut u64,
    carry: &mut LineCarry,
    watcher: &mut RecommendedWatcher,
) -> Result<bool> {
    let path_meta = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            error!("Watched file is missing: {}", path.display());
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };

    let open_meta = file.metadata().await?;
    let inode_changed = open_meta.dev() != path_meta.dev() || open_meta.ino() != path_meta.ino();
    let shrunk = path_meta.len() < *position;
    if !inode_changed && !shrunk {
        return Ok(true);
    }

    // Arm the new inode before reading so writes that land during the read
    // still produce a later event. A shrink keeps the same watch.
    if inode_changed {
        if let Err(error) = watcher.unwatch(path) {
            error!("Failed to drop watch on rotated file: {error:?}");
        }
        watcher.watch(path, RecursiveMode::NonRecursive)?;
    }

    *file = File::open(path).await?;
    *position = 0;
    *carry = LineCarry::default();
    Ok(true)
}

impl FileWatcher {
    pub async fn watch<F, D>(path: &PathBuf, d: &mut D, f: F) -> Result<()>
    where
        F: AsyncFn(&mut D, String) -> Result<()>,
    {
        let mut file = File::open(path).await?;
        // Existing lines are not replayed. Later reads start from this cursor.
        let mut position = file.metadata().await?.len();
        let mut carry = LineCarry::default();

        let (sender, mut receiver) = channel(CHANNEL_BUFFER_SIZE);
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                sender
                    .blocking_send(result)
                    .expect("Failed to send watch event to sender")
            },
            Config::default(),
        )?;
        watcher.watch(path, RecursiveMode::NonRecursive)?;

        let mut buffer = [0_u8; MAX_LINE_LEN];
        while let Some(result) = receiver.recv().await {
            match result {
                Ok(_event) => {
                    if !reopen_if_rotated(&mut file, path, &mut position, &mut carry, &mut watcher)
                        .await?
                    {
                        continue;
                    }
                    if file.metadata().await?.len() == position {
                        continue;
                    }

                    file.seek(SeekFrom::Start(position)).await?;
                    loop {
                        let read = file.read(&mut buffer).await?;
                        if read == 0 {
                            break;
                        }
                        // Count bytes read, not the file length. A partial line stays in `carry`.
                        position += read as u64;
                        let (lines, next) =
                            split_capped_lines(std::mem::take(&mut carry), &buffer[..read]);
                        carry = next;
                        for line in lines {
                            if let Err(error) = f(d, line).await {
                                error!("Failed to handle read line: {error:?}");
                            }
                        }
                    }
                }
                Err(error) => error!("{error:?}"),
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_complete_line_is_emitted() {
        let (lines, carry) = split_capped_lines(LineCarry::default(), b"ok\n");
        assert_eq!(lines, ["ok"]);
        assert_eq!(carry, LineCarry::default());
    }

    #[test]
    fn partial_line_completes_on_later_read() {
        let (lines, carry) = split_capped_lines(LineCarry::default(), b"hel");
        assert!(lines.is_empty());
        assert_eq!(carry.pending, b"hel");
        assert!(!carry.discarding);

        let (lines, carry) = split_capped_lines(carry, b"lo\n");
        assert_eq!(lines, ["hello"]);
        assert_eq!(carry, LineCarry::default());
    }

    #[test]
    fn overlong_line_is_dropped() {
        let mut buffered = vec![b'A'; MAX_LINE_LEN + 1];
        buffered.extend_from_slice(b"\nkept\n");
        let (lines, carry) = split_capped_lines(LineCarry::default(), &buffered);
        assert_eq!(lines, ["kept"]);
        assert_eq!(carry, LineCarry::default());

        let (lines, carry) =
            split_capped_lines(LineCarry::default(), &vec![b'B'; MAX_LINE_LEN + 1]);
        assert!(lines.is_empty());
        assert!(carry.pending.is_empty());
        assert!(carry.discarding);

        let (lines, carry) = split_capped_lines(carry, b"more\nkept\n");
        assert_eq!(lines, ["kept"]);
        assert_eq!(carry, LineCarry::default());
    }

    #[test]
    fn line_of_max_length_is_emitted() {
        let (lines, carry) = split_capped_lines(LineCarry::default(), &vec![b'C'; MAX_LINE_LEN]);
        assert!(lines.is_empty());
        assert_eq!(carry.pending.len(), MAX_LINE_LEN);

        let (lines, carry) = split_capped_lines(carry, b"\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_LINE_LEN);
        assert_eq!(carry, LineCarry::default());
    }
}
