use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use mll_core::config::Config;
use mll_core::ipc::{self, AsyncDaemonSocketStream};
use parking_lot::RwLock;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::MissedTickBehavior;

use crate::ctxt::DaemonCtxt;
use crate::engine::{EngineInstance, EngineState, LogFile, OutputHook, OutputStream, RunningEngine};

pub(crate) async fn load(
    dcx: Arc<DaemonCtxt>,
    mut daemon_socket_stream: AsyncDaemonSocketStream,
    model_name: String,
) {
    eprintln!("requested to load model `{}`", model_name);

    let Some(model_config) = dcx.loaded_config.read().models.iter().find(|model| model.name == model_name).cloned() else {
        eprintln!("unknown model `{}`", model_name);
        daemon_socket_stream.try_send(&ipc::LoadProgress::BadRequest(ipc::LoadRequestError::UnknownModel)).await;
        return;
    };

    if let Some(_model_engine_instance) = dcx.model_engine_instance(&model_name) {
        eprintln!("model `{}` already loaded", model_name);
        daemon_socket_stream.try_send(&ipc::LoadProgress::BadRequest(ipc::LoadRequestError::AlreadyLoadedModel)).await;
        return;
    }

    let Some(engine_config) = dcx.loaded_config.read().engines.iter().find(|engine| engine.name == model_config.engine).cloned() else {
        let error = format!("cannot find engine `{}` referenced by model `{}`", model_config.engine, model_name);
        eprintln!("config error: {}", error);
        daemon_socket_stream.try_send(&ipc::LoadProgress::BadRequest(ipc::LoadRequestError::BadConfig { msg: error.clone() })).await;
        return;
    };

    let engine_port_reservation = dcx.next_available_port();

    let logs_dir_path = dcx.loaded_config.read().logs_dir_path();
    match fs::create_dir_all(&logs_dir_path).await {
        Ok(()) => {}
        Err(error) => {
            eprintln!("io error: cannot create engine logs directory `{}`: {}", logs_dir_path.display(), error);
            let ipc_error = ipc::LoadError::AuxFileCreate { path: logs_dir_path, inner_error: format!("{}", error) };
            daemon_socket_stream.try_send(&ipc::LoadProgress::Completion(ipc::Completion::Failure(ipc_error))).await;
            return;
        }
    };

    let log_file_path = logs_dir_path.join(format!("{}.log", model_name));
    let mut log_file = match LogFile::create(&log_file_path).await {
        Ok(file) => file,
        Err(error) => {
            eprintln!("io error: cannot create engine log file `{}`: {}", log_file_path.display(), error);
            let ipc_error = ipc::LoadError::AuxFileCreate { path: log_file_path, inner_error: format!("{}", error) };
            daemon_socket_stream.try_send(&ipc::LoadProgress::Completion(ipc::Completion::Failure(ipc_error))).await;
            return;
        }
    };

    let mut cmd = Command::new(&engine_config.path);
    cmd.arg("serve");

    cmd.arg(&model_config.path);
    cmd.arg(format!("--served-model-name={}", model_name));
    cmd.arg(format!("--port={}", engine_port_reservation.port()));
    cmd.arg(format!("--max-model-len={}", model_config.max_context_tokens));
    cmd.args(&model_config.engine_args);

    let engine_instance = Arc::new(EngineInstance {
        engine_config,
        model_config,
        log_file_path: log_file_path.clone(),
        engine_port: engine_port_reservation.port(),
        engine_state: RwLock::new(EngineState::Starting),
        // NOTE: None yet, will be populated once loading is complete and the command is fulfilled,
        //       when we span another task to keep monitoring the running engine.
        running_engine: RwLock::new(None),
    });
    dcx.engine_instances.write().push(Arc::clone(&engine_instance));
    // NOTE: Port reservation no longer required, as the port is now associated with an engine instance.
    drop(engine_port_reservation);

    let cmd_std = cmd.as_std();
    let cmd_display_str = format!("{:?}", cmd_std);
    eprintln!("running {}", cmd_display_str);

    log_file.try_write("running ").await;
    log_file.try_write_line(&cmd_display_str).await;

    let progress_msg = {
        let cwd = cmd_std.get_current_dir().map(|p| p.to_owned());

        let program = cmd_std.get_program().to_str().expect("program must be valid UTF-8").to_owned();
        let args = cmd_std.get_args().map(|arg| arg.to_str().expect("args must be valid UTF-8").to_owned()).collect::<Vec<_>>();

        let envs = cmd_std.get_envs()
            .map(|(key, val)| {
                let key = key.to_str().expect("environment variables must be valid UTF-8").to_owned();
                let val = val.map(|v| v.to_str().expect("environment variable values must be valid UTF-8").to_owned());
                (key, val)
            })
            .collect::<BTreeMap<_, _>>();

        ipc::LoadProgress::Milestone(ipc::LoadMilestone::Invoked { cwd, program, args, envs })
    };
    daemon_socket_stream.try_send(&progress_msg).await;

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("cannot spawn engine child process: {}", error);
            daemon_socket_stream.try_send(&ipc::LoadProgress::Completion(ipc::Completion::Failure(ipc::LoadError::ProcessSpawn { inner_error: format!("{}", error) }))).await;
            // Remove engine instance that failed to start.
            dcx.engine_instances.write().retain(|engine_instance| {
                !(engine_instance.model_name() == model_name && *engine_instance.engine_state.read() == EngineState::Starting)
            });
            return;
        }
    };

    let stdout = child.stdout.take().expect("engine child process stdout handle missing");
    let stderr = child.stderr.take().expect("engine child process stderr handle missing");

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    let mut stdout_eof = false;
    let mut stderr_eof = false;

    let mut flush_and_sync_interval = tokio::time::interval(Duration::from_secs(1));
    flush_and_sync_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut startup_failre_reason: Option<ipc::EngineStartupFailureReason> = None;

    loop {
        tokio::select! { biased;
            // NOTE: Periodically flush and sync log file to ensure we store intermediate output.
            _ = flush_and_sync_interval.tick() => {
                log_file.try_flush_and_sync().await;
            }
            line = stdout_lines.next_line(), if !stdout_eof => {
                match line {
                    Err(_error) => stdout_eof = true,
                    Ok(None) => stdout_eof = true,
                    Ok(Some(line)) => {
                        println!("stdout: {}", line);
                        log_file.try_write("stdout: ").await;
                        log_file.try_write_line(&line).await;
                        daemon_socket_stream.try_send(&ipc::LoadProgress::StdoutLine { line: line.clone() }).await;

                        if line.contains("ERROR") {
                            match () {
                                _ if line.contains("Free memory on device")
                                    && line.contains("on startup is less than desired GPU memory utilization")
                                => startup_failre_reason = Some(ipc::EngineStartupFailureReason::InsufficientGpuMemAvailableForReservation),

                                _ if line.contains("Failed to load model - not enough GPU memory.")
                                => startup_failre_reason = Some(ipc::EngineStartupFailureReason::InsufficientGpuMemAvailableForReservation),

                                _ if line.contains("No available memory for the cache blocks.")
                                => startup_failre_reason = Some(ipc::EngineStartupFailureReason::GpuMemReservationInsufficientForModel),

                                _ if line.contains("To serve at least one request with the models's max seq len")
                                    && line.contains("KV cache is needed, which is larger than the available KV cache memory")
                                => startup_failre_reason = Some(ipc::EngineStartupFailureReason::GpuMemReservationInsufficientForContext),

                                _ => {}
                            }
                        }
                    }
                }
            }
            line = stderr_lines.next_line(), if !stderr_eof => {
                match line {
                    Err(_error) => stderr_eof = true,
                    Ok(None) => stderr_eof = true,
                    Ok(Some(line)) => {
                        eprintln!("stderr: {}", line);
                        log_file.try_write("stderr: ").await;
                        log_file.try_write_line(&line).await;
                        daemon_socket_stream.try_send(&ipc::LoadProgress::StderrLine { line: line.clone() }).await;

                        if line.contains("Application startup complete.") {
                            break;
                        }
                    }
                }
            }
            exit_status = child.wait() => {
                let exit_status = exit_status.expect("failed to get engine child process exit status");
                match exit_status.code() {
                    Some(exit_code) => {
                        eprintln!("engine child process exited with exit code {}", exit_code);
                        log_file.try_write("process exited with exit code ").await;
                        log_file.try_write_line(&format!("{}", exit_code)).await;
                    }
                    None => {
                        eprintln!("engine child process exited with unknown exit code");
                        log_file.try_write_line("process exited with unknown exit code").await;
                    }
                }

                // NOTE: A successful startup breaks out of the select loop before this condition could be hit.
                //       Therefore, this corresponds with an unexpected engine child process exit during loading.
                let ipc_error = ipc::LoadError::ProcessExit { exit_code: exit_status.code(), reason: startup_failre_reason };
                daemon_socket_stream.try_send(&ipc::LoadProgress::Completion(ipc::Completion::Failure(ipc_error))).await;
                // Remove engine instance that failed to start.
                dcx.engine_instances.write().retain(|engine_instance| {
                    !(engine_instance.model_name() == model_name && *engine_instance.engine_state.read() == EngineState::Starting)
                });
                return;
            }
        }
    }

    eprintln!("startup complete: detaching");

    daemon_socket_stream.try_send(&ipc::LoadProgress::Completion(ipc::Completion::Success(()))).await;

    log_file.try_flush_and_sync().await;

    let (running_engine_output_hook_tx, mut running_engine_output_hook_rx) = mpsc::channel(1);
    let (running_engine_kill_signal_tx, mut running_engine_kill_signal_rx) = oneshot::channel::<()>();

    let running_engine_task = tokio::spawn(async move {
        let mut output_hook: Option<OutputHook> = None;

        loop {
            tokio::select! { biased;
                Some(new_output_hook) = running_engine_output_hook_rx.recv(), if !running_engine_output_hook_rx.is_closed() => {
                    output_hook = new_output_hook;
                }
                _ = &mut running_engine_kill_signal_rx, if !running_engine_kill_signal_rx.is_terminated() => {
                    eprintln!("killing engine process of model `{}`", model_name);

                    // NOTE: vLLM does not clean up its VLLM::EngineCore subprocess (thus not freeing up GPU resources)
                    //       when forcefully killed using `Child::start_kill` (SIGKILL).
                    //       Instead, we use an interrupt signal (SIGINT, i.e., Ctrl+C) to
                    //       signal to the engine process to gracefully shutdown.
                    if let Some(pid) = child.id() {
                        use nix::unistd::Pid;
                        use nix::sys::signal::{Signal, kill};

                        kill(Pid::from_raw(pid as i32), Signal::SIGINT).expect("cannot send interrupt signal to engine child process");
                    }
                }
                // NOTE: Periodically flush and sync log file to ensure we store intermediate output.
                _ = flush_and_sync_interval.tick() => {
                    log_file.try_flush_and_sync().await;
                }
                line = stdout_lines.next_line(), if !stdout_eof => {
                    match line {
                        Err(_error) => stdout_eof = true,
                        Ok(None) => stdout_eof = true,
                        Ok(Some(line)) => {
                            log_file.try_write("stdout: ").await;
                            log_file.try_write_line(&line).await;
                            if let Some(output_hook) = &mut output_hook {
                                output_hook(OutputStream::Stdout, line).await;
                            }
                        }
                    }
                }
                line = stderr_lines.next_line(), if !stderr_eof => {
                    match line {
                        Err(_error) => stderr_eof = true,
                        Ok(None) => stderr_eof = true,
                        Ok(Some(line)) => {
                            log_file.try_write("stderr: ").await;
                            log_file.try_write_line(&line).await;
                            if let Some(output_hook) = &mut output_hook {
                                output_hook(OutputStream::Stderr, line).await;
                            }
                        }
                    }
                }
                exit_status = child.wait() => {
                    let exit_status = exit_status.expect("failed to get engine child process exit status");
                    match exit_status.code() {
                        Some(exit_code) => {
                            eprintln!("engine child process exited with exit code {}", exit_code);
                            log_file.try_write("process exited with exit code ").await;
                            log_file.try_write_line(&format!("{}", exit_code)).await;
                        }
                        None => {
                            eprintln!("engine child process exited with unknown exit code");
                            log_file.try_write_line("process exited with unknown exit code").await;
                        }
                    }

                    log_file.try_flush_and_sync().await;
                    // Remove engine instance if it failed while running.
                    dcx.engine_instances.write().retain(|engine_instance| {
                        !(engine_instance.model_name() == model_name && *engine_instance.engine_state.read() == EngineState::Running)
                    });
                    return exit_status;
                }
            }
        }
    });

    *engine_instance.running_engine.write() = Some(RunningEngine {
        kill_signal_tx: running_engine_kill_signal_tx,
        output_hook_tx: running_engine_output_hook_tx,
        task: running_engine_task,
    });
    *engine_instance.engine_state.write() = EngineState::Running;
}

