use std::path::PathBuf;

use serde::{Serialize, Deserialize};

pub const DAEMON_SOCKET_PATH: &str = "/tmp/mlld/socket.sock";
pub const DEFAULT_CONFIG_FILE_PATH: &str = "/etc/mll/mll.toml";
pub const DEFAULT_LOGS_DIR_PATH: &str = "/tmp/mlld/logs";

#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Vllm,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Engine {
    pub name: String,
    pub kind: EngineKind,
    pub path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MemorySpec {
    gib: Option<f64>,
    mib: Option<f64>,
    kib: Option<f64>,
    bytes: Option<u64>,
}

impl MemorySpec {
    pub fn bytes(&self) -> u64 {
        let mut total_bytes = 0;

        if let Some(memory_gib) = self.gib {
            total_bytes += (memory_gib * 1024_f64 * 1024_f64 * 1024_f64) as u64;
        }
        if let Some(memory_mib) = self.mib {
            total_bytes += (memory_mib * 1024_f64 * 1024_f64) as u64;
        }
        if let Some(memory_kib) = self.kib {
            total_bytes += (memory_kib * 1024_f64) as u64;
        }
        if let Some(memory_bytes) = self.bytes {
            total_bytes += memory_bytes;
        }

        total_bytes
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MemoryRequirement {
    Relative { pct: f64 },
    Absolute(MemorySpec),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub engine: String,
    pub path: PathBuf,
    pub gpu_memory: MemoryRequirement,
    pub max_context_tokens: usize,
    pub engine_args: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
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
