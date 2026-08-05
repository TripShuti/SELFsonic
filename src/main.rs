//! SELFsonic: легкий TUI-клієнт Subsonic на Rust.
//!
//! entrypoint, event loop, зв'язка всіх модулів.

mod api;
mod cache;
mod config;
mod error;
mod playback;
mod ui;

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{self, Event, KeyEvent};
use mpris_server::{PlaybackStatus, Time, TrackId};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Tabs;
use ratatui::Frame;
use tracing::{debug, info, warn};

use crate::api::client::Client;
use crate::cache::db::Db;
use crate::config::{Config, cache_dir, state_dir};
use crate::error::AppError;
use crate::playback::dj::{self, DJ_BATCH, DJ_REFILL_AT};
use crate::playback::engine::{Engine, EngineEvent, EngineState, LoopMode, TrackMeta};
use crate::playback::mpris::{self, Mpris, MprisUpdate, PlayerCommand};
use crate::ui::app::{AppState, Tab};
use crate::ui::keybindings::Action;

const TICK: Duration = Duration::from_millis(50);

/// Мінімальний інтервал між DJ-дозаповненнями (захист від частих порожніх спроб).
const DJ_REFILL_MIN_INTERVAL: Duration = Duration::from_secs(1);

fn main() -> Result<()> {
    let config_path = parse_args()?;
    setup_logging()?;

    let config = Config::load(&config_path)?;
    info!("start, config: {}", config_path.display());

    let _lock = SingleInstance::acquire()?;

    let cache_dir = cache_dir()?;
    let db_path = cache_dir.join("library.db");
    let state_path = state_dir()?.join("state.json");

    let mut db = Db::open(&db_path).context("opening cache database")?;
    let client = Arc::new(
        Client::new(&config.server.url, &config.server.username, &config.server.password)
            .context("creating client")?,
    );

    let mut engine = Engine::new(client.clone(), config.audio.volume)
        .map_err(|e| anyhow!("audio unavailable: {e}"))?;

    let mut mpris = match Mpris::spawn() {
        Ok(m) => {
            info!("MPRIS enabled");
            Some(m)
        }
        Err(e) => {
            warn!("MPRIS disabled: {e}");
            None
        }
    };

    // Відновлення стану (черга/позиція/гучність).
    if let Ok(EngineState { queue, queue_pos, .. }) = load_state_json(&state_path) {
        match db.tracks_by_ids(&queue) {
            Ok(tracks) if !tracks.is_empty() => {
                let metas: Vec<TrackMeta> = tracks.iter().map(TrackMeta::from).collect();
                let restored = EngineState {
                    volume: engine.volume(),
                    shuffle: engine.shuffle(),
                    loop_mode: engine.loop_mode(),
                    queue: queue.clone(),
                    queue_pos,
                };
                engine.restore_state(&restored, metas);
                debug!("restored queue with {} tracks", queue.len());
            }
            _ => {}
        }
    }

    let mut app = AppState::new();
    if let Err(e) = app.load_current_tab_from_cache(&mut db) {
        warn!("cache empty/corrupted: {e}");
    }
    if let Err(e) = app.refresh_starred(&db) {
        warn!("starred cache read failed: {e}");
    }
    if db.artists().map(|a| a.is_empty()).unwrap_or(true) {
        warn!("library empty, first refresh");
        if let Err(e) = app.refresh(&client, &mut db) {
            warn!("initial refresh failed: {e}");
        }
    }

    let mut last_queue_gen = None;
    let mut pos = PositionGuard::new();
    refresh_mpris(mpris.as_ref(), &engine, &client, &mut last_queue_gen, &mut pos);

    let res = run_tui(
        &mut app,
        &mut db,
        &client,
        &mut engine,
        &mut mpris,
        &mut last_queue_gen,
        &mut pos,
    );

    engine.save_state(&state_path);
    if let Some(m) = mpris.as_mut() {
        m.shutdown();
    }
    res
}

