use std::collections::HashMap;
use std::process::{self, Command};

use mll_core::config::Config;
use serde::Serialize;

#[derive(Serialize)]
struct OpencodeModelLimits {
    context: usize,
    output: usize,
}

#[derive(Serialize)]
struct OpencodeModel {
    name: String,
    limit: Option<OpencodeModelLimits>,
}

#[derive(Serialize)]
struct OpencodeProviderOptions {
    #[serde(rename = "baseURL")]
    base_uri: String,
}

#[derive(Serialize)]
struct OpencodeProvider {
    npm: String,
    name: String,
    options: OpencodeProviderOptions,
    models: HashMap<String, OpencodeModel>,
}

#[derive(Serialize)]
struct OpencodeConfig {
    #[serde(rename = "$schema")]
    schema: &'static str,
    provider: HashMap<String, OpencodeProvider>,
}

pub fn opencode(config: Config, passed_args: Vec<String>) -> ! {
    let opencode_models = config.models.iter()
        .map(|model_config| {
            let opencode_model = OpencodeModel {
                name: model_config.name.clone(),
                limit: Some(OpencodeModelLimits {
                    context: model_config.max_context_tokens,
                    output: 65536,
                }),
            };
            (model_config.name.clone(), opencode_model)
        })
        .collect::<HashMap<_, _>>();

    let opencode_mll_provider = OpencodeProvider {
        npm: "@ai-sdk/openai-compatible".to_owned(),
        name: "MLL (Local)".to_owned(),
        options: OpencodeProviderOptions {
            base_uri: format!("http://localhost:{}/v1", config.daemon_port),
        },
        models: opencode_models,
    };

    let opencode_config = OpencodeConfig {
        schema: "https://opencode.ai/config.json",
        provider: [("mll".to_owned(), opencode_mll_provider)].into_iter().collect::<HashMap<_, _>>(),
    };

    let opencode_config_str = match serde_json::to_string(&opencode_config) {
        Ok(config_str) => config_str,
        Err(error) => {
            eprintln!("internal error: cannot serialize opencode config: {}", error);
            process::exit(1);
        }
    };
    eprintln!("setting OPENCODE_CONFIG_CONTENT={}", opencode_config_str);

    let mut cmd = Command::new("opencode");
    cmd.env("OPENCODE_CONFIG_CONTENT", opencode_config_str);
    cmd.args(passed_args);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("cannot spawn tool: {}", error);
            process::exit(1);
        }
    };
    let exit_status = match child.wait() {
        Ok(exit_status) => exit_status,
        Err(error) => {
            eprintln!("cannot wait for tool subprocess: {}", error);
            process::exit(1);
        }
    };
    match exit_status.code() {
        Some(exit_code) => process::exit(exit_code),
        None => {
            eprintln!("tool subprocess terminated by signal");
            process::exit(1);
        }
    }
}
