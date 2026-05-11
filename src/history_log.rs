use std::fs::{OpenOptions, create_dir_all};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

static NEXT_HISTORY_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct HistoryLogger {
    path: PathBuf,
    sender: Option<mpsc::Sender<Vec<u8>>>,
    handle: Option<JoinHandle<()>>,
}

impl HistoryLogger {
    pub(crate) fn new(dir: &Path, shell: &str) -> std::io::Result<Self> {
        create_dir_all(dir)?;
        let id = NEXT_HISTORY_ID.fetch_add(1, Ordering::Relaxed);
        let created_ms = now_unix_ms();
        let stem = format!("agent-terminal-{created_ms}-{id}");
        let path = dir.join(format!("{stem}.ansi"));
        let meta_path = dir.join(format!("{stem}.meta.json"));

        let writer = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        write_metadata(&meta_path, &path, shell, created_ms, id)?;

        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let handle = thread::Builder::new()
            .name("agent-history-log".to_string())
            .spawn(move || {
                let mut writer = BufWriter::new(writer);
                while let Ok(bytes) = receiver.recv() {
                    if writer.write_all(&bytes).is_err() {
                        break;
                    }
                }
                let _ = writer.flush();
            })?;

        Ok(Self {
            path,
            sender: Some(sender),
            handle: Some(handle),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn record_pty_output(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            if let Some(sender) = &self.sender {
                let _ = sender.send(bytes.to_vec());
            }
        }
    }
}

impl Drop for HistoryLogger {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn write_metadata(
    meta_path: &Path,
    transcript_path: &Path,
    shell: &str,
    created_ms: u128,
    id: u64,
) -> std::io::Result<()> {
    let metadata = json!({
        "created_unix_ms": created_ms,
        "id": id,
        "shell": shell,
        "format": "raw-pty-output-ansi",
        "transcript": transcript_path.to_string_lossy(),
        "privacy": "records PTY output only; local keystrokes that the terminal does not echo are not written by this logger"
    });
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(meta_path)?;
    writeln!(file, "{}", metadata)
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::HistoryLogger;
    use std::fs;

    #[test]
    fn writes_pty_output_and_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "agent-terminal-history-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        let transcript_path = {
            let logger = HistoryLogger::new(&dir, "/bin/zsh").expect("create history logger");
            let path = logger.path().to_path_buf();
            logger.record_pty_output(b"hello\n");
            logger.record_pty_output(b"\x1b[31mred\x1b[0m\n");
            path
        };

        let transcript = fs::read(&transcript_path).expect("read transcript");
        assert_eq!(transcript, b"hello\n\x1b[31mred\x1b[0m\n");

        let meta_path = transcript_path.with_extension("meta.json");
        let metadata = fs::read_to_string(meta_path).expect("read metadata");
        assert!(metadata.contains("raw-pty-output-ansi"));
        assert!(metadata.contains("/bin/zsh"));

        let _ = fs::remove_dir_all(&dir);
    }
}
