use std::{
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use tao::event_loop::EventLoopProxy;

use crate::stremio_app::{
    discord::DiscordRpc,
    enhanced::{EnhancedAction, EnhancedManager},
    ipc::{CacheDirectoryRequest, RPCRequest, RPCResponse},
    macos::app::UserEvent,
    open_media::OpenRequest,
    stremio_server,
};

/// Shared channels and state needed by the WebView JS -> Rust IPC worker thread.
pub struct IpcContext {
    pub proxy: EventLoopProxy<UserEvent>,
    pub enhanced: Arc<Mutex<EnhancedManager>>,
    pub player_tx: flume::Sender<String>,
    pub open_sender: flume::Sender<OpenRequest>,
    pub web_out_tx: flume::Sender<String>,
    pub local_server_url: Arc<Mutex<Option<String>>>,
    pub discord_rpc: DiscordRpc,
}

/// Spawns the background thread that receives JSON messages posted from WKWebView.
pub fn spawn_ipc_listener(ipc_in_rx: flume::Receiver<String>, ctx: IpcContext) {
    thread::spawn(move || {
        while let Ok(raw) = ipc_in_rx.recv() {
            if let Ok(action) = serde_json::from_str::<EnhancedAction>(&raw) {
                handle_enhanced_action(action, &ctx);
                continue;
            }

            if let Ok(msg) = serde_json::from_str::<RPCRequest>(&raw) {
                handle_rpc_request(msg, &ctx);
            }
        }
    });
}

fn push_enhanced_data(mgr: &EnhancedManager, proxy: &EventLoopProxy<UserEvent>) {
    let running = stremio_server::is_streaming_server_online();
    let data = mgr.ui_data(running);
    if let Ok(json) = serde_json::to_string(&data) {
        let script =
            format!("window.__receiveEnhancedData && window.__receiveEnhancedData({json});");
        proxy.send_event(UserEvent::ExecuteScript(script)).ok();
    }
}

fn push_theme_css(css: Option<&str>, proxy: &EventLoopProxy<UserEvent>) {
    let css_json =
        serde_json::to_string(css.unwrap_or_default()).unwrap_or_else(|_| "\"\"".to_string());
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
    proxy
        .send_event(UserEvent::ExecuteScript(theme_script))
        .ok();
}

fn handle_enhanced_action(action: EnhancedAction, ctx: &IpcContext) {
    match action {
        EnhancedAction::GetData => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.reload();
            push_enhanced_data(&mgr, &ctx.proxy);
        }
        EnhancedAction::OpenThemesFolder => {
            ctx.enhanced.lock().unwrap().open_themes_folder();
        }
        EnhancedAction::OpenPluginsFolder => {
            ctx.enhanced.lock().unwrap().open_plugins_folder();
        }
        EnhancedAction::OpenStreamingServerFolder => {
            ctx.enhanced.lock().unwrap().open_streaming_server_folder();
        }
        EnhancedAction::GetServerStatus { id } => {
            let running = stremio_server::is_streaming_server_online();
            let status = ctx.enhanced.lock().unwrap().get_server_status(running);
            if let Ok(json) = serde_json::to_string(&status) {
                let script = format!(
                    "window.__receiveServerStatus && window.__receiveServerStatus({id:?}, {json});"
                );
                ctx.proxy.send_event(UserEvent::ExecuteScript(script)).ok();
            }
        }
        EnhancedAction::StartStreamingServer { id } => {
            if stremio_server::is_streaming_server_online() {
                let script = format!(
                    "window.__receiveServerStartResult && window.__receiveServerStartResult({id:?}, true);"
                );
                ctx.proxy.send_event(UserEvent::ExecuteScript(script)).ok();
                return;
            }
            ctx.proxy.send_event(UserEvent::RestartServer).ok();
            let proxy_cb = ctx.proxy.clone();
            thread::spawn(move || {
                let mut running = false;
                for _ in 0..40 {
                    thread::sleep(Duration::from_millis(250));
                    if stremio_server::is_streaming_server_online() {
                        running = true;
                        break;
                    }
                }
                let script = format!(
                    "window.__receiveServerStartResult && window.__receiveServerStartResult({id:?}, {running});"
                );
                proxy_cb.send_event(UserEvent::ExecuteScript(script)).ok();
            });
        }
        EnhancedAction::LoadUrl { url } => {
            ctx.proxy.send_event(UserEvent::Navigate(url)).ok();
        }
        EnhancedAction::TogglePlugin { file_name, enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.toggle_plugin(&file_name, enabled);
            push_enhanced_data(&mgr, &ctx.proxy);
        }
        EnhancedAction::ApplyTheme { file_name } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            let new_theme = (file_name != "Default").then_some(file_name.as_str());
            let css = mgr.set_active_theme(new_theme);
            push_theme_css(css.as_deref(), &ctx.proxy);
            push_enhanced_data(&mgr, &ctx.proxy);
        }
        EnhancedAction::Log { level, message } => match level.as_str() {
            "error" => eprintln!("[Enhanced ERROR] {message}"),
            "warn" => eprintln!("[Enhanced WARN] {message}"),
            _ => println!("[Enhanced INFO] {message}"),
        },
        EnhancedAction::HttpGet { id, url } => {
            let proxy = ctx.proxy.clone();
            thread::spawn(move || {
                let (status, payload) = match crate::stremio_app::enhanced::http_get(&url) {
                    Ok((status, body)) => (status, body),
                    Err(err) => (500, err),
                };
                let payload_json =
                    serde_json::to_string(&payload).unwrap_or_else(|_| "\"\"".to_string());
                let script = format!(
                    "window.__receiveHttpResponse && window.__receiveHttpResponse({id:?}, {status}, {payload_json});"
                );
                proxy.send_event(UserEvent::ExecuteScript(script)).ok();
            });
        }
        EnhancedAction::InstallMod {
            mod_type,
            file_name,
            content,
        } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            if let Err(e) = mgr.install_mod(&mod_type, &file_name, &content) {
                eprintln!("Failed to install mod {file_name}: {e}");
            }
            push_enhanced_data(&mgr, &ctx.proxy);
        }
        EnhancedAction::DeleteMod {
            mod_type,
            file_name,
        } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            if let Err(e) = mgr.delete_mod(&mod_type, &file_name) {
                eprintln!("Failed to delete mod {file_name}: {e}");
            }
            let css = mgr.active_theme().map(|t| t.content.clone());
            push_theme_css(css.as_deref(), &ctx.proxy);
            push_enhanced_data(&mgr, &ctx.proxy);
        }
        EnhancedAction::PrepareServerUpdate { version } => {
            ctx.proxy.send_event(UserEvent::StopServer).ok();
            if let Err(e) = ctx.enhanced.lock().unwrap().prepare_server_update(&version) {
                eprintln!("Failed to prepare server update: {e}");
            }
        }
        EnhancedAction::SkipServerVersion { version } => {
            if let Err(e) = ctx.enhanced.lock().unwrap().skip_server_version(&version) {
                eprintln!("Failed to skip server version: {e}");
            }
        }
        EnhancedAction::SkipClientVersion { version } => {
            if let Err(e) = ctx.enhanced.lock().unwrap().skip_client_version(&version) {
                eprintln!("Failed to skip client version: {e}");
            }
        }
        EnhancedAction::DownloadFfmpeg { id } => {
            let proxy = ctx.proxy.clone();
            thread::spawn(move || {
                let success = stremio_server::download_and_extract_ffmpeg().is_ok();
                let script = format!(
                    "window.__receiveFfmpegResult && window.__receiveFfmpegResult({id:?}, {success});"
                );
                proxy.send_event(UserEvent::ExecuteScript(script)).ok();
            });
        }
    }
}

