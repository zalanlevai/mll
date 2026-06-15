use std::sync::Arc;
use std::time::{Duration, Instant};

use nvml_wrapper as nvml;
use nvml_wrapper::Nvml;
use nvml_wrapper::error::NvmlError;
use parking_lot::{RwLock, RwLockUpgradableReadGuard};

use crate::engine::EngineInstance;

pub enum GpuMonitoringInterfaceHandle {
    None,
    Nvidia(Nvml),
}

#[derive(Clone, Debug)]
pub struct GpuDevice {
    pub index: usize,
    pub name: String,
    pub total_memory_bytes: u64,
    pub free_memory_bytes: u64,
    pub reserved_memory_bytes: u64,
    pub used_memory_bytes: u64,
}

#[derive(Clone, Debug)]
pub enum GpuAllocationOwner {
    EngineInstance(Arc<EngineInstance>),
    Other { process_id: u32 },
}

#[derive(Clone, Debug)]
pub struct GpuAllocation {
    pub owner: GpuAllocationOwner,
    pub gpu_index: usize,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct GpuMonitoringSnapshot {
    pub time: Instant,
    pub gpu_devices: Vec<GpuDevice>,
    pub gpu_allocations: Vec<GpuAllocation>,
}

impl GpuMonitoringSnapshot {
    pub fn model_gpu_allocations(&self, model_name: &str) -> impl Iterator<Item = &GpuAllocation> {
        self.gpu_allocations.iter().filter(move |gpu_allocation| {
            match &gpu_allocation.owner {
                GpuAllocationOwner::EngineInstance(engine_instance) => engine_instance.model_name() == model_name,
                GpuAllocationOwner::Other { .. } => false,
            }
        })
    }
}

pub struct GpuMonitoringInterface {
    handle: GpuMonitoringInterfaceHandle,
    last_snapshot: RwLock<Option<Arc<GpuMonitoringSnapshot>>>,
}

impl GpuMonitoringInterface {
    pub fn new(handle: GpuMonitoringInterfaceHandle) -> Self {
        Self {
            handle,
            last_snapshot: RwLock::new(None),
        }
    }

    pub fn memory_usage_snapshot(&self, engine_instances: &[Arc<EngineInstance>]) -> Option<Arc<GpuMonitoringSnapshot>> {
        fn nvidia_memory_usage_snapshot(nvml: &Nvml, snapshot: &mut GpuMonitoringSnapshot, engine_instances: &[Arc<EngineInstance>]) -> Result<(), NvmlError> {
            let gpus_count = nvml.device_count()?;

            for gpu_index in 0..gpus_count {
                let gpu = nvml.device_by_index(gpu_index)?;
                let gpu_name = gpu.name()?;
                let gpu_memory_info = gpu.memory_info()?;

                snapshot.gpu_devices.push(GpuDevice {
                    index: gpu_index as usize,
                    name: gpu_name,
                    total_memory_bytes: gpu_memory_info.total,
                    free_memory_bytes: gpu_memory_info.free,
                    reserved_memory_bytes: gpu_memory_info.reserved,
                    used_memory_bytes: gpu_memory_info.used,
                });

                let gpu_running_compute_processes = gpu.running_compute_processes()?;
                for gpu_process_info in gpu_running_compute_processes {
                    let engine_instance = engine_instances.iter().find(|engine_instance| {
                        let Some(running_engine) = &*engine_instance.running_engine.read() else { return false; };
                        running_engine.gpu_process_id() == gpu_process_info.pid
                    });

                    let owner = match engine_instance {
                        Some(engine_instance) => GpuAllocationOwner::EngineInstance(Arc::clone(engine_instance)),
                        None => GpuAllocationOwner::Other { process_id: gpu_process_info.pid },
                    };

                    snapshot.gpu_allocations.push(GpuAllocation {
                        owner,
                        gpu_index: gpu_index as usize,
                        memory_bytes: match gpu_process_info.used_gpu_memory {
                            nvml::enums::device::UsedGpuMemory::Used(v) => v,
                            nvml::enums::device::UsedGpuMemory::Unavailable => return Err(NvmlError::NoData),
                        },
                    });
                }
            }

            Ok(())
        }

        let last_snapshot_upgradable_read_guard = self.last_snapshot.upgradable_read();
        if let Some(snapshot) = &*last_snapshot_upgradable_read_guard && snapshot.time.elapsed() < Duration::from_secs(1) {
            return Some(Arc::clone(snapshot));
        }
        let mut last_snapshot_write_guard = RwLockUpgradableReadGuard::upgrade(last_snapshot_upgradable_read_guard);

        let mut snapshot_arc = last_snapshot_write_guard.get_or_insert_with(|| {
            Arc::new(GpuMonitoringSnapshot {
                time: Instant::now(),
                gpu_devices: Vec::with_capacity(8),
                gpu_allocations: Vec::with_capacity(64),
            })
        });

        // NOTE: Reuse the snapshot allocation if no other accessor exists to it (i.e., the Arc is unique),
        //       otherwise create a new one.
        let snapshot = Arc::make_mut(&mut snapshot_arc);
        snapshot.time = Instant::now();
        snapshot.gpu_devices.clear();
        snapshot.gpu_allocations.clear();

        match &self.handle {
            GpuMonitoringInterfaceHandle::None => {}
            GpuMonitoringInterfaceHandle::Nvidia(nvml) => {
                if let Err(error) = nvidia_memory_usage_snapshot(nvml, snapshot, engine_instances) {
                    eprintln!("cannot get GPU monitoring information: {}", error);
                    return None;
                }
            }
        }

        Some(Arc::clone(&snapshot_arc))
    }
}
