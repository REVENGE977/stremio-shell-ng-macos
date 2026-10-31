use crate::stremio_app::gpu_video_processing;
use crate::stremio_app::ipc;
use crate::stremio_app::RPCResponse;
use flume::{Receiver, Sender};
use libmpv2::{events::Event, Format, Mpv, SetData};
use std::{
    collections::VecDeque,
    convert::TryFrom,
    ffi::CString,
    os::raw::c_char,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::stremio_app::stremio_player::{
    CmdVal, InMsg, InMsgArgs, InMsgFn, PlayerEnded, PlayerEvent, PlayerProprChange, PlayerResponse,
    PropKey, PropVal,
};

pub mod macos_gl {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use once_cell::sync::Lazy;
    use std::ffi::{c_void, CString};
    use std::os::raw::{c_char, c_int};
    use std::sync::atomic::{AtomicPtr, Ordering};

    static MPV_HANDLE: AtomicPtr<libmpv2_sys::mpv_handle> = AtomicPtr::new(std::ptr::null_mut());
    static MPV_RENDER_CTX: AtomicPtr<libmpv2_sys::mpv_render_context> =
        AtomicPtr::new(std::ptr::null_mut());
    static MPV_GL_LAYER: AtomicPtr<Object> = AtomicPtr::new(std::ptr::null_mut());

    extern "C" {
        static _dispatch_main_q: libc::c_void;
        fn dispatch_async_f(
            queue: *const libc::c_void,
            context: *mut libc::c_void,
            work: unsafe extern "C" fn(*mut libc::c_void),
        );
        fn CGLChoosePixelFormat(
            attribs: *const u32,
            pix: *mut *mut libc::c_void,
            npix: *mut i32,
        ) -> i32;
        fn CGLSetParameter(ctx: *mut libc::c_void, pname: u32, params: *const i32) -> i32;
        fn glGetIntegerv(pname: u32, data: *mut i32);
        fn glClearColor(red: f32, green: f32, blue: f32, alpha: f32);
        fn glClear(mask: u32);
        fn glFlush();
    }

    unsafe extern "C" fn get_proc_address(
        _ctx: *mut c_void,
        name: *const c_char,
    ) -> *mut c_void {
        static GL_FRAMEWORK: Lazy<usize> = Lazy::new(|| {
            let path = CString::new(
                "/System/Library/Frameworks/OpenGL.framework/Versions/Current/OpenGL",
            )
            .unwrap();
            let handle =
                unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            handle as usize
        });
        let handle = *GL_FRAMEWORK as *mut libc::c_void;
        if !handle.is_null() {
            let sym = unsafe { libc::dlsym(handle, name) };
            if !sym.is_null() {
                return sym;
            }
        }
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name) }
    }

    unsafe extern "C" fn handle_main_thread_update(_ctx: *mut libc::c_void) {
        let rctx = MPV_RENDER_CTX.load(Ordering::Acquire);
        let layer = MPV_GL_LAYER.load(Ordering::Acquire);
        if !rctx.is_null() && !layer.is_null() {
            let flags = unsafe { libmpv2_sys::mpv_render_context_update(rctx) };
            if (flags & (libmpv2_sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64)) != 0 {
                let _: () = unsafe { msg_send![layer, setNeedsDisplay] };
            }
        } else if !layer.is_null() {
            let _: () = unsafe { msg_send![layer, setNeedsDisplay] };
        }
    }

    unsafe extern "C" fn on_mpv_render_update(_ctx: *mut c_void) {
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const libc::c_void,
                std::ptr::null_mut(),
                handle_main_thread_update,
            );
        }
    }

    extern "C" fn copy_cgl_pixel_format(
        this: &Object,
        _cmd: Sel,
        mask: u32,
    ) -> *mut libc::c_void {
        const KCGL_PFA_DOUBLE_BUFFER: u32 = 5;
        const KCGL_PFA_COLOR_SIZE: u32 = 8;
        const KCGL_PFA_ALPHA_SIZE: u32 = 11;
        const KCGL_PFA_ACCELERATED: u32 = 73;
        const KCGL_PFA_ALLOW_OFFLINE_RENDERERS: u32 = 96;
        const KCGL_PFA_OPENGL_PROFILE: u32 = 99;
        const KCGL_OGLP_VERSION_3_2_CORE: u32 = 0x3200;

        let attribs_core: [u32; 10] = [
            KCGL_PFA_OPENGL_PROFILE,
            KCGL_OGLP_VERSION_3_2_CORE,
            KCGL_PFA_ACCELERATED,
            KCGL_PFA_DOUBLE_BUFFER,
            KCGL_PFA_COLOR_SIZE,
            24,
            KCGL_PFA_ALPHA_SIZE,
            8,
            KCGL_PFA_ALLOW_OFFLINE_RENDERERS,
            0,
        ];
        let mut pix: *mut libc::c_void = std::ptr::null_mut();
        let mut npix: i32 = 0;
        let err = unsafe { CGLChoosePixelFormat(attribs_core.as_ptr(), &mut pix, &mut npix) };
        if err == 0 && !pix.is_null() {
            return pix;
        }

        let attribs_sw: [u32; 8] = [
            KCGL_PFA_OPENGL_PROFILE,
            KCGL_OGLP_VERSION_3_2_CORE,
            KCGL_PFA_DOUBLE_BUFFER,
            KCGL_PFA_COLOR_SIZE,
            24,
            KCGL_PFA_ALPHA_SIZE,
            8,
            0,
        ];
        let err = unsafe { CGLChoosePixelFormat(attribs_sw.as_ptr(), &mut pix, &mut npix) };
        if err == 0 && !pix.is_null() {
            return pix;
        }

        unsafe {
            msg_send![super(this, class!(CAOpenGLLayer)), copyCGLPixelFormatForDisplayMask: mask]
        }
    }

    extern "C" fn copy_cgl_context(
        this: &Object,
        _cmd: Sel,
        pixel_format: *mut libc::c_void,
    ) -> *mut libc::c_void {
        let ctx: *mut libc::c_void = unsafe {
            msg_send![super(this, class!(CAOpenGLLayer)), copyCGLContextForPixelFormat: pixel_format]
        };
        if !ctx.is_null() {
            const KCGL_CP_SWAP_INTERVAL: u32 = 222;
            let swap_interval: i32 = 1;
            unsafe {
                CGLSetParameter(ctx, KCGL_CP_SWAP_INTERVAL, &swap_interval);
            }
        }
        ctx
    }

    extern "C" fn can_draw_in_cgl_context(
        _this: &Object,
        _cmd: Sel,
        _ctx: *mut libc::c_void,
        _pf: *mut libc::c_void,
        _time: f64,
        _display_time: *const libc::c_void,
    ) -> BOOL {
        YES
    }

    extern "C" fn draw_in_cgl_context(
        this: &Object,
        _cmd: Sel,
        _ctx: *mut libc::c_void,
        _pf: *mut libc::c_void,
        _time: f64,
        _display_time: *const libc::c_void,
    ) {
        let mut rctx = MPV_RENDER_CTX.load(Ordering::Acquire);
        if rctx.is_null() {
            let mpv_handle = MPV_HANDLE.load(Ordering::Acquire);
            if !mpv_handle.is_null() {
                let mut init_params = libmpv2_sys::mpv_opengl_init_params {
                    get_proc_address: Some(get_proc_address),
                    get_proc_address_ctx: std::ptr::null_mut(),
                };
                let mut params = [
                    libmpv2_sys::mpv_render_param {
                        type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                        data: libmpv2_sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
                    },
                    libmpv2_sys::mpv_render_param {
                        type_:
                            libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                        data: &mut init_params as *mut _ as *mut c_void,
                    },
                    libmpv2_sys::mpv_render_param {
                        type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                        data: std::ptr::null_mut(),
                    },
                ];
                let mut new_rctx: *mut libmpv2_sys::mpv_render_context = std::ptr::null_mut();
                let err = unsafe {
                    libmpv2_sys::mpv_render_context_create(
                        &mut new_rctx,
                        mpv_handle,
                        params.as_mut_ptr(),
                    )
                };
                if err >= 0 && !new_rctx.is_null() {
                    unsafe {
                        libmpv2_sys::mpv_render_context_set_update_callback(
                            new_rctx,
                            Some(on_mpv_render_update),
                            this as *const Object as *mut c_void,
                        );
                    }
                    MPV_RENDER_CTX.store(new_rctx, Ordering::Release);
                    rctx = new_rctx;
                } else {
                    eprintln!("mpv: mpv_render_context_create failed with error {err}");
                }
            }
        }

        const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
        const GL_VIEWPORT: u32 = 0x0BA2;
        const GL_COLOR_BUFFER_BIT: u32 = 0x00004000;

        let mut fbo: i32 = 0;
        let mut viewport: [i32; 4] = [0, 0, 0, 0];
        unsafe {
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo);
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
        }

        let width = viewport[2].max(1);
        let height = viewport[3].max(1);

        if !rctx.is_null() {
            let mut fbo_data = libmpv2_sys::mpv_opengl_fbo {
                fbo,
                w: width,
                h: height,
                internal_format: 0,
            };
            let mut flip_y: c_int = 1;
            let mut render_params = [
                libmpv2_sys::mpv_render_param {
                    type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                    data: &mut fbo_data as *mut _ as *mut c_void,
                },
                libmpv2_sys::mpv_render_param {
                    type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                    data: &mut flip_y as *mut _ as *mut c_void,
                },
                libmpv2_sys::mpv_render_param {
                    type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            unsafe {
                libmpv2_sys::mpv_render_context_render(rctx, render_params.as_mut_ptr());
                libmpv2_sys::mpv_render_context_report_swap(rctx);
                glFlush();
            }
        } else {
            unsafe {
                glClearColor(0.0, 0.0, 0.0, 1.0);
                glClear(GL_COLOR_BUFFER_BIT);
                glFlush();
            }
        }
    }

    fn register_layer_class() -> &'static Class {
        static LAYER_CLASS: Lazy<&'static Class> = Lazy::new(|| unsafe {
            let superclass = class!(CAOpenGLLayer);
            let mut decl = ClassDecl::new("StremioMpvOpenGLLayer", superclass)
                .expect("Failed to declare StremioMpvOpenGLLayer");
            decl.add_method(
                sel!(copyCGLPixelFormatForDisplayMask:),
                copy_cgl_pixel_format as extern "C" fn(&Object, Sel, u32) -> *mut libc::c_void,
            );
            decl.add_method(
                sel!(copyCGLContextForPixelFormat:),
                copy_cgl_context
                    as extern "C" fn(&Object, Sel, *mut libc::c_void) -> *mut libc::c_void,
            );
            decl.add_method(
                sel!(canDrawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                can_draw_in_cgl_context
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut libc::c_void,
                        *mut libc::c_void,
                        f64,
                        *const libc::c_void,
                    ) -> BOOL,
            );
            decl.add_method(
                sel!(drawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                draw_in_cgl_context
                    as extern "C" fn(
                        &Object,
                        Sel,
                        *mut libc::c_void,
                        *mut libc::c_void,
                        f64,
                        *const libc::c_void,
                    ),
            );
            decl.register()
        });
        *LAYER_CLASS
    }

    pub unsafe fn create_mpv_opengl_layer(scale_factor: f64) -> *mut Object {
        let cls = register_layer_class();
        let layer: *mut Object = msg_send![cls, layer];
        let _: () = msg_send![layer, setAsynchronous: NO];
        let _: () = msg_send![layer, setNeedsDisplayOnBoundsChange: YES];
        let _: () = msg_send![layer, setOpaque: YES];
        let _: () = msg_send![layer, setContentsScale: scale_factor];
        MPV_GL_LAYER.store(layer, Ordering::Release);
        layer
    }

    pub unsafe fn update_mpv_layer_scale(scale_factor: f64) {
        let layer = MPV_GL_LAYER.load(Ordering::Acquire);
        if !layer.is_null() {
            let _: () = msg_send![layer, setContentsScale: scale_factor];
            let _: () = msg_send![layer, setNeedsDisplay];
        }
    }

    pub fn attach_mpv_handle(mpv_handle: *mut libmpv2_sys::mpv_handle, layer_handle: i64) {
        if layer_handle != 0 {
            MPV_GL_LAYER.store(layer_handle as *mut Object, Ordering::Release);
        }
        MPV_HANDLE.store(mpv_handle, Ordering::Release);
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const libc::c_void,
                std::ptr::null_mut(),
                handle_main_thread_update,
            );
        }
    }
}

