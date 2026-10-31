use std::{fs, process::Command};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use url::Url;

const M3U_DATA_URI_PREFIX: &str = "data:application/octet-stream;charset=utf-8;base64,";

#[derive(Debug, PartialEq)]
enum Request {
    Vlc(Url),
    Playlist(Url),
}

pub fn play(input: &str) -> Result<(), &'static str> {
    match parse(input)? {
        Request::Vlc(url) => spawn_player("VLC", &url),
        Request::Playlist(url) => open_playlist(&url),
    }
}

fn parse(input: &str) -> Result<Request, &'static str> {
    if let Some(encoded) = input.strip_prefix(M3U_DATA_URI_PREFIX) {
        return parse_playlist(encoded).map(Request::Playlist);
    }
    let (scheme, rest) = input
        .split_once("://")
        .ok_or("unsupported external player")?;
    match scheme.to_ascii_lowercase().as_str() {
        "vlc" => stream_url(rest).map(Request::Vlc),
        _ => Err("unsupported external player"),
    }
}

fn parse_playlist(encoded: &str) -> Result<Url, &'static str> {
    let decoded = BASE64.decode(encoded).map_err(|_| "invalid playlist")?;
    let playlist = String::from_utf8(decoded).map_err(|_| "invalid playlist")?;
    let mut lines = playlist.lines();
    if lines.next() != Some("#EXTM3U") || lines.next() != Some("#EXTINF:0") {
        return Err("invalid playlist");
    }
    let url = lines.next().ok_or("invalid playlist")?;
    if lines.next().is_some() {
        return Err("invalid playlist");
    }
    stream_url(url)
}

fn stream_url(input: &str) -> Result<Url, &'static str> {
    const ERROR: &str = "stream URL must be absolute HTTP(S)";
    if input.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(ERROR);
    }
    let url = Url::parse(input).map_err(|_| ERROR)?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(ERROR);
    }
    Ok(url)
}

fn open_playlist(url: &Url) -> Result<(), &'static str> {
    let directory = dirs::download_dir()
        .ok_or("Downloads directory is unavailable")?
        .join("Stremio");
    fs::create_dir_all(&directory).map_err(|_| "cannot create playlist directory")?;
    let path = directory.join("playlist.m3u");
    fs::write(&path, format!("#EXTM3U\n#EXTINF:0\n{url}"))
        .map_err(|_| "cannot save M3U playlist")?;
    open::that(&path).map_err(|_| "cannot open M3U playlist")
}

fn spawn_player(name: &str, url: &Url) -> Result<(), &'static str> {
    let mac_candidates = [
        "/Applications/VLC.app/Contents/MacOS/VLC",
        "/Applications/MacPorts/VLC.app/Contents/MacOS/VLC",
        "/opt/local/bin/vlc",
        "/opt/homebrew/bin/vlc",
        "/usr/local/bin/vlc",
        "vlc",
    ];
    for candidate in mac_candidates {
        if Command::new(candidate)
            .arg("--")
            .arg(url.as_str())
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
    }

    let status = Command::new("open")
        .args(["-a", "VLC", url.as_str()])
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            let fallback = Command::new("open").arg(url.as_str()).status();
            if fallback.is_ok_and(|s| s.success()) {
                Ok(())
            } else {
                eprintln!("{name} was not found on this computer.");
                Err("external player is not installed")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM_URL: &str = "https://example.com/video.mkv?token=secret%2Bvalue";

    fn data_uri(playlist: &str) -> String {
        format!("{}{}", M3U_DATA_URI_PREFIX, BASE64.encode(playlist))
    }

    #[test]
    fn accepts_core_forms() {
        let stream = Url::parse(STREAM_URL).unwrap();
        assert_eq!(
            parse(&format!("vlc://{}", STREAM_URL)),
            Ok(Request::Vlc(stream.clone()))
        );
        assert_eq!(
            parse(&data_uri(&format!("#EXTM3U\n#EXTINF:0\n{}", STREAM_URL))),
            Ok(Request::Playlist(stream))
        );
    }

    #[test]
    fn rejects_unsafe_values() {
        for input in [
            "",
            "calc",
            "file:///System/Applications/Calculator.app",
            "mpv://https://example.com/video.mkv",
            "vlc://file:///System/Applications/Calculator.app",
            "vlc://https://example.com/video.mkv\n--script=attack.lua",
            "vlc://javascript:alert(1)",
            "potplayer://https://example.com/video.mkv",
            "iina://weblink?url=https%3A%2F%2Fexample.com%2Fvideo.mkv",
            &data_uri("#EXTM3U\n#EXTINF:0\nfile:///System/Applications/Calculator.app"),
            &data_uri("#EXTM3U\n#EXTINF:0\nhttps://example.com/one\nhttps://example.com/two"),
            &data_uri("#EXTM3U\n#EXTINF:-1\nhttps://example.com/video.mkv"),
            "data:application/octet-stream;charset=utf-8;base64,not base64",
        ] {
            assert!(parse(input).is_err(), "{}", input);
        }
    }
}
