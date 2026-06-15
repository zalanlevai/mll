use std::process;

use mll_core::ipc::{self, DaemonSocketStream};

use crate::display::{DurationDisplayExt, MemoryDisplayExt, MemoryUnit};

pub fn load(mut daemon_socket_stream: DaemonSocketStream, model_name: &str) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::Load { model_name: model_name.to_owned() });

    loop {
        match daemon_socket_stream.must_recv::<ipc::LoadProgress>() {
            ipc::LoadProgress::BadRequest(load_request_error) => {
                match load_request_error {
                    ipc::LoadRequestError::UnknownModel => {
                        eprintln!("error: unknown model `{}`", model_name);
                        process::exit(1);
                    }
                    ipc::LoadRequestError::AlreadyLoadedModel => {
                        eprintln!("model `{}` already loaded", model_name);
                        process::exit(0);
                    }
                    ipc::LoadRequestError::BadConfig { msg: error } => {
                        eprintln!("error: bad configuration: {}", error);
                        process::exit(1);
                    }
                }
            }
            ipc::LoadProgress::Milestone(load_milestone) => {
                match load_milestone {
                    ipc::LoadMilestone::Invoked { cwd: _, program, args, envs: _ } => {
                        eprint!("invoking engine \"{}\"", program);
                        for arg in &args {
                            eprint!(" \"{}\"", arg);
                        }
                        eprintln!();
                    }
                    _ => {}
                }
            }
            ipc::LoadProgress::StdoutLine { line } => { println!("engine: {}", line); }
            ipc::LoadProgress::StderrLine { line } => { eprintln!("engine: {}", line); }
            ipc::LoadProgress::Completion(completion) => {
                match completion {
                    ipc::Completion::Success(()) => {
                        eprintln!("model `{}` loaded successfully", model_name);
                        process::exit(0);
                    }
                    ipc::Completion::Failure(load_error) => {
                        eprint!("failed to load model `{}`: ", model_name);
                        match load_error {
                            ipc::LoadError::AuxFileCreate { path, inner_error } => {
                                eprintln!("cannot create auxiliary filesystem entry `{}`: {}", path.display(), inner_error);
                            }
                            ipc::LoadError::ProcessSpawn { inner_error } => {
                                eprintln!("cannot spawn engine process: {}", inner_error);
                            }
                            ipc::LoadError::ProcessExit { exit_code, reason } => {
                                if let Some(reason) = reason {
                                    match reason {
                                        ipc::EngineStartupFailureReason::InsufficientGpuMemAvailableForReservation => {
                                            eprint!("the model requested a larger GPU memory reservation than the free GPU memory available: ");
                                        }
                                        ipc::EngineStartupFailureReason::GpuMemReservationInsufficientForModel => {
                                            eprint!("the requested GPU memory reservation is insufficient to load the model weights: ");
                                        }
                                        ipc::EngineStartupFailureReason::GpuMemReservationInsufficientForContext => {
                                            eprint!("the requested GPU memory reservation is insufficient to serve requests with the specified model context length: ");
                                        }
                                    }
                                }
                                match exit_code {
                                    Some(exit_code) => {
                                        eprintln!("engine process exited with exit code {}", exit_code);
                                    }
                                    None => {
                                        eprintln!("engine process exited with unknown exit code");
                                    }
                                }
                            }
                        }
                        process::exit(1);
                    }
                }
            }
        }
    }
}

pub fn unload(mut daemon_socket_stream: DaemonSocketStream, model_name: &str, force: bool) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::Unload { model_name: model_name.to_owned(), force });

    loop {
        match daemon_socket_stream.must_recv::<ipc::UnloadProgress>() {
            ipc::UnloadProgress::BadRequest(unload_request_error) => {
                match unload_request_error {
                    ipc::UnloadRequestError::ModelNotLoaded => {
                        eprintln!("error: model `{}` not loaded", model_name);
                        process::exit(1);
                    }
                    ipc::UnloadRequestError::ModelLoading => {
                        eprintln!("model `{}` loading", model_name);
                        process::exit(1);
                    }
                    ipc::UnloadRequestError::ModelPendingRequests => {
                        eprintln!("model `{}` has pending requests: use the `--force` flag to override", model_name);
                        process::exit(1);
                    }
                }
            }
            ipc::UnloadProgress::StdoutLine { line } => { println!("engine: {}", line); }
            ipc::UnloadProgress::StderrLine { line } => { eprintln!("engine: {}", line); }
            ipc::UnloadProgress::Completion(completion) => {
                match completion {
                    ipc::Completion::Success(()) => {
                        eprintln!("model `{}` unloaded successfully", model_name);
                        process::exit(0);
                    }
                }
            }
        }
    }
}

pub enum ModelListFilter {
    Loaded,
    Active,
    Inactive,
}

pub struct ListModelsOpts {
    pub filters: Vec<ModelListFilter>,
    pub show_activity: bool,
}

