//! Serde-моделі відповідей Subsonic REST API (f=json).
//!
//! Поля віддзеркалюють wire-формат сервера; деякі атрибути ми не
//! використовуємо в UI, але зберігаємо для повноти моделі.
#![allow(dead_code)]

use serde::Deserialize;

/// Обгортка відповіді: `{ "subsonic-response": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct SubsonicResponse<T> {
    #[serde(rename = "subsonic-response")]
    pub response: ResponseEnvelope<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope<T> {
    pub status: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub error: Option<ApiError>,
    #[serde(flatten)]
    pub data: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: i32,
    #[serde(default)]
    pub message: String,
}

// ---------- getArtists ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistsPayload {
    pub artists: ArtistsIndex,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistsIndex {
    #[serde(default)]
    pub index: Vec<ArtistIndexEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistIndexEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Vec<ArtistId3>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtistId3 {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub album_count: Option<i32>,
}

// ---------- getAlbumList2 ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumList2Payload {
    #[serde(default)]
    pub album_list_2: AlbumList,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlbumList {
    #[serde(default)]
    pub album: Vec<AlbumId3>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlbumId3 {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub artist_id: Option<String>,
    #[serde(default)]
    pub cover_art: Option<String>,
    #[serde(default)]
    pub song_count: Option<i32>,
    #[serde(default)]
    pub duration: Option<i32>,
    #[serde(default)]
    pub year: Option<i32>,
}

// ---------- getArtist / getAlbum ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPayload {
    pub artist: ArtistWithAlbums,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtistWithAlbums {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub album_count: Option<i32>,
    #[serde(default)]
    pub album: Vec<AlbumId3>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPayload {
    pub album: AlbumWithSongs,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlbumWithSongs {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub artist_id: Option<String>,
    #[serde(default)]
    pub cover_art: Option<String>,
    #[serde(default)]
    pub song_count: Option<i32>,
    #[serde(default)]
    pub duration: Option<i32>,
    #[serde(default)]
    pub song: Vec<Child>,
}

// ---------- Child (пісня/папка) ----------

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Child {
    pub id: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub is_dir: Option<bool>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub cover_art: Option<String>,
    #[serde(default)]
    pub duration: Option<i32>,
    #[serde(default)]
    pub track: Option<i32>,
    #[serde(default)]
    pub disc_number: Option<i32>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
}

// ---------- getPlaylists / getPlaylist ----------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsPayload {
    #[serde(default)]
    pub playlists: Option<PlaylistList>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistList {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub song_count: Option<i32>,
    #[serde(default)]
    pub duration: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPayload {
    pub playlist: PlaylistWithSongs,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistWithSongs {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub song_count: Option<i32>,
    #[serde(default)]
    pub duration: Option<i32>,
    #[serde(default)]
    pub entry: Vec<Child>,
}

// ---------- getSimilarSongs / getRandomSongs / search3 ----------

/// Список пісень-відповідь `{ "song": [Child, ...] }`.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SongList {
    #[serde(default)]
    pub song: Vec<Child>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilarSongsPayload {
    pub similar_songs: SongList,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongsPayload {
    pub random_songs: SongList,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult3Payload {
    pub search_result_3: SearchResult3,
}

// ---------- getStarred2 ----------

/// Відповідь `getStarred2`: `{ "starred2": { "song": [...] } }` — пісні,
/// які зазірочені на сервері (Subsonic-еквівалент «favorites»).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Starred2Payload {
    #[serde(default)]
    pub starred2: SongList,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult3 {
    #[serde(default)]
    pub song: Vec<Child>,
    #[serde(default)]
    pub album: Vec<AlbumId3>,
    #[serde(default)]
    pub artist: Vec<ArtistId3>,
}

// ---------- scrobble (відповідь без payload) ----------

/// Відповідь без корисного навантаження: scrobble повертає лише `status`.
/// Типуємо її як `SubsonicResponse<EmptyPayload>`, а сам `data` відсутній.
#[derive(Debug, Deserialize)]
pub struct EmptyPayload {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_artists() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "artists": {
              "index": [
                { "name": "A", "artist": [ { "id": "1", "name": "Artist One", "albumCount": 3 } ] }
              ]
            }
          }
        }"#;
        let resp: SubsonicResponse<ArtistsPayload> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.response.status, "ok");
        let artists = resp.response.data.unwrap().artists;
        assert_eq!(artists.index[0].artist[0].name, "Artist One");
        assert_eq!(artists.index[0].artist[0].album_count, Some(3));
    }

    #[test]
    fn parse_empty_playlists() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "playlists": {}
          }
        }"#;
        let resp: SubsonicResponse<PlaylistsPayload> = serde_json::from_str(json).unwrap();
        let payload = resp.response.data.unwrap();
        let lists = payload.playlists.unwrap();
        assert!(lists.playlist.is_empty());
    }

    #[test]
    fn parse_scrobble_ok_has_no_data() {
        // scrobble не повертає даних — лише `status`. Порожній `EmptyPayload`
        // десеріалізується навіть з відсутнього `data`, тож `call()` проходить.
        let json = r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#;
        let resp: SubsonicResponse<EmptyPayload> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.response.status, "ok");
        assert!(resp.response.data.is_some());
    }

    #[test]
    fn parse_similar_songs_ok() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "similarSongs": {
              "song": [ { "id": "s1", "title": "T1", "artist": "A1", "album": "B1", "duration": 200 } ]
            }
          }
        }"#;
        let resp: SubsonicResponse<SimilarSongsPayload> = serde_json::from_str(json).unwrap();
        let songs = resp.response.data.unwrap().similar_songs.song;
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id, "s1");
        assert_eq!(songs[0].duration, Some(200));
    }

    /// Navidrome повертає порожній `"similarSongs": {}` — десеріалізується у пустий список.
    #[test]
    fn parse_similar_songs_empty() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "similarSongs": {}
          }
        }"#;
        let resp: SubsonicResponse<SimilarSongsPayload> = serde_json::from_str(json).unwrap();
        let songs = resp.response.data.unwrap().similar_songs.song;
        assert!(songs.is_empty());
    }

    #[test]
    fn parse_random_songs() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "randomSongs": {
              "song": [ { "id": "r1" }, { "id": "r2" } ]
            }
          }
        }"#;
        let resp: SubsonicResponse<RandomSongsPayload> = serde_json::from_str(json).unwrap();
        let songs = resp.response.data.unwrap().random_songs.song;
        assert_eq!(songs.len(), 2);
    }

    #[test]
    fn parse_search_result3() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "searchResult3": {
              "song": [ { "id": "p1", "title": "Song" } ],
              "album": [],
              "artist": []
            }
          }
        }"#;
        let resp: SubsonicResponse<SearchResult3Payload> = serde_json::from_str(json).unwrap();
        let songs = resp.response.data.unwrap().search_result_3.song;
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].title.as_deref(), Some("Song"));
    }

    #[test]
    fn parse_starred2() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "starred2": {
              "song": [ { "id": "f1", "title": "Fav", "artist": "A1", "album": "B1", "duration": 180 } ]
            }
          }
        }"#;
        let resp: SubsonicResponse<Starred2Payload> = serde_json::from_str(json).unwrap();
        let songs = resp.response.data.unwrap().starred2.song;
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id, "f1");
        assert_eq!(songs[0].title.as_deref(), Some("Fav"));
    }

    /// Navidrome повертає порожній `"starred2": {}` — десеріалізується у пустий список.
    #[test]
    fn parse_starred2_empty() {
        let json = r#"{
          "subsonic-response": {
            "status": "ok",
            "version": "1.16.1",
            "starred2": {}
          }
        }"#;
        let resp: SubsonicResponse<Starred2Payload> = serde_json::from_str(json).unwrap();
        assert!(resp.response.data.unwrap().starred2.song.is_empty());
    }

    #[test]
    fn parse_error() {
        let json = r#"{
          "subsonic-response": {
            "status": "failed",
            "version": "1.16.1",
            "error": { "code": 40, "message": "Wrong username or password" }
          }
        }"#;
        let resp: SubsonicResponse<ArtistsPayload> = serde_json::from_str(json).unwrap();
        let err = resp.response.error.unwrap();
        assert_eq!(err.code, 40);
    }
}
