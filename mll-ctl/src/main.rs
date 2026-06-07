use std::process;

use interprocess::local_socket::prelude::*;
use mll_core::ipc::{DAEMON_SOCKET_PATH, DaemonSocketStream};

mod ops;

fn main() {
    let matches = clap::command!()
        .subcommand_required(true)
        .arg_required_else_help(true)
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
        .subcommand(clap::Command::new("load")
            .about("Load the specified model.")
            .arg(clap::arg!(<MODEL> "Name of the model to load, as specified in mll.toml."))
        )
        .subcommand(clap::Command::new("unload")
            .about("Unload the specified model.")
            .arg(clap::arg!(<MODEL> "Name of the model to unload, as specified in mll.toml."))
        )
        .arg(clap::arg!(-h --help "Print help information: this message or the help of the given subcommand.").action(clap::ArgAction::Help).global(true))
        .arg(clap::arg!(-V --version "Print version information.").action(clap::ArgAction::Version))
        .get_matches();

    let daemon_socket_name = DAEMON_SOCKET_PATH.to_fs_name::<interprocess::local_socket::GenericFilePath>().expect("unsupported daemon socket name");

    let daemon_socket_stream = match LocalSocketStream::connect(daemon_socket_name) {
        Ok(stream) => DaemonSocketStream::new(stream),
        Err(error) => {
            eprintln!("cannot connect to daemon socket `{}`: {}", DAEMON_SOCKET_PATH, error);
            eprintln!("ensure that the mlld daemon is running");
            process::exit(101);
        }
    };

    match matches.subcommand() {
        Some(("load", matches)) => {
            let model_name = matches.get_one::<String>("MODEL").unwrap();
            ops::load(daemon_socket_stream, model_name);
        }
        Some(("unload", matches)) => {
            let model_name = matches.get_one::<String>("MODEL").unwrap();
            ops::unload(daemon_socket_stream, model_name);
        }
        _ => unreachable!("invalid subcommand"),
    }
}
