use std::process;

use interprocess::local_socket::prelude::*;
use mll_core::ipc::{self, DAEMON_SOCKET_PATH, DaemonSocketStream};

mod display;
mod launch;
mod ops;

use crate::ops::{ListModelsOpts, ModelListFilter};

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
            .arg(clap::arg!(--force "Forcefully unload models with pending requests."))
        )
        .subcommand(clap::Command::new("list")
            .about("List configured models.")
            .arg(clap::arg!([CATEGORY] "List only the specified models.").value_parser(["all", "loaded", "active", "inactive"]).default_value("all"))
            .arg(clap::arg!(--activity "Show information about model requests."))
        )
        .subcommand(clap::Command::new("reload-config")
            .about("Reload configuration options from the configuration file.")
        )
        .subcommand(clap::Command::new("launch")
            .about("Launch the specified tool configured with mll-managed models.")
            .arg(clap::arg!(<TOOL>).value_parser(["opencode"]))
            .arg(clap::Arg::new("PASSED_ARGS").num_args(..).trailing_var_arg(true).allow_hyphen_values(true))
        )
        .arg(clap::arg!(-h --help "Print help information: this message or the help of the given subcommand.").action(clap::ArgAction::Help).global(true))
        .arg(clap::arg!(-V --version "Print version information.").action(clap::ArgAction::Version))
        .get_matches();

    let daemon_socket_name = DAEMON_SOCKET_PATH.to_fs_name::<interprocess::local_socket::GenericFilePath>().expect("unsupported daemon socket name");

    let mut daemon_socket_stream = match LocalSocketStream::connect(daemon_socket_name) {
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
            let force = matches.get_flag("force");
            ops::unload(daemon_socket_stream, model_name, force);
        }
        Some(("list", matches)) => {
            let filters = match matches.get_one::<String>("CATEGORY").unwrap().as_ref() {
                "all" => vec![],
                "loaded" => vec![ModelListFilter::Loaded],
                "active" => vec![ModelListFilter::Active],
                "inactive" => vec![ModelListFilter::Inactive],
                _ => unreachable!("invalid category argument"),
            };
            let show_activity = matches.get_flag("activity");
            let opts = ListModelsOpts { filters, show_activity };
            ops::list_models(daemon_socket_stream, opts);
        }
        Some(("reload-config", matches)) => {
            ops::reload_config(daemon_socket_stream);
        }
        Some(("launch", matches)) => {
            let passed_args = matches.get_many::<String>("PASSED_ARGS").unwrap_or_default().map(ToOwned::to_owned).collect::<Vec<_>>();

            daemon_socket_stream.must_send(&ipc::ControlMessage::GetConfig);
            let config_response = daemon_socket_stream.must_recv::<ipc::GetConfigResponse>();

            match matches.get_one::<String>("TOOL").unwrap().as_ref() {
                "opencode" => launch::opencode(config_response.config, passed_args),
                _ => unreachable!("invalid tool argument"),
            }
        }
        _ => unreachable!("invalid subcommand"),
    }
}
