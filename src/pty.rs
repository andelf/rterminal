use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use anyhow::{Context as _, Result};
use async_channel::Receiver;
use parking_lot::Mutex;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub(crate) struct PtySession {
    pub(crate) master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    pub(crate) writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub(crate) child: Arc<Mutex<Box<dyn Child + Send>>>,
    pub(crate) output_rx: Receiver<Vec<u8>>,
    pub(crate) shell: String,
}

impl PtySession {
    pub(crate) fn spawn(rows: u16, cols: u16) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to create PTY")?;

        let mut command = CommandBuilder::new(shell.clone());
        command.arg("-i");

        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to spawn shell")?;

        let master = Arc::new(Mutex::new(pair.master));
        let writer = {
            let writer = master
                .lock()
                .take_writer()
                .context("failed to get PTY writer")?;
            Arc::new(Mutex::new(writer))
        };

        let mut reader = master
            .lock()
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let (tx, rx) = async_channel::unbounded();
        thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.send_blocking(buf[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master,
            writer,
            child: Arc::new(Mutex::new(child)),
            output_rx: rx,
            shell,
        })
    }
}

pub(crate) fn write_to_pty(writer: &Arc<Mutex<Box<dyn Write + Send>>>, bytes: &[u8]) -> Result<()> {
    let mut writer = writer.lock();
    writer
        .write_all(bytes)
        .context("failed to write bytes to PTY")?;
    writer.flush().context("failed to flush PTY writer")?;
    Ok(())
}
