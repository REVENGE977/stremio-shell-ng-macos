use std::time::Duration;
use url::Url;

pub const APP_NAME: &str = "Stremio Enhanced";
pub const IPC_PATH: &str = "/tmp/com.stremio5.";
pub const IPC_ATTEMPTS: u32 = 10;
pub const IPC_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const DEV_ENDPOINT: &str = "http://127.0.0.1:11470";
pub const WEB_ENDPOINT: &str = "https://web.stremio.com/";
pub const STA_ENDPOINT: &str = "https://staging.strem.io/";
pub const WINDOW_MIN_WIDTH: i32 = 1000;
pub const WINDOW_MIN_HEIGHT: i32 = 600;
pub const STREMIO_SERVER_DEV_MODE: &str = "STREMIO_SERVER_DEV_MODE";
pub const SRV_BUFFER_SIZE: usize = 1024;
pub const SERVER_IPC_KEY: &str = "SERVER_IPC_KEY";
pub const SRV_LOG_SIZE: usize = 20;

pub const WARNING_URL: &str = "https://www.stremio.com/warning#";
pub const WHITELISTED_HOSTS: &[&str] = &[
    "stremio.com",
    "www.stremio.com",
    "web.stremio.com",
    "app.stremio.com",
    "strem.io",
    "api.strem.io",
    "stremio.zendesk.com",
    "google.com",
    "www.google.com",
    "youtube.com",
    "www.youtube.com",
    "twitch.tv",
    "twitter.com",
    "x.com",
    "netflix.com",
    "adex.network",
    "amazon.com",
    "forms.gle",
    "www.hbomax.com",
    "play.hbomax.com",
    "www.disneyplus.com",
    "imdb.com",
];

pub fn web_endpoint_with_streaming_server(server_url: &str) -> String {
    let server_url = server_url.trim_end_matches('/');
    let encoded_web_endpoint =
        url::form_urlencoded::byte_serialize(WEB_ENDPOINT.trim_end_matches('/').as_bytes())
            .collect::<String>();

    format!("{server_url}/proxy/d={encoded_web_endpoint}/#/")
}

pub fn safe_url(uri: &str) -> Option<String> {
    if let Ok(url) = Url::parse(uri) {
        if url.scheme() == "file" || url.scheme().len() == 1 {
            return None;
        }
        println!("URL is {url}");
        let is_whitelisted = url.host().is_some_and(|host| {
            WHITELISTED_HOSTS
                .iter()
                .any(|whitelisted_host| host.to_string() == *whitelisted_host)
        });

        let final_url = if is_whitelisted {
            url.to_string()
        } else {
            format!("{}{}", WARNING_URL, urlencoding::encode(url.as_ref()))
        };
        Some(final_url)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_file_urls_are_not_opened_externally() {
        for uri in [
            "file:///Users/Test%20User/subtitle.srt",
            "file://server/share/subtitle.srt",
        ] {
            assert_eq!(safe_url(uri), None, "{uri}");
        }
    }

    #[test]
    fn web_endpoint_encodes_streaming_server_url() {
        assert_eq!(
            web_endpoint_with_streaming_server("http://127.0.0.1:11470/"),
            "http://127.0.0.1:11470/proxy/d=https%3A%2F%2Fweb.stremio.com/#/"
        );
    }
}
