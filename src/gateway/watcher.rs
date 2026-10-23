use std::{
    io::SeekFrom,
    path::{Path, PathBuf},
};

use color_eyre::eyre::Result;
use log::{debug, error, info};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
    sync::mpsc::channel,
};

const CHANNEL_BUFFER_SIZE: usize = 32;
const MAX_LOG_LINE_LEN: usize = 8192;
const READ_CHUNK: usize = 8192;

pub struct FileWatcher;

/// Lines from one snapshot of appended log bytes, plus how many trailing
/// bytes belong to an unfinished line that must be read again.
#[derive(Debug, PartialEq, Eq)]
pub struct LogChunk {
    pub lines: Vec<String>,
    pub leftover: usize,
    pub discarded: usize,
}

impl FileWatcher {
    pub async fn watch<F, D>(path: &PathBuf, d: &mut D, f: F) -> Result<()>
    where
        F: AsyncFn(&mut D, String) -> Result<()>,
    {
        let mut file = File::open(path).await?;
        // Start at EOF so a restart does not replay historical prefix lines.
        let mut position = file_len(path).await?;

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

        while let Some(result) = receiver.recv().await {
            match result {
                Ok(_event) => {
                    if let Err(error) = reopen_if_replaced(path, &mut file, &mut position).await {
                        error!("Failed to reopen log file: {error:?}");
                        continue;
                    }
                    let chunk = match read_new_lines(&mut file, &mut position).await {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            error!("Failed to read log file: {error:?}");
                            continue;
                        }
                    };
                    if chunk.discarded > 0 {
                        debug!("Discarded {} overlong syslog lines", chunk.discarded);
                    }
                    for line in chunk.lines {
                        // The raw line is intentionally not logged. Stdout is often
                        // forwarded back into the file being watched, and an echoed
                        // match would retrigger prefix handling.
                        if let Err(error) = f(d, line).await {
                            error!("Failed to handle read line: {error:?}");
                        }
                    }
                }
                Err(error) => error!("{error:?}"),
            }
        }

        Ok(())
    }
}

pub fn extract_log_lines(data: &[u8], max_len: usize) -> LogChunk {
    let mut lines = Vec::new();
    let mut discarded = 0usize;
    let mut start = 0usize;
    let mut idx = 0usize;

    while idx < data.len() {
        if data[idx] != b'\n' {
            if idx - start >= max_len {
                discarded += 1;
                while idx < data.len() && data[idx] != b'\n' {
                    idx += 1;
                }
                if idx == data.len() {
                    return LogChunk {
                        lines,
                        leftover: 0,
                        discarded,
                    };
                }
                idx += 1;
                start = idx;
                continue;
            }
            idx += 1;
            continue;
        }

        let mut end = idx;
        if end > start && data[end - 1] == b'\r' {
            end -= 1;
        }
        let line = &data[start..end];
        if line.len() <= max_len
            && let Ok(text) = std::str::from_utf8(line)
        {
            lines.push(text.to_string());
        } else {
            discarded += 1;
        }
        idx += 1;
        start = idx;
    }

    let leftover = data.len() - start;
    if leftover > max_len {
        LogChunk {
            lines,
            leftover: 0,
            discarded: discarded + 1,
        }
    } else {
        LogChunk {
            lines,
            leftover,
            discarded,
        }
    }
}

async fn file_len(path: &Path) -> Result<u64> {
    Ok(tokio::fs::metadata(path).await?.len())
}

async fn reopen_if_replaced(path: &Path, file: &mut File, position: &mut u64) -> Result<()> {
    let path_meta = tokio::fs::metadata(path).await?;
    let open_meta = file.metadata().await?;
    if !same_file(&path_meta, &open_meta) {
        *file = File::open(path).await?;
        *position = 0;
        info!("Log file was replaced; reading the new file from the start");
    }
    Ok(())
}

async fn read_new_lines(file: &mut File, position: &mut u64) -> Result<LogChunk> {
    let snapshot = file.metadata().await?.len();
    if snapshot < *position {
        info!("Log file shrank; reading from the start");
        *position = 0;
    }
    if snapshot == *position {
        return Ok(LogChunk {
            lines: Vec::new(),
            leftover: 0,
            discarded: 0,
        });
    }

    file.seek(SeekFrom::Start(*position)).await?;
    let mut cursor = *position;
    let mut carry = Vec::new();
    let mut lines = Vec::new();
    let mut discarded = 0usize;
    let mut buf = [0u8; READ_CHUNK];

    while cursor < snapshot {
        let want = usize::min(buf.len(), (snapshot - cursor) as usize);
        let read = file.read(&mut buf[..want]).await?;
        if read == 0 {
            break;
        }
        carry.extend_from_slice(&buf[..read]);
        let parsed = extract_log_lines(&carry, MAX_LOG_LINE_LEN);
        lines.extend(parsed.lines);
        discarded += parsed.discarded;
        if parsed.leftover == 0 {
            carry.clear();
        } else {
            let drop_n = carry.len() - parsed.leftover;
            carry.drain(..drop_n);
        }
        cursor += read as u64;
    }

    *position = cursor - carry.len() as u64;
    Ok(LogChunk {
        lines,
        leftover: carry.len(),
        discarded,
    })
}

fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_ordinary_lines_and_keeps_a_partial_tail() {
        let chunk = extract_log_lines(b"one\ntwo\r\npartial", 8192);
        assert_eq!(chunk.lines, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(chunk.leftover, b"partial".len());
        assert_eq!(chunk.discarded, 0);
    }

    #[test]
    fn discards_lines_longer_than_the_cap() {
        let chunk = extract_log_lines(b"ok\nthis-line-is-too-long\nnext\n", 8);
        assert_eq!(chunk.lines, vec!["ok".to_string(), "next".to_string()]);
        assert_eq!(chunk.discarded, 1);
        assert_eq!(chunk.leftover, 0);
    }

    #[test]
    fn discards_an_unfinished_overlong_line() {
        let chunk = extract_log_lines(b"abcdef", 3);
        assert!(chunk.lines.is_empty());
        assert_eq!(chunk.leftover, 0);
        assert_eq!(chunk.discarded, 1);
    }

    #[tokio::test]
    async fn reader_stops_at_the_snapshot_and_resumes_a_partial_line() {
        let path = std::env::temp_dir().join(format!("ubitix-log-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"first\npar").unwrap();

        let mut file = File::open(&path).await.unwrap();
        let mut position = 0u64;
        let chunk = read_new_lines(&mut file, &mut position).await.unwrap();
        assert_eq!(chunk.lines, vec!["first".to_string()]);
        assert_eq!(position, b"first\n".len() as u64);

        let mut extra = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        extra.write_all(b"tial\n").unwrap();

        let chunk = read_new_lines(&mut file, &mut position).await.unwrap();
        assert_eq!(chunk.lines, vec!["partial".to_string()]);
        assert_eq!(position, std::fs::metadata(&path).unwrap().len());
        let _ = std::fs::remove_file(&path);
    }
}
