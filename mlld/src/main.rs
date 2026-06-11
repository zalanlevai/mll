use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use interprocess::local_socket::tokio::prelude::*;
use mll_core::config::{self, DEFAULT_CONFIG_FILE_PATH, Config};
use mll_core::ipc::{self, DAEMON_SOCKET_PATH, AsyncDaemonSocketStream, DaemonSocketReadError};
use tokio::net::TcpListener;

mod ctxt;
mod engine;
mod gateway;
mod ops;

use crate::ctxt::DaemonCtxt;

async fn handle_ipc_control_request(dcx: Arc<DaemonCtxt>, mut daemon_socket_stream: AsyncDaemonSocketStream) {
    let control_msg = match daemon_socket_stream.recv::<ipc::ControlMessage>().await {
        Ok(msg) => msg,
        Err(DaemonSocketReadError::Eof) => { return; }
        Err(DaemonSocketReadError::Io(error)) => {
            eprintln!("socket stream error: failed to read incoming message: {}", error);
            return;
        }
        Err(DaemonSocketReadError::Parsing(error, data)) => {
            eprintln!("socket stream error: received unexpected message `{}`: {}", data, error);
            return;
        }
    };

    match control_msg {
        ipc::ControlMessage::Load { model_name } => ops::load(dcx, daemon_socket_stream, model_name).await,
        ipc::ControlMessage::Unload { model_name, force } => ops::unload(dcx, daemon_socket_stream, model_name, force).await,
        ipc::ControlMessage::GetModels => ops::get_models(dcx, daemon_socket_stream).await,
        ipc::ControlMessage::ReloadConfig => ops::reload_config(dcx, daemon_socket_stream).await,
        ipc::ControlMessage::GetConfig => ops::get_config(dcx, daemon_socket_stream).await,
    }
}

#[tokio::main]
async fn main() {
    let matches = clap::command!()
        .disable_help_flag(true)
        .disable_version_flag(true)
        .styles({
            use clap::builder::styling::*;
            Styles::styled()
                .header(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightGreen))).bold())
                .usage(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightGreen))).bold())
                .literal(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlue))).bold())
                .placeholder(Style::new().fg_color(Some(Color::Ansi(AnsiColor::BrightBlue))))
        })
        .arg(clap::arg!(--"config-file" [CONFIG_FILE_PATH] "Read configuration options from the specified file.").env("MLL_CONFIG_FILE_PATH").value_parser(clap::value_parser!(PathBuf)).default_value(DEFAULT_CONFIG_FILE_PATH))
        .arg(clap::arg!(-h --help "Print help information: this message or the help of the given subcommand.").action(clap::ArgAction::Help).global(true))
        .arg(clap::arg!(-V --version "Print version information.").action(clap::ArgAction::Version))
        .get_matches();

    let config_file_path = matches.get_one::<PathBuf>("config-file").unwrap().clone();
    let config_file = match fs::read_to_string(&config_file_path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("cannot read config file `{}`: {}", config_file_path.display(), error);
            process::exit(1);
        }
    };
    let config = match toml::from_str::<Config>(&config_file) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("cannot parse config file `{}`: {}", config_file_path.display(), error);
            process::exit(1);
        }
    };

    let daemon_socket_parent_dir_path = Path::new(DAEMON_SOCKET_PATH).parent().expect("invalid daemon socket path");
    if let Err(error) = fs::create_dir_all(daemon_socket_parent_dir_path) {
        eprintln!("cannot create daemon directory `{}`: {}", daemon_socket_parent_dir_path.display(), error);
        process::exit(1);
    }

    let daemon_socket_name = DAEMON_SOCKET_PATH.to_fs_name::<interprocess::local_socket::GenericFilePath>().expect("unsupported daemon socket name");

    let daemon_socket_listener = match interprocess::local_socket::ListenerOptions::new().name(daemon_socket_name).try_overwrite(true).create_tokio() {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            eprintln!("cannot start daemon: socket `{}` already in use", DAEMON_SOCKET_PATH);
            process::exit(-1);
        }
        Err(error) => {
            eprintln!("cannot start daemon: cannot access socket `{}`: {}", DAEMON_SOCKET_PATH, error);
            process::exit(-1);
        }
    };

    let daemon_port = config.daemon_port;
    let tcp_listener = match TcpListener::bind(("0.0.0.0", daemon_port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("cannot bind port {}: {}", daemon_port, error);
            process::exit(1);
        }
    };

    eprintln!("daemon started: listening on socket `{}`", DAEMON_SOCKET_PATH);

    let dcx = Arc::new(DaemonCtxt::new(config_file_path, config));

    let ipc_dcx = Arc::clone(&dcx);
    tokio::spawn(async move {
        loop {
            let daemon_socket_stream = match daemon_socket_listener.accept().await {
                Ok(stream) => stream,
                Err(error) => {
                    eprintln!("error accepting incoming daemon socket connection: {}", error);
                    continue;
                }
            };

            let dcx = Arc::clone(&ipc_dcx);
            tokio::spawn(async move {
                let daemon_socket_stream = AsyncDaemonSocketStream::new(daemon_socket_stream);
                handle_ipc_control_request(dcx, daemon_socket_stream).await;
            });
        }
    });

    eprintln!("serving gateway on 0.0.0.0:{}", daemon_port);
    let router = gateway::setup_routes(dcx);
    axum::serve(tcp_listener, router).await.unwrap();
}