pub fn list_models(mut daemon_socket_stream: DaemonSocketStream, opts: ListModelsOpts) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::GetModels);
    let mut models_response = daemon_socket_stream.must_recv::<ipc::GetModelsResponse>();

    models_response.models.retain(|model| {
        opts.filters.iter().all(|filter| match filter {
            ModelListFilter::Loaded => matches!(model.model_state, ipc::ModelState::Loaded(_)),
            ModelListFilter::Active => {
                let ipc::ModelState::Loaded(loaded_model) = &model.model_state else { return false; };
                loaded_model.model_activity.pending_requests_count >= 1
            }
            ModelListFilter::Inactive => {
                let ipc::ModelState::Loaded(loaded_model) = &model.model_state else { return false; };
                loaded_model.model_activity.pending_requests_count == 0
            }
        })
    });

    let model_name_w = models_response.models.iter().map(|model| model.name.len()).max().unwrap_or(0);

    for model in models_response.models {
        print!("{:model_name_w$}", model.name);

        if opts.show_activity {
            match &model.model_state {
                ipc::ModelState::NotLoaded => {}
                ipc::ModelState::Loading => print!("   loading"),
                ipc::ModelState::Loaded(loaded_model) => {
                    match loaded_model.model_activity.latest_response_end_time {
                        _ if loaded_model.model_activity.pending_requests_count >= 1 => {
                            match loaded_model.model_activity.pending_requests_count {
                                1 => print!("   active now (1 pending request)"),
                                v => print!("   active now ({} pending requests)", v),
                            }
                        }
                        Some(latest_response_end_time) => {
                            print!("   last active {}", latest_response_end_time.elapsed().unwrap_or_default().display_imprecise_ago())
                        }
                        // No model request yet.
                        None => print!("   never active"),
                    }
                }
                ipc::ModelState::Unloading => print!("   unloading"),
            }
        }

        println!();
    }

    process::exit(0);
}

pub fn usage(mut daemon_socket_stream: DaemonSocketStream) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::GetUsage);
    let usage_response = daemon_socket_stream.must_recv::<ipc::GetUsageResponse>();

    let Some(gpu_usage) = usage_response.gpu_usage else {
        eprintln!("error: cannot read resource usage metrics");
        process::exit(1);
    };

    const OWNER_MIN_LEN: usize = "<free>".len();
    let owner_w = gpu_usage.gpu_allocations.iter()
        .map(|gpu_allocation| {
            match &gpu_allocation.owner {
                ipc::GpuAllocationOwner::Model { model_name } => model_name.len(),
                ipc::GpuAllocationOwner::Other { process_id } => "other ()".len() + process_id.checked_ilog10().unwrap_or(0) as usize + 1,
            }
        })
        .max()
        .unwrap_or(0);
    let owner_w = usize::max(owner_w, OWNER_MIN_LEN);

    let mut gpu_devices_iter = gpu_usage.gpu_devices.iter().peekable();
    while let Some(gpu_device) = gpu_devices_iter.next() {
        let used_memory_display = (gpu_device.used_memory_bytes + gpu_device.reserved_memory_bytes).display_memory_in(MemoryUnit::GiB);
        let total_memory_display = gpu_device.total_memory_bytes.display_memory_in(MemoryUnit::GiB);
        println!("GPU {}: {} ({} / {})", gpu_device.index, gpu_device.name, used_memory_display, total_memory_display);

        let gpu_device_allocations = gpu_usage.gpu_allocations.iter().filter(|gpu_allocation| gpu_allocation.gpu_index == gpu_device.index);

        for gpu_allocation in gpu_device_allocations {
            match &gpu_allocation.owner {
                ipc::GpuAllocationOwner::Model { model_name } => print!("{:owner_w$}", model_name),
                ipc::GpuAllocationOwner::Other { process_id } => print!("{:owner_w$}", format!("other ({})", process_id)),
            }
            print!("   {}", gpu_allocation.memory_bytes.display_memory());
            println!();
        }

        println!("{:owner_w$}   {}", "<free>", gpu_device.free_memory_bytes.display_memory());

        if gpu_devices_iter.peek().is_some() { println!(); }
    }

    process::exit(0);
}

pub fn reload_config(mut daemon_socket_stream: DaemonSocketStream) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::ReloadConfig);

    loop {
        match daemon_socket_stream.must_recv::<ipc::ReloadConfigProgress>() {
            ipc::ReloadConfigProgress::Completion(completion) => {
                match completion {
                    ipc::Completion::Success((config_file_path, warnings)) => {
                        for warning in warnings {
                            match warning {
                                ipc::ConfigWarning::PortChangeRequiresRestart => {
                                    eprintln!("warning: detected a change to the daemon port: this configuration change requires a daemon restart");
                                }
                            }
                        }
                        eprintln!("reloaded configuration options from `{}`", config_file_path.display());
                        process::exit(0);
                    }
                    ipc::Completion::Failure((config_file_path, error)) => {
                        match error {
                            ipc::ReloadConfigError::Io { inner_error } => {
                                eprintln!("error: cannot read configuration file: {}", inner_error);
                            }
                            ipc::ReloadConfigError::Parsing { inner_error } => {
                                eprintln!("error: cannot parse configuration file: {}", inner_error);
                            }
                        }
                        eprintln!("failed to reload configuration options from `{}`", config_file_path.display());
                        process::exit(1);
                    }
                }
            }
        }
    }
}
