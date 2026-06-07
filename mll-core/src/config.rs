use std::path::PathBuf;

use serde::Deserialize;

pub const DAEMON_SOCKET_PATH: &str = "/tmp/mlld/socket.sock";
pub const DEFAULT_CONFIG_FILE_PATH: &str = "/etc/mll/mll.toml";
pub const DEFAULT_LOGS_DIR_PATH: &str = "/tmp/mlld/logs";

#[derive(Copy, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Vllm,
}

#[derive(Clone, Deserialize)]
pub struct Engine {
    pub name: String,
    pub kind: EngineKind,
    pub path: PathBuf,
}

#[derive(Clone, Deserialize)]
pub struct Model {
    pub name: String,
    pub engine: String,
    pub path: PathBuf,
    pub max_context_tokens: usize,
    pub engine_args: Vec<String>,
}

#[derive(Deserialize)]
pub struct Config {
    pub daemon_port: u16,
    pub engine_base_port: Option<u16>,
    #[serde(rename = "logs_dir")]
    pub logs_dir_path: Option<PathBuf>,
    pub engines: Vec<Engine>,
    pub models: Vec<Model>,
}

impl Config {
    pub fn engine_base_port(&self) -> u16 {
        self.engine_base_port.unwrap_or_else(|| self.daemon_port + 1)
    }

    pub fn logs_dir_path(&self) -> PathBuf {
        self.logs_dir_path.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_LOGS_DIR_PATH))
    }
}
