use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde::{Serialize, Deserialize};

pub use crate::config::{DAEMON_SOCKET_PATH, Config};

#[derive(Debug)]
pub enum DaemonSocketReadError {
    Eof,
    Io(io::Error),
    Parsing(serde_json::Error, String),
}

#[derive(Debug)]
pub enum DaemonSocketWriteError {
    Serialization(serde_json::Error),
    Io(io::Error),
}

mod sync_socket {
    use std::io::{self, BufRead, BufReader, Write};
    use std::process;

    use interprocess::local_socket::prelude::*;
    use serde::{Serialize, Deserialize};

    use super::{DaemonSocketReadError, DaemonSocketWriteError};

    pub struct DaemonSocketStream {
        socket_stream_buf_reader: BufReader<LocalSocketStream>,
        read_buffer: String,
    }

    impl DaemonSocketStream {
        pub fn new(socket_stream: LocalSocketStream) -> Self {
            Self {
                socket_stream_buf_reader: BufReader::new(socket_stream),
                read_buffer: String::with_capacity(8192),
            }
        }

        pub fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), DaemonSocketWriteError> {
            let msg_str = serde_json::to_string(msg).map_err(DaemonSocketWriteError::Serialization)?;
            writeln!(self.socket_stream_buf_reader.get_mut(), "{}", msg_str).map_err(DaemonSocketWriteError::Io)?;
            Ok(())
        }

        #[track_caller]
        pub fn must_send<T: Serialize>(&mut self, msg: &T) {
            match self.send(msg) {
                Ok(()) => {}
                Err(DaemonSocketWriteError::Serialization(error)) => {
                    let location = std::panic::Location::caller();
                    eprintln!("internal error at {}:{}:{}: failed to serialize socket message: {}", location.file(), location.line(), location.column(), error);
                    process::exit(101);
                }
                Err(DaemonSocketWriteError::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
                    eprintln!("error: socket stream was closed prematurely");
                    process::exit(101);
                }
                Err(DaemonSocketWriteError::Io(error)) => {
                    eprintln!("error: cannot write to socket stream: {}", error);
                    process::exit(101);
                }
            }
        }

        pub fn recv<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T, DaemonSocketReadError> {
            self.read_buffer.clear();
            match self.socket_stream_buf_reader.read_line(&mut self.read_buffer) {
                Ok(0) => return Err(DaemonSocketReadError::Eof),
                Ok(_) => {}
                Err(error) => return Err(DaemonSocketReadError::Io(error)),
            }
            serde_json::from_str::<T>(&self.read_buffer).map_err(|error| DaemonSocketReadError::Parsing(error, self.read_buffer.clone()))
        }

        pub fn must_recv<T: for<'de> Deserialize<'de>>(&mut self) -> T {
            match self.recv::<T>() {
                Ok(data) => data,
                Err(DaemonSocketReadError::Eof) => {
                    eprintln!("error: daemon socket pipe closed prematurely");
                    process::exit(101);
                }
                Err(DaemonSocketReadError::Io(error)) => {
                    eprintln!("error: cannot read incoming socket message: {}", error);
                    process::exit(101);
                }
                Err(DaemonSocketReadError::Parsing(error, data)) => {
                    eprintln!("error: received unexpected message `{}`: {}", data, error);
                    process::exit(101);
                }
            }
        }
    }
}

#[cfg(feature = "tokio")]
mod async_socket {
    use std::io;

    use interprocess::local_socket::tokio::prelude::*;
    use serde::{Serialize, Deserialize};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::{DaemonSocketReadError, DaemonSocketWriteError};

    pub struct AsyncDaemonSocketStream {
        socket_stream_buf_reader: BufReader<LocalSocketStream>,
        is_other_end_closed: bool,
        read_buffer: String,
    }

    impl AsyncDaemonSocketStream {
        pub fn new(socket_stream: LocalSocketStream) -> Self {
            Self {
                socket_stream_buf_reader: BufReader::new(socket_stream),
                is_other_end_closed: false,
                read_buffer: String::with_capacity(8192),
            }
        }

        pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), DaemonSocketWriteError> {
            let mut msg_line_str = serde_json::to_string(msg).map_err(DaemonSocketWriteError::Serialization)?;
            msg_line_str.push('\n');
            self.socket_stream_buf_reader.write_all(msg_line_str.as_bytes()).await.map_err(DaemonSocketWriteError::Io)?;
            Ok(())
        }