// ---------- аргументи / логування / single instance ----------

fn parse_args() -> Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut config_path = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                config_path = Some(args.next().ok_or_else(|| anyhow!("--config requires a path"))?);
            }
            "--help" | "-h" => {
                println!("SELFsonic — Subsonic TUI client");
                println!("  --config <path>  path to config.toml");
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    match config_path {
        Some(p) => Ok(PathBuf::from(p)),
        None => Config::default_path().map_err(|e| anyhow!("{e}")),
    }
}

fn setup_logging() -> Result<()> {
    let dir = state_dir()?;
    let file_appender = tracing_appender::rolling::daily(&dir, "SELFsonic.log");
    // Власні DEBUG-логи лишаються; DEBUG-спам залежностей (ureq, symphonia)
    // душить — інакше лог росте на ~600KB/день. Перевизначення — через
    // RUST_LOG (env-filter увімкнений у tracing-subscriber).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "selfsonic=debug,\
             ureq=warn,ureq_proto=warn,\
             symphonia_core=warn,symphonia_bundle_mp3=warn,\
             symphonia_bundle_flac=warn,symphonia_metadata=warn",
        )
    });
    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_env_filter(filter)
        .init();
    Ok(())
}

/// Lock-файл single instance (AGENT.md). Знімається автоматично при drop.
struct SingleInstance {
    path: PathBuf,
}

impl SingleInstance {
    fn acquire() -> Result<Self> {
        let path = state_dir()?.join("lock");
        let pid = std::process::id();
        for attempt in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = writeln!(f, "{pid}");
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let other_pid = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok());
                    let alive = other_pid.is_some_and(pid_alive);
                    if !alive {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if attempt == 0 {
                        warn!("another instance is already running (pid={:?})", other_pid);
                    }
                    bail!("SELFsonic is already running (pid={})", other_pid.unwrap_or(0));
                }
                Err(e) => return Err(AppError::io(&path, e).into()),
            }
        }
        bail!("failed to create lock file {}", path.display())
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

// ---------- TUI event loop ----------

