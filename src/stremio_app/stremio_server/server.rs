use super::process::{ServerEvent, ServerProcess};
use std::{env, fs, path::PathBuf, process::Command, time::Duration};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct StremioServer {
    pub notify: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    process: std::cell::RefCell<Option<ServerProcess>>,
}

impl Default for StremioServer {
    fn default() -> Self {
        Self {
            notify: None,
            process: std::cell::RefCell::new(None),
        }
    }
}

pub fn server_dir() -> PathBuf {
    let dir = if let Some(cfg) = dirs::config_dir() {
        cfg.join("stremio-enhanced").join("streamingserver")
    } else if let Some(home) = dirs::home_dir() {
        home.join("Library")
            .join("Application Support")
            .join("stremio-enhanced")
            .join("streamingserver")
    } else {
        PathBuf::from("streamingserver")
    };
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn find_shipped_bin_dir() -> Option<PathBuf> {
    if let Ok(mut app_dir) = env::current_exe() {
        app_dir.pop();
        if app_dir.join("ffmpeg").exists() && app_dir.join("ffprobe").exists() {
            return Some(app_dir);
        }
        if let Some(contents_dir) = app_dir.parent() {
            let resources_dir = contents_dir.join("Resources");
            if resources_dir.join("ffmpeg").exists() && resources_dir.join("ffprobe").exists() {
                return Some(resources_dir);
            }
        }
    }

    for sys_dir in ["/opt/local/bin", "/opt/homebrew/bin", "/usr/local/bin"] {
        let p = PathBuf::from(sys_dir);
        if p.join("ffmpeg").exists() && p.join("ffprobe").exists() {
            return Some(p);
        }
    }

    None
}

pub fn is_ffmpeg_installed() -> bool {
    let dir = server_dir();
    if dir.join("ffmpeg").exists() && dir.join("ffprobe").exists() {
        return true;
    }
    if std::path::Path::new("/opt/local/bin/ffmpeg").exists()
        || std::path::Path::new("/opt/homebrew/bin/ffmpeg").exists()
        || std::path::Path::new("/usr/local/bin/ffmpeg").exists()
        || Command::new("ffmpeg").arg("-version").output().is_ok()
    {
        return true;
    }
    find_shipped_bin_dir().is_some()
}

pub fn download_and_extract_ffmpeg() -> Result<(), String> {
    if is_ffmpeg_installed() {
        return Ok(());
    }
    Err(
        "FFmpeg not found. Please install ffmpeg via MacPorts ('sudo port install ffmpeg') or Homebrew ('brew install ffmpeg')."
            .to_string(),
    )
}

pub fn default_server_path() -> PathBuf {
    server_dir().join("server.js")
}

pub fn find_server_script() -> Option<PathBuf> {
    let default_path = default_server_path();
    if default_path.exists() {
        return Some(default_path);
    }

    if let Ok(path_str) = env::var("SERVER_PATH") {
        let path = PathBuf::from(&path_str);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(mut path) = env::current_exe() {
        path.pop();
        let app_path = path.join("server.js");
        if app_path.exists() {
            return Some(app_path);
        }
        if let Some(contents_dir) = path.parent() {
            let res_path = contents_dir.join("Resources").join("server.js");
            if res_path.exists() {
                return Some(res_path);
            }
        }
    }

    let cwd_path = PathBuf::from("server.js");
    if cwd_path.exists() {
        return Some(cwd_path);
    }

    None
}

pub fn find_runtime_exe() -> Result<PathBuf, String> {
    if let Ok(mut path) = env::current_exe() {
        path.pop();
        let runtime = path.join("stremio-runtime");
        if runtime.exists() {
            return Ok(runtime);
        }
        let bundled_node = path.join("node");
        if bundled_node.exists() {
            return Ok(bundled_node);
        }
        if let Some(contents_dir) = path.parent() {
            let res_node = contents_dir.join("Resources").join("node");
            if res_node.exists() {
                return Ok(res_node);
            }
        }
    }

    for candidate in [
        "/opt/local/bin/node",
        "/opt/homebrew/bin/node",
        "/usr/local/bin/node",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Ok(p);
        }
    }

    Ok(PathBuf::from("node"))
}

pub fn is_streaming_server_online() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11470".parse().unwrap(),
        std::time::Duration::from_millis(150),
    )
    .is_ok()
}

impl StremioServer {
    pub fn restart(&self) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.stop();
        }
        let notify = self.notify.clone();
        let process = ServerProcess::new(server_command, STARTUP_TIMEOUT, move || {
            if let Some(ref cb) = notify {
                cb();
            }
        });
        process.start();
        *self.process.borrow_mut() = Some(process);
    }

    pub fn stop(&self) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.stop();
        }
    }

    pub fn events(&self) -> Vec<ServerEvent> {
        if let Some(process) = self.process.borrow().as_ref() {
            process.events().collect()
        } else {
            Vec::new()
        }
    }
}

fn server_command() -> Result<Command, String> {
    let runtime = find_runtime_exe()?;
    let server_js = find_server_script()
        .ok_or_else(|| "server.js not found in expected locations".to_string())?;

    if !is_ffmpeg_installed() {
        let _ = download_and_extract_ffmpeg();
    }

    let s_dir = server_dir();
    let mut command = Command::new(runtime);
    command.arg(&server_js).current_dir(&s_dir);

    let mut path_entries = vec![s_dir.display().to_string()];
    if let Some(shipped) = find_shipped_bin_dir() {
        if let Ok(abs) = shipped.canonicalize() {
            path_entries.push(abs.display().to_string());
        } else {
            path_entries.push(shipped.display().to_string());
        }
    }
    for sys_dir in ["/opt/local/bin", "/opt/homebrew/bin", "/usr/local/bin"] {
        if std::path::Path::new(sys_dir).exists() {
            path_entries.push(sys_dir.to_string());
        }
    }
    if let Ok(path_var) = env::var("PATH") {
        path_entries.push(path_var);
    }
    command.env("PATH", path_entries.join(":"));

    Ok(command)
}