pub(crate) async fn unload(
    dcx: Arc<DaemonCtxt>,
    mut daemon_socket_stream: AsyncDaemonSocketStream,
    model_name: String,
) {
    eprintln!("requested to unload model `{}`", model_name);

    let model_engine_instance = 'model_engine_instance: {
        let mut engine_instances_write_guard = dcx.engine_instances.write();
        let Some(model_engine_instance_idx) = engine_instances_write_guard.iter()
            .position(|engine_instance| engine_instance.model_name() == model_name)
        else { break 'model_engine_instance None; };
        Some(engine_instances_write_guard.remove(model_engine_instance_idx))
    };

    let Some(model_engine_instance) = model_engine_instance else {
        eprintln!("model `{}` not loaded", model_name);
        daemon_socket_stream.try_send(&ipc::UnloadProgress::BadRequest(ipc::UnloadRequestError::NotLoadedModel)).await;
        return;
    };

    let Some(running_engine) = model_engine_instance.running_engine.write().take() else {
        eprintln!("model `{}` loading", model_name);
        daemon_socket_stream.try_send(&ipc::UnloadProgress::BadRequest(ipc::UnloadRequestError::LoadingModel)).await;
        return;
    };
    *model_engine_instance.engine_state.write() = EngineState::Stopping;

    // HACK: Setting up an async output hook and sharing the daemon socket stream to it
    //       requires lots of indirection.
    let shared_daemon_socket_stream = Arc::new(tokio::sync::Mutex::new(daemon_socket_stream));
    let passed_daemon_socket_stream = Arc::clone(&shared_daemon_socket_stream);
    let output_hook: OutputHook = Box::new(move |output_stream, line| {
        let daemon_socket_stream = Arc::clone(&passed_daemon_socket_stream);
        Box::pin(async move {
            match output_stream {
                OutputStream::Stdout => {
                    println!("stdout: {}", line);
                    daemon_socket_stream.lock().await.try_send(&ipc::UnloadProgress::StdoutLine { line }).await;
                }
                OutputStream::Stderr => {
                    eprintln!("stderr: {}", line);
                    daemon_socket_stream.lock().await.try_send(&ipc::UnloadProgress::StderrLine { line }).await;
                }
            }
        })
    });
    if let Err(_) = running_engine.output_hook_tx.send(Some(output_hook)).await {
        eprintln!("cannot capture output of engine child process: hook channel closed");
    }

    if let Err(_) = running_engine.kill_signal_tx.send(()) {
        eprintln!("cannot send kill signal to engine child process: control channel closed");
    }

    let _exit_status = match running_engine.task.await {
        Ok(exit_status) => Some(exit_status),
        Err(error) => {
            if error.is_panic() {
                eprintln!("internal error: task monitoring engine child process panicked: {}", error);
            }
            None
        }
    };

    // NOTE: Tear down sharing wrappers to get back exclusive ownership over the daemon socket stream.
    daemon_socket_stream = Arc::into_inner(shared_daemon_socket_stream).unwrap().into_inner();

    daemon_socket_stream.try_send(&ipc::UnloadProgress::Completion(ipc::Completion::Success(()))).await;
}

