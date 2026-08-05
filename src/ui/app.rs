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
    Favorites,
    Queue,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Artists,
        Tab::Albums,
        Tab::Playlists,
        Tab::Favorites,
        Tab::Queue,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Artists => "Artists",
            Tab::Albums => "Albums",
            Tab::Playlists => "Playlists",
            Tab::Favorites => "Favorites",
            Tab::Queue => "Queue",
        }
    }

    pub fn next(&self) -> Tab {
        let i = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(&self) -> Tab {
        let i = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
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
    /// Id зазірочених треків (для позначки ♥ у списках).
    pub starred_ids: std::collections::HashSet<String>,
    /// Pagination of the Albums tab.
    pub albums_offset: usize,
    pub albums_exhausted: bool,
    /// Queue tab is freshly opened: snap selection onto the current track.
    queue_just_opened: bool,
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
            starred_ids: std::collections::HashSet::new(),
            albums_offset: 0,
            albums_exhausted: false,
            queue_just_opened: false,
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

    /// Вибраний у списку трек (для запуску DJ клавішею `d`).
    pub fn selected_track(&self) -> Option<TrackMeta> {
        match self.selected_item() {
            Some(ListItem::Track(t)) => Some(t.clone()),
            _ => None,
        }
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
            let artist_ids: Vec<String> = artists.iter().map(|a| a.id.clone()).collect();
            db.prune_artists(&artist_ids)?;
            let albums = client.get_all_albums()?;
            db.upsert_albums(&albums)?;
            let album_ids: Vec<String> = albums.iter().map(|a| a.id.clone()).collect();
            db.prune_albums(&album_ids)?;
            let playlists = client.get_playlists()?;
            db.upsert_playlists(&playlists)?;
            let playlist_ids: Vec<String> = playlists.iter().map(|p| p.id.clone()).collect();
            db.prune_playlists(&playlist_ids)?;
            db.prune_orphan_tracks()?;
            let starred = client.get_starred2()?;
            db.sync_starred(&starred)?;
            self.refresh_starred(db)?;
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
            Tab::Favorites => {
                let tracks = db.starred_tracks()?;
                self.ctx_tracks = tracks.iter().map(TrackMeta::from).collect();
                self.set_list(
                    tracks.iter().map(items_from_track).collect(),
                    format!("Favorites ({})", tracks.len()),
                );
            }
            Tab::Queue => {
                // Живий список наповнюється з main-циклу (engine.queue()).
                self.queue_just_opened = true;
                self.set_list(Vec::new(), "Queue");
            }
        }
        Ok(())
    }

    /// Живий список черги движка для вкладки Queue (викликається кожен тік).
    pub fn sync_queue(&mut self, engine: &Engine) {
        let sel = self.selected;
        self.list = engine.queue().iter().cloned().map(ListItem::Track).collect();
        self.list_title = format!("Queue ({})", self.list.len());
        if self.list.is_empty() {
            self.selected = 0;
            self.queue_just_opened = false;
            return;
        }
        if self.queue_just_opened {
            self.queue_just_opened = false;
            if let Some(cur) = engine.current()
                && let Some(idx) = self
                    .list
                    .iter()
                    .position(|i| matches!(i, ListItem::Track(t) if t.id == cur.id))
            {
                self.selected = idx;
                return;
            }
        }
        self.selected = sel.min(self.list.len() - 1);
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
            ListItem::Album { id, name, .. } => self.open_album(client, db, &id, &name),
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
            ListItem::Playlist { id, name, .. } => self.open_playlist(client, db, &id, &name),
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

    /// Відкрити альбом: показати треки (без відтворення — реалізується Enter на треці).
    fn open_album(&mut self, client: &Client, db: &mut Db, id: &str, name: &str) -> Result<()> {
        let tracks = db.tracks_by_album(id)?;
        let tracks = if tracks.is_empty() {
            let album = client.get_album(id)?;
            let children: Vec<_> = album.song.iter().filter(|c| !c.is_dir.unwrap_or(false)).cloned().collect();
            db.upsert_album_tracks(id, &children)?;
            db.tracks_by_album(id)?
        } else {
            tracks
        };
        self.ctx_tracks = tracks.iter().map(TrackMeta::from).collect();
        self.push_level();
        self.set_list(tracks.iter().map(items_from_track).collect(), name);
        Ok(())
    }

    fn open_playlist(
        &mut self,
        client: &Client,
        db: &mut Db,
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
        // Відкриваємо список треків плейліста; відтворення — Enter на треку.
        self.ctx_tracks = tracks.iter().map(TrackMeta::from).collect();
        self.push_level();
        self.set_list(tracks.iter().map(items_from_track).collect(), name);
        Ok(())
    }

    pub fn queue_and_play(&self, engine: &mut Engine, tracks: Vec<TrackMeta>, start: usize) {
        engine.set_queue(tracks, start);
    }

    // ---------- favorites ----------

    /// Перезавантажити множину зазірочених id з кешу (для позначки ♥).
    pub fn refresh_starred(&mut self, db: &Db) -> Result<()> {
        self.starred_ids = db.starred_ids()?.into_iter().collect();
        Ok(())
    }

    /// Перемкнути favorite вибраного треку: server (star/unstar) → кеш →
    /// оновлення позначок. У вкладці Favorites зазірочений рядок прибирається
    /// на місці (вибір зберігається); інші вкладки не чіпаємо — сердечки
    /// малюються з `starred_ids` живцем.
    /// Повертає `Some(true)`, якщо тепер трек у favorites, `Some(false)` — якщо
    /// прибрано; `None` — якщо не було вибраного треку.
    pub fn toggle_favorite(&mut self, client: &Client, db: &mut Db) -> Result<Option<bool>> {
        let track = match self.selected_track() {
            Some(t) => t,
            None => {
                self.set_message("Favorites: select a track first (f)", true);
                return Ok(None);
            }
        };
        let now_starred = if self.starred_ids.contains(&track.id) {
            client.unstar(&track.id)?;
            db.unstar_track(&track.id)?;
            false
        } else {
            client.star(&track.id)?;
            db.star_track(&child_from_meta(&track))?;
            true
        };
        self.refresh_starred(db)?;
        if self.tab == Tab::Favorites && !now_starred {
            let before = self.selected;
            self.list.retain(|i| !matches!(i, ListItem::Track(t) if t.id == track.id));
            self.ctx_tracks.retain(|t| t.id != track.id);
            self.selected = before.min(self.list.len().saturating_sub(1));
            self.list_title = format!("Favorites ({})", self.list.len());
        }
        Ok(Some(now_starred))
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

/// TrackMeta → Child (для збереження favorite у кеші).
fn child_from_meta(t: &TrackMeta) -> crate::api::models::Child {
    crate::api::models::Child {
        id: t.id.clone(),
        title: Some(t.title.clone()),
        album: Some(t.album.clone()),
        album_id: Some(t.album_id.clone()),
        artist: Some(t.artist.clone()),
        cover_art: t.cover_art.clone(),
        duration: Some(t.duration),
        track: t.track_number,
        disc_number: t.disc_number,
        ..Default::default()
    }
}

pub fn list_row_style(selected: bool) -> Style {
    if selected {
        theme::selected()
    } else {
        theme::base()
    }
}
