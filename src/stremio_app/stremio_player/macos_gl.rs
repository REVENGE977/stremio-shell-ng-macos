use std::{
    ffi::{c_void, CStr, CString},
    os::raw::{c_char, c_int},
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
};

use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel, BOOL, NO, YES},
    sel, sel_impl,
};
use once_cell::sync::Lazy;

static MPV_HANDLE: AtomicPtr<libmpv2_sys::mpv_handle> = AtomicPtr::new(std::ptr::null_mut());
static MPV_RENDER_CTX: AtomicPtr<libmpv2_sys::mpv_render_context> =
    AtomicPtr::new(std::ptr::null_mut());
static MPV_GL_LAYER: AtomicPtr<Object> = AtomicPtr::new(std::ptr::null_mut());
static MPV_CGL_PIX: AtomicPtr<libc::c_void> = AtomicPtr::new(std::ptr::null_mut());
static MPV_CGL_CTX: AtomicPtr<libc::c_void> = AtomicPtr::new(std::ptr::null_mut());

static IS_SOFTWARE_GL: AtomicBool = AtomicBool::new(false);
static RENDER_UPDATE_QUEUED: AtomicBool = AtomicBool::new(false);
static HAS_PENDING_FRAME: AtomicBool = AtomicBool::new(false);
static PENDING_FRAME_SEQ: AtomicU64 = AtomicU64::new(0);
static PLAYBACK_AWAKE_WANTED: AtomicBool = AtomicBool::new(false);
static WEBUI_AWAKE_WANTED: AtomicBool = AtomicBool::new(false);
static LAYER_VISIBLE_WANTED: AtomicBool = AtomicBool::new(false);
static PLAYBACK_ACTIVITY_TOKEN: AtomicPtr<Object> = AtomicPtr::new(std::ptr::null_mut());

const OCCLUSION_WATCHDOG_NS: i64 = 120_000_000;

extern "C" {
    static _dispatch_main_q: libc::c_void;
    fn dispatch_async_f(
        queue: *const libc::c_void,
        context: *mut libc::c_void,
        work: unsafe extern "C" fn(*mut libc::c_void),
    );
    fn dispatch_after_f(
        when: u64,
        queue: *const libc::c_void,
        context: *mut libc::c_void,
        work: unsafe extern "C" fn(*mut libc::c_void),
    );
    fn dispatch_time(when: u64, delta: i64) -> u64;

    fn CGLChoosePixelFormat(
        attribs: *const u32,
        pix: *mut *mut libc::c_void,
        npix: *mut i32,
    ) -> i32;
    fn CGLCreateContext(
        pix: *mut libc::c_void,
        share: *mut libc::c_void,
        ctx: *mut *mut libc::c_void,
    ) -> i32;
    fn CGLEnable(ctx: *mut libc::c_void, pname: u32) -> i32;
    fn CGLRetainPixelFormat(pix: *mut libc::c_void) -> *mut libc::c_void;
    fn CGLRetainContext(ctx: *mut libc::c_void) -> *mut libc::c_void;
    fn CGLLockContext(ctx: *mut libc::c_void) -> i32;
    fn CGLUnlockContext(ctx: *mut libc::c_void) -> i32;
    fn CGLSetCurrentContext(ctx: *mut libc::c_void) -> i32;
    fn CGLGetCurrentContext() -> *mut libc::c_void;
    fn CGLSetParameter(ctx: *mut libc::c_void, pname: u32, params: *const i32) -> i32;

    fn glGetString(name: u32) -> *const u8;
    fn glGetIntegerv(pname: u32, data: *mut i32);
    fn glClearColor(red: f32, green: f32, blue: f32, alpha: f32);
    fn glClear(mask: u32);
    fn glFlush();
}

unsafe fn query_gl_string(name: u32) -> String {
    let ptr = glGetString(name);
    if ptr.is_null() {
        return String::new();
    }
    CStr::from_ptr(ptr as *const c_char)
        .to_string_lossy()
        .into_owned()
}