fn handle_rpc_request(msg: RPCRequest, ctx: &IpcContext) {
    let local_server = ctx.local_server_url.lock().unwrap().clone();
    match msg.get_method() {
        None if msg.is_handshake() => {
            ctx.web_out_tx
                .send(RPCResponse::get_handshake(local_server.as_deref()))
                .ok();
        }
        Some("app-ready") => {
            println!("[macOS Shell] Web UI reported app-ready.");
            ctx.web_out_tx
                .send(RPCResponse::visibility_change(true, 1, false))
                .ok();
            ctx.open_sender.send(OpenRequest::Ready).ok();
        }
        Some("pick-cache-directory") => {
            let Some(request) = msg.get_params().and_then(|params| {
                serde_json::from_value::<CacheDirectoryRequest>(params.clone()).ok()
            }) else {
                return;
            };
            if local_server.as_deref() != Some(request.server_url.as_str()) {
                ctx.web_out_tx
                    .send(RPCResponse::cache_directory_selected(
                        request.request_id,
                        Err(
                            "The folder picker is only available for the shell's local server"
                                .to_owned(),
                        ),
                    ))
                    .ok();
                return;
            }
            pick_cache_directory(request, ctx.web_out_tx.clone());
        }
        Some("win-set-interface-scale") => {
            if let Some(scale) = msg
                .get_params()
                .and_then(|params| params.get("scale"))
                .and_then(|value| value.as_u64())
                .filter(|scale| (75..=175).contains(scale))
            {
                ctx.proxy
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
                ctx.proxy
                    .send_event(UserEvent::SetFullscreen(fullscreen))
                    .ok();
            }
        }
        Some("win-focus") => {
            ctx.proxy.send_event(UserEvent::FocusWindow).ok();
        }
        Some("screensaver-toggle") => {
            if let Some(disabled) = msg
                .get_params()
                .and_then(|params| params.get("disabled"))
                .and_then(|v| v.as_bool())
            {
                crate::stremio_app::stremio_player::macos_gl::set_webui_sleep_inhibited(disabled);
            }
        }
        Some("set-window-mode") => {
            if let Some(mode) = msg.get_params().and_then(|v| v.as_str()) {
                crate::stremio_app::stremio_player::macos_gl::set_webui_sleep_inhibited(
                    mode == "player",
                );
            }
        }
        Some("quit") => {
            ctx.proxy.send_event(UserEvent::Quit).ok();
        }
        Some("open-external") => {
            if let Some(url) = msg
                .get_params()
                .and_then(|v| v.as_str())
                .and_then(crate::stremio_app::constants::safe_url)
            {
                open::that(url).ok();
            }
        }
        Some("play-external") => {
            if let Some(arg) = msg.get_params().and_then(|v| v.as_str()) {
                let _ = crate::stremio_app::external_player::play(arg);
            }
        }
        Some("discord-connect") => {
            if let Err(e) = ctx.discord_rpc.connect() {
                eprintln!("Discord connect error: {e}");
                ctx.web_out_tx.send(RPCResponse::discord_status(false)).ok();
            }
        }
        Some("discord-disconnect") => {
            let _ = ctx.discord_rpc.disconnect();
            ctx.web_out_tx.send(RPCResponse::discord_status(false)).ok();
        }
        Some("discord-set-activity") => {
            if let Some(params) = msg.get_params() {
                let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let details = params.get("details").and_then(|v| v.as_str()).unwrap_or("");
                let image = params.get("image").and_then(|v| v.as_str());
                let start_ts = params.get("startTimestamp").and_then(|v| v.as_i64());
                let end_ts = params.get("endTimestamp").and_then(|v| v.as_i64());
                let _ = ctx
                    .discord_rpc
                    .set_activity(state, details, image, start_ts, end_ts);
            }
        }
        Some("discord-clear-activity") => {
            let _ = ctx.discord_rpc.clear_activity();
        }
        Some(player_cmd) if player_cmd.starts_with("mpv-") => {
            if let Some(args) = msg.args {
                if let Ok(resp_json) = serde_json::to_string(&args) {
                    ctx.player_tx.send(resp_json).ok();
                }
            }
        }
        _ => {}
    }
}

fn pick_cache_directory(request: CacheDirectoryRequest, out_tx: flume::Sender<String>) {
    thread::spawn(move || {
        let script = match request
            .directory
            .filter(|d| Path::new(d).is_dir())
        {
            Some(dir) => format!(
                "POSIX path of (choose folder with prompt \"Select Stremio Cache Folder\" default location (POSIX file {:?}))",
                dir
            ),
            None => "POSIX path of (choose folder with prompt \"Select Stremio Cache Folder\")"
                .to_string(),
        };
        let res = match Command::new("osascript").args(["-e", &script]).output() {
            Ok(o) if o.status.success() => {
                let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
                Ok((!path.is_empty()).then_some(path))
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
