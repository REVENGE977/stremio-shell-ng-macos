#![allow(unexpected_cfgs)]

use std::{path::Path, process::exit};
use url::Url;
use whoami::username;

use clap::Parser;

mod stremio_app;
use crate::stremio_app::{
    acquire_single_instance,
    constants::{
        IPC_ATTEMPTS, IPC_PATH, IPC_RETRY_DELAY, SERVER_IPC_KEY, STREMIO_SERVER_DEV_MODE,
        WEB_ENDPOINT,
    },
    SingleInstance,
};

#[derive(Parser, Debug, Clone)]
#[clap(version)]
pub struct Opt {
    pub command: Option<String>,
    #[clap(
        long,
        help = "Start the app only in system tray and keep the window hidden"
    )]
    pub start_hidden: bool,
    #[clap(long, help = "Do not show the splash image")]
    pub no_splash: bool,
    #[clap(
        long,
        help = "Deprecated: dev tools are always available from the context menu"
    )]
    pub dev_tools: bool,
    #[clap(long, help = "Disable the server and load the WebUI from localhost")]
    pub development: bool,
    #[clap(long, help = "Shortcut for --webui-url=https://staging.strem.io/")]
    pub staging: bool,
    #[clap(long, default_value = WEB_ENDPOINT, help = "Override the WebUI URL")]
    pub webui_url: String,
    #[clap(long, help = "Override autoupdater endpoint")]
    pub autoupdater_endpoint: Option<Url>,
    #[clap(long, help = "Forces reinstalling current version")]
    pub force_update: bool,
    #[clap(long, help = "Check for RC updates")]
    pub release_candidate: bool,
    #[clap(
        long,
        default_value = "",
        help = "Secret key for communication with the server. By default it is randomly generated on startup"
    )]
    pub server_ipc_key: String,
}

fn main() {
    // Ensure streaming server and enhanced directories exist at startup
    let _ = stremio_app::stremio_server::server_dir();

    let opt = Opt::parse();

    std::env::set_var(
        SERVER_IPC_KEY,
        if opt.server_ipc_key.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            opt.server_ipc_key.clone()
        },
    );

    let command = opt
        .command
        .clone()
        .filter(|command| !command.is_empty())
        .map(stremio_app::open_media::normalize)
        .unwrap_or_default();

    // Single application IPC over Unix domain socket
    let mut commands_path = IPC_PATH.to_string();
    commands_path.push_str(&username());
    let socket_path = Path::new(&commands_path);
    let commands_server = match acquire_single_instance(
        socket_path,
        command.as_bytes(),
        IPC_ATTEMPTS,
        IPC_RETRY_DELAY,
    ) {
        SingleInstance::Forwarded => exit(0),
        SingleInstance::Primary(server) => Some(server),
        SingleInstance::Unavailable(error) => {
            eprintln!("Single application IPC is unavailable; launching without it: {error}");
            None
        }
    };

    std::env::set_var(
        STREMIO_SERVER_DEV_MODE,
        if opt.development { "true" } else { "false" },
    );

    if let Err(err) = crate::stremio_app::run_macos(opt, commands_server) {
        eprintln!("macOS runtime error: {err}");
        exit(1);
    }
}
