//! Движок відтворення: rodio `Player` + черга + gapless (попереднє декодування
//! наступного треку) + спостереження за кінцем треку без блокування.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;
use rodio::Source;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::api::client::{Client, StreamFile};
use crate::api::models::Child;
use crate::cache::db::CachedTrack;
use crate::error::{AppError, Result};

pub type Decoder = rodio::Decoder<StreamFile>;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LoopMode {
    #[default]
    None,
    Track,
    Playlist,
}

impl LoopMode {
    pub fn cycle(&self) -> Self {
        match self {
            Self::None => Self::Track,
            Self::Track => Self::Playlist,
            Self::Playlist => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: String,
    pub cover_art: Option<String>,
    pub duration: i32,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
}

impl From<&CachedTrack> for TrackMeta {
    fn from(t: &CachedTrack) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            album_id: t.album_id.clone(),
            cover_art: t.cover_art.clone(),
            duration: t.duration,
            track_number: t.track_number,
            disc_number: t.disc_number,
        }
    }
}

impl From<&Child> for TrackMeta {
    fn from(c: &Child) -> Self {
        Self {
            id: c.id.clone(),
            title: c.title.clone().unwrap_or_default(),
            artist: c.artist.clone().unwrap_or_default(),
            album: c.album.clone().unwrap_or_default(),
            album_id: c.album_id.clone().unwrap_or_default(),
            cover_art: c.cover_art.clone(),
            duration: c.duration.unwrap_or(0),
            track_number: c.track,
            disc_number: c.disc_number,
        }
    }
}

/// Мінімальний час відтворення для зарахування прослуховування (Last.fm): 4 хвилини.
const SCROBBLE_MIN_PLAY_SECS: u64 = 4 * 60;
/// Вікно одразу після перемикання треку, коли rodio ще може віддавати позицію
/// попереднього — скроблимо лише після нього, щоб не зарахувати новий трек.
const SCROBBLE_SWITCH_GUARD: Duration = Duration::from_millis(200);

/// Чи настав час зараховувати прослуховування (submission=true)?
/// Поріг: половина тривалості або 4 хвилини відтворення — що настане раніше.
fn should_scrobble(position: Duration, duration: Duration) -> bool {
    position >= scrobble_threshold(duration)
}

/// Поріг зарахування: `min(тривалість / 2, 4 хв)`. Невідома тривалість (0)
/// трактується як довгий трек — тоді спрацьовує 4-хвилинний поріг.
fn scrobble_threshold(duration: Duration) -> Duration {
    let half = duration / 2;
    let min_play = Duration::from_secs(SCROBBLE_MIN_PLAY_SECS);
    if half.is_zero() {
        min_play
    } else {
        half.min(min_play)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// Старт нового треку (id індексу черги).
    TrackStarted { queue_index: usize },
    /// Поточний трек закінчився.
    TrackEnded,
    /// Черга вичерпана (без loop).
    QueueFinished,
    /// Не вдалося завантажити трек.
    LoadFailed(String),
}

/// Стан для збереження між запусками.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineState {
    pub volume: f32,
    pub shuffle: bool,
    pub loop_mode: LoopMode,
    pub queue: Vec<String>,
    pub queue_pos: usize,
}

/// Обгортка джерела, що сигналізує про кінець треку через `AtomicBool`.
/// Опрос відбувається окремим потоком (не блокує audio callback).
struct EndWatcher<S> {
    inner: S,
    ended: Arc<AtomicBool>,
}

impl<S: Source> Iterator for EndWatcher<S> {
    type Item = S::Item;
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.inner.next();
        if item.is_none() {
            self.ended.store(true, Ordering::SeqCst);
        }
        item
    }
}

impl<S: Source> Source for EndWatcher<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> std::result::Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

pub struct Engine {
    client: Arc<Client>,
    _sink: rodio::stream::MixerDeviceSink,
    player: rodio::Player,
    volume: f32,
    paused: bool,
    stopped: bool,
    loop_mode: LoopMode,
    shuffle: bool,

