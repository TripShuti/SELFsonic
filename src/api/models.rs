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
