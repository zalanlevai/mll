use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use mll_core::config::Config;

use crate::engine::{self, EngineInstance};

pub struct PortReservation {
    dcx: Arc<DaemonCtxt>,
    port: u16,
}

impl PortReservation {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for PortReservation {
    fn drop(&mut self) {
        self.dcx.reserved_ports.write().retain(|port| *port != self.port);
    }
}

pub struct DaemonCtxt {
    pub(crate) config_file_path: PathBuf,
    pub(crate) loaded_config: RwLock<Config>,
    reserved_ports: RwLock<Vec<u16>>,
    pub(crate) engine_instances: RwLock<Vec<Arc<EngineInstance>>>,
}

impl DaemonCtxt {
    pub fn new(config_file_path: PathBuf, config: Config) -> Self {
        Self {
            config_file_path,
            loaded_config: RwLock::new(config),
            reserved_ports: RwLock::new(Vec::with_capacity(8)),
            engine_instances: RwLock::new(Vec::with_capacity(64)),
        }
    }

    pub fn model_engine_instance(&self, model_name: &str) -> Option<Arc<EngineInstance>> {
        self.engine_instances.read().iter().find(|engine_instance| engine_instance.model_name() == model_name).map(Arc::clone)
    }

    /// Return the next available engine port according to internal accounting.
    ///
    /// The returned port is "reserved", meaning that
    /// it will not be given out by subsequent calls until the reservation is held.
    /// Therefore, it is important to hold the port reservation until
    /// the port is assigned to an engine instance.
    pub fn next_available_port(self: &Arc<Self>) -> PortReservation {
        let base_port = self.loaded_config.read().engine_base_port();

        // NOTE: We do not need to hold the read guard on engine instances for the duration of the function
        //       because their ports are sourced from previously given out port reservations,
        //       which we account for in the next step.
        let mut allocated_ports = self.engine_instances.read().iter()
            .map(|engine_instance| engine_instance.engine_port)
            .collect::<Vec<_>>();

        // NOTE: Prevent new port reservations until we give out this one.
        let mut reserved_ports_write_guard = self.reserved_ports.write();
        allocated_ports.extend(reserved_ports_write_guard.iter().copied());

        let port = engine::next_available_port(base_port, &mut allocated_ports);

        reserved_ports_write_guard.push(port);
        PortReservation { dcx: Arc::clone(self), port }
    }
}
