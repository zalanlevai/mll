use std::collections::HashMap;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicU64};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;
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

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct PendingEngineRequestId(u64);

impl PendingEngineRequestId {
    pub fn new() -> Self {
        // NOTE: Using an ever increasing, wrapping atomic counter is sufficient for disambiguating engine requests,
        //       as they only have to distinguish pending requests.
        static ENGINE_REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(ENGINE_REQUEST_ID_COUNTER.fetch_add(1, atomic::Ordering::Relaxed))
    }
}

pub struct PendingEngineRequest {
    pub start_time: Instant,
}

impl PendingEngineRequest {
    pub fn elapsed_since_start(&self) -> Duration {
        self.start_time.elapsed()
    }
}

pub struct EngineRequestResponderHandle {
    request_id: PendingEngineRequestId,
    engine_instance: Arc<EngineInstance>,
    request: Arc<PendingEngineRequest>,
}

impl Deref for EngineRequestResponderHandle {
    type Target = PendingEngineRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

impl Drop for EngineRequestResponderHandle {
    fn drop(&mut self) {
        let mut engine_activity_write_guard = self.engine_instance.engine_activity.write();
        engine_activity_write_guard.pending_engine_requests.remove(&self.request_id);
        engine_activity_write_guard.latest_response_end_time = Some(SystemTime::now());
    }
}

pub struct EngineActivity {
    pub(crate) pending_engine_requests: HashMap<PendingEngineRequestId, Arc<PendingEngineRequest>>,
    pub(crate) latest_request_start_time: Option<SystemTime>,
    pub(crate) latest_response_end_time: Option<SystemTime>,
}

impl EngineActivity {
    pub fn has_pending_requests(&self) -> bool {
        !self.pending_engine_requests.is_empty()
    }
}

pub struct EngineInstance {
    pub(crate) engine_config: config::Engine,
    pub(crate) model_config: config::Model,
    pub(crate) log_file_path: PathBuf,
    pub(crate) engine_port: u16,
    pub(crate) engine_state: RwLock<EngineState>,
    pub(crate) running_engine: RwLock<Option<RunningEngine>>,
    pub(crate) engine_activity: RwLock<EngineActivity>,
}

impl EngineInstance {
    pub fn model_name(&self) -> &str {
        &self.model_config.name
    }

    pub(crate) fn track_pending_engine_request(self: &Arc<Self>) -> EngineRequestResponderHandle {
        let pending_engine_request_id = PendingEngineRequestId::new();

        let pending_engine_request = Arc::new(PendingEngineRequest {
            start_time: Instant::now(),
        });

        let mut engine_activity_write_guard = self.engine_activity.write();
        engine_activity_write_guard.pending_engine_requests.insert(pending_engine_request_id, Arc::clone(&pending_engine_request));
        engine_activity_write_guard.latest_request_start_time = Some(SystemTime::now());
        drop(engine_activity_write_guard);

        EngineRequestResponderHandle {
            request_id: pending_engine_request_id,
            engine_instance: Arc::clone(self),
            request: pending_engine_request,
        }
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
