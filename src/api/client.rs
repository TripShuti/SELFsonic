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
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
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
            // Без пулу: кожен стрім — свіже TCP-з'єднання.
            // Мертві idle-конекшни після сну системи неможливі.
            .max_idle_connections(0)
            .max_idle_connections_per_host(0)
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
            // Значення (пароль, id треку тощо) кодуємо: `&`, `=`, `#`, `%`,
            // пробіл і не-ASCII символи ламали б query або авторизацію.
            url.push_str(&utf8_percent_encode(v, NON_ALPHANUMERIC).to_string());
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

    /// Повний обхід каталогу альбомів сторінками (по 500), щоб мати
    /// повний список для прунінгу. Зупиняється на короткій сторінці.
    pub fn get_all_albums(&self) -> Result<Vec<AlbumId3>> {
        const PAGE_SIZE: i32 = 500;
        const MAX_PAGES: i32 = 500;
        let mut albums = Vec::new();
        let mut page = 0;
        loop {
            let slice = self.get_album_list2(page * PAGE_SIZE, PAGE_SIZE)?;
            albums.extend(slice);
            page += 1;
            if albums.len() < page as usize * PAGE_SIZE as usize || page >= MAX_PAGES {
                break;
            }
        }
        Ok(albums)
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

    /// Scrobble: `submission=false` — "Now Playing", `submission=true` —
    /// зарахування прослуховування (Last.fm; Navidrome позначає трек як played).
    /// Відповідь без payload (лише `status`) — `EmptyPayload` десеріалізується
    /// навіть з порожнього `data`. Виконується у фоновому потоці.
    pub fn scrobble(&self, id: &str, submission: bool) -> Result<()> {
        let _: EmptyPayload = self.call(
            "scrobble",
            &[
                ("id", id.to_string()),
                ("submission", submission.to_string()),
            ],
        )?;
        Ok(())
    }

    /// URL обкладинки (MPRIS `mpris:artUrl` — віддаємо напряму).
    pub fn cover_art_url(&self, cover_art: &str) -> String {
        self.url("getCoverArt", &[("id", cover_art.to_string())])
    }

    /// Зазірочені пісні (Subsonic-еквівалент «favorites»).
    pub fn get_starred2(&self) -> Result<Vec<Child>> {
        let payload: Starred2Payload = self.call("getStarred2", &[])?;
        Ok(payload.starred2.song)
    }

    /// Додати трек у «favorites» (`star`). Navidrome приймає GET-параметри;
    /// старі сервери (Airsonic) теж. Якщо сервер вимагатиме POST form-data
    /// (спека Subsonic 1.15+) — перейти на `send_form`.
    pub fn star(&self, id: &str) -> Result<()> {
        let _: EmptyPayload = self.call("star", &[("id", id.to_string())])?;
        Ok(())
    }

    /// Прибрати трек з «favorites» (`unstar`).
    pub fn unstar(&self, id: &str) -> Result<()> {
        let _: EmptyPayload = self.call("unstar", &[("id", id.to_string())])?;
        Ok(())
    }

    /// Схожі пісні (Last.fm, per-track). Navidrome ігнорує `count < 14` і
    /// повертає порожньо — завжди питаємо `count >= 14`. Джерело для DJ.
    pub fn get_similar_songs(&self, id: &str, count: u32) -> Result<Vec<Child>> {
        let payload: SimilarSongsPayload = self.call(
            "getSimilarSongs",
            &[("id", id.to_string()), ("count", count.to_string())],
        )?;
        Ok(payload.similar_songs.song)
    }

    /// Пошук пісень по імені артиста (DJ-фолбек «пісні того ж артиста»,
    /// бо `getAlbumList2 type=byArtist` у Navidrome не реалізовано).
    pub fn search3_songs(&self, query: &str, count: u32) -> Result<Vec<Child>> {
        let payload: SearchResult3Payload = self.call(
            "search3",
            &[
                ("query", query.to_string()),
                ("songCount", count.to_string()),
                ("albumCount", "0".into()),
                ("artistCount", "0".into()),
            ],
        )?;
        Ok(payload.search_result_3.song)
    }

    /// Випадкові пісні (DJ-фолбек, коли схожих і пісень артиста немає).
    pub fn get_random_songs(&self, size: u32) -> Result<Vec<Child>> {
        let payload: RandomSongsPayload =
            self.call("getRandomSongs", &[("size", size.to_string())])?;
        Ok(payload.random_songs.song)
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
                    // Retry на транспортні помилки і 5xx (як у call()).
                    let mut attempt = 0;
                    let resp = loop {
                        match agent.get(&url).call() {
                            Ok(resp) => {
                                let status = resp.status().as_u16();
                                if status < 500 {
                                    break resp;
                                }
                                if attempt >= MAX_RETRIES {
                                    return Err(AppError::Network(format!(
                                        "stream {track_id}: server responded {status} after {MAX_RETRIES} attempts"
                                    )));
                                }
                                attempt += 1;
                                let delay = BACKOFF_BASE_MS * (1 << (attempt - 1));
                                debug!("stream {track_id} 5xx ({status}), retry {attempt} in {delay}ms");
                                thread::sleep(Duration::from_millis(delay));
                            }
                            Err(e) => {
                                if attempt >= MAX_RETRIES {
                                    return Err(AppError::Network(format!(
                                        "stream {track_id}: {e}"
                                    )));
                                }
                                attempt += 1;
                                let delay = BACKOFF_BASE_MS * (1 << (attempt - 1));
                                debug!("stream {track_id} error: {e}, retry {attempt}");
                                thread::sleep(Duration::from_millis(delay));
                            }
                        }
                    };
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

    /// URL-encoding значень параметрів: `&`, `=` та пробіл у паролі мають
    /// бути закодовані, а після декодування — збігатися з оригіналом.
    #[test]
    fn url_percent_encodes_param_values() {
        let client = Client::new("http://example.com", "user", "p@ss word&x=1%#").unwrap();
        let url = client.url("ping", &[("id", "track id 1".into())]);
        let (_, query) = url.split_once('?').expect("має бути query-рядок");
        let params: std::collections::HashMap<_, _> = query
            .split('&')
            .map(|pair| {
                let (k, v) = pair.split_once('=').expect("пара к=значення");
                let v = percent_encoding::percent_decode_str(v)
                    .decode_utf8()
                    .unwrap()
                    .to_string();
                (k, v)
            })
            .collect();
        assert_eq!(params.get("p").unwrap(), "p@ss word&x=1%#");
        assert_eq!(params.get("id").unwrap(), "track id 1");
        // Сирі символи не мають з'явитися у query.
        assert!(!query.contains("p@ss word"));
    }

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
