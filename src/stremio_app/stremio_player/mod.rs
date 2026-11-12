pub mod communication;
pub mod macos_gl;
pub mod player;
pub mod subtitle_worker;
pub mod video_ready;

pub use communication::{
    CmdVal, InMsg, InMsgArgs, InMsgFn, PlayerEnded, PlayerEvent, PlayerProprChange, PlayerResponse,
    PropKey, PropVal,
};
pub use player::Player;

#[cfg(test)]
mod communication_tests;
