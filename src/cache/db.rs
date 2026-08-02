//! Локальний кеш бібліотеки: SQLite у WAL-режимі як source of truth для UI.
//! Синк з сервером — ручний refresh (не polling), сторінками.

use std::path::Path;

use rusqlite::Connection;

use crate::api::models::{AlbumId3, ArtistId3, ArtistWithAlbums, Child, Playlist, PlaylistWithSongs};
use crate::error::Result;

pub struct Db {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct CachedAlbum {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub duration: i32,
    pub year: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct CachedTrack {
    pub id: String,
    pub title: String,
    pub album: String,
    pub album_id: String,
    pub artist: String,
    pub cover_art: Option<String>,
    pub duration: i32,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct CachedPlaylist {
    pub id: String,
    pub name: String,
    pub song_count: i32,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::AppError::io(parent, e))?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS artists (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                album_count INTEGER,
                updated_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS albums (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                artist      TEXT NOT NULL DEFAULT '',
                artist_id   TEXT NOT NULL DEFAULT '',
                cover_art   TEXT,
                song_count  INTEGER NOT NULL DEFAULT 0,
                duration    INTEGER NOT NULL DEFAULT 0,
                year        INTEGER,
                updated_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tracks (
                id           TEXT PRIMARY KEY,
                album_id     TEXT NOT NULL DEFAULT '',
                title        TEXT NOT NULL,
                album        TEXT NOT NULL DEFAULT '',
                artist       TEXT NOT NULL DEFAULT '',
                cover_art    TEXT,
                duration     INTEGER NOT NULL DEFAULT 0,
                track_number INTEGER,
                disc_number  INTEGER,
                updated_at   INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS playlists (
                id         TEXT PRIMARY KEY,
                name       TEXT NOT NULL,
                song_count INTEGER NOT NULL DEFAULT 0,
                duration   INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS playlist_tracks (
                playlist_id TEXT NOT NULL,
                position    INTEGER NOT NULL,
                track_id    TEXT NOT NULL,
                PRIMARY KEY (playlist_id, position)
            );
            CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums(artist_id);
            CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks(album_id);
            CREATE INDEX IF NOT EXISTS idx_playlist_tracks_id ON playlist_tracks(playlist_id);",
        )?;
        Ok(Self { conn })
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    // ---------- artists ----------

    pub fn upsert_artists(&mut self, artists: &[ArtistId3]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO artists (id, name, album_count, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name,
                   album_count = COALESCE(excluded.album_count, artists.album_count),
                   updated_at = excluded.updated_at",
            )?;
            for a in artists {
                stmt.execute(rusqlite::params![
                    a.id,
                    a.name,
                    a.album_count,
                    Self::now()
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn artists(&self) -> Result<Vec<ArtistId3>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, album_count FROM artists ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArtistId3 {
                id: r.get(0)?,
                name: r.get(1)?,
                album_count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- albums ----------

    pub fn upsert_artist_albums(&mut self, artist: &ArtistWithAlbums) -> Result<()> {
        self.upsert_albums(&artist.album)
    }

    pub fn upsert_albums(&mut self, albums: &[AlbumId3]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO albums (id, name, artist, artist_id, cover_art, song_count, duration, year, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                   name       = excluded.name,
                   artist     = excluded.artist,
                   artist_id  = excluded.artist_id,
                   cover_art  = COALESCE(excluded.cover_art, albums.cover_art),
                   song_count = COALESCE(excluded.song_count, albums.song_count),
                   duration   = COALESCE(excluded.duration, albums.duration),
                   year       = COALESCE(excluded.year, albums.year),
                   updated_at = excluded.updated_at",
            )?;
            for a in albums {
                stmt.execute(rusqlite::params![
                    a.id,
                    a.name,
                    a.artist.clone().unwrap_or_default(),
                    a.artist_id.clone().unwrap_or_default(),
                    a.cover_art.clone(),
                    a.song_count.unwrap_or(0),
                    a.duration.unwrap_or(0),
                    a.year,
                    Self::now()
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn albums_by_artist(&self, artist_id: &str) -> Result<Vec<CachedAlbum>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, artist, duration, year
             FROM albums WHERE artist_id = ?1 ORDER BY year, name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([artist_id], map_album)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn recent_albums(&self, limit: i32) -> Result<Vec<CachedAlbum>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, artist, duration, year
             FROM albums ORDER BY updated_at DESC, name COLLATE NOCASE LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], map_album)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- tracks ----------

    pub fn upsert_album_tracks(&mut self, album_id: &str, tracks: &[Child]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO tracks (id, album_id, title, album, artist, cover_art, duration, track_number, disc_number, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                   album_id     = excluded.album_id,
                   title        = excluded.title,
                   album        = excluded.album,
                   artist       = excluded.artist,
                   cover_art    = COALESCE(excluded.cover_art, tracks.cover_art),
                   duration     = COALESCE(excluded.duration, tracks.duration),
                   track_number = COALESCE(excluded.track_number, tracks.track_number),
                   disc_number  = COALESCE(excluded.disc_number, tracks.disc_number),
                   updated_at   = excluded.updated_at",
            )?;
            for t in tracks {
                stmt.execute(rusqlite::params![
                    t.id,
                    album_id,
                    t.title.clone().unwrap_or_default(),
                    t.album.clone().unwrap_or_default(),
                    t.artist.clone().unwrap_or_default(),
                    t.cover_art.clone(),
                    t.duration.unwrap_or(0),
                    t.track,
                    t.disc_number,
                    Self::now()
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn tracks_by_album(&self, album_id: &str) -> Result<Vec<CachedTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, album, album_id, artist, cover_art, duration, track_number, disc_number
             FROM tracks WHERE album_id = ?1
             ORDER BY COALESCE(disc_number, 1), COALESCE(track_number, 0), title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([album_id], map_track)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn tracks_by_ids(&self, ids: &[String]) -> Result<Vec<CachedTrack>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT id, title, album, album_id, artist, cover_art, duration, track_number, disc_number
             FROM tracks WHERE id IN ({placeholders})"
        );
        let params = rusqlite::params_from_iter(ids.iter());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, map_track)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- playlists ----------

    pub fn upsert_playlists(&mut self, playlists: &[Playlist]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlists (id, name, song_count, duration, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                   name       = excluded.name,
                   song_count = COALESCE(excluded.song_count, playlists.song_count),
                   duration   = COALESCE(excluded.duration, playlists.duration),
                   updated_at = excluded.updated_at",
            )?;
            for p in playlists {
                stmt.execute(rusqlite::params![
                    p.id,
                    p.name,
                    p.song_count.unwrap_or(0),
                    p.duration.unwrap_or(0),
                    Self::now()
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn playlists(&self) -> Result<Vec<CachedPlaylist>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, song_count FROM playlists ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CachedPlaylist {
                id: r.get(0)?,
                name: r.get(1)?,
                song_count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Зберігає треки плейлиста (за ід плейлиста) як альбомну групу.
    pub fn upsert_playlist_tracks(&mut self, playlist: &PlaylistWithSongs) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [&playlist.id])?;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR REPLACE INTO playlist_tracks (playlist_id, position, track_id)
                     VALUES (?1, ?2, ?3)",
                )?;
                for (i, t) in playlist.entry.iter().enumerate() {
                    stmt.execute(rusqlite::params![playlist.id, i as i64, t.id])?;
                }
            }
            {
                // Копіюємо треки у загальну таблицю з album_id = "playlist:<id>"
                let mut stmt = tx.prepare(
                    "INSERT INTO tracks (id, album_id, title, album, artist, cover_art, duration, track_number, disc_number, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(id) DO UPDATE SET
                       album_id     = excluded.album_id,
                       title        = excluded.title,
                       album        = excluded.album,
                       artist       = excluded.artist,
                       cover_art    = COALESCE(excluded.cover_art, tracks.cover_art),
                       duration     = COALESCE(excluded.duration, tracks.duration),
                       track_number = COALESCE(excluded.track_number, tracks.track_number),
                       disc_number  = COALESCE(excluded.disc_number, tracks.disc_number),
                       updated_at   = excluded.updated_at",
                )?;
                for t in &playlist.entry {
                    stmt.execute(rusqlite::params![
                        t.id,
                        format!("playlist:{}", playlist.id),
                        t.title.clone().unwrap_or_default(),
                        t.album.clone().unwrap_or_default(),
                        t.artist.clone().unwrap_or_default(),
                        t.cover_art.clone(),
                        t.duration.unwrap_or(0),
                        t.track,
                        t.disc_number,
                        Self::now()
                    ])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<CachedTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.title, t.album, t.album_id, t.artist, t.cover_art, t.duration, t.track_number, t.disc_number
             FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 ORDER BY pt.position",
        )?;
        let rows = stmt.query_map([playlist_id], map_track)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn map_album(r: &rusqlite::Row<'_>) -> rusqlite::Result<CachedAlbum> {
    Ok(CachedAlbum {
        id: r.get(0)?,
        name: r.get(1)?,
        artist: r.get(2)?,
        duration: r.get(3)?,
        year: r.get(4)?,
    })
}

fn map_track(r: &rusqlite::Row<'_>) -> rusqlite::Result<CachedTrack> {
    Ok(CachedTrack {
        id: r.get(0)?,
        title: r.get(1)?,
        album: r.get(2)?,
        album_id: r.get(3)?,
        artist: r.get(4)?,
        cover_art: r.get(5)?,
        duration: r.get(6)?,
        track_number: r.get(7)?,
        disc_number: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::AlbumId3;

    fn album(id: &str, name: &str) -> AlbumId3 {
        AlbumId3 {
            id: id.into(),
            name: name.into(),
            artist: Some("Artist".into()),
            artist_id: Some("a1".into()),
            cover_art: None,
            song_count: Some(2),
            duration: Some(300),
            year: Some(2020),
        }
    }

    #[test]
    fn albums_and_tracks_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("test.db")).unwrap();

        let artist = ArtistWithAlbums {
            id: "a1".into(),
            name: "Artist".into(),
            album_count: Some(1),
            album: vec![album("al1", "Album One")],
        };
        db.upsert_artist_albums(&artist).unwrap();

        let tracks = vec![Child {
            id: "t1".into(),
            title: Some("Track One".into()),
            album: Some("Album One".into()),
            artist: Some("Artist".into()),
            duration: Some(240),
            track: Some(1),
            ..Default::default()
        }];
        db.upsert_album_tracks("al1", &tracks).unwrap();

        let albums = db.albums_by_artist("a1").unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Album One");

        let tracks = db.tracks_by_album("al1").unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Track One");
        assert_eq!(tracks[0].duration, 240);

        let restored = db.tracks_by_ids(&["t1".to_string()]).unwrap();
        assert_eq!(restored[0].id, "t1");
    }

    #[test]
    fn wal_mode_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db")).unwrap();
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }
}
