pub mod macos;
pub use macos::run_macos;

pub mod constants;
pub mod discord;
pub mod enhanced;
pub mod external_player;
pub mod gpu_video_processing;
pub mod ipc;
pub mod named_pipe;
pub mod open_media;
pub mod stremio_player;
pub mod stremio_server;
pub mod window_settings;

pub use ipc::RPCResponse;
pub use named_pipe::{acquire_single_instance, PipeServer, SingleInstance};
