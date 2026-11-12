use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use flume::Sender;

use crate::stremio_app::{ipc::RPCResponse, stremio_player::PlayerResponse};

#[derive(Debug, Default)]
pub struct VideoReadyState {
    next_load_id: u64,
    current_load_id: Option<u64>,
    pending_load_ids: VecDeque<u64>,
    active_load_id: Option<u64>,
    file_loaded_id: Option<u64>,
    ready_sent_id: Option<u64>,
}

impl VideoReadyState {
    pub fn begin_transition(&mut self, loads_file: bool) -> u64 {
        self.next_load_id += 1;
        self.current_load_id = Some(self.next_load_id);
        self.file_loaded_id = None;
        self.ready_sent_id = None;
        if loads_file {
            self.pending_load_ids.push_back(self.next_load_id);
        }
        self.next_load_id
    }

    pub fn start_file(&mut self) {
        self.active_load_id = self.pending_load_ids.pop_front();
        self.file_loaded_id = None;
    }

    pub fn file_loaded(&mut self) {
        if self.active_load_id == self.current_load_id {
            self.file_loaded_id = self.active_load_id;
        }
    }

    pub fn playback_restarted(&mut self) -> Option<u64> {
        let load_id = self.current_load_id?;
        if self.file_loaded_id != Some(load_id) || self.ready_sent_id == Some(load_id) {
            return None;
        }
        self.ready_sent_id = Some(load_id);
        Some(load_id)
    }
}

pub type SharedVideoReadyState = Arc<Mutex<VideoReadyState>>;

pub fn video_ready_response(load_id: u64, ready: bool) -> String {
    RPCResponse::response_message(PlayerResponse::video_ready(load_id, ready).to_value())
}

pub fn send_video_ready(rpc_response_sender: &Sender<String>, load_id: u64, ready: bool) {
    if let Err(error) = rpc_response_sender.send(video_ready_response(load_id, ready)) {
        eprintln!("failed to send video readiness: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::VideoReadyState;

    #[test]
    fn ready_follows_file_loaded_restart_once() {
        let mut state = VideoReadyState::default();
        let load_id = state.begin_transition(true);
        state.start_file();
        assert_eq!(state.playback_restarted(), None);
        state.file_loaded();
        assert_eq!(state.playback_restarted(), Some(load_id));
        assert_eq!(state.playback_restarted(), None);
    }

    #[test]
    fn new_transition_ignores_previous_file_events() {
        let mut state = VideoReadyState::default();
        state.begin_transition(true);
        state.start_file();

        let current_load = state.begin_transition(true);
        state.file_loaded();
        assert_eq!(state.playback_restarted(), None);

        state.start_file();
        state.file_loaded();
        assert_eq!(state.playback_restarted(), Some(current_load));
    }

    #[test]
    fn stop_never_becomes_ready() {
        let mut state = VideoReadyState::default();
        state.begin_transition(false);
        state.file_loaded();
        assert_eq!(state.playback_restarted(), None);
    }
}