    queue: Vec<TrackMeta>,
    /// Порядок відтворення (identity або перестановка для shuffle).
    order: Vec<usize>,
    pos: usize,
    current: Option<TrackMeta>,

    /// Попередньо декодований наступний трек (з id для перевірки відповідності).
    next_ready: Arc<Mutex<Option<ReadyTrack<Decoder>>>>,
    preload_gen: Arc<AtomicUsize>,
    preload_tx: std::sync::mpsc::Sender<PreloadCmd>,
    watcher_flag: Arc<AtomicBool>,
    events: std::sync::mpsc::Receiver<EngineEvent>,
    event_tx: std::sync::mpsc::Sender<EngineEvent>,
    end_tx: std::sync::mpsc::Sender<Arc<AtomicBool>>,
    scrobble_tx: std::sync::mpsc::Sender<ScrobbleCmd>,
    /// Чи зараховано поточний трек (submission=true) — скидається на TrackStarted.
    scrobbled: bool,
    /// Момент старту поточного треку (захист від стрибка позиції при перемиканні).
    track_started_at: Instant,
    /// Лічильник змін черги (для MPRIS TrackList).
    queue_gen: usize,
    /// DJ-режим: черга автоматично дозаповнюється схожими треками.
    dj_enabled: bool,
    /// «Якір» DJ — трек, що тримає тематику підбору (спершу сідо).
    dj_anchor: Option<TrackMeta>,
    /// Треки, додані випадковим фолбеком (не мають рухати якір).
    dj_random_ids: HashSet<String>,
}

enum PreloadCmd {
    Decode { track: TrackMeta, gen_id: usize },
    Shutdown,
}

/// Попередньо декодований трек: id зберігаємо, щоб `take_ready` не віддав
/// декодер чужого треку (навігація могла змінити ціль після запиту прелода).
struct ReadyTrack<T> {
    id: String,
    value: T,
}

/// Рішення `take_ready` щодо пре-декодованого треку.
#[derive(Debug)]
enum TakeReady<T> {
    /// Слот містив саме потрібний трек — значення вилучено.
    Consumed(T),
    /// Слот містив інший трек — викинуто (щоб не зіграти чужу музику).
    Discarded(String),
    /// Слот порожній — треба декодувати свіжо.
    Empty,
}

/// Виймає пре-декодоване значення лише якщо воно належить треку `want_id`.
/// Чуже значення викидається: назва поточного (`engine.current`) завжди
/// відповідає аудіо, що грає.
fn take_matching<T>(slot: &Mutex<Option<ReadyTrack<T>>>, want_id: &str) -> TakeReady<T> {
    let Some(ready) = slot.lock().expect("next_ready lock").take() else {
        return TakeReady::Empty;
    };
    if ready.id == want_id {
        TakeReady::Consumed(ready.value)
    } else {
        TakeReady::Discarded(ready.id)
    }
}

enum ScrobbleCmd {
    /// Надіслати scrobble-запит: `submission=false` — Now Playing, `true` — зарахування.
    Send { id: String, submission: bool },
    Shutdown,
}

impl Engine {
    pub fn new(client: Arc<Client>, volume: f32) -> Result<Self> {
        let sink = rodio::speakers::SpeakersBuilder::new()
            .default_device()
            .map_err(|e| AppError::Audio(format!("audio device: {e}")))?
            .default_config()
            .map_err(|e| AppError::Audio(format!("audio config: {e}")))?
            .open_mixer()
            .map_err(|e| AppError::Audio(format!("open mixer: {e}")))?;
        let player = rodio::Player::connect_new(sink.mixer());

        let (event_tx, events) = std::sync::mpsc::channel();
        let (preload_tx, preload_rx) = std::sync::mpsc::channel();
        let (end_tx, end_rx) = std::sync::mpsc::channel();
        let (scrobble_tx, scrobble_rx) = std::sync::mpsc::channel();
        let next_ready = Arc::new(Mutex::new(None));
        let preload_gen = Arc::new(AtomicUsize::new(0));

        spawn_preloader(preload_rx, next_ready.clone(), preload_gen.clone(), client.clone());
        spawn_end_monitor(end_rx, event_tx.clone());
        spawn_scrobbler(scrobble_rx, client.clone());

        let engine = Self {
            client,
            _sink: sink,
            player,
            volume,
            paused: false,
            stopped: true,
            loop_mode: LoopMode::None,
            shuffle: false,
            queue: Vec::new(),
            order: Vec::new(),
            pos: 0,
            current: None,
            next_ready,
            preload_gen,
            preload_tx,
            watcher_flag: Arc::new(AtomicBool::new(false)),
            events,
            event_tx,
            end_tx,
            scrobble_tx,
            scrobbled: false,
            track_started_at: Instant::now(),
            queue_gen: 0,
            dj_enabled: false,
            dj_anchor: None,
            dj_random_ids: HashSet::new(),
        };
        engine.player.set_volume(volume);
        Ok(engine)
    }