pub fn is_software_renderer() -> bool {
    ensure_cgl_context();
    IS_SOFTWARE_GL.load(Ordering::Acquire)
}

pub fn ensure_cgl_context() -> (*mut libc::c_void, *mut libc::c_void) {
    let existing_pix = MPV_CGL_PIX.load(Ordering::Acquire);
    let existing_ctx = MPV_CGL_CTX.load(Ordering::Acquire);
    if !existing_pix.is_null() && !existing_ctx.is_null() {
        return (existing_pix, existing_ctx);
    }

    const KCGL_PFA_DOUBLE_BUFFER: u32 = 5;
    const KCGL_PFA_COLOR_SIZE: u32 = 8;
    const KCGL_PFA_ALPHA_SIZE: u32 = 11;
    const KCGL_PFA_ACCELERATED: u32 = 73;
    const KCGL_PFA_ALLOW_OFFLINE_RENDERERS: u32 = 96;
    const KCGL_PFA_OPENGL_PROFILE: u32 = 99;
    const KCGL_OGLP_VERSION_3_2_CORE: u32 = 0x3200;
    const KCGL_CP_SWAP_INTERVAL: u32 = 222;
    const KCGL_CE_MP_ENGINE: u32 = 313;

    const GL_VENDOR: u32 = 0x1F00;
    const GL_RENDERER: u32 = 0x1F01;
    const GL_VERSION: u32 = 0x1F02;

    let attribs_hw: [u32; 10] = [
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
    let err = unsafe { CGLChoosePixelFormat(attribs_hw.as_ptr(), &mut pix, &mut npix) };
    let mut software_gl = false;

    if err != 0 || pix.is_null() {
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
        let err_sw = unsafe { CGLChoosePixelFormat(attribs_sw.as_ptr(), &mut pix, &mut npix) };
        if err_sw == 0 && !pix.is_null() {
            software_gl = true;
        }
    }

    let mut ctx: *mut libc::c_void = std::ptr::null_mut();
    if !pix.is_null() {
        let err_ctx = unsafe { CGLCreateContext(pix, std::ptr::null_mut(), &mut ctx) };
        if err_ctx == 0 && !ctx.is_null() {
            unsafe {
                CGLLockContext(ctx);
                let prev_ctx = CGLGetCurrentContext();
                CGLSetCurrentContext(ctx);

                // Disable blocking VSync wait inside CAOpenGLLayer so main thread never stalls.
                let swap_interval: i32 = 0;
                CGLSetParameter(ctx, KCGL_CP_SWAP_INTERVAL, &swap_interval);

                let vendor = query_gl_string(GL_VENDOR);
                let renderer = query_gl_string(GL_RENDERER);
                let version = query_gl_string(GL_VERSION);
                let renderer_lower = renderer.to_ascii_lowercase();
                if renderer_lower.contains("software")
                    || renderer_lower.contains("generic")
                    || renderer_lower.contains("llvmpipe")
                {
                    software_gl = true;
                }

                if !software_gl {
                    let _ = CGLEnable(ctx, KCGL_CE_MP_ENGINE);
                }

                println!(
                    "[macOS GL] Initialized {} OpenGL context (Vendor: {}, Renderer: {}, Version: {})",
                    if software_gl { "software" } else { "hardware" },
                    vendor,
                    renderer,
                    version
                );

                CGLSetCurrentContext(prev_ctx);
                CGLUnlockContext(ctx);
            }

            IS_SOFTWARE_GL.store(software_gl, Ordering::Release);
            MPV_CGL_PIX.store(pix, Ordering::Release);
            MPV_CGL_CTX.store(ctx, Ordering::Release);
        }
    }

    (pix, ctx)
}

unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    static GL_FRAMEWORK: Lazy<usize> = Lazy::new(|| {
        let candidates = [
            "/System/Library/Frameworks/OpenGL.framework/Versions/Current/OpenGL",
            "/System/Library/Frameworks/OpenGL.framework/OpenGL",
        ];
        for candidate in candidates {
            if let Ok(path) = CString::new(candidate) {
                let handle =
                    unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
                if !handle.is_null() {
                    return handle as usize;
                }
            }
        }
        0
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

fn ensure_render_context_locked(
    cgl_ctx: *mut libc::c_void,
) -> *mut libmpv2_sys::mpv_render_context {
    let mut rctx = MPV_RENDER_CTX.load(Ordering::Acquire);
    if !rctx.is_null() {
        return rctx;
    }
    let mpv_handle = MPV_HANDLE.load(Ordering::Acquire);
    if mpv_handle.is_null() || cgl_ctx.is_null() {
        return std::ptr::null_mut();
    }

    let mut init_params = libmpv2_sys::mpv_opengl_init_params {
        get_proc_address: Some(get_proc_address),
        get_proc_address_ctx: std::ptr::null_mut(),
    };
    let mut adv_control: c_int = 0;
    let mut params = [
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            data: libmpv2_sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut c_void,
        },
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
            data: &mut init_params as *mut _ as *mut c_void,
        },
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_ADVANCED_CONTROL,
            data: &mut adv_control as *mut _ as *mut c_void,
        },
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    let mut new_rctx: *mut libmpv2_sys::mpv_render_context = std::ptr::null_mut();
    let err = unsafe {
        libmpv2_sys::mpv_render_context_create(&mut new_rctx, mpv_handle, params.as_mut_ptr())
    };
    if err >= 0 && !new_rctx.is_null() {
        unsafe {
            libmpv2_sys::mpv_render_context_set_update_callback(
                new_rctx,
                Some(on_mpv_render_update),
                std::ptr::null_mut(),
            );
        }
        MPV_RENDER_CTX.store(new_rctx, Ordering::Release);
        rctx = new_rctx;
    } else {
        eprintln!("mpv: mpv_render_context_create failed with error {err}");
    }
    rctx
}

unsafe fn skip_pending_frame(rctx: *mut libmpv2_sys::mpv_render_context) {
    let mut skip_rendering: c_int = 1;
    let mut block_for_target_time: c_int = 0;
    let mut skip_params = [
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_SKIP_RENDERING,
            data: &mut skip_rendering as *mut _ as *mut c_void,
        },
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
            data: &mut block_for_target_time as *mut _ as *mut c_void,
        },
        libmpv2_sys::mpv_render_param {
            type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    libmpv2_sys::mpv_render_context_render(rctx, skip_params.as_mut_ptr());
}

unsafe extern "C" fn handle_occlusion_watchdog(seq_ptr: *mut libc::c_void) {
    let target_seq = seq_ptr as usize as u64;
    if !HAS_PENDING_FRAME.load(Ordering::Acquire)
        || PENDING_FRAME_SEQ.load(Ordering::Acquire) != target_seq
    {
        return;
    }

    if !HAS_PENDING_FRAME.swap(false, Ordering::AcqRel) {
        return;
    }

    let cgl_ctx = MPV_CGL_CTX.load(Ordering::Acquire);
    let rctx = MPV_RENDER_CTX.load(Ordering::Acquire);
    if cgl_ctx.is_null() || rctx.is_null() {
        return;
    }

    CGLLockContext(cgl_ctx);
    let prev_ctx = CGLGetCurrentContext();
    CGLSetCurrentContext(cgl_ctx);
    libmpv2_sys::mpv_render_context_update(rctx);
    skip_pending_frame(rctx);
    CGLSetCurrentContext(prev_ctx);
    CGLUnlockContext(cgl_ctx);
}

unsafe extern "C" fn handle_main_thread_update(_ctx: *mut libc::c_void) {
    RENDER_UPDATE_QUEUED.store(false, Ordering::Release);

    let (_, cgl_ctx) = ensure_cgl_context();
    let layer = MPV_GL_LAYER.load(Ordering::Acquire);
    if cgl_ctx.is_null() || layer.is_null() {
        return;
    }

    let mut should_draw = false;
    CGLLockContext(cgl_ctx);
    let prev_ctx = CGLGetCurrentContext();
    CGLSetCurrentContext(cgl_ctx);

    let rctx = ensure_render_context_locked(cgl_ctx);
    if !rctx.is_null() {
        let flags = libmpv2_sys::mpv_render_context_update(rctx);
        if (flags & (libmpv2_sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64)) != 0 {
            should_draw = true;
        }
    }

    CGLSetCurrentContext(prev_ctx);
    CGLUnlockContext(cgl_ctx);

    if should_draw {
        if !LAYER_VISIBLE_WANTED.swap(true, Ordering::AcqRel) {
            let _: () = msg_send![layer, setHidden: NO];
        }
        let seq = PENDING_FRAME_SEQ.fetch_add(1, Ordering::AcqRel) + 1;
        HAS_PENDING_FRAME.store(true, Ordering::Release);

        let _: () = msg_send![layer, setNeedsDisplay];
        let _: () = msg_send![layer, displayIfNeeded];

        // CoreAnimation frequently defers drawInCGLContext: until the CATransaction commit
        // at the end of the runloop pass (~1ms). Only skip rendering if CoreAnimation still
        // has not drawn the frame after OCCLUSION_WATCHDOG_NS (window hidden/minimized/occluded),
        // so normal streaming never drops frames while hidden views still advance playback state.
        if HAS_PENDING_FRAME.load(Ordering::Acquire) {
            let when = dispatch_time(0, OCCLUSION_WATCHDOG_NS);
            dispatch_after_f(
                when,
                &_dispatch_main_q as *const libc::c_void,
                seq as usize as *mut libc::c_void,
                handle_occlusion_watchdog,
            );
        }
    }
}

unsafe extern "C" fn on_mpv_render_update(_ctx: *mut c_void) {
    if !RENDER_UPDATE_QUEUED.swap(true, Ordering::AcqRel) {
        dispatch_async_f(
            &_dispatch_main_q as *const libc::c_void,
            std::ptr::null_mut(),
            handle_main_thread_update,
        );
    }
}

extern "C" fn copy_cgl_pixel_format(this: &Object, _cmd: Sel, mask: u32) -> *mut libc::c_void {
    let (pix, _) = ensure_cgl_context();
    if !pix.is_null() {
        return unsafe { CGLRetainPixelFormat(pix) };
    }
    unsafe { msg_send![super(this, class!(CAOpenGLLayer)), copyCGLPixelFormatForDisplayMask: mask] }
}

extern "C" fn copy_cgl_context(
    this: &Object,
    _cmd: Sel,
    pixel_format: *mut libc::c_void,
) -> *mut libc::c_void {
    let (_, ctx) = ensure_cgl_context();
    if !ctx.is_null() {
        return unsafe { CGLRetainContext(ctx) };
    }
    unsafe {
        msg_send![super(this, class!(CAOpenGLLayer)), copyCGLContextForPixelFormat: pixel_format]
    }
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
    _this: &Object,
    _cmd: Sel,
    ctx: *mut libc::c_void,
    _pf: *mut libc::c_void,
    _time: f64,
    _display_time: *const libc::c_void,
) {
    HAS_PENDING_FRAME.store(false, Ordering::Release);
    PENDING_FRAME_SEQ.fetch_add(1, Ordering::AcqRel);

    let rctx = ensure_render_context_locked(ctx);

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
        let mut block_for_target_time: c_int = 0;
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
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: &mut block_for_target_time as *mut _ as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        unsafe {
            libmpv2_sys::mpv_render_context_update(rctx);
            libmpv2_sys::mpv_render_context_render(rctx, render_params.as_mut_ptr());
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
            copy_cgl_context as extern "C" fn(&Object, Sel, *mut libc::c_void) -> *mut libc::c_void,
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

/// Attaches a `CAOpenGLLayer`-backed `NSView` inside the window's `contentView`
/// directly below the `WKWebView`, returning the layer pointer as an opaque handle.
pub unsafe fn attach_layer_to_window(ns_window_ptr: *mut libc::c_void, scale_factor: f64) -> i64 {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: f64,
        height: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    let ns_window = ns_window_ptr as *mut Object;
    let content_view: *mut Object = msg_send![ns_window, contentView];
    let _: () = msg_send![content_view, setWantsLayer: true];
    let bounds: NSRect = msg_send![content_view, bounds];

    let mpv_view: *mut Object = msg_send![class!(NSView), alloc];
    let mpv_view: *mut Object = msg_send![mpv_view, initWithFrame: bounds];
    let gl_layer = create_mpv_opengl_layer(scale_factor);
    let _: () = msg_send![mpv_view, setLayer: gl_layer];
    let _: () = msg_send![mpv_view, setWantsLayer: true];
    // NSViewWidthSizable (2) | NSViewHeightSizable (16) = 18
    let _: () = msg_send![mpv_view, setAutoresizingMask: 18u64];

    // Position mpv_view below WKWebView (NSWindowBelow = -1)
    let nil_view: *mut Object = std::ptr::null_mut();
    let _: () =
        msg_send![content_view, addSubview: mpv_view positioned: -1isize relativeTo: nil_view];
    let _: () = msg_send![gl_layer, setNeedsDisplay];

    println!(
        "[macOS Shell] Attached embedded MPV OpenGL layer {:?} inside contentView {:?}",
        gl_layer, content_view
    );
    gl_layer as i64
}

pub unsafe fn create_mpv_opengl_layer(scale_factor: f64) -> *mut Object {
    ensure_cgl_context();
    let cls = register_layer_class();
    let layer: *mut Object = msg_send![cls, layer];
    let _: () = msg_send![layer, setAsynchronous: NO];
    let _: () = msg_send![layer, setNeedsDisplayOnBoundsChange: YES];
    let _: () = msg_send![layer, setOpaque: YES];
    let _: () = msg_send![layer, setHidden: YES];
    let _: () = msg_send![layer, setContentsScale: scale_factor];
    LAYER_VISIBLE_WANTED.store(false, Ordering::Release);
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

    // Initialize mpv_render_context synchronously on the main thread before any
    // playback or worker threads start, as required by libmpv's render.h contract.
    let (_, cgl_ctx) = ensure_cgl_context();
    if !cgl_ctx.is_null() {
        unsafe {
            CGLLockContext(cgl_ctx);
            let prev_ctx = CGLGetCurrentContext();
            CGLSetCurrentContext(cgl_ctx);
            let _ = ensure_render_context_locked(cgl_ctx);
            CGLSetCurrentContext(prev_ctx);
            CGLUnlockContext(cgl_ctx);
        }
    }
}

pub fn detach_mpv_handle() {
    set_webui_sleep_inhibited(false);
    set_playback_sleep_inhibited(false);
    set_layer_visible(false);
    HAS_PENDING_FRAME.store(false, Ordering::Release);
    PENDING_FRAME_SEQ.fetch_add(1, Ordering::AcqRel);
    MPV_HANDLE.store(std::ptr::null_mut(), Ordering::Release);

    let rctx = MPV_RENDER_CTX.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if rctx.is_null() {
        return;
    }

    let cgl_ctx = MPV_CGL_CTX.load(Ordering::Acquire);
    if !cgl_ctx.is_null() {
        unsafe {
            CGLLockContext(cgl_ctx);
            let prev_ctx = CGLGetCurrentContext();
            CGLSetCurrentContext(cgl_ctx);
            libmpv2_sys::mpv_render_context_set_update_callback(rctx, None, std::ptr::null_mut());
            libmpv2_sys::mpv_render_context_free(rctx);
            CGLSetCurrentContext(prev_ctx);
            CGLUnlockContext(cgl_ctx);
        }
    } else {
        unsafe {
            libmpv2_sys::mpv_render_context_set_update_callback(rctx, None, std::ptr::null_mut());
            libmpv2_sys::mpv_render_context_free(rctx);
        }
    }
}

unsafe extern "C" fn sync_layer_visibility_on_main(_ctx: *mut libc::c_void) {
    let layer = MPV_GL_LAYER.load(Ordering::Acquire);
    if layer.is_null() {
        return;
    }
    let visible = LAYER_VISIBLE_WANTED.load(Ordering::Acquire);
    let hidden: BOOL = if visible { NO } else { YES };
    let _: () = msg_send![layer, setHidden: hidden];
    if visible {
        let _: () = msg_send![layer, setNeedsDisplay];
    }
}

/// Shows the embedded OpenGL video layer during active playback and hides it when idle
/// so CoreAnimation does not retain/composite an unused GL backing store while browsing UI.
pub fn set_layer_visible(visible: bool) {
    let prev = LAYER_VISIBLE_WANTED.swap(visible, Ordering::AcqRel);
    if prev == visible {
        return;
    }
    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const libc::c_void,
            std::ptr::null_mut(),
            sync_layer_visibility_on_main,
        );
    }
}

unsafe extern "C" fn sync_playback_activity_on_main(_ctx: *mut libc::c_void) {
    let wanted =
        PLAYBACK_AWAKE_WANTED.load(Ordering::Acquire) || WEBUI_AWAKE_WANTED.load(Ordering::Acquire);
    let existing = PLAYBACK_ACTIVITY_TOKEN.load(Ordering::Acquire);

    if wanted && existing.is_null() {
        // NSActivityIdleDisplaySleepDisabled (1 << 40) | NSActivityUserInitiated (0x00FFFFFF | (1 << 20))
        // Prevents macOS App Nap throttling and display sleep during active video playback.
        const NS_ACTIVITY_IDLE_DISPLAY_SLEEP_DISABLED: u64 = 1u64 << 40;
        const NS_ACTIVITY_IDLE_SYSTEM_SLEEP_DISABLED: u64 = 1u64 << 20;
        const NS_ACTIVITY_USER_INITIATED: u64 =
            0x00FF_FFFF | NS_ACTIVITY_IDLE_SYSTEM_SLEEP_DISABLED;
        let options: u64 = NS_ACTIVITY_USER_INITIATED | NS_ACTIVITY_IDLE_DISPLAY_SLEEP_DISABLED;

        let process_info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let reason_cstr = CString::new("Active video playback").unwrap();
        let ns_reason: *mut Object = msg_send![class!(NSString), alloc];
        let ns_reason: *mut Object = msg_send![ns_reason, initWithUTF8String: reason_cstr.as_ptr()];
        let token: *mut Object =
            msg_send![process_info, beginActivityWithOptions: options reason: ns_reason];
        let _: () = msg_send![ns_reason, release];

        if !token.is_null() {
            let _: *mut Object = msg_send![token, retain];
            PLAYBACK_ACTIVITY_TOKEN.store(token, Ordering::Release);
        }
    } else if !wanted && !existing.is_null() {
        let token = PLAYBACK_ACTIVITY_TOKEN.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !token.is_null() {
            let process_info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
            let _: () = msg_send![process_info, endActivity: token];
            let _: () = msg_send![token, release];
        }
    }
}

fn update_sleep_inhibited(flag: &AtomicBool, active: bool) {
    let prev = flag.swap(active, Ordering::AcqRel);
    if prev == active {
        return;
    }
    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const libc::c_void,
            std::ptr::null_mut(),
            sync_playback_activity_on_main,
        );
    }
}

/// Prevents macOS App Nap throttling and display sleep while video is actively playing.
pub fn set_playback_sleep_inhibited(active: bool) {
    update_sleep_inhibited(&PLAYBACK_AWAKE_WANTED, active);
}

/// Mirrors WebUI `screensaver-toggle` / `set-window-mode` requests cleanly alongside playback state.
pub fn set_webui_sleep_inhibited(active: bool) {
    update_sleep_inhibited(&WEBUI_AWAKE_WANTED, active);
}