fn run_tui(
    app: &mut AppState,
    db: &mut Db,
    client: &Arc<Client>,
    engine: &mut Engine,
    mpris: &mut Option<Mpris>,
    last_queue_gen: &mut Option<usize>,
    pos: &mut PositionGuard,
) -> Result<()> {
    ui::theme::init();
    let mut terminal = ratatui::init();
    let mut quit = false;
    let mut last_dj_refill: Option<Instant> = None;
    // Остання намальована секунда позиції: прогрес-бар оновлюємо раз на секунду,
    // решта перемалювань — подієва (клавіші, події движка, DJ, Resize, повідомлення).
    let mut last_secs: Option<u64> = None;

    let result: Result<()> = (|| {
        while !quit {
            // Жива черга для вкладки Queue (DJ дозаповнює її в ре-таймі).
            if app.tab == Tab::Queue {
                app.sync_queue(engine);
            }

            let mut dirty = false;

            if event::poll(TICK).context("event::poll")? {
                match event::read().context("event::read")? {
                    Event::Key(key) => {
                        if !handle_key(app, db, client, engine, key) {
                            refresh_mpris(mpris.as_ref(), engine, client, last_queue_gen, pos);
                        } else {
                            quit = true;
                        }
                        dirty = true;
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
            }

            dirty |= drain_mpris_commands(mpris.as_ref(), engine, app, &mut quit);
            for ev in engine.poll_events() {
                handle_engine_event(mpris.as_ref(), app, ev, pos, engine.position());
                dirty = true;
            }
            engine.maybe_scrobble();
            refresh_mpris(mpris.as_ref(), engine, client, last_queue_gen, pos);

            // DJ: синхронне дозаповнення черги, коли вона на межі вичерпання.
            // Тротлінг інтервалом захищає від частих порожніх спроб.
            let dj_due = engine.dj_enabled()
                && engine.queue_remaining() < DJ_REFILL_AT
                && match last_dj_refill {
                    Some(t) => t.elapsed() >= DJ_REFILL_MIN_INTERVAL,
                    None => true,
                };
            if dj_due {
                last_dj_refill = Some(Instant::now());
                refill_dj(app, client, engine);
                dirty = true;
            }

            // Авто-підвантаження наступної сторінки альбомів.
            let was_loading = app.loading;
            if app.tab == Tab::Albums
                && !app.loading
                && matches!(app.selected_item(), Some(ui::app::ListItem::More))
            {
                app.loading = true;
                if let Err(e) = app.load_more_albums(client, db) {
                    app.set_message(format!("Pagination: {e}"), true);
                }
                app.loading = false;
            }
            dirty |= was_loading != app.loading;

            // Прогрес-бар малюємо щонайбільше раз на секунду.
            let secs = engine.position().as_secs();
            if last_secs != Some(secs) {
                last_secs = Some(secs);
                dirty = true;
            }

            if dirty {
                terminal
                    .draw(|f| draw(f, app, engine))
                    .context("drawing")?;
            }
        }
        Ok(())
    })();

    ratatui::restore();
    result
}

fn handle_key(
    app: &mut AppState,
    db: &mut Db,
    client: &Arc<Client>,
    engine: &mut Engine,
    key: KeyEvent,
) -> bool {
    let Some(action) = Action::from_key(key) else {
        return false;
    };

    if action == Action::Quit {
        return true;
    }

    match action {
        Action::Quit => {}
        Action::Down => app.move_down(),
        Action::Up => app.move_up(),
        Action::PageDown => app.page(20, true),
        Action::PageUp => app.page(20, false),
        Action::Top => app.top(),
        Action::Bottom => app.bottom(),
        Action::NextTab => {
            app.activate_tab(app.tab.next(), db);
        }
        Action::PrevTab => {
            app.activate_tab(app.tab.prev(), db);
        }
        Action::Back => {
            if app.has_parent() {
                app.back();
            }
        }
        Action::Select => {
            if app.tab == Tab::Queue {
                // У черзі Enter перемикає відтворення на вибраний трек.
                if app.selected < engine.queue().len() {
                    engine.play_index(app.selected);
                }
            } else if let Err(e) = app.select_current(client, db, engine) {
                app.set_message(format!("{e}"), true);
                warn!("select: {e}");
            }
        }
        Action::PlayPause => engine.toggle(),
        Action::Next => engine.next(),
        Action::Previous => engine.previous(),
        Action::CycleLoop => {
            engine.cycle_loop();
        }
        Action::ToggleShuffle => {
            engine.toggle_shuffle();
        }
        Action::VolumeUp => engine.set_volume(engine.volume() + 0.05),
        Action::VolumeDown => engine.set_volume(engine.volume() - 0.05),
        Action::SeekForward => engine.seek_relative(10),
        Action::SeekBackward => engine.seek_relative(-10),
        Action::Refresh => {
            if let Err(e) = app.refresh(client, db) {
                warn!("refresh: {e}");
            }
        }
        Action::ToggleFavorite => {
            match app.toggle_favorite(client, db) {
                Ok(Some(true)) => app.set_message("Starred ♥", false),
                Ok(Some(false)) => app.set_message("Unstarred", false),
                Ok(None) => {}
                Err(e) => {
                    app.set_message(format!("Favorite: {e}"), true);
                    warn!("toggle favorite: {e}");
                }
            }
        }
        Action::DJ => {
            let seed = match (app.selected_track(), engine.current().cloned()) {
                (Some(t), _) => t,
                (None, Some(t)) => t,
                (None, None) => {
                    app.set_message("DJ: select a track first (d)", true);
                    return false;
                }
            };
            if engine.dj_enabled() {
                engine.set_dj(false);
                app.set_message("DJ: off", false);
            } else {
                // Нова черга скидає DJ — стартуємо з сіда, потім ставимо якір.
                engine.set_queue(vec![seed.clone()], 0);
                engine.start_dj(seed);
                app.set_message("DJ: picking similar tracks...", false);
            }
        }
    }
    false
}

/// Одна ітерація DJ: каскад кандидатів із якорем (схожі → артист → random),
/// відфільтрованих проти id з черги. Порожній результат вимикає DJ.
/// Random-треки позначаються (не рухають якір); виграш поточної бази
/// переміщує якір на поточний трек.
fn refill_dj(app: &mut AppState, client: &Client, engine: &mut Engine) {
    let Some(current) = engine.current().cloned() else {
        engine.set_dj(false);
        return;
    };
    let exclude: HashSet<String> = engine.queue().iter().map(|t| t.id.clone()).collect();
    let outcome = match dj::collect(
        client,
        engine.dj_anchor(),
        &current,
        engine.is_dj_random(&current.id),
        &exclude,
    ) {
        Ok(o) => o,
        Err(e) => {
            warn!("DJ collect: {e}");
            engine.set_dj(false);
            app.set_message(format!("DJ: {e}"), true);
            return;
        }
    };
    let batch: Vec<TrackMeta> = outcome.tracks.into_iter().take(DJ_BATCH).collect();
    if batch.is_empty() {
        engine.set_dj(false);
        app.set_message("DJ: no more similar tracks", false);
        return;
    }
    match outcome.source {
        dj::DjStep::Random => {
            debug!("DJ refill: random +{}", batch.len());
            let ids: Vec<String> = batch.iter().map(|t| t.id.clone()).collect();
            engine.mark_dj_random(&ids);
        }
        dj::DjStep::Similar => debug!("DJ refill: similar +{}", batch.len()),
        dj::DjStep::Artist => debug!("DJ refill: artist +{}", batch.len()),
    }
    if outcome.reanchor_current {
        engine.set_dj_anchor(current.clone());
        engine.unmark_dj_random(&current.id);
    }
    if let Some(msg) = &app.message
        && msg.text.starts_with("DJ:")
    {
        app.clear_message();
    }
    engine.append_tracks(batch);
}

// ---------- MPRIS: команди та оновлення ----------

fn drain_mpris_commands(
    mpris: Option<&Mpris>,
    engine: &mut Engine,
    app: &mut AppState,
    quit: &mut bool,
) -> bool {
    let Some(mpris) = mpris else { return false };
    let mut handled = false;
    while let Ok(cmd) = mpris.commands().try_recv() {
        handled = true;
        match cmd {
            PlayerCommand::Play | PlayerCommand::PlayPause => engine.toggle(),
            PlayerCommand::Pause => {
                if !engine.paused() && !engine.stopped() {
                    engine.toggle();
                }
            }
            PlayerCommand::Next => engine.next(),
            PlayerCommand::Previous => engine.previous(),
            PlayerCommand::Stop => engine.stop(),
            PlayerCommand::Seek { offset_us } => {
                engine.seek_relative(offset_us / 1_000_000);
                // Спека MPRIS: після нелінійної зміни позиції клієнти мають
                // отримати Seeked з фактичною позицією, інакше вони інтерполюють
                // від старого значення.
                mpris.update(MprisUpdate::Seeked(Time::from_secs(
                    engine.position().as_secs() as i64,
                )));
            }
            PlayerCommand::SetPosition { track_id, position_us } => {
                if let Ok(tid) = TrackId::try_from(track_id)
                    && let Some(id) = mpris::track_id_to_subsonic(&tid)
                    && engine.current().is_some_and(|t| t.id == id)
                {
                    engine.seek_to(Duration::from_micros(position_us as u64));
                    mpris.update(MprisUpdate::Seeked(Time::from_secs(
                        engine.position().as_secs() as i64,
                    )));
                }
            }
            PlayerCommand::OpenUri(uri) => {
                warn!("OpenUri is not supported: {uri}");
            }
            PlayerCommand::SetLoopStatus(status) => {
                engine.set_loop_mode(mpris::loop_status_from_mpris(status));
            }
            PlayerCommand::SetShuffle(shuffle) => {
                if engine.shuffle() != shuffle {
                    engine.toggle_shuffle();
                }
            }
            PlayerCommand::SetVolume(v) => engine.set_volume(v as f32),
            PlayerCommand::GoTo(path) => {
                if let Some(id) = path.strip_prefix("/org/mpris/MediaPlayer2/Track/")
                    && let Some(idx) = engine.queue().iter().position(|t| t.id == id)
                {
                    engine.play_index(idx);
                }
            }
            PlayerCommand::Quit => *quit = true,
        }
        app.clear_message();
    }
    handled
}

fn handle_engine_event(
    mpris: Option<&Mpris>,
    app: &mut AppState,
    ev: EngineEvent,
    pos: &mut PositionGuard,
    raw_position: Duration,
) {
    match ev {
        EngineEvent::TrackStarted { .. } => {
            // Спека MPRIS: нелінійна зміна позиції → Seeked. Без цього клієнти
            // (Quickshell) інтерполюють позицію від старого треку.
            if let Some(m) = mpris {
                m.update(MprisUpdate::Seeked(Time::ZERO));
            }
            // Фіксуємо позицію, яку rodio віддає прямо зараз: у вікні
            // перемикання вона ще може належати старому треку (див.
            // PositionGuard::on_track_started).
            pos.on_track_started(raw_position);
        }
        EngineEvent::TrackEnded => {}
        EngineEvent::QueueFinished => {
            app.set_message("Queue finished", false);
        }
        EngineEvent::LoadFailed(title) => {
            app.set_message(format!("Failed to load \"{title}\""), true);
        }
    }
}

/// Час, після якого позиція публікується завжди, навіть якщо вона не змінилась
/// відносно `baseline` (запобіжник: старий і новий трек можуть мати однакову
/// позицію в момент переходу, напр. обидва на позиції 0).
const SWITCH_TIMEOUT: Duration = Duration::from_millis(500);

/// Guard публікації позиції в MPRIS.
///
/// Після зміни треку rodio скидає позицію в 0 асинхронно (5мс тік аудіопотоку),
/// тож у вікні перемикання `engine.position()` ще віддає позицію СТАРОГО треку.
/// Якщо її опублікувати, клієнт (Quickshell) перезапише скинуту `Seeked(0)`
/// позицію старою і прогрес-бар зависне на прогресу попереднього треку.
/// Публікуємо реальну позицію лише тоді, коли вона змінилась відносно позиції,
/// зафіксованої в момент `TrackStarted` (тобто rodio вже віддає позицію нового
/// треку), або через `SWITCH_TIMEOUT` — порівняння з нулем тут не працює,
/// бо `Duration` беззнаковий і ніколи не буває меншим за 0.
struct PositionGuard {
    switch_pending: bool,
    /// Позиція в момент `TrackStarted` — ще може бути позицією старого треку.
    baseline: Duration,
    /// Момент початку перемикання (для запобіжного таймауту).
    switch_started: Instant,
    /// Id треку, для якого вже опубліковано Metadata (кеш: публікуємо
    /// тільки при зміні треку, а не щотік).
    last_meta_id: Option<String>,
}

impl PositionGuard {
    fn new() -> Self {
        Self {
            switch_pending: false,
            baseline: Duration::ZERO,
            switch_started: Instant::now(),
            last_meta_id: None,
        }
    }

    /// Викликається на `TrackStarted`: фіксуємо позицію, яку rodio віддає
    /// прямо зараз (позицію старого треку, що ще не скинулась).
    fn on_track_started(&mut self, raw: Duration) {
        self.switch_pending = true;
        self.baseline = raw;
        self.switch_started = Instant::now();
    }

    fn next(&mut self, raw: Duration) -> Duration {
        self.step(raw, self.switch_started.elapsed())
    }

    /// Чиста логіка публікації: `elapsed` передається явно, щоб тестувати
    /// поведінку таймауту без реального очікування.
    fn step(&mut self, raw: Duration, elapsed: Duration) -> Duration {
        if self.switch_pending {
            // Позиція змінилась відносно зафіксованої — це точно вже новий
            // трек. Або минув запобіжний таймаут.
            if raw != self.baseline || elapsed >= SWITCH_TIMEOUT {
                self.switch_pending = false;
                self.baseline = raw;
                raw
            } else {
                // rodio ще віддає позицію старого треку — тримаємо 0 (після Seeked).
                Duration::ZERO
            }
        } else {
            self.baseline = raw;
            raw
        }
    }
}

/// Повний зліп стану движка → MPRIS: метадані, статуси та (при зміні черги) TrackList.
fn refresh_mpris(
    mpris: Option<&Mpris>,
    engine: &Engine,
    client: &Client,
    last_queue_gen: &mut Option<usize>,
    pos: &mut PositionGuard,
) {
    let Some(mpris) = mpris else {
        *last_queue_gen = Some(engine.queue_gen());
        return;
    };
    let playing = !engine.stopped() && !engine.paused();
    let status = match engine.current() {
        Some(_) if playing => PlaybackStatus::Playing,
        Some(_) => PlaybackStatus::Paused,
        None => PlaybackStatus::Stopped,
    };
    mpris.update(MprisUpdate::PlaybackStatus(status));

    let has_next =
        engine.queue_pos() + 1 < engine.queue().len() || engine.loop_mode() != LoopMode::None;
    let has_prev = engine.queue_pos() > 0 || engine.loop_mode() == LoopMode::Playlist;
    mpris.update(MprisUpdate::CanGoNext(has_next));
    mpris.update(MprisUpdate::CanGoPrevious(has_prev));
    mpris.update(MprisUpdate::CanPlay(!engine.queue().is_empty()));
    mpris.update(MprisUpdate::CanPause(engine.current().is_some()));
    mpris.update(MprisUpdate::CanSeek(engine.current().is_some()));
    mpris.update(MprisUpdate::Volume(engine.volume() as f64));
    mpris.update(MprisUpdate::LoopStatus(mpris::loop_status_to_mpris(&engine.loop_mode())));
    mpris.update(MprisUpdate::Shuffle(engine.shuffle()));

    let raw = engine.position();
    let published = pos.next(raw);
    mpris.update(MprisUpdate::Position(Time::from_secs(published.as_secs() as i64)));

    // Metadata публікуємо лише при зміні треку — поля Metadata константні
    // для трека, кешуємо по id і не будуємо/клонуємо його щотік.
    match engine.current() {
        Some(track) => {
            if pos.last_meta_id.as_deref() != Some(track.id.as_str()) {
                pos.last_meta_id = Some(track.id.clone());
                let art = track.cover_art.as_deref().map(|c| client.cover_art_url(c));
                mpris.update(MprisUpdate::Metadata(mpris::build_metadata(track, art)));
            }
        }
        None => {
            if pos.last_meta_id.take().is_some() {
                mpris.update(MprisUpdate::Metadata(mpris_server::Metadata::new()));
            }
        }
    }

    if *last_queue_gen != Some(engine.queue_gen()) {
        *last_queue_gen = Some(engine.queue_gen());
        let tracks: Vec<_> = engine
            .queue()
            .iter()
            .map(|t| {
                let art = t
                    .cover_art
                    .as_deref()
                    .map(|c| client.cover_art_url(c));
                (mpris::subsonic_track_id(&t.id), mpris::build_metadata(t, art))
            })
            .collect();
        let current = engine.current().map(|t| mpris::subsonic_track_id(&t.id));
        mpris.update(MprisUpdate::TrackList { tracks, current });
    }
}

// ---------- стан ----------

fn load_state_json(path: &Path) -> Result<EngineState> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

// ---------- малювання ----------

fn draw(frame: &mut Frame, app: &AppState, engine: &Engine) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(6),
    ])
    .split(frame.area());

    let tabs = Tabs::new(
        Tab::ALL
            .iter()
            .map(|t| t.label().to_string())
            .collect::<Vec<_>>(),
    )
    .select(Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0))
    .divider(" | ")
    .style(ui::theme::base())
    .highlight_style(ui::theme::title(true));
    frame.render_widget(tabs, areas[0]);

    match app.tab {
        Tab::Artists => ui::views::artists::render(frame, areas[1], app),
        Tab::Albums => ui::views::albums::render(frame, areas[1], app),
        Tab::Playlists => ui::views::tracks::render(frame, areas[1], app),
        Tab::Favorites => ui::views::tracks::render(frame, areas[1], app),
        Tab::Queue => ui::views::queue::render(frame, areas[1], app, engine),
    }
    ui::views::now_playing::render(frame, areas[2], engine, app);
}