struct ObserveProperty {
    name: String,
    format: Format,
}

#[derive(Debug, Eq, PartialEq)]
struct SubtitleCommand {
    name: &'static str,
    args: Vec<String>,
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

enum SubtitleWorkerRequest {
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

fn create_subtitle_thread(
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
                        let event =
                            unsafe { &*libmpv2_sys::mpv_wait_event(mpv.ctx.as_ptr(), 0.0) };
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

#[derive(Debug, Default)]
struct VideoReadyState {
    next_load_id: u64,
    current_load_id: Option<u64>,
    pending_load_ids: VecDeque<u64>,
    active_load_id: Option<u64>,
    file_loaded_id: Option<u64>,
    ready_sent_id: Option<u64>,
}

impl VideoReadyState {
    fn begin_transition(&mut self, loads_file: bool) -> u64 {
        self.next_load_id += 1;
        self.current_load_id = Some(self.next_load_id);
        self.file_loaded_id = None;
        self.ready_sent_id = None;
        if loads_file {
            self.pending_load_ids.push_back(self.next_load_id);
        }
        self.next_load_id
    }

    fn start_file(&mut self) {
        self.active_load_id = self.pending_load_ids.pop_front();
        self.file_loaded_id = None;
    }

    fn file_loaded(&mut self) {
        if self.active_load_id == self.current_load_id {
            self.file_loaded_id = self.active_load_id;
        }
    }

    fn playback_restarted(&mut self) -> Option<u64> {
        let load_id = self.current_load_id?;
        if self.file_loaded_id != Some(load_id) || self.ready_sent_id == Some(load_id) {
            return None;
        }
        self.ready_sent_id = Some(load_id);
        Some(load_id)
    }
}

type SharedVideoReadyState = Arc<Mutex<VideoReadyState>>;

fn video_ready_response(load_id: u64, ready: bool) -> String {
    RPCResponse::response_message(PlayerResponse::video_ready(load_id, ready).to_value())
}

fn send_video_ready(rpc_response_sender: &Sender<String>, load_id: u64, ready: bool) {
    if let Err(error) = rpc_response_sender.send(video_ready_response(load_id, ready)) {
        eprintln!("failed to send video readiness: {error}");
    }
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

fn create_mpv(window_handle: i64) -> Mpv {
    let mpv = Mpv::with_initializer(|initializer| {
        macro_rules! set_property {
            ($name:literal, $value:expr) => {
                initializer
                    .set_property($name, $value)
                    .expect(concat!("failed to set ", $name));
            };
        }
        set_property!("title", crate::stremio_app::constants::APP_NAME);
        set_property!("audio-client-name", crate::stremio_app::constants::APP_NAME);
        set_property!("terminal", "yes");
        #[cfg(debug_assertions)]
        set_property!("msg-level", "all=no,cplayer=debug");
        #[cfg(not(debug_assertions))]
        set_property!("msg-level", "all=no");
        set_property!("quiet", "yes");
        set_property!("hwdec", "auto");
        // `%23%` escapes the 23-byte HTTP status list as one mpv option value.
        set_property!(
            "stream-lavf-o",
            "reconnect=1,reconnect_streamed=1,reconnect_on_network_error=1,reconnect_on_http_error=%23%408,429,500,502,503,504,reconnect_delay_max=15"
        );
        set_property!("vo", "libmpv");

        let mut options = vec![
            ("target-colorspace-hint", "auto"),
            ("target-colorspace-hint-mode", "target"),
            ("tone-mapping", "bt.2390"),
            ("dither-depth", "auto"),
            ("keep-open", "yes"),
        ];
        if gpu_video_processing::unified_memory_architecture() {
            options.push(("profile", "fast"));
        } else {
            options.extend([
                ("deband", "yes"),
                ("scale", "spline36"),
                ("cscale", "spline36"),
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

fn apply_display_output_mode(mpv: &Mpv, state: DisplayOutputState, _gpu_video_processing: bool) {
    let color = match state.mode {
        DisplayOutputMode::Auto => [
            ("target-colorspace-hint", "auto"),
            ("target-trc", "auto"),
            ("target-prim", "auto"),
        ],
    };

    for (name, value) in color {
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
        mpv_event_client
            .disable_deprecated_events()
            .expect("failed to disable deprecated MPV events");

        // -- Event handler loop --

        loop {
            for ObserveProperty { name, format } in observe_property_receiver.drain() {
                mpv_event_client
                    .observe_property(&name, format, 0)
                    .expect("failed to observer MPV property");
            }

            let event = match mpv_event_client.wait_event(0.1) {
                Some(Ok(event)) => event,
                Some(Err(error)) => {
                    eprintln!("Event errored: {error:?}");
                    continue;
                }
                // dummy event received (may be created on a wake up call or on timeout)
                None => continue,
            };

            if matches!(&event, Event::StartFile) {
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
                let load_id = video_ready_state
                    .lock()
                    .expect("cannot lock video readiness state")
                    .playback_restarted();
                if let Some(load_id) = load_id {
                    send_video_ready(&rpc_response_sender, load_id, true);
                }
                continue;
            }

            // even if you don't do anything with the events, it is still necessary to empty the event loop
            let player_response = match event {
                Event::PropertyChange { name, change, .. } => PlayerResponse(
                    "mpv-prop-change",
                    PlayerEvent::PropChange(PlayerProprChange::from_name_value(
                        name.to_string(),
                        change,
                    )),
                ),
                Event::EndFile(reason) => PlayerResponse(
                    "mpv-event-ended",
                    PlayerEvent::End(PlayerEnded::from_end_reason(reason)),
                ),
                Event::Shutdown => {
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
        // -- Helpers --
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
            let (acknowledge, acknowledged) = flume::bounded(1);
            if subtitle_request_sender
                .send(SubtitleWorkerRequest::BeginMediaChange(acknowledge))
                .is_err()
                || acknowledged.recv().is_err()
            {
                eprintln!("mpv: subtitle worker stopped before media change");
                return;
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
                if let Err(error) = mpv.command(name, &args) {
                    eprintln!("failed to execute MPV command: '{error:#}'")
                }
            }
            if subtitle_request_sender
                .send(SubtitleWorkerRequest::EndMediaChange)
                .is_err()
            {
                eprintln!("mpv: subtitle worker stopped after media change");
            }
        };

        fn set_property(name: impl ToString, value: impl SetData, mpv: &Mpv) {
            if let Err(error) = mpv.set_property(&name.to_string(), value) {
                eprintln!("cannot set MPV property: '{error:#}'")
            }
        }

        // -- InMsg handler loop --

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
                    set_property(name, value, &mpv);
                }
                InMsg(InMsgFn::MpvSetProp, InMsgArgs::StProp(name, PropVal::Num(value))) => {
                    set_property(name, value, &mpv);
                }
                InMsg(InMsgFn::MpvSetProp, InMsgArgs::StProp(name, PropVal::Str(value))) => {
                    let is_vo = name.to_string() == "vo";
                    if !is_vo {
                        set_property(name, value, &mpv);
                    }
                    // vo reinit reverts color props to defaults; re-assert for the current display.
                    if is_vo {
                        let state = current_display_output_state(&mpv, window_handle);
                        apply_display_output_mode(
                            &mpv,
                            state,
                            gpu_video_processing.load(Ordering::Relaxed),
                        );
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
