use std::{
    collections::VecDeque,
    convert::TryFrom,
    ffi::CString,
    os::raw::c_char,
    thread::{self, JoinHandle},
};

use flume::{Receiver, Sender};
use libmpv2::Mpv;

use crate::stremio_app::stremio_player::CmdVal;

#[derive(Debug, Eq, PartialEq)]
pub struct SubtitleCommand {
    pub name: &'static str,
    pub args: Vec<String>,
}

impl TryFrom<CmdVal> for SubtitleCommand {
    type Error = CmdVal;

    fn try_from(command: CmdVal) -> Result<Self, Self::Error> {
        match command {
            CmdVal::SubAdd(url, title, language) => Ok(Self {
                name: "sub-add",
                args: vec![
                    "sub-add".to_string(),
                    url,
                    "auto".to_string(),
                    title,
                    language,
                ],
            }),
            CmdVal::SubRemove(track_id) => Ok(Self {
                name: "sub-remove",
                args: vec!["sub-remove".to_string(), track_id],
            }),
            command => Err(command),
        }
    }
}

pub enum SubtitleWorkerRequest {
    Command(SubtitleCommand),
    BeginMediaChange(Sender<()>),
    EndMediaChange,
}

struct InFlightSubtitle {
    reply_id: u64,
    name: &'static str,
}

enum SubtitleWorkerEvent {
    Request(Result<SubtitleWorkerRequest, flume::RecvError>),
    Wake(Result<(), flume::RecvError>),
}

fn start_next_subtitle(
    mpv: &Mpv,
    queue: &mut VecDeque<SubtitleCommand>,
    in_flight: &mut Option<InFlightSubtitle>,
    next_reply_id: &mut u64,
    media_change: bool,
) {
    while !media_change && in_flight.is_none() {
        let Some(command) = queue.pop_front() else {
            return;
        };
        let args = match command
            .args
            .iter()
            .map(|argument| CString::new(argument.as_str()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(args) => args,
            Err(_) => {
                eprintln!("mpv: invalid {} argument", command.name);
                continue;
            }
        };
        let mut arg_ptrs = args
            .iter()
            .map(|arg| arg.as_ptr())
            .chain(std::iter::once(std::ptr::null::<c_char>()))
            .collect::<Vec<_>>();
        let reply_id = *next_reply_id;
        *next_reply_id = (*next_reply_id).wrapping_add(1).max(1);
        let result = unsafe {
            libmpv2_sys::mpv_command_async(mpv.ctx.as_ptr(), reply_id, arg_ptrs.as_mut_ptr())
        };
        if result < 0 {
            eprintln!(
                "mpv: failed to queue {}: {}",
                command.name,
                libmpv2_sys::mpv_error_str(result)
            );
            continue;
        }
        *in_flight = Some(InFlightSubtitle {
            reply_id,
            name: command.name,
        });
    }
}

pub fn create_subtitle_thread(
    mut mpv: Mpv,
    request_receiver: Receiver<SubtitleWorkerRequest>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("subtitle-loader".to_string())
        .spawn(move || {
            let (wake_sender, wake_receiver) = flume::bounded(1);
            mpv.set_wakeup_callback(move || {
                let _ = wake_sender.try_send(());
            });

            let mut queue = VecDeque::new();
            let mut in_flight: Option<InFlightSubtitle> = None;
            let mut next_reply_id = 1;
            let mut media_change = false;
            let mut media_change_ack: Option<Sender<()>> = None;

            loop {
                let event = flume::Selector::new()
                    .recv(&request_receiver, SubtitleWorkerEvent::Request)
                    .recv(&wake_receiver, SubtitleWorkerEvent::Wake)
                    .wait();

                match event {
                    SubtitleWorkerEvent::Request(Ok(SubtitleWorkerRequest::Command(command))) => {
                        queue.push_back(command);
                    }
                    SubtitleWorkerEvent::Request(Ok(SubtitleWorkerRequest::BeginMediaChange(
                        acknowledge,
                    ))) => {
                        media_change = true;
                        queue.clear();
                        if let Some(command) = &in_flight {
                            media_change_ack = Some(acknowledge);
                            unsafe {
                                libmpv2_sys::mpv_abort_async_command(
                                    mpv.ctx.as_ptr(),
                                    command.reply_id,
                                );
                            }
                        } else {
                            let _ = acknowledge.send(());
                        }
                    }
                    SubtitleWorkerEvent::Request(Ok(SubtitleWorkerRequest::EndMediaChange)) => {
                        media_change = false;
                    }
                    SubtitleWorkerEvent::Request(Err(_)) | SubtitleWorkerEvent::Wake(Err(_)) => {
                        break;
                    }
                    SubtitleWorkerEvent::Wake(Ok(())) => loop {
                        let event = unsafe { &*libmpv2_sys::mpv_wait_event(mpv.ctx.as_ptr(), 0.0) };
                        match event.event_id {
                            libmpv2_sys::mpv_event_id_MPV_EVENT_NONE => break,
                            libmpv2_sys::mpv_event_id_MPV_EVENT_SHUTDOWN => return,
                            libmpv2_sys::mpv_event_id_MPV_EVENT_COMMAND_REPLY => {
                                let Some(command) = &in_flight else {
                                    continue;
                                };
                                if command.reply_id != event.reply_userdata {
                                    continue;
                                }
                                let command = in_flight.take().expect("missing subtitle command");
                                if event.error < 0 {
                                    eprintln!(
                                        "mpv: {} failed: {}",
                                        command.name,
                                        libmpv2_sys::mpv_error_str(event.error)
                                    );
                                }
                                if let Some(acknowledge) = media_change_ack.take() {
                                    let _ = acknowledge.send(());
                                }
                            }
                            _ => {}
                        }
                    },
                }

                start_next_subtitle(
                    &mpv,
                    &mut queue,
                    &mut in_flight,
                    &mut next_reply_id,
                    media_change,
                );
            }
        })
        .expect("failed to start subtitle worker")
}
