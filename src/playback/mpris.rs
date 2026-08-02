//! MPRIS (D-Bus): `org.mpris.MediaPlayer2.SELFsonic`.
//!
//! Використовуємо `mpris-server` для Root/Player інтерфейсів; TrackList
//! реалізовано вручну через `LocalServer::new_with_track_list` + zbus
//! (AGENT.md). Все працює на окремому потоці, комунікація — канали.

use std::cell::{Cell, RefCell};
use std::rc::Weak;
use std::sync::mpsc::{Receiver, Sender};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::{FutureExt as _, StreamExt as _};
use mpris_server::{
    LocalRootInterface, LocalPlayerInterface, LocalServer, LocalTrackListInterface, LoopStatus,
    Metadata, PlaybackRate, PlaybackStatus, Property, Time, TrackId, TrackListProperty,
    TrackListSignal, Uri, Volume,
};
use mpris_server::zbus::{self, fdo};
use tracing::{debug, warn};

use crate::playback::engine::{LoopMode, TrackMeta};

const BUS_SUFFIX: &str = "SELFsonic";
const TRACK_PREFIX: &str = "/org/mpris/MediaPlayer2/Track/";

/// Команди від MPRIS до головного циклу (движка).
#[derive(Debug)]
pub enum PlayerCommand {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    Stop,
    Seek { offset_us: i64 },
    SetPosition { track_id: String, position_us: i64 },
    OpenUri(String),
    SetLoopStatus(LoopStatus),
    SetShuffle(bool),
    SetVolume(Volume),
    GoTo(String),
    Quit,
}

/// Оновлення стану з головного циклу в MPRIS-потік.
#[derive(Debug)]
pub enum MprisUpdate {
    PlaybackStatus(PlaybackStatus),
    Metadata(Metadata),
    Position(Time),
    Volume(Volume),
    LoopStatus(LoopStatus),
    Shuffle(bool),
    CanGoNext(bool),
    CanGoPrevious(bool),
    CanPlay(bool),
    CanPause(bool),
    CanSeek(bool),
    TrackList {
        tracks: Vec<(TrackId, Metadata)>,
        current: Option<TrackId>,
    },
    Shutdown,
}

