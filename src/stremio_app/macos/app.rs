use std::{
    io::Read,
    str,
    sync::{Arc, Mutex},
    thread,
};

use serde_json::json;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::WindowExtMacOS,
    window::{Fullscreen, Window, WindowBuilder},
};
use wry::{DragDropEvent, WebView, WebViewBuilder};

use crate::stremio_app::{
    constants::{
        web_endpoint_with_streaming_server, APP_NAME, DEV_ENDPOINT, SERVER_IPC_KEY, STA_ENDPOINT,
        WEB_ENDPOINT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    },
    discord::DiscordRpc,
    enhanced::{EnhancedManager, ENHANCED_SCRIPT},
    ipc::RPCResponse,
    macos::ipc_handler::{spawn_ipc_listener, IpcContext},
    open_media::{self, OpenRequest},
    stremio_player::{
        macos_gl,
        thumbnailer::{spawn_thumbnail_worker, ThumbnailCmd},
        Player,
    },
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

fn build_init_script(
    server_ipc_key: &str,
    initial_data_json: &str,
    theme_css_json: &str,
) -> String {
    // Note: wry defines window.ipc via Object.defineProperty(..., Object.freeze({ postMessage })).
    // Preserve window.ipc and layer window.chrome.webview + window.qt.webChannelTransport on top.
    format!(
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
        try {{
        {ENHANCED_SCRIPT}
        }} catch(e) {{
            console.error('[Enhanced] Script error:', e);
        }}
        "##
    )
}

fn navigate_webview(webview: &WebView, current_endpoint: &mut String, target_url: String) {
    let same_base = current_endpoint.split('#').next() == target_url.split('#').next();
    *current_endpoint = target_url.clone();
    if same_base && target_url.contains('#') {
        let fragment = target_url.split_once('#').map(|(_, h)| h).unwrap_or("");
        let js = format!(
            "window.location.hash = {}; window.location.reload();",
            json!(fragment)
        );
        let _ = webview.evaluate_script(&js);
    } else {
        let _ = webview.load_url(&target_url);
    }
}

fn shutdown_app(
    server: &mut StremioServer,
    player: &mut Option<Player>,
    thumbnail_tx: &flume::Sender<ThumbnailCmd>,
    window: &Window,
    shutdown_done: &mut bool,
) {
    if *shutdown_done {
        return;
    }
    *shutdown_done = true;

    let _ = thumbnail_tx.send(ThumbnailCmd::SetEnabled(false));

    let size = window.inner_size();
    let settings = WindowSettings {
        x: 0.0,
        y: 0.0,
        width: size.width as f64,
        height: size.height as f64,
        maximized: window.is_maximized(),
    };
    let _ = settings.save();

    drop(player.take());
    server.stop();
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

    let (ipc_in_tx, ipc_in_rx) = flume::unbounded::<String>();
    let (web_out_tx, web_out_rx) = flume::unbounded::<String>();

    let proxy_for_out = proxy.clone();
    thread::spawn(move || {
        while let Ok(msg) = web_out_rx.recv() {
            proxy_for_out
                .send_event(UserEvent::DispatchWebMessage(msg))
                .ok();
        }
    });

    // Open media coordinator
    let (open_sender, open_receiver) = flume::unbounded::<OpenRequest>();
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
                OpenRequest::Input(input) => pending = Some(input),
                OpenRequest::Ready => ready = true,
                OpenRequest::Reset => ready = false,
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
                            .send(OpenRequest::Input(s.to_string()))
                            .ok();
                    }
                }
            }
        });
    }

    // Initialize Enhanced Manager & build WebView script
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
    let init_script = build_init_script(&server_ipc_key, &initial_data_json, &theme_css_json);

    let server_already_online =
        !opt.development && crate::stremio_app::stremio_server::is_streaming_server_online();

    let initial_endpoint = if opt.development {
        DEV_ENDPOINT.to_string()
    } else if opt.staging {
        STA_ENDPOINT.to_string()
    } else if server_already_online
        && opt.webui_url.trim_end_matches('/') == WEB_ENDPOINT.trim_end_matches('/')
    {
        web_endpoint_with_streaming_server("http://127.0.0.1:11470")
    } else {
        opt.webui_url.clone()
    };

    // Build transparent WKWebView FIRST (wry replaces ns_window.contentView with WryWebViewParent)
    let ipc_in_handler = ipc_in_tx.clone();
    let web_out_drag = web_out_tx.clone();
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_autoplay(true)
        .with_initialization_script(&init_script)
        .with_new_window_req_handler(|url| {
            if url.starts_with("https://") || url.starts_with("http://") {
                let _ = open::that(&url);
            }
            false
        })
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

    // Attach CAOpenGLLayer-backed mpv view below WKWebView on the main thread before Player::init
    let mpv_layer_ptr =
        unsafe { macos_gl::attach_layer_to_window(window.ns_window(), window.scale_factor()) };

    let mut player = Player::default();
    player.init(mpv_layer_ptr);

    let (player_tx, player_rx) = player
        .channel
        .borrow()
        .clone()
        .expect("Cannot obtain communication channel for Player");

    let (thumb_script_tx, thumb_script_rx) = flume::unbounded::<String>();
    let proxy_for_thumb = proxy.clone();
    thread::spawn(move || {
        while let Ok(script) = thumb_script_rx.recv() {
            proxy_for_thumb
                .send_event(UserEvent::ExecuteScript(script))
                .ok();
        }
    });

    let initial_thumb_enabled = enhanced.lock().unwrap().settings.thumbnail_preview;
    let (thumbnail_tx, _thumbnail_handle) =
        spawn_thumbnail_worker(thumb_script_tx, initial_thumb_enabled);
    let active_stream_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let web_out_player = web_out_tx.clone();
    let thumbnail_tx_player = thumbnail_tx.clone();
    let active_stream_url_player = active_stream_url.clone();
    thread::spawn(move || {
        while let Ok(msg) = player_rx.recv() {
            if msg.contains("mpv-event-video-ready") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg) {
                    let is_ready = v
                        .get("args")
                        .and_then(|a| a.get(1))
                        .and_then(|p| p.get("ready"))
                        .and_then(|r| r.as_bool())
                        .unwrap_or(false);
                    if is_ready {
                        let url = active_stream_url_player.lock().unwrap().clone();
                        let _ = thumbnail_tx_player.send(ThumbnailCmd::SetSource(url));
                    }
                }
            } else if msg.contains("mpv-event-ended") {
                let _ = thumbnail_tx_player.send(ThumbnailCmd::SetSource(None));
            }
            web_out_player.send(msg).ok();
        }
    });

    // Manage Stremio Streaming Server
    let local_server_url = Arc::new(Mutex::new(
        server_already_online.then(|| "http://127.0.0.1:11470/".to_string()),
    ));
    let mut server = StremioServer::default();
    let proxy_for_server_notify = proxy.clone();
    server.notify = Some(Arc::new(move || {
        proxy_for_server_notify
            .send_event(UserEvent::PollServerEvents)
            .ok();
    }));
    if !opt.development && !server_already_online {
        server.restart();
    } else if server_already_online {
        println!(
            "[macOS Shell] Streaming server already listening on 127.0.0.1:11470; using existing instance."
        );
    }

    let discord_rpc = DiscordRpc::new(web_out_tx.clone());
    spawn_ipc_listener(
        ipc_in_rx,
        IpcContext {
            proxy,
            enhanced,
            player_tx,
            thumbnail_tx: thumbnail_tx.clone(),
            active_stream_url,
            open_sender: open_sender.clone(),
            web_out_tx: web_out_tx.clone(),
            local_server_url: local_server_url.clone(),
            discord_rpc,
        },
    );

    let webui_url_base = opt.webui_url.clone();
    let mut current_endpoint = initial_endpoint;
    let mut player = Some(player);
    let mut shutdown_done = false;

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
                    open_sender.send(OpenRequest::Reset).ok();
                    navigate_webview(&webview, &mut current_endpoint, url);
                }
                UserEvent::SetFullscreen(fs) => {
                    window.set_fullscreen(fs.then_some(Fullscreen::Borderless(None)));
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
                                open_sender.send(OpenRequest::Reset).ok();
                                let target_url = if webui_url_base.trim_end_matches('/')
                                    == WEB_ENDPOINT.trim_end_matches('/')
                                {
                                    web_endpoint_with_streaming_server(&endpoint)
                                } else {
                                    webui_url_base.clone()
                                };
                                navigate_webview(&webview, &mut current_endpoint, target_url);
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
                    shutdown_app(
                        &mut server,
                        &mut player,
                        &thumbnail_tx,
                        &window,
                        &mut shutdown_done,
                    );
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
                    shutdown_app(
                        &mut server,
                        &mut player,
                        &thumbnail_tx,
                        &window,
                        &mut shutdown_done,
                    );
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            Event::LoopDestroyed => {
                shutdown_app(
                    &mut server,
                    &mut player,
                    &thumbnail_tx,
                    &window,
                    &mut shutdown_done,
                );
            }
            _ => {}
        }
    });
}