    // ---------- стан ----------

    pub fn current(&self) -> Option<&TrackMeta> {
        self.current.as_ref()
    }

    pub fn queue(&self) -> &[TrackMeta] {
        &self.queue
    }

    pub fn queue_pos(&self) -> usize {
        self.pos
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode.clone()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    /// Перевірка моменту зарахування (submission=true), викликається з UI-циклу
    /// що ~50мс. Сам мережевий виклик іде у фоновому потоці — UI не блокується.
    pub fn maybe_scrobble(&mut self) {
        if self.scrobbled || self.stopped {
            return;
        }
        let Some(track) = self.current.as_ref() else { return };
        // rodio одразу після перемикання треку ще віддає позицію старого
        // (див. PositionGuard у main.rs) — чекаємо вікно перемикання.
        if self.track_started_at.elapsed() < SCROBBLE_SWITCH_GUARD {
            return;
        }
        let duration = Duration::from_secs(track.duration.max(0) as u64);
        if should_scrobble(self.position(), duration) {
            self.scrobbled = true;
            self.scrobble_tx
                .send(ScrobbleCmd::Send { id: track.id.clone(), submission: true })
                .ok();
        }
    }

    pub fn state_snapshot(&self) -> EngineState {
        EngineState {
            volume: self.volume,
            shuffle: self.shuffle,
            loop_mode: self.loop_mode.clone(),
            queue: self.queue.iter().map(|t| t.id.clone()).collect(),
            queue_pos: self.pos,
        }
    }

    pub fn save_state(&self, path: &Path) {
        let state = self.state_snapshot();
        if let Err(e) = save_state_json(path, &state) {
            warn!("failed to save state: {e}");
        }
    }

    pub fn restore_state(&mut self, state: &EngineState, tracks: Vec<TrackMeta>) {
        self.volume = state.volume.clamp(0.0, 1.0);
        self.player.set_volume(self.volume);
        self.shuffle = state.shuffle;
        self.loop_mode = state.loop_mode.clone();
        if !tracks.is_empty() && !state.queue.is_empty() {
            let mut queue: Vec<TrackMeta> = Vec::with_capacity(tracks.len());
            for id in &state.queue {
                if let Some(t) = tracks.iter().find(|t| &t.id == id) {
                    queue.push(t.clone());
                }
            }
            if !queue.is_empty() {
                let start = state.queue_pos.min(queue.len() - 1);
                self.set_queue_inner(queue, start);
            }
        }
    }

    // ---------- керування чергою ----------

    /// Замінює чергу і починає відтворення з `start` (не відтворює, якщо start >= len).
    pub fn set_queue(&mut self, tracks: Vec<TrackMeta>, start: usize) {
        let start = start.min(tracks.len().saturating_sub(1));
        self.set_queue_inner(tracks, start);
        self.stopped = false;
        self.play_track_at(start);
    }

    fn set_queue_inner(&mut self, tracks: Vec<TrackMeta>, start: usize) {
        self.queue = tracks;
        self.pos = start.min(self.queue.len().saturating_sub(1));
        self.rebuild_order();
        self.player.stop();
        self.player.clear();
        self.current = None;
        self.preload_gen.fetch_add(1, Ordering::SeqCst);
        self.queue_gen += 1;
        // Нова черга (альбом/плейліст/відновлення) завжди вимикає DJ.
        self.dj_enabled = false;
        self.dj_anchor = None;
        self.dj_random_ids.clear();
    }

    /// Лічильник змін черги (для синхронізації MPRIS TrackList).
    pub fn queue_gen(&self) -> usize {
        self.queue_gen
    }

    /// DJ-режим: черга дозаповнюється схожими треками (викликається з UI-циклу).
    pub fn set_dj(&mut self, enabled: bool) {
        self.dj_enabled = enabled;
    }

    pub fn dj_enabled(&self) -> bool {
        self.dj_enabled
    }

    /// Старт DJ: якір = сідо, множина random-треків чиста.
    pub fn start_dj(&mut self, seed: TrackMeta) {
        self.dj_enabled = true;
        self.dj_anchor = Some(seed);
        self.dj_random_ids.clear();
    }

    pub fn dj_anchor(&self) -> Option<&TrackMeta> {
        self.dj_anchor.as_ref()
    }

    pub fn set_dj_anchor(&mut self, track: TrackMeta) {
        self.dj_anchor = Some(track);
    }

    /// Позначити додані random-фолбеком треки (не стають новим якорем).
    pub fn mark_dj_random(&mut self, ids: &[String]) {
        self.dj_random_ids.extend(ids.iter().cloned());
    }

    pub fn unmark_dj_random(&mut self, id: &str) {
        self.dj_random_ids.remove(id);
    }

    pub fn is_dj_random(&self, id: &str) -> bool {
        self.dj_random_ids.contains(id)
    }

    /// Кількість треків у черзі після поточного (для рішення про дозаповнення).
    pub fn queue_remaining(&self) -> usize {
        self.order
            .len()
            .saturating_sub((self.pos + 1).min(self.order.len()))
    }

    /// Додає треки в кінець черги (DJ-дозаповнення) без скидання відтворення.
    /// Якщо движок був зупинений на вичерпаній черзі — відновлює відтворення
    /// з першого доданого треку. Нові треки в shuffle перемішуються між собою.
    pub fn append_tracks(&mut self, mut tracks: Vec<TrackMeta>) {
        if tracks.is_empty() {
            return;
        }
        if self.shuffle {
            tracks.shuffle(&mut rand::rng());
        }
        let base = self.queue.len();
        self.queue.extend(tracks);
        if self.order.is_empty() {
            self.order = (0..self.queue.len()).collect();
        } else {
            self.order.extend(base..self.queue.len());
        }
        self.preload_gen.fetch_add(1, Ordering::SeqCst);
        self.queue_gen += 1;
        if self.stopped && self.current.is_none() {
            // Черга була вичерпана — стартуємо з першого доданого треку.
            self.stopped = false;
            let idx = if self.pos + 1 < self.order.len() {
                self.pos + 1
            } else {
                self.pos
            };
            self.play_track_at(idx);
        } else {
            self.request_preload();
        }
    }

    fn rebuild_order(&mut self) {
        let n = self.queue.len();
        if n == 0 {
            self.order.clear();
            return;
        }
        let current_idx = self.order.get(self.pos).copied().unwrap_or(0);
        self.order = (0..n).collect();
        if self.shuffle && n > 1 {
            let mut rng = rand::rng();
            self.order.shuffle(&mut rng);
            // Залишаємо поточний трек на поточній позиції.
            if let Some(p) = self.order.iter().position(|&i| i == current_idx) {
                self.order.swap(self.pos, p);
            }
        }
    }

    fn play_track_at(&mut self, order_idx: usize) {
        if self.order.is_empty() {
            return;
        }
        self.pos = order_idx % self.order.len();
        let track_idx = self.order[self.pos];
        let track = self.queue[track_idx].clone();

        let decoder = match self.take_ready(&track) {
            Some(d) => d,
            None => match self.decode(&track) {
                Ok(d) => d,
                Err(e) => {
                    warn!("failed to load {}: {e}", track.title);
                    self.event_tx.send(EngineEvent::LoadFailed(track.title)).ok();
                    self.stopped = true;
                    self.current = None;
                    return;
                }
            },
        };

        if self.stopped {
            self.stopped = false;
        }
        self.player.append(EndWatcher {
            inner: decoder,
            ended: self.watcher_flag.clone(),
        });
        self.end_tx.send(self.watcher_flag.clone()).ok();
        // Новий флаг для наступного треку.
        self.watcher_flag = Arc::new(AtomicBool::new(false));

        self.current = Some(track);
        self.paused = false;
        self.scrobbled = false;
        self.track_started_at = Instant::now();
        // Now Playing (submission=false) — асинхронно, у фоновому потоці.
        if let Some(t) = self.current.as_ref() {
            self.scrobble_tx
                .send(ScrobbleCmd::Send { id: t.id.clone(), submission: false })
                .ok();
        }
        self.player.play();
        self.event_tx.send(EngineEvent::TrackStarted { queue_index: track_idx }).ok();
        self.request_preload();
    }

    fn take_ready(&mut self, track: &TrackMeta) -> Option<Decoder> {
        match take_matching::<Decoder>(&self.next_ready, &track.id) {
            TakeReady::Consumed(dec) => {
                debug!("used pre-decoded track {}", track.title);
                Some(dec)
            }
            TakeReady::Discarded(id) => {
                debug!("discarded stale preload '{}' (wanted {})", id, track.title);
                self.preload_gen.fetch_add(1, Ordering::SeqCst);
                None
            }
            TakeReady::Empty => {
                // Попереднє декодування могло ще йти — скасовуємо генерацію.
                self.preload_gen.fetch_add(1, Ordering::SeqCst);
                None
            }
        }
    }

    fn decode(&self, track: &TrackMeta) -> Result<Decoder> {
        let stream = self.client.stream(&track.id)?;
        // seekable=true: без нього symphonia вважає потік forward-only — перемотка назад не працює
        Decoder::builder()
            .with_seekable(true)
            .with_data(stream.reader)
            .build()
            .map_err(|e| AppError::Audio(format!("decode '{}': {e}", track.title)))
    }

    fn request_preload(&mut self) {
        let next = self.peek_next();
        if let Some(track) = next {
            let gen_id = next_preload_gen(&self.preload_gen);
            self.preload_tx
                .send(PreloadCmd::Decode { track, gen_id })
                .ok();
        }
    }

    /// Наступний трек з урахуванням loop-режиму (без зміни стану).
    fn peek_next(&self) -> Option<TrackMeta> {
        if self.order.is_empty() {
            return None;
        }
        let next = self.pos + 1;
        if next < self.order.len() {
            return self.queue.get(self.order[next]).cloned();
        }
        // У DJ-режимі кінець черги не зациклюється — дозаповнення відновить її сам.
        if self.dj_enabled {
            return None;
        }
        match self.loop_mode {
            LoopMode::Track => self.queue.get(self.order[self.pos]).cloned(),
            LoopMode::Playlist => self.queue.get(self.order[0]).cloned(),
            LoopMode::None => None,
        }
    }

    // ---------- керування відтворенням ----------

    pub fn toggle(&mut self) {
        if self.stopped {
            if self.current.is_some() {
                self.stopped = false;
                self.player.play();
                self.paused = false;
            } else if !self.queue.is_empty() {
                self.stopped = false;
                self.play_track_at(self.pos);
            }
            return;
        }
        if self.paused {
            self.player.play();
            self.paused = false;
        } else {
            self.player.pause();
            self.paused = true;
        }
    }

    /// Повна зупинка відтворення (також вимикає DJ).
    pub fn stop(&mut self) {
        self.shut_down();
        self.dj_enabled = false;
        self.dj_anchor = None;
        self.dj_random_ids.clear();
    }

    /// Внутрішня зупинка без вимкнення DJ — викликається на кінці черги
    /// у DJ-режимі: черга ще має бути дозаповнена і відтворення відновлено.
    fn shut_down(&mut self) {
        self.stopped = true;
        self.current = None;
        self.player.stop();
        self.player.clear();
        self.preload_gen.fetch_add(1, Ordering::SeqCst);
    }

    pub fn next(&mut self) {
        if self.order.is_empty() {
            return;
        }
        let n = self.order.len();
        let next = if self.loop_mode == LoopMode::Playlist {
            (self.pos + 1) % n
        } else if self.pos + 1 < n {
            self.pos + 1
        } else {
            self.event_tx.send(EngineEvent::QueueFinished).ok();
            self.stop();
            return;
        };
        // Ручний перехід — скидаємо поточний трек одразу.
        self.player.stop();
        self.play_track_at(next);
    }

    pub fn previous(&mut self) {
        if self.order.is_empty() {
            return;
        }
        let n = self.order.len();
        let prev = if self.pos == 0 {
            if self.loop_mode == LoopMode::Playlist {
                n - 1
            } else {
                self.pos
            }
        } else {
            self.pos - 1
        };
        self.player.stop();
        self.play_track_at(prev);
    }

    /// Відносне перемотування у секундах (від'ємне — назад).
    pub fn seek_relative(&mut self, delta_secs: i64) {
        let pos = self.position();
        let target = if delta_secs < 0 {
            pos.saturating_sub(Duration::from_secs(delta_secs.unsigned_abs()))
        } else {
            pos.saturating_add(Duration::from_secs(delta_secs as u64))
        };
        self.seek_to(target);
    }

    pub fn seek_to(&mut self, target: Duration) {
        if self.stopped {
            return;
        }
        if let Err(e) = self.player.try_seek(target) {
            warn!("seek failed: {e}");
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(self.volume);
    }

    pub fn cycle_loop(&mut self) -> LoopMode {
        self.loop_mode = self.loop_mode.cycle();
        self.loop_mode.clone()
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
    }

    pub fn toggle_shuffle(&mut self) -> bool {
        self.shuffle = !self.shuffle;
        self.rebuild_order();
        self.shuffle
    }

    /// Перехід до треку за індексом черги (для MPRIS GoTo).
    pub fn play_index(&mut self, queue_idx: usize) {
        if queue_idx >= self.queue.len() {
            return;
        }
        if let Some(order_pos) = self.order.iter().position(|&i| i == queue_idx) {
            self.player.stop();
            self.play_track_at(order_pos);
        }
    }

    /// Обробка подій движка (викликається з головного циклу).
    pub fn poll_events(&mut self) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                EngineEvent::TrackEnded => {
                    out.push(EngineEvent::TrackEnded);
                    if self.stopped {
                        continue;
                    }
                    if self.dj_enabled {
                        // DJ не повторює поточний трек (ігнорує LoopMode::Track)
                        // і не зациклює чергу — на кінці чекає дозаповнення.
                        if self.pos + 1 < self.order.len() {
                            self.play_track_at(self.pos + 1);
                        } else {
                            self.shut_down();
                        }
                        continue;
                    }
                    match self.loop_mode {
                        LoopMode::Track => self.play_track_at(self.pos),
                        LoopMode::Playlist => self.play_track_at(self.pos + 1),
                        LoopMode::None => {
                            if self.pos + 1 < self.order.len() {
                                self.play_track_at(self.pos + 1);
                            } else {
                                self.stop();
                                self.event_tx.send(EngineEvent::QueueFinished).ok();
                                out.push(EngineEvent::QueueFinished);
                            }
                        }
                    }
                }
                other => out.push(other),
            }
        }
        out
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.preload_tx.send(PreloadCmd::Shutdown).ok();
        self.scrobble_tx.send(ScrobbleCmd::Shutdown).ok();
    }
}

