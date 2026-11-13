use std::{
    ffi::CString,
    os::raw::c_char,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use flume::{Receiver, Sender};
use libmpv2::{events::Event, Format, Mpv, SetData};

use crate::stremio_app::{
    ipc,
    stremio_player::{
        communication::StrProp,
        macos_gl,
        subtitle_worker::{create_subtitle_thread, SubtitleCommand, SubtitleWorkerRequest},
        video_ready::{send_video_ready, SharedVideoReadyState, VideoReadyState},
        CmdVal, InMsg, InMsgArgs, InMsgFn, PlayerEnded, PlayerEvent, PlayerProprChange,
        PlayerResponse, PropKey, PropVal,
    },
    RPCResponse,
};

struct ObserveProperty {
    name: String,
    format: Format,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DisplayOutputMode {
    Auto,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct DisplayOutputState {
    mode: DisplayOutputMode,
}

#[derive(Default)]
pub struct Player {
    pub channel: ipc::Channel,
}

impl Player {
    pub fn init(&mut self, window_handle: i64) {
        let (in_msg_sender, in_msg_receiver) = flume::unbounded();
        let (rpc_response_sender, rpc_response_receiver) = flume::unbounded();
        let (observe_property_sender, observe_property_receiver) = flume::unbounded();
        self.channel = ipc::Channel::new(Some((in_msg_sender, rpc_response_receiver)));
        let video_ready_state = Arc::new(Mutex::new(VideoReadyState::default()));

        let mpv = Arc::new(create_mpv(window_handle));
        let mpv_event_client = mpv
            .create_client(None)
            .expect("cannot create MPV event client");
        let _event_thread = create_event_thread(
            mpv_event_client,
            observe_property_receiver,
            rpc_response_sender.clone(),
            Arc::clone(&video_ready_state),
        );
        let gpu_video_processing = Arc::new(AtomicBool::new(false));
        let _display_thread = create_display_output_thread(
            Arc::clone(&mpv),
            window_handle as isize,
            Arc::clone(&gpu_video_processing),
        );
        let _message_thread = create_message_thread(
            mpv,
            window_handle as isize,
            gpu_video_processing,
            observe_property_sender,
            in_msg_receiver,
            rpc_response_sender,
            video_ready_state,
        );
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        macos_gl::detach_mpv_handle();
    }
}

fn create_mpv(window_handle: i64) -> Mpv {
    // libmpv requires LC_NUMERIC="C" (matching Qt/stremio-shell) so decimal parsing
    // never breaks on non-English macOS system locales.
    unsafe {
        libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
    }

    // Initialize the shared CGLContext on the main thread first so we know
    // whether we are running on a hardware GPU or Apple's Software Renderer
    // before setting libmpv decoder and shader options.
    macos_gl::ensure_cgl_context();
    let is_software_gl = macos_gl::is_software_renderer();

    let mpv = Mpv::with_initializer(|initializer| {
        macro_rules! set_opt {
            ($name:literal, $value:expr) => {
                if let Err(error) = initializer.set_property($name, $value) {
                    eprintln!("mpv: cannot set {}={:?}: {error:?}", $name, $value);
                }
            };
        }

        set_opt!("title", crate::stremio_app::constants::APP_NAME);
        set_opt!(
            "audio-client-name",
            crate::stremio_app::constants::APP_NAME
        );
        set_opt!("terminal", "yes");
        #[cfg(debug_assertions)]
        set_opt!("msg-level", "all=warn,cplayer=info");
        #[cfg(not(debug_assertions))]
        set_opt!("msg-level", "all=no");
        set_opt!("quiet", "yes");

        // Embedded CoreAnimation OpenGL render loop (`vo=libmpv`).
        set_opt!("vo", "libmpv");
        set_opt!(
            "gpu-hwdec-interop",
            if is_software_gl { "no" } else { "auto" }
        );
        set_opt!("audio-fallback-to-null", "yes");
        set_opt!("video-timing-offset", "0");
        set_opt!("video-sync", "audio");
        set_opt!("hr-seek-framedrop", "yes");

        // Network & demuxer streaming resilience (HTTP/HTTPS, HLS, and local EngineFS).
        // `%23%` escapes the 23-byte HTTP status list as one mpv option value.
        set_opt!(
            "stream-lavf-o",
            "reconnect=1,reconnect_streamed=1,reconnect_on_network_error=1,reconnect_on_http_error=%23%408,429,500,502,503,504,reconnect_delay_max=15"
        );
        set_opt!("cache", "yes");
        set_opt!("cache-pause-initial", "yes");
        set_opt!("cache-pause-wait", "0.5");
        set_opt!("demuxer-hysteresis-secs", "10");
        set_opt!("demuxer-max-bytes", if is_software_gl { "157286400" } else { "209715200" });
        set_opt!("demuxer-max-back-bytes", "52428800");
        set_opt!("demuxer-readahead-secs", "20");
        set_opt!("network-timeout", "15");
        set_opt!("force-seekable", "yes");
        set_opt!("hls-bitrate", "max");
        set_opt!("keep-open", "yes");

        let mut options: Vec<(&str, &str)> = Vec::new();
        if is_software_gl {
            // macOS VM without GPU acceleration:
            // Avoid VideoToolbox IOSurface probing and OpenGL direct-rendering PBOs,
            // and keep shaders minimal so software OpenGL does not freeze the main thread.
            set_opt!("hwdec", "no");
            options.extend([
                ("vd-lavc-dr", "no"),
                ("profile", "fast"),
                ("scale", "bilinear"),
                ("cscale", "bilinear"),
                ("dscale", "bilinear"),
                ("deband", "no"),
                ("dither", "no"),
                ("correct-downscaling", "no"),
                ("linear-downscaling", "no"),
                ("sigmoid-upscaling", "no"),
                ("hdr-compute-peak", "no"),
                ("vd-lavc-threads", "0"),
            ]);
        } else {
            // Hardware macOS GPU (Apple Silicon / Intel / AMD):
            // `auto-safe` selects zero-copy VideoToolbox (`CVPixelBuffer` -> `IOSurface`
            // -> `CGLTexImageIOSurface2D`) for hardware-supported streams and falls back
            // to multi-threaded direct-rendered CPU decoding for unsupported profiles (e.g. AV1 on M1/M2).
            set_opt!("hwdec", "auto-safe");
            set_opt!("hwdec-codecs", "all");
            options.extend([
                ("vd-lavc-dr", "auto"),
                ("vd-lavc-threads", "0"),
                ("profile", "fast"),
                ("scale", "bilinear"),
                ("cscale", "bilinear"),
                ("dscale", "bilinear"),
                ("deband", "no"),
                // macOS OpenGL 3.2/4.1 lacks GL 4.3 compute shaders; disable compute peak
                // detection and match the 8-bit CAOpenGLLayer framebuffer for smooth HDR->SDR.
                ("hdr-compute-peak", "no"),
                ("tone-mapping", "bt.2390"),
                ("dither-depth", "8"),
                ("temporal-dither", "no"),
            ]);
        }

        for (name, value) in options {
            if let Err(error) = initializer.set_property(name, value) {
                eprintln!("mpv: cannot set {name}={value}: {error:?}");
            }
        }
        Ok(())
    })
    .expect("cannot build MPV");

    macos_gl::attach_mpv_handle(mpv.ctx.as_ptr(), window_handle);

    mpv
}

fn create_display_output_thread(
    mpv: Arc<Mpv>,
    window_handle: isize,
    gpu_video_processing: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut last_state = None;

        loop {
            let state = current_display_output_state(&mpv, window_handle);
            let gpu = gpu_video_processing.load(Ordering::Relaxed);
            if last_state != Some((state, gpu)) {
                apply_display_output_mode(&mpv, state, gpu);
                last_state = Some((state, gpu));
            }
            thread::sleep(Duration::from_millis(500));
        }
    })
}

fn apply_display_output_mode(mpv: &Mpv, _state: DisplayOutputState, gpu_video_processing: bool) {
    if macos_gl::is_software_renderer() {
        return;
    }

    let options: [(&str, &str); 3] = if gpu_video_processing {
        [
            ("scale", "spline36"),
            ("cscale", "spline36"),
            ("deband", "yes"),
        ]
    } else {
        [
            ("scale", "bilinear"),
            ("cscale", "bilinear"),
            ("deband", "no"),
        ]
    };

    for (name, value) in options {
        if let Err(error) = mpv.set_property(name, value) {
            eprintln!("mpv: cannot set {name}={value}: {error:?}");
        }
    }
}

fn create_event_thread(
    mpv_event_client: Mpv,
    observe_property_receiver: Receiver<ObserveProperty>,
    rpc_response_sender: Sender<String>,
    video_ready_state: SharedVideoReadyState,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = mpv_event_client.disable_deprecated_events() {
            eprintln!("mpv: failed to disable deprecated events: {error:?}");
        }

        loop {
            for ObserveProperty { name, format } in observe_property_receiver.drain() {
                if let Err(error) = mpv_event_client.observe_property(&name, format, 0) {
                    eprintln!("mpv: failed to observe property '{name}': {error:?}");
                }
            }

            let event = match mpv_event_client.wait_event(0.1) {
                Some(Ok(event)) => event,
                Some(Err(error)) => {
                    eprintln!("Event errored: {error:?}");
                    continue;
                }
                None => continue,
            };

            if matches!(&event, Event::StartFile) {
                macos_gl::set_layer_visible(true);
                video_ready_state
                    .lock()
                    .expect("cannot lock video readiness state")
                    .start_file();
                continue;
            }

            if matches!(&event, Event::FileLoaded) {
                video_ready_state
                    .lock()
                    .expect("cannot lock video readiness state")
                    .file_loaded();
                continue;
            }

            if matches!(&event, Event::PlaybackRestart) {
                let paused = mpv_event_client
                    .get_property::<bool>("pause")
                    .unwrap_or(false);
                macos_gl::set_layer_visible(true);
                macos_gl::set_playback_sleep_inhibited(!paused);
                if let Ok(hwdec_active) = mpv_event_client.get_property::<String>("hwdec-current") {
                    println!("[MPV] Playback started (hwdec-current: {hwdec_active})");
                }
                let load_id = video_ready_state
                    .lock()
                    .expect("cannot lock video readiness state")
                    .playback_restarted();
                if let Some(load_id) = load_id {
                    send_video_ready(&rpc_response_sender, load_id, true);
                }
                continue;
            }

            let player_response = match event {
                Event::PropertyChange { name, change, .. } => {
                    if name == "pause" {
                        if let libmpv2::events::PropertyData::Flag(paused) = change {
                            macos_gl::set_playback_sleep_inhibited(!paused);
                        }
                    }
                    PlayerResponse(
                        "mpv-prop-change",
                        PlayerEvent::PropChange(PlayerProprChange::from_name_value(
                            name.to_string(),
                            change,
                        )),
                    )
                }
                Event::EndFile(reason) => {
                    macos_gl::set_playback_sleep_inhibited(false);
                    macos_gl::set_layer_visible(false);
                    PlayerResponse(
                        "mpv-event-ended",
                        PlayerEvent::End(PlayerEnded::from_end_reason(reason)),
                    )
                }
                Event::Shutdown => {
                    macos_gl::set_playback_sleep_inhibited(false);
                    macos_gl::set_layer_visible(false);
                    break;
                }
                _ => continue,
            };

            if let Err(error) =
                rpc_response_sender.send(RPCResponse::response_message(player_response.to_value()))
            {
                eprintln!("failed to send RPCResponse: {error}");
                break;
            }
        }
    })
}

