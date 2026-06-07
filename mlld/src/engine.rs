use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitStatus;

use tokio::fs;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config;

pub struct LogFile {
    path: PathBuf,
    file_buf_writer: BufWriter<fs::File>,
    is_errored: bool,
}

impl LogFile {
    pub async fn create(path: &Path) -> io::Result<Self> {
        let file = fs::File::create(path).await?;

        Ok(Self {
            path: path.to_owned(),
            file_buf_writer: BufWriter::new(file),
            is_errored: false,
        })
    }

    #[inline]
    pub async fn write(&mut self, text: &str) -> io::Result<()> {
        self.file_buf_writer.write_all(text.as_bytes()).await
    }

    pub async fn try_write(&mut self, text: &str) {
        if self.is_errored { return; }

        match self.write(text).await {
            Ok(()) => {}
            Err(error) => {
                self.is_errored = true;
                eprintln!("io error: cannot write to log file `{}`: {}", self.path.display(), error);
            }
        }
    }

    #[inline]
    pub async fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.file_buf_writer.write_all(line.as_bytes()).await?;
        self.file_buf_writer.write_all(b"\n").await?;
        Ok(())
    }

    pub async fn try_write_line(&mut self, line: &str) {
        if self.is_errored { return; }

        match self.write_line(line).await {
            Ok(()) => {}
            Err(error) => {
                self.is_errored = true;
                eprintln!("io error: cannot write to log file `{}`: {}", self.path.display(), error);
            }
        }
    }

    pub async fn flush_and_sync(&mut self) -> io::Result<()> {
        self.file_buf_writer.flush().await?;
        self.file_buf_writer.get_ref().sync_all().await?;
        Ok(())
    }

    pub async fn try_flush_and_sync(&mut self) {
        if self.is_errored { return }

        match self.flush_and_sync().await {
            Ok(()) => {}
            Err(error) => {
                self.is_errored = true;
                eprintln!("io error: cannot write to log file `{}`: {}", self.path.display(), error);
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum EngineState {
    Starting,
    Running,
    Stopping,
}

pub enum OutputStream {
    Stdout,
    Stderr,
}

pub(crate) type OutputHook = Box<dyn FnMut(OutputStream, String) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync + 'static>;

pub struct RunningEngine {
    pub(crate) kill_signal_tx: oneshot::Sender<()>,
    pub(crate) output_hook_tx: mpsc::Sender<Option<OutputHook>>,
    pub(crate) task: JoinHandle<ExitStatus>,
}

pub struct EngineInstance {
    pub(crate) engine_config: config::Engine,
    pub(crate) model_config: config::Model,
    pub(crate) log_file_path: PathBuf,
    pub(crate) engine_port: u16,
    pub(crate) engine_state: EngineState,
    pub(crate) running_engine: Option<RunningEngine>,
}

impl EngineInstance {
    pub fn model_name(&self) -> &str {
        &self.model_config.name
    }
}

pub fn next_available_port(base_port: u16, allocated_ports: &mut Vec<u16>) -> u16 {
    allocated_ports.sort();

    let mut port = base_port;
    for &mut allocated_port in allocated_ports {
        if port < allocated_port { return port; }
        if port == allocated_port { port += 1; }
        if port > allocated_port { continue; }
    }

    port
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_available_port() {
        assert_eq!(3000, next_available_port(3000, &mut vec![]), "return base port, if no allocations");
        assert_eq!(3000, next_available_port(3000, &mut vec![3001]), "return base port, if available");
        assert_eq!(3003, next_available_port(3000, &mut vec![3000, 3001, 3002]), "return new port, if all previous ones allocated");
        assert_eq!(3001, next_available_port(3000, &mut vec![3000, 3002, 3003]), "return first available port");
        assert_eq!(3002, next_available_port(3000, &mut vec![3000, 3001, 3005]), "return first available port in the presence of large port allocation gaps");
        assert_eq!(3002, next_available_port(3000, &mut vec![3000, 3001, 3001, 3005]), "ignore duplicate port allocations");
    }
}