/// Виділяє наступний ідентифікатор генерації preload-декодування.
/// `fetch_add` повертає значення ДО інкременту, тож додаємо 1, щоб
/// `gen_id` збігався зі значенням атоміка вже на момент надсилання
/// команди — інакше перевірка `load() != my_gen` у `spawn_preloader`
/// хибно відкидає завжди актуальний preload.
fn next_preload_gen(counter: &AtomicUsize) -> usize {
    counter.fetch_add(1, Ordering::SeqCst) + 1
}

fn save_state_json(path: &Path, state: &EngineState) -> Result<()> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| AppError::Other(format!("state serialize: {e}")))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }
    std::fs::write(path, raw).map_err(|e| AppError::io(path.to_path_buf(), e))?;
    Ok(())
}

// ---------- фонові потоки ----------

/// Потік попереднього декодування: готує `Decoder` наступного треку,
/// поки грає поточний (мережеве читання не блокує UI).
fn spawn_preloader(
    rx: std::sync::mpsc::Receiver<PreloadCmd>,
    next_ready: Arc<Mutex<Option<ReadyTrack<Decoder>>>>,
    preload_gen: Arc<AtomicUsize>,
    client: Arc<Client>,
) {
    let _ = std::thread::Builder::new()
        .name("selfsonic-preload".into())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                let (track, my_gen) = match cmd {
                    PreloadCmd::Decode { track, gen_id } => (track, gen_id),
                    PreloadCmd::Shutdown => break,
                };
                let res = (|| -> Result<Decoder> {
                    let stream = client.stream(&track.id)?;
                    // seekable=true: без нього symphonia вважає потік forward-only — перемотка назад не працює
                    Decoder::builder()
                        .with_seekable(true)
                        .with_data(stream.reader)
                        .build()
                        .map_err(|e| AppError::Audio(format!("preload '{}': {e}", track.title)))
                })();
                if preload_gen.load(Ordering::SeqCst) != my_gen {
                    continue;
                }
                match res {
                    Ok(dec) => {
                        if let Ok(mut guard) = next_ready.lock() {
                            *guard = Some(ReadyTrack {
                                id: track.id.clone(),
                                value: dec,
                            });
                        }
                    }
                    Err(e) => warn!("preload '{}': {e}", track.title),
                }
            }
        });
}