fn run_mpv_command_async(mpv: &Mpv, name: &str, args: &[&str]) -> Result<(), i32> {
    let mut c_strings = Vec::with_capacity(args.len() + 1);
    let Ok(cmd_cstr) = CString::new(name) else {
        return Err(-1);
    };
    c_strings.push(cmd_cstr);
    for arg in args {
        let Ok(arg_cstr) = CString::new(*arg) else {
            return Err(-1);
        };
        c_strings.push(arg_cstr);
    }
    let mut ptrs: Vec<*const c_char> = c_strings.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(std::ptr::null());

    let rc = unsafe { libmpv2_sys::mpv_command_async(mpv.ctx.as_ptr(), 0, ptrs.as_mut_ptr()) };
    if rc < 0 {
        Err(rc)
    } else {
        Ok(())
    }
}

fn set_mpv_property(name: &str, value: impl SetData, mpv: &Mpv) {
    if let Err(error) = mpv.set_property(name, value) {
        eprintln!("cannot set MPV property '{name}': '{error:#}'");
    }
}

fn create_message_thread(
    mpv: Arc<Mpv>,
    window_handle: isize,
    gpu_video_processing: Arc<AtomicBool>,
    observe_property_sender: Sender<ObserveProperty>,
    in_msg_receiver: Receiver<String>,
    rpc_response_sender: Sender<String>,
    video_ready_state: SharedVideoReadyState,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let subtitle_mpv = mpv
            .create_client(None)
            .expect("cannot create MPV subtitle client");
        let (subtitle_request_sender, subtitle_request_receiver) = flume::unbounded();
        let _subtitle_thread = create_subtitle_thread(subtitle_mpv, subtitle_request_receiver);

        let observe_property = |name: String, format: Format| {
            if let Err(error) = observe_property_sender.send(ObserveProperty { name, format }) {
                eprintln!("cannot send ObserveProperty: {error}");
            }
        };

        let send_command = |cmd: CmdVal| {
            let cmd = match SubtitleCommand::try_from(cmd) {
                Ok(command) => {
                    if subtitle_request_sender
                        .send(SubtitleWorkerRequest::Command(command))
                        .is_err()
                    {
                        eprintln!("mpv: subtitle worker stopped");
                    }
                    return;
                }
                Err(command) => command,
            };
            let media_transition = cmd.media_transition();
            if media_transition.is_some() {
                let (acknowledge, acknowledged) = flume::bounded(1);
                if subtitle_request_sender
                    .send(SubtitleWorkerRequest::BeginMediaChange(acknowledge))
                    .is_ok()
                {
                    let _ = acknowledged.recv_timeout(Duration::from_millis(250));
                }
            }
            let parts: Vec<String> = cmd.into();
            if let Some((name, args)) = parts.split_first() {
                if let Some(loads_file) = media_transition {
                    let load_id = video_ready_state
                        .lock()
                        .expect("cannot lock video readiness state")
                        .begin_transition(loads_file);
                    send_video_ready(&rpc_response_sender, load_id, false);
                }
                let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                if let Err(rc) = run_mpv_command_async(&mpv, name, &args) {
                    eprintln!(
                        "failed to execute async MPV command '{name}' (rc={rc}), falling back to sync"
                    );
                    if let Err(error) = mpv.command(name, &args) {
                        eprintln!("failed to execute MPV command: '{error:#}'");
                    }
                }
            }
            if media_transition.is_some() {
                let _ = subtitle_request_sender.send(SubtitleWorkerRequest::EndMediaChange);
            }
        };

        for msg in in_msg_receiver.iter() {
            let in_msg: InMsg = match serde_json::from_str(&msg) {
                Ok(in_msg) => in_msg,
                Err(error) => {
                    eprintln!("cannot parse InMsg:{:?} {error:#}", msg);
                    continue;
                }
            };

            match in_msg {
                InMsg(InMsgFn::MpvObserveProp, InMsgArgs::ObProp(PropKey::Bool(prop))) => {
                    observe_property(prop.to_string(), Format::Flag);
                }
                InMsg(InMsgFn::MpvObserveProp, InMsgArgs::ObProp(PropKey::Int(prop))) => {
                    observe_property(prop.to_string(), Format::Int64);
                }
                InMsg(InMsgFn::MpvObserveProp, InMsgArgs::ObProp(PropKey::Fp(prop))) => {
                    observe_property(prop.to_string(), Format::Double);
                }
                InMsg(InMsgFn::MpvObserveProp, InMsgArgs::ObProp(PropKey::Str(prop))) => {
                    observe_property(prop.to_string(), Format::String);
                }
                InMsg(InMsgFn::MpvSetProp, InMsgArgs::StProp(name, PropVal::Bool(value))) => {
                    set_mpv_property(&name.to_string(), value, &mpv);
                }
                InMsg(InMsgFn::MpvSetProp, InMsgArgs::StProp(name, PropVal::Num(value))) => {
                    let prop_name = name.to_string();
                    if matches!(name, PropKey::Int(_)) && value.is_finite() && value.fract() == 0.0
                    {
                        set_mpv_property(&prop_name, value as i64, &mpv);
                    } else {
                        set_mpv_property(&prop_name, value, &mpv);
                    }
                }
                InMsg(InMsgFn::MpvSetProp, InMsgArgs::StProp(name, PropVal::Str(value))) => {
                    match name {
                        PropKey::Str(StrProp::Vo) => {
                            // Never allow the WebUI to override `vo=libmpv`, and re-assert our
                            // active shader scaling options.
                            let state = current_display_output_state(&mpv, window_handle);
                            apply_display_output_mode(
                                &mpv,
                                state,
                                gpu_video_processing.load(Ordering::Relaxed),
                            );
                        }
                        PropKey::Str(StrProp::Hwdec) => {
                            if macos_gl::is_software_renderer() {
                                // Keep hwdec disabled inside non-GPU VMs even if WebUI settings
                                // request `auto`.
                                continue;
                            }
                            let normalized = if value == "auto" {
                                "auto-safe"
                            } else {
                                value.as_str()
                            };
                            set_mpv_property("hwdec", normalized, &mpv);
                        }
                        _ => set_mpv_property(&name.to_string(), value, &mpv),
                    }
                }
                InMsg(InMsgFn::MpvSetGpuVideoProcessing, InMsgArgs::Flag(enabled)) => {
                    gpu_video_processing.store(enabled, Ordering::Relaxed);
                    let state = current_display_output_state(&mpv, window_handle);
                    apply_display_output_mode(&mpv, state, enabled);
                }
                InMsg(InMsgFn::MpvCommand, InMsgArgs::Cmd(cmd)) => {
                    send_command(cmd);
                }
                msg => {
                    eprintln!("MPV unsupported message: '{msg:?}'");
                }
            }
        }
    })
}

fn current_display_output_state(_mpv: &Mpv, _window_handle: isize) -> DisplayOutputState {
    DisplayOutputState {
        mode: DisplayOutputMode::Auto,
    }
}