// ---------- тести ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_alive_self() {
        assert!(pid_alive(std::process::id()));
    }

    /// Перший трек сесії: baseline=0, і `raw < 0` неможливий — старий механізм
    /// тут застрягав назавжди. Тепер вихід — за різницею від baseline.
    #[test]
    fn position_publishes_after_first_track_from_zero() {
        let mut pos = PositionGuard::new();
        pos.on_track_started(Duration::ZERO);
        // Поки rodio тримає 0 — публікуємо 0 (після Seeked(0)).
        assert_eq!(pos.next(Duration::ZERO), Duration::ZERO);
        assert_eq!(pos.next(Duration::ZERO), Duration::ZERO);
        // Позиція рушила з місця — це вже новий трек, публікуємо реальну.
        let p = pos.next(Duration::from_millis(50));
        assert_eq!(p, Duration::from_millis(50));
        // Надалі позиція публікується напряму.
        assert_eq!(pos.next(Duration::from_secs(12)), Duration::from_secs(12));
    }

    /// Перемикання зі старого треку на 45-й секунді: поки rodio тримає
    /// стару позицію — тримаємо 0, щоб не затерти Seeked(0).
    #[test]
    fn position_hides_old_track_value_until_switch() {
        let mut pos = PositionGuard::new();
        pos.on_track_started(Duration::from_secs(45));
        assert_eq!(pos.next(Duration::from_secs(45)), Duration::ZERO);
        assert_eq!(pos.next(Duration::from_secs(45)), Duration::ZERO);
        // rodio скинув позицію — публікуємо значення нового треку.
        assert_eq!(pos.next(Duration::ZERO), Duration::ZERO);
        assert_eq!(pos.next(Duration::from_millis(10)), Duration::from_millis(10));
    }

    /// Запобіжник: старий і новий трек мають однакову позицію (0) — різниця
    /// від baseline не спрацює, вихід за таймаутом.
    #[test]
    fn position_switch_falls_back_to_timeout() {
        let mut pos = PositionGuard::new();
        pos.on_track_started(Duration::ZERO);
        assert_eq!(pos.next(Duration::ZERO), Duration::ZERO);
        // Після SWITCH_TIMEOUT публікуємо позицію навіть без її зміни.
        assert_eq!(pos.step(Duration::ZERO, SWITCH_TIMEOUT), Duration::ZERO);
        // Guard більше не pending — позиція публікується напряму.
        assert_eq!(pos.next(Duration::from_secs(1)), Duration::from_secs(1));
    }

    /// Звичайна гра без перемикань: позиція проходить наскрізь.
    #[test]
    fn position_passes_through_when_no_switch() {
        let mut pos = PositionGuard::new();
        assert_eq!(pos.next(Duration::from_secs(1)), Duration::from_secs(1));
        assert_eq!(pos.next(Duration::from_secs(2)), Duration::from_secs(2));
    }
}