pub(crate) async fn get_models(
    dcx: Arc<DaemonCtxt>,
    mut daemon_socket_stream: AsyncDaemonSocketStream,
) {
    eprintln!("requested models");

    let models = dcx.loaded_config.read().models.iter()
        .map(|model_config| {
            let model_engine_instance = dcx.model_engine_instance(&model_config.name);

            let model_state = match model_engine_instance.map(|engine_instance| *engine_instance.engine_state.read()) {
                None => ipc::ModelState::NotLoaded,
                Some(EngineState::Starting) => ipc::ModelState::Loading,
                Some(EngineState::Running) => ipc::ModelState::Loaded,
                Some(EngineState::Stopping) => ipc::ModelState::Unloading,
            };

            ipc::Model {
                name: model_config.name.clone(),
                max_context_tokens: model_config.max_context_tokens,
                model_state,
            }
        })
        .collect::<Vec<_>>();

    daemon_socket_stream.try_send(&ipc::GetModelsResponse { models }).await;
}

pub(crate) async fn reload_config(
    dcx: Arc<DaemonCtxt>,
    mut daemon_socket_stream: AsyncDaemonSocketStream,
) {
    let config_file_path = &dcx.config_file_path;
    eprintln!("requested to reload config file `{}`", config_file_path.display());

    let config_file_canonical_path = match fs::canonicalize(config_file_path).await {
        Ok(path) => path,
        Err(error) => {
            eprintln!("io error: failed to canonicalize path `{}`: {}", config_file_path.display(), error);
            let ipc_error = ipc::ReloadConfigError::Io { inner_error: format!("{}", error) };
            daemon_socket_stream.try_send(&ipc::ReloadConfigProgress::Completion(ipc::Completion::Failure((config_file_path.to_owned(), ipc_error)))).await;
            return;
        }
    };

    let config_file = match fs::read_to_string(config_file_path).await {
        Ok(file) => file,
        Err(error) => {
            eprintln!("io error: cannot read config file `{}`: {}", config_file_path.display(), error);
            let ipc_error = ipc::ReloadConfigError::Io { inner_error: format!("{}", error) };
            daemon_socket_stream.try_send(&ipc::ReloadConfigProgress::Completion(ipc::Completion::Failure((config_file_canonical_path, ipc_error)))).await;
            return;
        }
    };
    let mut new_config = match toml::from_str::<Config>(&config_file) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("io error: cannot parse config file `{}`: {}", config_file_path.display(), error);
            let ipc_error = ipc::ReloadConfigError::Parsing { inner_error: format!("{}", error) };
            daemon_socket_stream.try_send(&ipc::ReloadConfigProgress::Completion(ipc::Completion::Failure((config_file_canonical_path, ipc_error)))).await;
            return;
        }
    };

    // NOTE: Unfortunately, manually dropping the write guard before the await point generates a coroutine that is too broad,
    //       and therefore is deduced to be `!Send`. We can place the write guard in a block to circumvent this.
    //       See https://github.com/rust-lang/rust/issues/69663.
    let warnings = {
        let mut warnings = Vec::new();
        let mut loaded_config_write_guard = dcx.loaded_config.write();

        if new_config.daemon_port != loaded_config_write_guard.daemon_port {
            eprintln!("warning: new config attempts to change the daemon port: change will take effect upon next startup");
            warnings.push(ipc::ConfigWarning::PortChangeRequiresRestart);
            new_config.daemon_port = loaded_config_write_guard.daemon_port;
        }

        *loaded_config_write_guard = new_config;
        warnings
    };

    eprintln!("reloaded config");
    daemon_socket_stream.try_send(&ipc::ReloadConfigProgress::Completion(ipc::Completion::Success((config_file_canonical_path, warnings)))).await;
}

pub(crate) async fn get_config(
    dcx: Arc<DaemonCtxt>,
    mut daemon_socket_stream: AsyncDaemonSocketStream,
) {
    eprintln!("requested loaded config");

    let loaded_config = dcx.loaded_config.read().clone();

    daemon_socket_stream.try_send(&ipc::GetConfigResponse { config: loaded_config }).await;
}
