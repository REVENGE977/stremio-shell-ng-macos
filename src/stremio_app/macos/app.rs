use std::{
    cell::RefCell,
    collections::VecDeque,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    str,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use flume::{Receiver, Sender};
use serde_json::json;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder},
    platform::macos::WindowExtMacOS,
    window::{Window, WindowBuilder},
};
use url::Url;
use wry::{WebView, WebViewBuilder};

use crate::stremio_app::{
    constants::{
        safe_url, web_endpoint_with_streaming_server, APP_NAME, DEV_ENDPOINT, SERVER_IPC_KEY,
        STA_ENDPOINT, WEB_ENDPOINT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    },
    discord::DiscordRpc,
    enhanced::{EnhancedAction, EnhancedManager, ENHANCED_SCRIPT},
    ipc::{CacheDirectoryRequest, RPCRequest, RPCResponse},
    open_media,
    stremio_player::Player,
    stremio_server::{ServerEvent, StremioServer},
    window_settings::WindowSettings,
    PipeServer,
};

pub enum UserEvent {
    ExecuteScript(String),
    Navigate(String),
    SetFullscreen(bool),
    Quit,
}

pub fn run_macos(
    opt: crate::Opt,
    mut commands_server: Option<PipeServer>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("[macOS Shell] Initializing Stremio Enhanced macOS runtime...");

    // Create event loop
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Load window settings or use default size
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

    // Create native window
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

    // Setup native MPV view inside NSWindow content view
    let mpv_view_ptr: isize = unsafe {
        use cocoa::appkit::NSView;
        use cocoa::base::id;
        use cocoa::foundation::NSRect;
        use objc::{msg_send, sel, sel_impl};

        let content_view: id = window.ns_view() as id;
        let () = msg_send![content_view, setWantsLayer: true];
        let bounds: NSRect = msg_send![content_view, bounds];

        // Allocate and initialize custom NSView for mpv
        let mpv_view: id = msg_send![objc::class!(NSView), alloc];
        let mpv_view: id = msg_send![mpv_view, initWithFrame: bounds];
        let () = msg_send![mpv_view, setWantsLayer: true];
        // NSViewWidthSizable (2) | NSViewHeightSizable (16) = 18
        let () = msg_send![mpv_view, setAutoresizingMask: 18u64];

        // Add mpv_view positioned below other subviews (NSWindowBelow = -1)
        let () = msg_send![content_view, addSubview: mpv_view positioned: -1isize relativeTo: 0isize];
        println!("[macOS Shell] Created embedded mpv NSView: {:?}", mpv_view);
        mpv_view as isize
    };

    // Initialize player with the mpv NSView pointer
    let mut player = Player::default();
    player.init(mpv_view_ptr as i64);

    let (player_tx, player_rx) = player
        .channel
        .borrow()
        .clone()
        .expect("Cannot obtain communication channel for Player");

    // Channels for Web communication
    let (web_tx, web_rx) = flume::unbounded::<String>();
    let web_tx_player = web_tx.clone();
    let web_tx_web = web_tx.clone();
    let web_tx_open = web_tx.clone();

    // Open media handler
    let (open_sender, open_receiver) = flume::unbounded::<crate::stremio_app::open_media::OpenRequest>();
    let command_arg = opt
        .command
        .as_deref()
        .filter(|cmd| !cmd.is_empty())
        .map(open_media::normalize)
        .unwrap_or_default();

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
                    web_tx_open
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
                            .send(crate::stremio_app::open_media::OpenRequest::Input(s.to_string()))
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
        (mgr.ui_data(running), mgr.active_theme().map(|t| t.content.clone()))
    };

    let initial_data_json = serde_json::to_string(&ui_data).unwrap_or_else(|_| "{}".to_string());
    let theme_css_json = serde_json::to_string(&active_theme_css.unwrap_or_default())
        .unwrap_or_else(|_| "\"\"".to_string());

    let server_ipc_key = std::env::var(SERVER_IPC_KEY).unwrap_or_default();

    // Prepare initialization script for WKWebView
    let init_script = format!(
        r##"
        (function() {{
            try {{
                window.stremio_server_ipc_key = {server_ipc_key:?};
                window.__STREMIO_SHELL__ = true;
                window.__INITIAL_ENHANCED_DATA__ = {initial_data_json};

                // Shim Qt WebChannel transport to route through window.ipc
                window.qt = {{
                    webChannelTransport: {{
                        send: function(msg) {{
                            try {{
                                if (window.ipc) {{
                                    window.ipc.postMessage(typeof msg === 'string' ? msg : JSON.stringify(msg));
                                }}
                            }} catch(e) {{}}
                        }}
                    }}
                }};

                // Shim Chrome WebView2 postMessage to route through window.ipc
                window.chrome = {{
                    webview: {{
                        postMessage: function(msg) {{
                            try {{
                                if (window.ipc) {{
                                    window.ipc.postMessage(typeof msg === 'string' ? msg : JSON.stringify(msg));
                                }}
                            }} catch(e) {{}}
                        }}
                    }}
                }};

                // Intercept clicks for external player triggers
                document.addEventListener('click', function(event) {{
                    const link = event.target.closest('a[href]');
                    const href = link && link.getAttribute('href');
                    if (href && (href.startsWith('data:application/octet-stream;charset=utf-8;base64,') || /^vlc:/i.test(href))) {{
                        event.preventDefault();
                        if (window.ipc) {{
                            window.ipc.postMessage(JSON.stringify({{ id: 1, args: ['play-external', href] }}));
                        }}
                    }}
                }}, true);

                // Inject active theme
                try {{
                    var s = document.createElement('style');
                    s.id = 'activeTheme';
                    s.textContent = {theme_css_json};
                    (document.head || document.documentElement).appendChild(s);
                }} catch(e) {{}}
            }} catch(e) {{
                console.error('[macOS Shell] Initialization script error:', e);
            }}
        }})();
        {ENHANCED_SCRIPT}
        "##
    );

    // Initial endpoint URL
    let initial_endpoint = if opt.development {
        DEV_ENDPOINT.to_string()
    } else if opt.staging {
        STA_ENDPOINT.to_string()
    } else {
        opt.webui_url.clone()
    };

    // Build transparent WKWebView
    let web_tx_handler = web_tx.clone();
    let webview = WebViewBuilder::new(&window)
        .with_transparent(true)
        .with_initialization_script(&init_script)
        .with_ipc_handler(move |msg: String| {
            web_tx_handler.send(msg).ok();
        })
        .with_url(&initial_endpoint)
        .build()?;

    let webview = Arc::new(Mutex::new(webview));
    let proxy_for_player = proxy.clone();

    // Forward MPV player events to WebUI
    thread::spawn(move || {
        while let Ok(msg) = player_rx.recv() {
            let escaped_msg = serde_json::to_string(&msg).unwrap_or_else(|_| "\"\"".to_string());
            let script = format!(
                "if (window.qt && window.qt.webChannelTransport && window.qt.webChannelTransport.onmessage) {{ window.qt.webChannelTransport.onmessage({{ data: {escaped_msg} }}); }}"
            );
            proxy_for_player
                .send_event(UserEvent::ExecuteScript(script))
                .ok();
        }
    });

    // Start Stremio Streaming Server
    let local_server_url = Arc::new(Mutex::new(None::<String>));
    let local_server_clone = local_server_url.clone();
    let proxy_for_server = proxy.clone();
    let webui_url_base = opt.webui_url.clone();

    if !opt.development {
        thread::spawn(move || {
            let mut server = StremioServer::default();
            server.restart();
            for event in server.events() {
                match event {
                    ServerEvent::Ready(endpoint) => {
                        println!("[macOS Shell] Streaming server ready at {endpoint}");
                        *local_server_clone.lock().unwrap() = Some(endpoint.clone());
                        let target_url = if webui_url_base.trim_end_matches('/')
                            == WEB_ENDPOINT.trim_end_matches('/')
                        {
                            web_endpoint_with_streaming_server(&endpoint)
                        } else {
                            webui_url_base.clone()
                        };
                        proxy_for_server
                            .send_event(UserEvent::Navigate(target_url))
                            .ok();
                    }
                    ServerEvent::Failed(err) => {
                        eprintln!("[macOS Shell] Streaming server failed: {err}");
                    }
                }
            }
        });
    }

    // Discord RPC
    let discord_rpc = DiscordRpc::new(web_tx.clone());

    // Main Web message processing thread
    let proxy_for_web = proxy.clone();
    let enhanced_clone = enhanced.clone();
    let player_tx_clone = player_tx.clone();
    let open_sender_web = open_sender.clone();

    thread::spawn(move || {
        while let Ok(raw) = web_rx.recv() {
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
                        let running =
                            crate::stremio_app::stremio_server::is_streaming_server_online();
                        let script = format!(
                            "window.__receiveServerStartResult && window.__receiveServerStartResult({id:?}, {running});"
                        );
                        proxy_for_web
                            .send_event(UserEvent::ExecuteScript(script))
                            .ok();
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
                        let _ = mgr.install_mod(&mod_type, &file_name, &content);
                    }
                    EnhancedAction::DeleteMod { mod_type, file_name } => {
                        let mut mgr = enhanced_clone.lock().unwrap();
                        let _ = mgr.delete_mod(&mod_type, &file_name);
                    }
                    EnhancedAction::PrepareServerUpdate { version } => {
                        let _ = enhanced_clone.lock().unwrap().prepare_server_update(&version);
                    }
                    EnhancedAction::SkipServerVersion { version } => {
                        let _ = enhanced_clone.lock().unwrap().skip_server_version(&version);
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
                let local_server = local_server_url.lock().unwrap().clone();
                match msg.get_method() {
                    None if msg.is_handshake() => {
                        let resp = RPCResponse::get_handshake(local_server.as_deref());
                        let resp_json =
                            serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
                        let script = format!(
                            "if (window.qt && window.qt.webChannelTransport && window.qt.webChannelTransport.onmessage) {{ window.qt.webChannelTransport.onmessage({{ data: {resp_json} }}); }}"
                        );
                        proxy_for_web
                            .send_event(UserEvent::ExecuteScript(script))
                            .ok();
                    }
                    Some("app-ready") => {
                        println!("[macOS Shell] Web UI reported app-ready.");
                        open_sender_web
                            .send(crate::stremio_app::open_media::OpenRequest::Ready)
                            .ok();
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
                        let _ = discord_rpc.connect();
                    }
                    Some("discord-disconnect") => {
                        let _ = discord_rpc.disconnect();
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

    // Run Tao event loop
    let webview_for_loop = webview.clone();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(user_event) => match user_event {
                UserEvent::ExecuteScript(script) => {
                    if let Ok(wv) = webview_for_loop.lock() {
                        let _ = wv.evaluate_script(&script);
                    }
                }
                UserEvent::Navigate(url) => {
                    if let Ok(wv) = webview_for_loop.lock() {
                        let _ = wv.load_url(&url);
                    }
                }
                UserEvent::SetFullscreen(fs) => {
                    window.set_fullscreen(if fs {
                        Some(tao::window::Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                }
                UserEvent::Quit => {
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
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
