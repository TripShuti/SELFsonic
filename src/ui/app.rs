//! Стан UI + дії, що працюють з клієнтом/кешем/движком.

use ratatui::style::Style;

use crate::api::client::Client;
use crate::api::models::ArtistId3;
use crate::cache::db::{CachedAlbum, CachedTrack, Db};
use crate::error::Result;
use crate::playback::engine::{Engine, TrackMeta};
use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Artists,
    Albums,
    Playlists,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Artists, Tab::Albums, Tab::Playlists];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Artists => "Artists",
            Tab::Albums => "Albums",
            Tab::Playlists => "Playlists",
        }
    }

    pub fn next(&self) -> Tab {
        match self {
            Tab::Artists => Tab::Albums,
            Tab::Albums => Tab::Playlists,
            Tab::Playlists => Tab::Artists,
        }
    }

    pub fn prev(&self) -> Tab {
        match self {
            Tab::Artists => Tab::Playlists,
            Tab::Albums => Tab::Artists,
            Tab::Playlists => Tab::Albums,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ListItem {
    Artist {
        id: String,
        name: String,
        album_count: Option<i32>,
    },
    Album {
        id: String,
        name: String,
        artist: String,
        year: Option<i32>,
        duration: i32,
    },
    Track(TrackMeta),
    Playlist {
        id: String,
        name: String,
        song_count: i32,
    },
    More,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub text: String,
    pub is_error: bool,
}

pub struct AppState {
    pub tab: Tab,
    pub list: Vec<ListItem>,
    pub selected: usize,
    pub list_title: String,
    nav_stack: Vec<(Tab, Vec<ListItem>, usize, String)>,
    pub loading: bool,
    pub message: Option<Message>,
    /// Context tracks of the current level (for Enter playback).
    ctx_tracks: Vec<TrackMeta>,
    /// Pagination of the Albums tab.
    pub albums_offset: usize,
    pub albums_exhausted: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            tab: Tab::Artists,
            list: Vec::new(),
            selected: 0,
            list_title: String::new(),
            nav_stack: Vec::new(),
            loading: false,
            message: None,
            ctx_tracks: Vec::new(),
            albums_offset: 0,
            albums_exhausted: false,
        }
    }

    // ---------- список ----------

    pub fn set_list(&mut self, items: Vec<ListItem>, title: impl Into<String>) {
        self.list = items;
        self.selected = 0;
        self.list_title = title.into();
    }

    fn push_level(&mut self) {
        if self.list.is_empty() {
            return;
        }
        self.nav_stack
            .push((self.tab, std::mem::take(&mut self.list), self.selected, std::mem::take(&mut self.list_title)));
        self.selected = 0;
    }

    pub fn back(&mut self) {
        if let Some((tab, list, selected, title)) = self.nav_stack.pop() {
            self.tab = tab;
            self.list = list;
            self.selected = selected;
            self.list_title = title;
            self.ctx_tracks.clear();
        }
    }

    pub fn has_parent(&self) -> bool {
        !self.nav_stack.is_empty()
    }

    pub fn move_down(&mut self) {
        if !self.list.is_empty() {
            self.selected = (self.selected + 1).min(self.list.len() - 1);
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn page(&mut self, height: usize, down: bool) {
        if height == 0 || self.list.is_empty() {
            return;
        }
        let n = self.list.len().saturating_sub(1);
        self.selected = if down {
            (self.selected + height).min(n)
        } else {
            self.selected.saturating_sub(height)
        };
    }

    pub fn top(&mut self) {
        self.selected = 0;
    }

    pub fn bottom(&mut self) {
        self.selected = self.list.len().saturating_sub(1);
    }

    pub fn selected_item(&self) -> Option<&ListItem> {
        self.list.get(self.selected)
    }

    pub fn set_message(&mut self, text: impl Into<String>, is_error: bool) {
        self.message = Some(Message {
            text: text.into(),
            is_error,
        });
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    // ---------- дії з даними ----------

    /// Full manual refresh (AGENT.md: no polling).
    pub fn refresh(&mut self, client: &Client, db: &mut Db) -> Result<()> {
        self.loading = true;
        self.message = None;
        let result = (|| -> Result<()> {
            let artists = client.get_artists()?;
            db.upsert_artists(&artists)?;
            let albums = client.get_album_list2(0, 500)?;
            db.upsert_albums(&albums)?;
            let playlists = client.get_playlists()?;
            db.upsert_playlists(&playlists)?;
            Ok(())
        })();
        self.loading = false;
        match result {
            Ok(()) => {
                self.set_message("Library updated", false);
                self.load_current_tab_from_cache(db)?;
                Ok(())
            }
            Err(e) => {
                self.set_message(format!("Refresh error: {e}"), true);
                Err(e)
            }
        }
    }

    /// Switch tab, loading its content from cache.
    pub fn activate_tab(&mut self, tab: Tab, db: &mut Db) {
        self.tab = tab;
        let _ = self.load_current_tab_from_cache(db);
    }
    pub fn load_current_tab_from_cache(&mut self, db: &mut Db) -> Result<()> {
        match self.tab {
            Tab::Artists => {
                let artists = db.artists()?;
                self.set_list(items_from_artists(&artists), "Artists");
            }
            Tab::Albums => {
                self.albums_offset = 0;
                self.albums_exhausted = false;
                self.load_albums_from_cache(db)?;
            }
            Tab::Playlists => {
                let playlists = db.playlists()?;
                self.set_list(
                    playlists
                        .iter()
                        .map(|p| ListItem::Playlist {
                            id: p.id.clone(),
                            name: p.name.clone(),
                            song_count: p.song_count,
                        })
                        .collect(),
                    "Playlists",
                );
            }
        }
        Ok(())
    }

    pub fn load_albums_from_cache(&mut self, db: &mut Db) -> Result<()> {
        let albums = db.recent_albums(500)?;
        let mut items: Vec<ListItem> = albums.iter().map(items_from_cached_album).collect();
        if !self.albums_exhausted {
            items.push(ListItem::More);
        }
        self.set_list(items, "Albums");
        Ok(())
    }

    /// Load the next page of albums.
    pub fn load_more_albums(&mut self, client: &Client, db: &mut Db) -> Result<()> {
        let size = 200;
        let offset = self.albums_offset + 500;
        let albums = client.get_album_list2(offset as i32, size)?;
        if albums.is_empty() {
            self.albums_exhausted = true;
            self.load_albums_from_cache(db)?;
            return Ok(());
        }
        db.upsert_albums(&albums)?;
        self.albums_offset = offset;
        self.load_albums_from_cache(db)?;
        Ok(())
    }

    /// Enter по вибраному елементу.
    pub fn select_current(&mut self, client: &Client, db: &mut Db, engine: &mut Engine) -> Result<()> {
        let item = match self.selected_item() {
            Some(i) => i.clone(),
            None => return Ok(()),
        };
        self.clear_message();
        match item {
            ListItem::Artist { id, name, .. } => self.open_artist(client, db, &id, &name),
            ListItem::Album { id, .. } => self.play_album(client, db, engine, &id),
            ListItem::Track(t) => {
                // Відтворити вибраний трек у межах поточного контексту.
                let start = self
                    .ctx_tracks
                    .iter()
                    .position(|x| x.id == t.id)
                    .unwrap_or(0);
                self.queue_and_play(engine, self.ctx_tracks.clone(), start);
                Ok(())
            }
            ListItem::Playlist { id, name, .. } => self.open_playlist(client, db, engine, &id, &name),
            ListItem::More => self.load_more_albums(client, db),
        }
    }

    fn open_artist(&mut self, client: &Client, db: &mut Db, id: &str, name: &str) -> Result<()> {
        let albums = db.albums_by_artist(id)?;
        let albums = if albums.is_empty() {
            let artist = client.get_artist(id)?;
            db.upsert_artist_albums(&artist)?;
            db.albums_by_artist(id)?
        } else {
            albums
        };
        self.push_level();
        self.set_list(albums.iter().map(items_from_cached_album).collect(), name);
        Ok(())
    }

    fn play_album(&mut self, client: &Client, db: &mut Db, engine: &mut Engine, id: &str) -> Result<()> {
        let tracks = db.tracks_by_album(id)?;
        let tracks = if tracks.is_empty() {
            let album = client.get_album(id)?;
            let children: Vec<_> = album.song.iter().filter(|c| !c.is_dir.unwrap_or(false)).cloned().collect();
            db.upsert_album_tracks(id, &children)?;
            db.tracks_by_album(id)?
        } else {
            tracks
        };
        let metas: Vec<TrackMeta> = tracks.iter().map(TrackMeta::from).collect();
        self.ctx_tracks = metas.clone();
        self.queue_and_play(engine, metas, 0);
        Ok(())
    }

    fn open_playlist(
        &mut self,
        client: &Client,
        db: &mut Db,
        engine: &mut Engine,
        id: &str,
        name: &str,
    ) -> Result<()> {
        let tracks = db.playlist_tracks(id)?;
        let tracks = if tracks.is_empty() {
            let playlist = client.get_playlist(id)?;
            db.upsert_playlist_tracks(&playlist)?;
            db.playlist_tracks(id)?
        } else {
            tracks
        };
        let metas: Vec<TrackMeta> = tracks.iter().map(TrackMeta::from).collect();
        self.ctx_tracks = metas.clone();
        self.push_level();
        self.set_list(tracks.iter().map(items_from_track).collect(), name);
        self.queue_and_play(engine, metas, 0);
        Ok(())
    }

    pub fn queue_and_play(&self, engine: &mut Engine, tracks: Vec<TrackMeta>, start: usize) {
        engine.set_queue(tracks, start);
    }
}

// ---------- конвертери в ListItem ----------

fn items_from_artists(artists: &[ArtistId3]) -> Vec<ListItem> {
    artists
        .iter()
        .map(|a| ListItem::Artist {
            id: a.id.clone(),
            name: a.name.clone(),
            album_count: a.album_count,
        })
        .collect()
}

fn items_from_cached_album(a: &CachedAlbum) -> ListItem {
    ListItem::Album {
        id: a.id.clone(),
        name: a.name.clone(),
        artist: a.artist.clone(),
        year: a.year,
        duration: a.duration,
    }
}

fn items_from_track(t: &CachedTrack) -> ListItem {
    ListItem::Track(TrackMeta::from(t))
}

pub fn list_row_style(selected: bool) -> Style {
    if selected {
        theme::selected()
    } else {
        theme::base()
    }
}
