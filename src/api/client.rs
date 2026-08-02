//! Синхронний REST-клієнт Subsonic на ureq.
//!
//! Один `Agent` на клієнт (connection pooling). Retry з exponential backoff
//! тільки на 5xx, без retry на 4xx (AGENT.md).

use std::io::{Read, Seek, SeekFrom, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use crate::api::auth;
use crate::api::models::*;
use crate::config::{API_VERSION, CLIENT_NAME};
use crate::error::{AppError, Result};

const API_PATH: &str = "/rest/";
const MAX_RETRIES: u32 = 3;
const BACKOFF_BASE_MS: u64 = 500;

pub struct Client {
    api_agent: ureq::Agent,
    stream_agent: ureq::Agent,
    base_url: String,
    username: String,
    password: String,
}

pub struct Stream {
    /// Читалка з блокуванням: чекає, поки downloader-потік досягне потрібної позиції.
    pub reader: StreamFile,
    /// Тимчасовий файл (видаляється при drop).
    _tmp: tempfile::NamedTempFile,
}

impl Client {
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self> {
        let api_cfg = ureq::config::Config::builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .build();
        let stream_cfg = ureq::config::Config::builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            .build();
        Ok(Self {
            api_agent: ureq::Agent::new_with_config(api_cfg),
            stream_agent: ureq::Agent::new_with_config(stream_cfg),
            base_url: base_url.trim_end_matches('/').to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    fn url(&self, method: &str, extra: &[(&str, String)]) -> String {
        let mut params = auth::base_params(
            &self.username,
            &self.password,
            API_VERSION,
            CLIENT_NAME,
        );
        params.extend(extra.iter().cloned());
        let mut url = format!("{}{API_PATH}{method}", self.base_url);
        for (i, (k, v)) in params.iter().enumerate() {
            url.push_str(if i == 0 { "?" } else { "&" });
            url.push_str(k);
            url.push('=');
            url.push_str(v);
        }
        url
    }

    /// GET + парсинг JSON з retry на 5xx (exponential backoff).
    fn call<T: DeserializeOwned>(&self, method: &str, extra: &[(&str, String)]) -> Result<T> {
        let url = self.url(method, extra);
        let mut attempt = 0;
        loop {
            match self.api_agent.get(&url).call() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if status >= 500 {
                        if attempt < MAX_RETRIES {
                            attempt += 1;
                            let delay = BACKOFF_BASE_MS * (1 << (attempt - 1));
                            debug!("{method} 5xx ({status}), retry {attempt} in {delay}ms");
                            thread::sleep(Duration::from_millis(delay));
                            continue;
                        }
                        return Err(AppError::Network(format!(
                            "{method}: server responded {status} after {MAX_RETRIES} attempts"
                        )));
                    }
                    return parse_response(resp);
                }
                Err(e) => {
                    if attempt < MAX_RETRIES {
                        attempt += 1;
                        let delay = BACKOFF_BASE_MS * (1 << (attempt - 1));
                        debug!("{method} error: {e}, retry {attempt}");
                        thread::sleep(Duration::from_millis(delay));
                        continue;
                    }
                    return Err(AppError::Network(format!("{method}: {e}")));
                }
            }
        }
    }

    pub fn get_artists(&self) -> Result<Vec<ArtistId3>> {
        let payload: ArtistsPayload = self.call("getArtists", &[])?;
        Ok(payload
            .artists
            .index
            .into_iter()
            .flat_map(|i| i.artist)
            .collect())
    }

    pub fn get_album_list2(&self, offset: i32, size: i32) -> Result<Vec<AlbumId3>> {
        let payload: AlbumList2Payload = self.call(
            "getAlbumList2",
            &[
                ("type", "newest".into()),
                ("offset", offset.to_string()),
                ("size", size.to_string()),
            ],
        )?;
        Ok(payload.album_list_2.album)
    }

    pub fn get_artist(&self, id: &str) -> Result<ArtistWithAlbums> {
        let payload: ArtistPayload = self.call("getArtist", &[("id", id.to_string())])?;
        Ok(payload.artist)
    }

    pub fn get_album(&self, id: &str) -> Result<AlbumWithSongs> {
        let payload: AlbumPayload = self.call("getAlbum", &[("id", id.to_string())])?;
        Ok(payload.album)
    }

    pub fn get_playlists(&self) -> Result<Vec<Playlist>> {
        let payload: PlaylistsPayload = self.call("getPlaylists", &[])?;
        Ok(payload.playlists.map(|l| l.playlist).unwrap_or_default())
    }

    pub fn get_playlist(&self, id: &str) -> Result<PlaylistWithSongs> {
        let payload: PlaylistPayload = self.call("getPlaylist", &[("id", id.to_string())])?;
        Ok(payload.playlist)
    }

    /// URL обкладинки (MPRIS `mpris:artUrl` — віддаємо напряму).
    pub fn cover_art_url(&self, cover_art: &str) -> String {
        self.url("getCoverArt", &[("id", cover_art.to_string())])
    }

    /// Відкриває аудіо-стрім: downloader-потік пише в тимчасовий файл,
    /// `StreamFile` блокує читання, поки дані не дійдуть (не в пам'яті).
    pub fn stream(&self, track_id: &str) -> Result<Stream> {
        let url = self.url("stream", &[("id", track_id.to_string())]);
        let track_id = track_id.to_string();
        let tmp = tempfile::NamedTempFile::new()
            .map_err(|e| AppError::Other(format!("tempfile: {e}")))?;
        let reader = tmp.reopen()
            .map_err(|e| AppError::Other(format!("tempfile reopen: {e}")))?;
        let done = Arc::new(AtomicBool::new(false));

        let agent = self.stream_agent.clone();
        let tmp_path: PathBuf = tmp.path().to_path_buf();
        let d = done.clone();
        let _ = thread::Builder::new()
            .name("selfsonic-stream".into())
            .spawn(move || {
                let res = (|| -> std::result::Result<(), AppError> {
                    let resp = agent.get(&url).call().map_err(|e| {
                        AppError::Network(format!("stream {track_id}: {e}"))
                    })?;
                    if resp.status().as_u16() >= 400 {
                        return Err(AppError::Network(format!(
                            "stream {track_id}: HTTP {}",
                            resp.status()
                        )));
                    }
                    let mut body = resp.into_body().into_with_config().reader();
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&tmp_path)
                        .map_err(|e| AppError::io(&tmp_path, e))?;
                    let mut buf = [0u8; 64 * 1024];
                    loop {
                        let n = body
                            .read(&mut buf)
                            .map_err(|e| AppError::Network(format!("stream read: {e}")))?;
                        if n == 0 {
                            break;
                        }
                        file.write_all(&buf[..n])
                            .map_err(|e| AppError::io(&tmp_path, e))?;
                    }
                    file.flush().map_err(|e| AppError::io(&tmp_path, e))?;
                    Ok(())
                })();
                if let Err(e) = res {
                    warn!("{e}");
                }
                d.store(true, Ordering::Release);
            })
            .map_err(|e| AppError::Other(format!("stream thread: {e}")))?;

        Ok(Stream {
            reader: StreamFile {
                file: reader,
                done: done.clone(),
            },
            _tmp: tmp,
        })
    }
}

fn parse_response<T: DeserializeOwned>(mut resp: ureq::http::Response<ureq::Body>) -> Result<T> {
    let status = resp.status().as_u16();
    if (400..500).contains(&status) {
        return Err(AppError::Network(format!("HTTP {status}")));
    }
    let parsed: SubsonicResponse<T> = resp
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_json()
        .map_err(|e| AppError::Network(format!("JSON: {e}")))?;
    if parsed.response.status != "ok" {
        if let Some(err) = parsed.response.error {
            return Err(AppError::Api {
                code: err.code,
                message: err.message,
            });
        }
        return Err(AppError::Api {
            code: 0,
            message: "unknown server error".into(),
        });
    }
    parsed
        .response
        .data
        .ok_or_else(|| AppError::Api {
            code: 0,
            message: "empty response".into(),
        })
}

/// Читалка, яка блокується, поки downloader не досяг потрібної позиції
/// у тимчасовому файлі. По завершенню завантаження веде себе як звичайний файл.
pub struct StreamFile {
    file: std::fs::File,
    done: Arc<AtomicBool>,
}

const WAIT_STEP: Duration = Duration::from_millis(10);
const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

impl Read for StreamFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut waited = Duration::ZERO;
        loop {
            match self.file.read(buf) {
                Ok(0) => {
                    if self.done.load(Ordering::Acquire) {
                        return Ok(0);
                    }
                    // Дані ще йдуть — почекати.
                    if waited >= WAIT_TIMEOUT {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "stream wait timeout",
                        ));
                    }
                    thread::sleep(WAIT_STEP);
                    waited += WAIT_STEP;
                }
                other => return other,
            }
        }
    }
}

impl Seek for StreamFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Діагностика проти реального сервера (треба конфіг користувача).
    #[test]
    #[ignore]
    fn live_get_artists() {
        let path = crate::config::Config::default_path().unwrap();
        let cfg = crate::config::Config::load(&path).unwrap();
        let client = Client::new(&cfg.server.url, &cfg.server.username, &cfg.server.password).unwrap();
        match client.get_artists() {
            Ok(a) => println!("get_artists: OK {} artists", a.len()),
            Err(e) => println!("get_artists: ERR {e:?}"),
        }
        match client.get_album_list2(0, 100) {
            Ok(a) => println!("get_album_list2: OK {} albums", a.len()),
            Err(e) => println!("get_album_list2: ERR {e:?}"),
        }
        match client.get_playlists() {
            Ok(p) => println!("get_playlists: OK {} playlists", p.len()),
            Err(e) => println!("get_playlists: ERR {e:?}"),
        }
    }
}