pub struct Mpris {
    cmd_rx: Receiver<PlayerCommand>,
    upd_tx: UnboundedSender<MprisUpdate>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Mpris {
    /// Спроба підняти MPRIS-сервер. Якщо D-Bus недоступний або ім'я зайняте —
    /// повертає Err (клієнт може продовжити без MPRIS).
    pub fn spawn() -> Result<Self, String> {
        let (upd_tx, upd_rx) = futures::channel::mpsc::unbounded();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("selfsonic-mpris".into())
            .spawn(move || mpris_thread(cmd_tx, upd_rx))
            .map_err(|e| format!("thread: {e}"))?;
        Ok(Self {
            cmd_rx,
            upd_tx,
            thread: Some(thread),
        })
    }

    pub fn update(&self, update: MprisUpdate) {
        self.upd_tx.unbounded_send(update).ok();
    }

    pub fn commands(&self) -> &Receiver<PlayerCommand> {
        &self.cmd_rx
    }

    pub fn shutdown(&mut self) {
        self.update(MprisUpdate::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn mpris_thread(
    cmd_tx: Sender<PlayerCommand>,
    upd_rx: UnboundedReceiver<MprisUpdate>,
) {
    zbus::block_on(async move {
        let state = MprisState {
            server: RefCell::new(None),
            cmd_tx,
            playback_status: Cell::new(PlaybackStatus::Stopped),
            loop_status: Cell::new(LoopStatus::None),
            shuffle: Cell::new(false),
            metadata: RefCell::new(Metadata::new()),
            volume: Cell::new(Volume::default()),
            position: Cell::new(Time::ZERO),
            can_go_next: Cell::new(false),
            can_go_previous: Cell::new(false),
            can_play: Cell::new(false),
            can_pause: Cell::new(false),
            can_seek: Cell::new(false),
            tracks: RefCell::new(Vec::new()),
            track_metadata: RefCell::new(std::collections::HashMap::new()),
        };
        let server = match LocalServer::new_with_track_list(BUS_SUFFIX, state).await {
            Ok(s) => std::rc::Rc::new(s),
            Err(e) => {
                warn!("MPRIS unavailable (continuing without it): {e}");
                return;
            }
        };
        server.imp().server.replace(Some(std::rc::Rc::downgrade(&server)));
        debug!("MPRIS: {}", server.bus_name());

        let mut run_task = server.run().fuse();
        let mut upd_rx = upd_rx;
        loop {
            futures::select! {
                _ = run_task => break,
                upd = upd_rx.next() => {
                    match upd {
                        Some(update) => {
                            if server.imp().apply(update).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = server.release_bus_name().await;
    });
}

// ---------- стан ----------

struct MprisState {
    server: RefCell<Option<Weak<LocalServer<MprisState>>>>,
    cmd_tx: Sender<PlayerCommand>,
    playback_status: Cell<PlaybackStatus>,
    loop_status: Cell<LoopStatus>,
    shuffle: Cell<bool>,
    metadata: RefCell<Metadata>,
    volume: Cell<Volume>,
    position: Cell<Time>,
    can_go_next: Cell<bool>,
    can_go_previous: Cell<bool>,
    can_play: Cell<bool>,
    can_pause: Cell<bool>,
    can_seek: Cell<bool>,
    tracks: RefCell<Vec<TrackId>>,
    track_metadata: RefCell<std::collections::HashMap<TrackId, Metadata>>,
}

impl MprisState {
    fn server(&self) -> Option<std::rc::Rc<LocalServer<MprisState>>> {
        self.server.borrow().as_ref()?.upgrade()
    }

    fn send(&self, cmd: PlayerCommand) {
        self.cmd_tx.send(cmd).ok();
    }

    async fn apply(&self, update: MprisUpdate) -> zbus::Result<()> {
        let Some(server) = self.server() else {
            return Ok(());
        };
        match update {
            MprisUpdate::PlaybackStatus(s) => {
                if self.playback_status.get() != s {
                    self.playback_status.set(s);
                    server
                        .properties_changed([Property::PlaybackStatus(s)])
                        .await?;
                }
            }
            MprisUpdate::Metadata(m) => {
                if *self.metadata.borrow() != m {
                    self.metadata.replace(m.clone());
                    server.properties_changed([Property::Metadata(m)]).await?;
                }
            }
            MprisUpdate::Position(p) => {
                self.position.set(p);
            }
            MprisUpdate::Volume(v) => {
                if self.volume.get() != v {
                    self.volume.set(v);
                    server.properties_changed([Property::Volume(v)]).await?;
                }
            }
            MprisUpdate::LoopStatus(l) => {
                if self.loop_status.get() != l {
                    self.loop_status.set(l);
                    server.properties_changed([Property::LoopStatus(l)]).await?;
                }
            }
            MprisUpdate::Shuffle(s) => {
                if self.shuffle.get() != s {
                    self.shuffle.set(s);
                    server.properties_changed([Property::Shuffle(s)]).await?;
                }
            }
            MprisUpdate::CanGoNext(v) => {
                if self.can_go_next.get() != v {
                    self.can_go_next.set(v);
                    server.properties_changed([Property::CanGoNext(v)]).await?;
                }
            }
            MprisUpdate::CanGoPrevious(v) => {
                if self.can_go_previous.get() != v {
                    self.can_go_previous.set(v);
                    server.properties_changed([Property::CanGoPrevious(v)]).await?;
                }
            }
            MprisUpdate::CanPlay(v) => {
                if self.can_play.get() != v {
                    self.can_play.set(v);
                    server.properties_changed([Property::CanPlay(v)]).await?;
                }
            }
            MprisUpdate::CanPause(v) => {
                if self.can_pause.get() != v {
                    self.can_pause.set(v);
                    server.properties_changed([Property::CanPause(v)]).await?;
                }
            }
            MprisUpdate::CanSeek(v) => {
                if self.can_seek.get() != v {
                    self.can_seek.set(v);
                    server.properties_changed([Property::CanSeek(v)]).await?;
                }
            }
            MprisUpdate::TrackList { tracks, current } => {
                let ids: Vec<TrackId> = tracks.iter().map(|(id, _)| id.clone()).collect();
                let mut map = std::collections::HashMap::new();
                for (id, meta) in &tracks {
                    map.insert(id.clone(), meta.clone());
                }
                *self.tracks.borrow_mut() = ids.clone();
                *self.track_metadata.borrow_mut() = map;
                server
                    .track_list_emit(TrackListSignal::TrackListReplaced {
                        tracks: ids,
                        current_track: current.unwrap_or(TrackId::NO_TRACK),
                    })
                    .await?;
                server
                    .track_list_properties_changed([TrackListProperty::Tracks])
                    .await?;
            }
            MprisUpdate::Shutdown => return Err(zbus::Error::Failure("shutdown".into())),
        }
        Ok(())
    }
}

impl LocalRootInterface for MprisState {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Quit);
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("SELFsonic".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("SELFsonic".into())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec!["http".into(), "https".into()])
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(vec![
            "audio/flac".into(),
            "audio/mpeg".into(),
            "audio/ogg".into(),
            "application/ogg".into(),
            "audio/opus".into(),
            "audio/wav".into(),
        ])
    }
}

impl LocalPlayerInterface for MprisState {
    async fn next(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Next);
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Previous);
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Pause);
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::PlayPause);
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Stop);
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        self.send(PlayerCommand::Play);
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        self.send(PlayerCommand::Seek {
            offset_us: offset.as_micros(),
        });
        Ok(())
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        self.send(PlayerCommand::SetPosition {
            track_id: track_id.to_string(),
            position_us: position.as_micros(),
        });
        Ok(())
    }

    async fn open_uri(&self, uri: String) -> fdo::Result<()> {
        self.send(PlayerCommand::OpenUri(uri));
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.playback_status.get())
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(self.loop_status.get())
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> zbus::Result<()> {
        self.send(PlayerCommand::SetLoopStatus(loop_status));
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.shuffle.get())
    }

    async fn set_shuffle(&self, shuffle: bool) -> zbus::Result<()> {
        self.send(PlayerCommand::SetShuffle(shuffle));
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.metadata.borrow().clone())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.volume.get())
    }

    async fn set_volume(&self, volume: Volume) -> zbus::Result<()> {
        self.send(PlayerCommand::SetVolume(volume.max(0.0)));
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(self.position.get())
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.can_go_next.get())
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.can_go_previous.get())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.can_play.get())
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(self.can_pause.get())
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.can_seek.get())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