/// Монітор кінця треку: чекає флаг кожного треку і шле `TrackEnded`.
fn spawn_end_monitor(
    rx: std::sync::mpsc::Receiver<Arc<AtomicBool>>,
    events: std::sync::mpsc::Sender<EngineEvent>,
) {
    let _ = std::thread::Builder::new()
        .name("selfsonic-end".into())
        .spawn(move || {
            let mut current: Option<Arc<AtomicBool>> = None;
            loop {
                // Беремо найновіший флаг, пропускаючи застарілі.
                if let Ok(flag) = rx.try_recv() {
                    current = Some(flag);
                    continue;
                }
                let Some(flag) = current.as_ref() else {
                    match rx.recv() {
                        Ok(flag) => current = Some(flag),
                        Err(_) => break,
                    }
                    continue;
                };
                if flag.load(Ordering::Acquire) {
                    events.send(EngineEvent::TrackEnded).ok();
                    // Чекаємо наступний флаг.
                    if let Ok(flag) = rx.recv() {
                        current = Some(flag);
                    } else {
                        break;
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
}

/// Потік скроблінгу: мережеві запити (Now Playing / зарахування) виконуються
/// тут, а не в UI-потоці. Помилки логуємо і продовжуємо — програти не має.
fn spawn_scrobbler(rx: std::sync::mpsc::Receiver<ScrobbleCmd>, client: Arc<Client>) {
    let _ = std::thread::Builder::new()
        .name("selfsonic-scrobble".into())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                let (id, submission) = match cmd {
                    ScrobbleCmd::Send { id, submission } => (id, submission),
                    ScrobbleCmd::Shutdown => break,
                };
                if let Err(e) = client.scrobble(&id, submission) {
                    warn!("scrobble (submission={submission}) '{id}': {e}");
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_mode_cycle() {
        assert_eq!(LoopMode::None.cycle(), LoopMode::Track);
        assert_eq!(LoopMode::Track.cycle(), LoopMode::Playlist);
        assert_eq!(LoopMode::Playlist.cycle(), LoopMode::None);
    }

    #[test]
    fn engine_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let state = EngineState {
            volume: 0.5,
            shuffle: true,
            loop_mode: LoopMode::Track,
            queue: vec!["a".into(), "b".into()],
            queue_pos: 1,
        };
        save_state_json(&path, &state).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let back: EngineState = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.volume, 0.5);
        assert_eq!(back.queue_pos, 1);
        assert_eq!(back.loop_mode, LoopMode::Track);
    }

    #[test]
    fn preload_gen_matches_value_seen_by_preloader() {
        // Імітація `request_preload()`: після алокації gen_id атомік вже
        // має саме це значення, тож перевірка `load() != my_gen` у
        // `spawn_preloader` не відкидає команду хибно.
        let counter = AtomicUsize::new(0);
        let my_gen = next_preload_gen(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), my_gen);
        // Якщо між алокацією і початком декодування генерацію ніхто не
        // інвалідував (зміна черги, stop) — порівняння залишається вірним.
        assert_eq!(counter.load(Ordering::SeqCst), my_gen);
        // Після інвалідації (fetch_add) застарілий gen_id вже не пройде.
        counter.fetch_add(1, Ordering::SeqCst);
        assert_ne!(counter.load(Ordering::SeqCst), my_gen);
    }

    /// Трек коротший за 8 хв: поріг — рівно половина тривалості.
    #[test]
    fn scrobble_short_track_threshold_is_half() {
        let duration = Duration::from_secs(6 * 60);
        assert_eq!(scrobble_threshold(duration), Duration::from_secs(3 * 60));
        assert!(!should_scrobble(Duration::from_secs(3 * 60 - 1), duration));
        assert!(should_scrobble(Duration::from_secs(3 * 60), duration));
        assert!(should_scrobble(Duration::from_secs(5 * 60), duration));
    }

    /// Довгий трек: 4-хвилинний поріг спрацьовує раніше за половину.
    #[test]
    fn scrobble_long_track_threshold_is_four_minutes() {
        let duration = Duration::from_secs(20 * 60);
        assert_eq!(scrobble_threshold(duration), Duration::from_secs(4 * 60));
        assert!(!should_scrobble(Duration::from_secs(4 * 60 - 1), duration));
        assert!(should_scrobble(Duration::from_secs(4 * 60), duration));
        // Половина (10 хв) — явно не поріг для такого треку.
        assert!(should_scrobble(Duration::from_secs(10 * 60), duration));
    }

    /// Невідома тривалість (0): не скроблимо одразу, а лише після 4 хв.
    #[test]
    fn scrobble_unknown_duration_falls_back_to_four_minutes() {
        let duration = Duration::ZERO;
        assert_eq!(scrobble_threshold(duration), Duration::from_secs(4 * 60));
        assert!(!should_scrobble(Duration::from_secs(3 * 60), duration));
        assert!(should_scrobble(Duration::from_secs(4 * 60), duration));
    }

    /// take_matching: слот із потрібним треком — віддає значення й очищує слот.
    #[test]
    fn take_matching_consumes_matching_track() {
        let slot = Mutex::new(Some(ReadyTrack { id: "a".into(), value: 42 }));
        assert!(matches!(
            take_matching(&slot, "a"),
            TakeReady::Consumed(42)
        ));
        assert!(slot.lock().unwrap().is_none());
    }

    /// take_matching: чужий трек у слоті — викидається (не грає чужу музику).
    #[test]
    fn take_matching_discards_foreign_track() {
        let slot = Mutex::new(Some(ReadyTrack { id: "b".into(), value: 7 }));
        match take_matching(&slot, "a") {
            TakeReady::Discarded(id) => assert_eq!(id, "b"),
            other => panic!("expected Discarded, got {other:?}"),
        }
        assert!(slot.lock().unwrap().is_none());
    }

    /// take_matching: порожній слот — Empty, декодувати свіжо.
    #[test]
    fn take_matching_reports_empty_slot() {
        let slot = Mutex::new(None::<ReadyTrack<i32>>);
        assert!(matches!(take_matching(&slot, "a"), TakeReady::Empty));
    }
}
