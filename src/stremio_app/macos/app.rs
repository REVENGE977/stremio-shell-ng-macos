use std::{
    io::Read,
    process::Command,
    str,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use serde_json::json;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::WindowExtMacOS,
    window::WindowBuilder,
};
use wry::{DragDropEvent, WebViewBuilder};

use crate::stremio_app::{
    constants::{
        safe_url, web_endpoint_with_streaming_server, APP_NAME, DEV_ENDPOINT, SERVER_IPC_KEY,
        STA_ENDPOINT, WEB_ENDPOINT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    },
    discord::DiscordRpc,
    enhanced::{EnhancedAction, EnhancedManager, ENHANCED_SCRIPT},
    ipc::{CacheDirectoryRequest, RPCRequest, RPCResponse},
    open_media,
    stremio_player::{player::macos_gl, Player},
    stremio_server::{ServerEvent, StremioServer},
    window_settings::WindowSettings,
    PipeServer,
};

pub enum UserEvent {
    ExecuteScript(String),
    DispatchWebMessage(String),
    Navigate(String),
    SetFullscreen(bool),
    SetInterfaceScale(u64),
    FocusWindow,
    PollServerEvents,
    RestartServer,
    StopServer,
    Quit,
}

pub fn run_macos(
    opt: crate::Opt,
    mut commands_server: Option<PipeServer>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("[macOS Shell] Initializing Stremio Enhanced macOS runtime...");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let saved_settings = WindowSettings::load().unwrap_or_default();
    let initial_width = if saved_settings.width > 0.0 {
        saved_settings.width
    } else {
        1280.0
    };
    let initial_height = if saved_settings.height > 0.0 {
        saved_settings.height
    } else {
        720.0
    };

    let window = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_inner_size(LogicalSize::new(initial_width, initial_height))
        .with_min_inner_size(LogicalSize::new(
            WINDOW_MIN_WIDTH as f64,
            WINDOW_MIN_HEIGHT as f64,
        ))
        .with_transparent(true)
        .with_visible(!opt.start_hidden)
        .build(&event_loop)?;

    // Separate incoming (JS -> Rust) and outgoing (Rust -> JS) channels
    let (ipc_in_tx, ipc_in_rx) = flume::unbounded::<String>();
    let (web_out_tx, web_out_rx) = flume::unbounded::<String>();

    // Forward outgoing RPC responses to the WebView via UserEvent::DispatchWebMessage
    let proxy_for_out = proxy.clone();
    thread::spawn(move || {
        while let Ok(msg) = web_out_rx.recv() {
            proxy_for_out
                .send_event(UserEvent::DispatchWebMessage(msg))
                .ok();
        }
    });

    // Open media handler
    let (open_sender, open_receiver) =
        flume::unbounded::<crate::stremio_app::open_media::OpenRequest>();
    let command_arg = opt
        .command
        .clone()
        .filter(|cmd| !cmd.is_empty())
        .map(open_media::normalize)
        .unwrap_or_default();

    let web_out_open = web_out_tx.clone();
    thread::spawn(move || {
        let mut ready = false;
        let mut pending = (!command_arg.is_empty()).then_some(command_arg);
        for request in open_receiver {
            match request {
                crate::stremio_app::open_media::OpenRequest::Input(input) => pending = Some(input),
                crate::stremio_app::open_media::OpenRequest::Ready => ready = true,
                crate::stremio_app::open_media::OpenRequest::Reset => ready = false,
            }
            if ready {
                if let Some(input) = pending.take() {
                    let message = open_media::message(&input);
                    web_out_open
                        .send(RPCResponse::response_message(Some(message)))
                        .ok();
                }
            }
        }
    });

    // Single application IPC listener
    if let Some(mut listener) = commands_server.take() {
        let open_sender_clone = open_sender.clone();
        thread::spawn(move || loop {
            if let Ok(mut stream) = listener.accept() {
                let mut buf = vec![];
                stream.read_to_end(&mut buf).ok();
                if let Ok(s) = str::from_utf8(&buf) {
                    if !s.is_empty() {
                        open_sender_clone
                            .send(crate::stremio_app::open_media::OpenRequest::Input(
                                s.to_string(),
                            ))
                            .ok();
                    }
                }
            }
        });
    }

    // Initialize Enhanced Manager
    let enhanced = Arc::new(Mutex::new(EnhancedManager::new()));
    let (ui_data, active_theme_css) = {
        let mgr = enhanced.lock().unwrap();
        let running = crate::stremio_app::stremio_server::is_streaming_server_online();
        (
            mgr.ui_data(running),
            mgr.active_theme().map(|t| t.content.clone()),
        )
    };

    let initial_data_json = serde_json::to_string(&ui_data).unwrap_or_else(|_| "{}".to_string());
    let theme_css_json = serde_json::to_string(&active_theme_css.unwrap_or_default())
        .unwrap_or_else(|_| "\"\"".to_string());

    let server_ipc_key = std::env::var(SERVER_IPC_KEY).unwrap_or_default();

    // Prepare initialization script for WKWebView.
    // Note: wry defines window.ipc via Object.defineProperty(..., Object.freeze({ postMessage })).
    // We preserve window.ipc and build window.chrome.webview and window.qt.webChannelTransport on top of it.
    let init_script = format!(
        r##"
        (function() {{
            if (window.self !== window.top) return;
            try {{
                window.stremio_server_ipc_key = {server_ipc_key:?};
                window.__STREMIO_SHELL__ = true;
                window.__INITIAL_ENHANCED_DATA__ = {initial_data_json};

                const listeners = [];
                const postToNative = function(msg) {{
                    try {{
                        const payload = typeof msg === 'string' ? msg : JSON.stringify(msg);
                        if (window.ipc && typeof window.ipc.postMessage === 'function') {{
                            window.ipc.postMessage(payload);
                        }} else if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc) {{
                            window.webkit.messageHandlers.ipc.postMessage(payload);
                        }}
                    }} catch(e) {{}}
                }};

                window.chrome = window.chrome || {{}};
                window.chrome.webview = {{
                    postMessage: postToNative,
                    addEventListener: function(name, listener) {{
                        if (name === 'message' && typeof listener === 'function') {{
                            listeners.push(listener);
                        }}
                    }},
                    removeEventListener: function(name, listener) {{
                        const idx = listeners.indexOf(listener);
                        if (idx !== -1) listeners.splice(idx, 1);
                    }},
                    __dispatchMessage: function(dataStr) {{
                        const ev = {{ data: dataStr }};
                        if (window.qt && window.qt.webChannelTransport && typeof window.qt.webChannelTransport.onmessage === 'function') {{
                            try {{ window.qt.webChannelTransport.onmessage(ev); }} catch(e) {{}}
                        }}
                        for (let i = 0; i < listeners.length; i++) {{
                            try {{ listeners[i](ev); }} catch(e) {{}}
                        }}
                    }}
                }};

                window.qt = {{
                    webChannelTransport: {{
                        send: postToNative
                    }}
                }};

                document.addEventListener('click', function(event) {{
                    const link = event.target && event.target.closest ? event.target.closest('a[href]') : null;
                    const href = link && link.getAttribute('href');
                    if (href && (href.startsWith('data:application/octet-stream;charset=utf-8;base64,') || /^vlc:/i.test(href))) {{
                        event.preventDefault();
                        postToNative(JSON.stringify({{ id: 1, args: ['play-external', href] }}));
                    }}
                }}, true);

                window.addEventListener('load', function() {{
                    if (typeof initShellComm === 'function') {{
                        try {{ initShellComm(); }} catch(e) {{}}
                    }}
                }}, false);

                try {{
                    const applyInitialTheme = function() {{
                        var s = document.getElementById('activeTheme') || document.createElement('style');
                        s.id = 'activeTheme';
                        s.textContent = {theme_css_json};
                        (document.head || document.documentElement).appendChild(s);
                    }};
                    if (document.head || document.documentElement) {{
                        applyInitialTheme();
                    }} else {{
                        document.addEventListener('DOMContentLoaded', applyInitialTheme);
                    }}
                }} catch(e) {{}}
            }} catch(e) {{
                console.error('[macOS Shell] Initialization script error:', e);
            }}
        }})();
        {ENHANCED_SCRIPT}
        "##
    );

    let initial_endpoint = if opt.development {
        DEV_ENDPOINT.to_string()
    } else if opt.staging {
        STA_ENDPOINT.to_string()
    } else {
        opt.webui_url.clone()
    };

    // Build transparent WKWebView FIRST (because wry replaces ns_window.contentView with WryWebViewParent)
    let ipc_in_handler = ipc_in_tx.clone();
    let web_out_drag = web_out_tx.clone();
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_autoplay(true)
        .with_initialization_script(&init_script)
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            ipc_in_handler.send(req.into_body()).ok();
        })
        .with_drag_drop_handler(move |event| {
            if let DragDropEvent::Drop { paths, .. } = event {
                if let Some(first) = paths.first().and_then(|p| p.to_str()) {
                    web_out_drag
                        .send(RPCResponse::response_message(Some(json!([
                            "dragdrop",
                            [first]
                        ]))))
                        .ok();
                }
            }
            true
        })
        .with_url(&initial_endpoint)
        .build(&window)?;

    // Now attach our CAOpenGLLayer-backed mpv_view into WryWebViewParent directly BELOW WryWebView!
    let mpv_layer_ptr: i64 = unsafe {
        use cocoa::base::id;
        use cocoa::foundation::NSRect;
        use objc::{class, msg_send, sel, sel_impl};

        let ns_window: id = window.ns_window() as id;
        let content_view: id = msg_send![ns_window, contentView];
        let _: () = msg_send![content_view, setWantsLayer: true];
        let bounds: NSRect = msg_send![content_view, bounds];

        let mpv_view: id = msg_send![class!(NSView), alloc];
        let mpv_view: id = msg_send![mpv_view, initWithFrame: bounds];
        let scale_factor = window.scale_factor();
        let gl_layer = macos_gl::create_mpv_opengl_layer(scale_factor);
        let _: () = msg_send![mpv_view, setLayer: gl_layer];
        let _: () = msg_send![mpv_view, setWantsLayer: true];
        // NSViewWidthSizable (2) | NSViewHeightSizable (16) = 18
        let _: () = msg_send![mpv_view, setAutoresizingMask: 18u64];

        // Position mpv_view at the bottom of WryWebViewParent (NSWindowBelow = -1)
        let nil_view: id = std::ptr::null_mut();
        let _: () =
            msg_send![content_view, addSubview: mpv_view positioned: -1isize relativeTo: nil_view];
        println!(
            "[macOS Shell] Attached embedded MPV OpenGL layer {:?} inside contentView {:?}",
            gl_layer, content_view
        );
        gl_layer as i64
    };

    // Initialize MPV Player with the CAOpenGLLayer handle
    let mut player = Player::default();
    player.init(mpv_layer_ptr);

    let (player_tx, player_rx) = player
        .channel
        .borrow()
        .clone()
        .expect("Cannot obtain communication channel for Player");

    // Forward MPV player events to WebUI
    let web_out_player = web_out_tx.clone();
    thread::spawn(move || {
        while let Ok(msg) = player_rx.recv() {
            web_out_player.send(msg).ok();
        }
    });

    // Manage Stremio Streaming Server
    let local_server_url = Arc::new(Mutex::new(None::<String>));
    let mut server = StremioServer::default();
    let proxy_for_server_notify = proxy.clone();
    server.notify = Some(Arc::new(move || {
        proxy_for_server_notify
            .send_event(UserEvent::PollServerEvents)
            .ok();
    }));
    if !opt.development {
        server.restart();
    }

    // Discord RPC
    let discord_rpc = DiscordRpc::new(web_out_tx.clone());

    // Main Web message processing thread (JS -> Rust)
    let proxy_for_web = proxy.clone();
    let enhanced_clone = enhanced.clone();
    let player_tx_clone = player_tx.clone();
    let open_sender_web = open_sender.clone();
    let web_out_rpc = web_out_tx.clone();
    let local_server_for_rpc = local_server_url.clone();

    thread::spawn(move || {
        while let Ok(raw) = ipc_in_rx.recv() {
            // Check for Enhanced actions
            if let Ok(action) = serde_json::from_str::<EnhancedAction>(&raw) {
                match action {
                    EnhancedAction::GetData => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        mgr.reload();
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let data = mgr.ui_data(running);
                        if let Ok(json) = serde_json::to_string(&data) {
                            let script = format!(
                                "window.__receiveEnhancedData && window.__receiveEnhancedData({json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::OpenThemesFolder => {
                        enhanced_clone.lock().unwrap().open_themes_folder();
                    }
                    EnhancedAction::OpenPluginsFolder => {
                        enhanced_clone.lock().unwrap().open_plugins_folder();
                    }
                    EnhancedAction::OpenStreamingServerFolder => {
                        enhanced_clone
                            .lock()
                            .unwrap()
                            .open_streaming_server_folder();
                    }
                    EnhancedAction::GetServerStatus { id } => {
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let status = enhanced_clone.lock().unwrap().get_server_status(running);
                        if let Ok(json) = serde_json::to_string(&status) {
                            let script = format!(
                                "window.__receiveServerStatus && window.__receiveServerStatus({id:?}, {json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::StartStreamingServer { id } => {
                        proxy_for_web.send_event(UserEvent::RestartServer).ok();
                        let proxy_cb = proxy_for_web.clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(1000));
                            let running =
                                crate::stremio_app::stremio_server::is_streaming_server_online();
                            let script = format!(
                                "window.__receiveServerStartResult && window.__receiveServerStartResult({id:?}, {running});"
                            );
                            proxy_cb.send_event(UserEvent::ExecuteScript(script)).ok();
                        });
                    }
                    EnhancedAction::LoadUrl { url } => {
                        proxy_for_web.send_event(UserEvent::Navigate(url)).ok();
                    }
                    EnhancedAction::TogglePlugin { file_name, enabled } => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        mgr.toggle_plugin(&file_name, enabled);
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let data = mgr.ui_data(running);
                        if let Ok(json) = serde_json::to_string(&data) {
                            let script = format!(
                                "window.__receiveEnhancedData && window.__receiveEnhancedData({json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::ApplyTheme { file_name } => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        let new_theme = if file_name == "Default" {
                            None
                        } else {
                            Some(file_name.as_str())
                        };
                        let css = mgr.set_active_theme(new_theme);
                        let css_json = serde_json::to_string(&css.unwrap_or_default())
                            .unwrap_or_else(|_| "\"\"".to_string());
                        let theme_script = format!(
                            r##"
                            (function() {{
                                try {{
                                    var s = document.getElementById('activeTheme') || document.createElement('style');
                                    s.id = 'activeTheme';
                                    s.textContent = {css_json};
                                    (document.head || document.documentElement).appendChild(s);
                                }} catch(e) {{}}
                            }})();
                            "##
                        );
                        proxy_for_web
                            .send_event(UserEvent::ExecuteScript(theme_script))
                            .ok();
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let data = mgr.ui_data(running);
                        if let Ok(json) = serde_json::to_string(&data) {
                            let script = format!(
                                "window.__receiveEnhancedData && window.__receiveEnhancedData({json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::Log { level, message } => match level.as_str() {
                        "error" => eprintln!("[Enhanced ERROR] {message}"),
                        "warn" => eprintln!("[Enhanced WARN] {message}"),
                        _ => println!("[Enhanced INFO] {message}"),
                    },
                    EnhancedAction::HttpGet { id, url } => {
                        let proxy = proxy_for_web.clone();
                        thread::spawn(move || {
                            match crate::stremio_app::enhanced::http_get(&url) {
                                Ok((status, body)) => {
                                    let body_json = serde_json::to_string(&body)
                                        .unwrap_or_else(|_| "\"\"".to_string());
                                    let script = format!(
                                        "window.__receiveHttpResponse && window.__receiveHttpResponse({id:?}, {status}, {body_json});"
                                    );
                                    proxy.send_event(UserEvent::ExecuteScript(script)).ok();
                                }
                                Err(err) => {
                                    let err_json = serde_json::to_string(&err)
                                        .unwrap_or_else(|_| "\"Error\"".to_string());
                                    let script = format!(
                                        "window.__receiveHttpResponse && window.__receiveHttpResponse({id:?}, 500, {err_json});"
                                    );
                                    proxy.send_event(UserEvent::ExecuteScript(script)).ok();
                                }
                            }
                        });
                    }
                    EnhancedAction::InstallMod {
                        mod_type,
                        file_name,
                        content,
                    } => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        if let Err(e) = mgr.install_mod(&mod_type, &file_name, &content) {
                            eprintln!("Failed to install mod {file_name}: {e}");
                        }
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let data = mgr.ui_data(running);
                        if let Ok(json) = serde_json::to_string(&data) {
                            let script = format!(
                                "window.__receiveEnhancedData && window.__receiveEnhancedData({json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::DeleteMod {
                        mod_type,
                        file_name,
                    } => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        if let Err(e) = mgr.delete_mod(&mod_type, &file_name) {
                            eprintln!("Failed to delete mod {file_name}: {e}");
                        }
                        let css = mgr.active_theme().map(|t| t.content.clone());
                        let css_json = serde_json::to_string(&css.unwrap_or_default())
                            .unwrap_or_else(|_| "\"\"".to_string());
                        let theme_script = format!(
                            r##"
                            (function() {{
                                try {{
                                    var s = document.getElementById('activeTheme') || document.createElement('style');
                                    s.id = 'activeTheme';
                                    s.textContent = {css_json};
                                    (document.head || document.documentElement).appendChild(s);
                                }} catch(e) {{}}
                            }})();
                            "##
                        );
                        proxy_for_web
                            .send_event(UserEvent::ExecuteScript(theme_script))
                            .ok();
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let data = mgr.ui_data(running);
                        if let Ok(json) = serde_json::to_string(&data) {
                            let script = format!(
                                "window.__receiveEnhancedData && window.__receiveEnhancedData({json});"
                            );
                            proxy_for_web
                                .send_event(UserEvent::ExecuteScript(script))
                                .ok();
                        }
                    }
                    EnhancedAction::PrepareServerUpdate { version } => {
                        proxy_for_web.send_event(UserEvent::StopServer).ok();
                        if let Err(e) = enhanced_clone
                            .lock()
                            .unwrap()
                            .prepare_server_update(&version)
                        {
                            eprintln!("Failed to prepare server update: {e}");
                        }
                    }
                    EnhancedAction::SkipServerVersion { version } => {
                        if let Err(e) = enhanced_clone.lock().unwrap().skip_server_version(&version)
                        {
                            eprintln!("Failed to skip server version: {e}");
                        }
                    }
                    EnhancedAction::DownloadFfmpeg { id } => {
                        let proxy = proxy_for_web.clone();
                        thread::spawn(move || {
                            let res =
                                crate::stremio_app::stremio_server::download_and_extract_ffmpeg();
                            let success = res.is_ok();
                            let script = format!(
                                "window.__receiveFfmpegResult && window.__receiveFfmpegResult({id:?}, {success});"
                            );
                            proxy.send_event(UserEvent::ExecuteScript(script)).ok();
                        });
                    }
                }
                continue;
            }

            // Check for standard Stremio RPC requests
            if let Ok(msg) = serde_json::from_str::<RPCRequest>(&raw) {
                let local_server = local_server_for_rpc.lock().unwrap().clone();
                match msg.get_method() {
                    None if msg.is_handshake() => {
                        web_out_rpc
                            .send(RPCResponse::get_handshake(local_server.as_deref()))
                            .ok();
                    }
                    Some("app-ready") => {
                        println!("[macOS Shell] Web UI reported app-ready.");
                        web_out_rpc
                            .send(RPCResponse::visibility_change(true, 1, false))
                            .ok();
                        open_sender_web
                            .send(crate::stremio_app::open_media::OpenRequest::Ready)
                            .ok();
                    }
                    Some("pick-cache-directory") => {
                        if let Some(request) = msg.get_params().and_then(|params| {
                            serde_json::from_value::<CacheDirectoryRequest>(params.clone()).ok()
                        }) {
                            let out_tx = web_out_rpc.clone();
                            thread::spawn(move || {
                                let output = Command::new("osascript")
                                    .args([
                                        "-e",
                                        "POSIX path of (choose folder with prompt \"Select Stremio Cache Folder\")",
                                    ])
                                    .output();
                                let res = match output {
                                    Ok(o) if o.status.success() => {
                                        let path =
                                            String::from_utf8_lossy(&o.stdout).trim().to_string();
                                        if path.is_empty() {
                                            Ok(None)
                                        } else {
                                            Ok(Some(path))
                                        }
                                    }
                                    _ => Ok(None),
                                };
                                out_tx
                                    .send(RPCResponse::cache_directory_selected(
                                        request.request_id,
                                        res,
                                    ))
                                    .ok();
                            });
                        }
                    }
                    Some("win-set-interface-scale") => {
                        if let Some(scale) = msg
                            .get_params()
                            .and_then(|params| params.get("scale"))
                            .and_then(|value| value.as_u64())
                            .filter(|scale| (75..=175).contains(scale))
                        {
                            proxy_for_web
                                .send_event(UserEvent::SetInterfaceScale(scale))
                                .ok();
                        }
                    }
                    Some("win-set-visibility") => {
                        if let Some(fullscreen) = msg
                            .get_params()
                            .and_then(|params| params.get("fullscreen"))
                            .and_then(|v| v.as_bool())
                        {
                            proxy_for_web
                                .send_event(UserEvent::SetFullscreen(fullscreen))
                                .ok();
                        }
                    }
                    Some("win-focus") => {
                        proxy_for_web.send_event(UserEvent::FocusWindow).ok();
                    }
                    Some("quit") => {
                        proxy_for_web.send_event(UserEvent::Quit).ok();
                    }
                    Some("open-external") => {
                        if let Some(arg) = msg.get_params().and_then(|v| v.as_str()) {
                            if let Some(url) = safe_url(arg) {
                                open::that(url).ok();
                            }
                        }
                    }
                    Some("play-external") => {
                        if let Some(arg) = msg.get_params().and_then(|v| v.as_str()) {
                            let _ = crate::stremio_app::external_player::play(arg);
                        }
                    }
                    Some("discord-connect") => {
                        if let Err(e) = discord_rpc.connect() {
                            eprintln!("Discord connect error: {e}");
                            web_out_rpc.send(RPCResponse::discord_status(false)).ok();
                        }
                    }
                    Some("discord-disconnect") => {
                        let _ = discord_rpc.disconnect();
                        web_out_rpc.send(RPCResponse::discord_status(false)).ok();
                    }
                    Some("discord-set-activity") => {
                        if let Some(params) = msg.get_params() {
                            let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("");
                            let details =
                                params.get("details").and_then(|v| v.as_str()).unwrap_or("");
                            let image = params.get("image").and_then(|v| v.as_str());
                            let start_ts = params.get("startTimestamp").and_then(|v| v.as_i64());
                            let end_ts = params.get("endTimestamp").and_then(|v| v.as_i64());
                            let _ =
                                discord_rpc.set_activity(state, details, image, start_ts, end_ts);
                        }
                    }
                    Some("discord-clear-activity") => {
                        let _ = discord_rpc.clear_activity();
                    }
                    Some(player_cmd) if player_cmd.starts_with("mpv-") => {
                        if let Some(args) = msg.args {
                            if let Ok(resp_json) = serde_json::to_string(&args) {
                                player_tx_clone.send(resp_json).ok();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    let webui_url_base = opt.webui_url.clone();
    let mut current_endpoint = initial_endpoint;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(user_event) => match user_event {
                UserEvent::ExecuteScript(script) => {
                    let _ = webview.evaluate_script(&script);
                }
                UserEvent::DispatchWebMessage(msg) => {
                    if let Ok(escaped) = serde_json::to_string(&msg) {
                        let js = format!(
                            "if (window.chrome && window.chrome.webview && window.chrome.webview.__dispatchMessage) {{ window.chrome.webview.__dispatchMessage({escaped}); }}"
                        );
                        let _ = webview.evaluate_script(&js);
                    }
                }
                UserEvent::Navigate(url) => {
                    let same_base =
                        current_endpoint.split('#').next() == url.split('#').next();
                    current_endpoint = url.clone();
                    if same_base && url.contains('#') {
                        let fragment = url.split_once('#').map(|(_, h)| h).unwrap_or("");
                        let js = format!(
                            "window.location.hash = {}; window.location.reload();",
                            json!(fragment)
                        );
                        let _ = webview.evaluate_script(&js);
                    } else {
                        let _ = webview.load_url(&url);
                    }
                }
                UserEvent::SetFullscreen(fs) => {
                    window.set_fullscreen(if fs {
                        Some(tao::window::Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                    let visibility = if fs { 3 } else { 1 };
                    web_out_tx
                        .send(RPCResponse::visibility_change(true, visibility, fs))
                        .ok();
                }
                UserEvent::SetInterfaceScale(scale) => {
                    let _ = webview.zoom(scale as f64 / 100.0);
                }
                UserEvent::FocusWindow => {
                    window.set_focus();
                }
                UserEvent::PollServerEvents => {
                    for srv_event in server.events() {
                        match srv_event {
                            ServerEvent::Ready(endpoint) => {
                                println!("[macOS Shell] Streaming server ready at {endpoint}");
                                *local_server_url.lock().unwrap() = Some(endpoint.clone());
                                let target_url = if webui_url_base.trim_end_matches('/')
                                    == WEB_ENDPOINT.trim_end_matches('/')
                                {
                                    web_endpoint_with_streaming_server(&endpoint)
                                } else {
                                    webui_url_base.clone()
                                };
                                let same_base = current_endpoint.split('#').next()
                                    == target_url.split('#').next();
                                current_endpoint = target_url.clone();
                                if same_base && target_url.contains('#') {
                                    let fragment =
                                        target_url.split_once('#').map(|(_, h)| h).unwrap_or("");
                                    let js = format!(
                                        "window.location.hash = {}; window.location.reload();",
                                        json!(fragment)
                                    );
                                    let _ = webview.evaluate_script(&js);
                                } else {
                                    let _ = webview.load_url(&target_url);
                                }
                            }
                            ServerEvent::Failed(err) => {
                                eprintln!("[macOS Shell] Streaming server failed: {err}");
                            }
                        }
                    }
                }
                UserEvent::RestartServer => {
                    server.restart();
                }
                UserEvent::StopServer => {
                    server.stop();
                }
                UserEvent::Quit => {
                    server.stop();
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(_) => unsafe {
                    macos_gl::update_mpv_layer_scale(window.scale_factor());
                },
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => unsafe {
                    macos_gl::update_mpv_layer_scale(scale_factor);
                },
                WindowEvent::CloseRequested => {
                    server.stop();
                    let size = window.inner_size();
                    let settings = WindowSettings {
                        x: 0.0,
                        y: 0.0,
                        width: size.width as f64,
                        height: size.height as f64,
                        maximized: window.is_maximized(),
                    };
                    let _ = settings.save();
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            _ => {}
        }
    });
}
