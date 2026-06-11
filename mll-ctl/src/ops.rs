use std::process;

use mll_core::ipc::{self, DaemonSocketStream};

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
}

pub fn list_models(mut daemon_socket_stream: DaemonSocketStream, filters: Vec<ModelListFilter>) -> ! {
    daemon_socket_stream.must_send(&ipc::ControlMessage::GetModels);
    let models_response = daemon_socket_stream.must_recv::<ipc::GetModelsResponse>();

    let filtered_models = models_response.models.into_iter().filter(|model| {
        filters.iter().all(|filter| match filter {
            ModelListFilter::Loaded => matches!(model.model_state, ipc::ModelState::Loaded),
        })
    });

    for model in filtered_models {
        println!("{}", model.name);
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
