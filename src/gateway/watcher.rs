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
    sync::mpsc::unbounded_channel,
};
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

struct FollowedFile {
    file: File,
    dev: u64,
    ino: u64,
}

/// Directory that receives create, delete, and modify events for `path`.
///
/// The watch is installed once, before the event loop. Reconfiguring it from
/// the loop deadlocks: `watch` waits on the notify thread, and that thread is
/// blocked in the event callback when the channel is full.
fn watch_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Point `followed` at the current file, or clear it when the path is gone.
///
/// A replaced or truncated file is reopened from the start. The caller keeps
/// watching the parent directory, so a later create is still delivered.
async fn sync_followed_file(
    followed: &mut Option<FollowedFile>,
    path: &Path,
    position: &mut u64,
    carry: &mut LineCarry,
) -> Result<bool> {
    let path_meta = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            error!("Watched file is missing: {}", path.display());
            *followed = None;
            *position = 0;
            *carry = LineCarry::default();
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };

    let same_file = followed
        .as_ref()
        .is_some_and(|open| open.dev == path_meta.dev() && open.ino == path_meta.ino());
    if same_file && path_meta.len() >= *position {
        return Ok(true);
    }

    let file = File::open(path).await?;
    let open_meta = file.metadata().await?;
    *followed = Some(FollowedFile {
        file,
        dev: open_meta.dev(),
        ino: open_meta.ino(),
    });
    *position = 0;
    *carry = LineCarry::default();
    Ok(true)
}

impl FileWatcher {
    pub async fn watch<F, D>(path: &PathBuf, d: &mut D, f: F) -> Result<()>
    where
        F: AsyncFn(&mut D, String) -> Result<()>,
    {
        let file = File::open(path).await?;
        // Existing lines are not replayed. Later reads start from this cursor.
        let meta = file.metadata().await?;
        let mut position = meta.len();
        let mut followed = Some(FollowedFile {
            dev: meta.dev(),
            ino: meta.ino(),
            file,
        });
        let mut carry = LineCarry::default();

        // `send` does not block. A blocking send here deadlocks with `watch`:
        // the notify thread sits in the callback while `watch` waits for it.
        let (sender, mut receiver) = unbounded_channel();
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                let _ = sender.send(result);
            },
            Config::default(),
        )?;
        watcher.watch(watch_directory(path), RecursiveMode::NonRecursive)?;

        let mut buffer = [0_u8; MAX_LINE_LEN];
        while let Some(result) = receiver.recv().await {
            match result {
                Ok(_event) => {
                    if !sync_followed_file(&mut followed, path, &mut position, &mut carry).await? {
                        continue;
                    }
                    let Some(open) = followed.as_mut() else {
                        continue;
                    };
                    if open.file.metadata().await?.len() == position {
                        continue;
                    }

                    open.file.seek(SeekFrom::Start(position)).await?;
                    loop {
                        let read = open.file.read(&mut buffer).await?;
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