impl LocalTrackListInterface for MprisState {
    async fn get_tracks_metadata(
        &self,
        track_ids: Vec<TrackId>,
    ) -> fdo::Result<Vec<Metadata>> {
        let map = self.track_metadata.borrow();
        Ok(track_ids
            .iter()
            .map(|id| map.get(id).cloned().unwrap_or_default())
            .collect())
    }

    async fn add_track(
        &self,
        _uri: Uri,
        _after_track: TrackId,
        _set_as_current: bool,
    ) -> fdo::Result<()> {
        Ok(())
    }

    async fn remove_track(&self, _track_id: TrackId) -> fdo::Result<()> {
        Ok(())
    }

    async fn go_to(&self, track_id: TrackId) -> fdo::Result<()> {
        self.send(PlayerCommand::GoTo(track_id.to_string()));
        Ok(())
    }

    async fn tracks(&self) -> fdo::Result<Vec<TrackId>> {
        Ok(self.tracks.borrow().clone())
    }

    async fn can_edit_tracks(&self) -> fdo::Result<bool> {
        Ok(false)
    }
}

// ---------- хелпери ----------

pub fn subsonic_track_id(id: &str) -> TrackId {
    TrackId::try_from(format!("{TRACK_PREFIX}{id}")).expect("subsonic id is a valid path")
}

/// Повертає subsonic id з MPRIS TrackId.
pub fn track_id_to_subsonic(track_id: &TrackId) -> Option<String> {
    let s = track_id.to_string();
    s.strip_prefix(TRACK_PREFIX).map(|id| id.to_string())
}

pub fn build_metadata(track: &TrackMeta, art_url: Option<String>) -> Metadata {
    let mut m = Metadata::new();
    m.set_trackid(Some(subsonic_track_id(&track.id)));
    if track.duration > 0 {
        m.set_length(Some(Time::from_secs(track.duration as i64)));
    }
    if !track.title.is_empty() {
        m.set_title(Some(track.title.clone()));
    }
    if !track.artist.is_empty() {
        m.set_artist(Some(vec![track.artist.clone()]));
    }
    if !track.album.is_empty() {
        m.set_album(Some(track.album.clone()));
        m.set_album_artist(Some(vec![track.artist.clone()]));
    }
    if let Some(url) = art_url {
        m.set_art_url(Some(url));
    }
    if let Some(n) = track.track_number {
        m.set_track_number(Some(n));
    }
    if let Some(n) = track.disc_number {
        m.set_disc_number(Some(n));
    }
    m
}

pub fn loop_status_to_mpris(mode: &LoopMode) -> LoopStatus {
    match mode {
        LoopMode::None => LoopStatus::None,
        LoopMode::Track => LoopStatus::Track,
        LoopMode::Playlist => LoopStatus::Playlist,
    }
}

pub fn loop_status_from_mpris(status: LoopStatus) -> LoopMode {
    match status {
        LoopStatus::None => LoopMode::None,
        LoopStatus::Track => LoopMode::Track,
        LoopStatus::Playlist => LoopMode::Playlist,
    }
}

#[cfg(test)]
mod live_probe_tests {
    use super::*;
    use std::time::Duration;

    /// Піднімає MPRIS-сервер з TrackList на ~90с — поки працює, опитати можна
    /// з шела: `python3 tracklist.py --player SELFsonic list/metadata`.
    #[test]
    #[ignore]
    fn live_tracklist_probe() {
        let mut m = Mpris::spawn().expect("mpris");
        let meta1 = build_metadata(
            &crate::playback::engine::TrackMeta {
                id: "aaa".into(),
                title: "Перший трек".into(),
                artist: "Артист A".into(),
                album: "Альбом X".into(),
                album_id: "al1".into(),
                cover_art: None,
                duration: 200,
                track_number: Some(1),
                disc_number: None,
            },
            None,
        );
        let meta2 = build_metadata(
            &crate::playback::engine::TrackMeta {
                id: "bbb".into(),
                title: "Другий трек".into(),
                artist: "Артист B".into(),
                album: "Альбом X".into(),
                album_id: "al1".into(),
                cover_art: None,
                duration: 180,
                track_number: Some(2),
                disc_number: None,
            },
            None,
        );
        m.update(MprisUpdate::TrackList {
            tracks: vec![
                (subsonic_track_id("aaa"), meta1),
                (subsonic_track_id("bbb"), meta2),
            ],
            current: Some(subsonic_track_id("aaa")),
        });
        m.update(MprisUpdate::PlaybackStatus(PlaybackStatus::Playing));
        println!("probe ready");
        std::thread::sleep(Duration::from_secs(90));
        m.shutdown();
    }
}