        #[track_caller]
        pub async fn try_send<T: Serialize>(&mut self, msg: &T) {
            if self.is_other_end_closed { return; }

            match self.send(msg).await {
                Ok(()) => {}
                Err(DaemonSocketWriteError::Serialization(error)) => {
                    let location = std::panic::Location::caller();
                    eprintln!("internal error at {}:{}:{}: failed to serialize socket message: {}", location.file(), location.line(), location.column(), error);
                }
                Err(DaemonSocketWriteError::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
                    self.is_other_end_closed = true;
                    eprintln!("socket stream was closed prematurely");
                }
                Err(DaemonSocketWriteError::Io(_error)) => {}
            }
        }

        pub async fn recv<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T, DaemonSocketReadError> {
            self.read_buffer.clear();
            match self.socket_stream_buf_reader.read_line(&mut self.read_buffer).await {
                Ok(0) => return Err(DaemonSocketReadError::Eof),
                Ok(_) => {}
                Err(error) => return Err(DaemonSocketReadError::Io(error)),
            }
            serde_json::from_str::<T>(&self.read_buffer).map_err(|error| DaemonSocketReadError::Parsing(error, self.read_buffer.clone()))
        }
    }
}

pub use sync_socket::*;
#[cfg(feature = "tokio")]
pub use async_socket::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "completion", content = "result")]
pub enum Completion<T, E> {
    Success(T),
    Failure(E),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "action")]
pub enum ControlMessage {
    Load { model_name: String },
    Unload { model_name: String },
    GetModels,
    ReloadConfig,
    GetConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "error")]
pub enum LoadRequestError {
    UnknownModel,
    AlreadyLoadedModel,
    BadConfig { msg: String },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "milestone")]
pub enum LoadMilestone {
    Invoked { cwd: Option<PathBuf>, program: String, args: Vec<String>, envs: BTreeMap<String, Option<String>> },
    PlatformDetected,
    Started,
    WeightsLoaded,
    GraphsCaptured,
    Complete,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "reason")]
pub enum EngineStartupFailureReason {
    /// The model requested a larger GPU memory reservation than the free GPU memory available.
    InsufficientGpuMemAvailableForReservation,
    /// The requested GPU memory reservation is insufficient to load the model weights.
    GpuMemReservationInsufficientForModel,
    /// The requested GPU memory reservation is insufficient to serve requests with the specified model context length.
    GpuMemReservationInsufficientForContext,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "error")]
pub enum LoadError {
    AuxFileCreate { path: PathBuf, inner_error: String },
    ProcessSpawn { inner_error: String },
    ProcessExit { exit_code: Option<i32>, reason: Option<EngineStartupFailureReason> },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "load_progress")]
pub enum LoadProgress {
    BadRequest(LoadRequestError),
    Milestone(LoadMilestone),
    StdoutLine { line: String },
    StderrLine { line: String },
    Completion(Completion<(), LoadError>),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "error")]
pub enum UnloadRequestError {
    ModelNotLoaded,
    ModelLoading,
    ModelPendingRequests,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "error")]
pub enum UnloadError {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "unload_progress")]
pub enum UnloadProgress {
    BadRequest(UnloadRequestError),
    StdoutLine { line: String },
    StderrLine { line: String },
    Completion(Completion<(), UnloadError> ),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    NotLoaded,
    Loading,
    Loaded,
    Unloading,
}

#[derive(Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub max_context_tokens: usize,
    pub model_state: ModelState,
}

#[derive(Serialize, Deserialize)]
pub struct GetModelsResponse {
    pub models: Vec<Model>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "warning")]
pub enum ConfigWarning {
    PortChangeRequiresRestart,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "error")]
pub enum ReloadConfigError {
    Io { inner_error: String },
    Parsing { inner_error: String },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "reload_config_progress")]
pub enum ReloadConfigProgress {
    Completion(Completion<(PathBuf, Vec<ConfigWarning>), (PathBuf, ReloadConfigError)>),
}

#[derive(Serialize, Deserialize)]
pub struct GetConfigResponse {
    pub config: Config,
}
