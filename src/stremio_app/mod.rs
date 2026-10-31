#[cfg(windows)]
pub mod app;
#[cfg(windows)]
pub use app::MainWindow;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::run_macos;

pub mod discord;
pub mod external_player;
pub mod gpu_video_processing;
pub mod ipc;
pub mod stremio_player;
pub mod stremio_server;

#[cfg(windows)]
pub mod stremio_wevbiew;

pub use ipc::RPCResponse;
pub mod named_pipe;
pub mod open_media;

#[cfg(windows)]
pub mod splash;
#[cfg(windows)]
pub mod systray;
#[cfg(windows)]
pub mod window_helper;

pub mod window_settings;
pub use named_pipe::{acquire_single_instance, PipeServer, SingleInstance};
pub mod constants;
pub mod updater;
pub mod enhanced;
