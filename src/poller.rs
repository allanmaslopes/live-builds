use crate::live::{GameSnapshot, LiveClient, LiveError};
use crate::riot_build::{RecommendedBuild, SkillOrder};
use crate::static_data::{StaticData, StaticSource};
use crate::view::{PanelView, build_view};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RECORD_EVERY: f64 = 10.0;
/// Ticks to wait before retrying a failed static load (avoids hitting the network every second).
const STATIC_RETRY_TICKS: u32 = 10;

#[derive(Clone, Debug, PartialEq)]
// Shape fixed by the plan; one value lives at a time, so the size gap is irrelevant.
#[allow(clippy::large_enum_variant)]
pub enum PanelState {
    Waiting,
    InGame(PanelView),
    Error(String),
}

pub trait SnapshotSource: Send {
    fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError>;
}

impl SnapshotSource for LiveClient {
    fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError> {
        LiveClient::fetch(self)
    }
}

pub struct PollConfig {
    pub in_game: Duration,
    pub idle: Duration,
    pub record_dir: Option<PathBuf>,
    /// Game patch, resolved once per game (the Live Client API does not expose it).
    pub game_version: Box<dyn Fn() -> Option<String> + Send>,
}

impl PollConfig {
    pub fn new(in_game: Duration, record_dir: Option<PathBuf>) -> PollConfig {
        PollConfig {
            in_game,
            idle: Duration::from_secs(5),
            record_dir,
            game_version: Box::new(latest_ddragon_version),
        }
    }
}

fn latest_ddragon_version() -> Option<String> {
    let mut response = ureq::get("https://ddragon.leagueoflegends.com/api/versions.json")
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()
        .ok()?;
    let json = response.body_mut().read_to_string().ok()?;
    let versions: Vec<String> = serde_json::from_str(&json).ok()?;
    versions.into_iter().next()
}

pub struct Poller {
    lane_pick: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl Poller {
    pub fn spawn(
        src: Box<dyn SnapshotSource>,
        statics: StaticSource,
        on_change: impl Fn(PanelState) + Send + 'static,
        cfg: PollConfig,
    ) -> Poller {
        let lane_pick = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let mut worker = Worker::new(src, statics, cfg, lane_pick.clone());
        let flag = stop.clone();
        thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                thread::sleep(worker.tick(&on_change));
            }
        });
        Poller { lane_pick, stop }
    }

    /// Takes effect on the next tick.
    pub fn set_lane_pick(&self, pick: Option<String>) {
        if let Ok(mut guard) = self.lane_pick.lock() {
            *guard = pick;
        }
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type RecKey = (String, String, String, u32);

struct Worker {
    src: Box<dyn SnapshotSource>,
    statics: StaticSource,
    cfg: PollConfig,
    lane_pick: Arc<Mutex<Option<String>>>,
    last: Option<PanelState>,
    ticks: u32,
    // per game
    data: Option<StaticData>,
    rec: Option<(RecKey, RecommendedBuild, Option<SkillOrder>)>,
    retry_at: u32,
    last_err: String,
    last_time: Option<f64>,
    next_record: f64,
    game_start: Option<u64>,
}

impl Worker {
    fn new(
        src: Box<dyn SnapshotSource>,
        statics: StaticSource,
        cfg: PollConfig,
        lane_pick: Arc<Mutex<Option<String>>>,
    ) -> Worker {
        Worker {
            src,
            statics,
            cfg,
            lane_pick,
            last: None,
            ticks: 0,
            data: None,
            rec: None,
            retry_at: 0,
            last_err: String::new(),
            last_time: None,
            next_record: 0.0,
            game_start: None,
        }
    }

    /// Runs one poll; returns how long to sleep before the next one.
    fn tick(&mut self, on_change: &dyn Fn(PanelState)) -> Duration {
        self.ticks += 1;
        let (state, next) = match self.src.fetch() {
            Ok(Some(s)) => (self.in_game(&s), self.cfg.in_game),
            Ok(None) => {
                self.reset_game();
                (PanelState::Waiting, self.cfg.idle)
            }
            Err(error) => (PanelState::Error(error.to_string()), self.cfg.idle),
        };
        if self.last.as_ref() != Some(&state) {
            self.last = Some(state.clone());
            on_change(state);
        }
        next
    }

    fn reset_game(&mut self) {
        self.data = None;
        self.rec = None;
        self.retry_at = 0;
        self.last_time = None;
        self.next_record = 0.0;
        self.game_start = None;
        if let Ok(mut pick) = self.lane_pick.lock() {
            *pick = None;
        }
    }

    fn in_game(&mut self, s: &GameSnapshot) -> PanelState {
        if self.last_time.is_some_and(|t| s.game_time + 1.0 < t) {
            self.reset_game(); // clock went back: a new game without a "no game" tick in between
        }
        self.last_time = Some(s.game_time);
        self.record(s);

        if self.data.is_none() {
            if self.ticks < self.retry_at {
                return PanelState::Error(self.last_err.clone());
            }
            let version = (self.cfg.game_version)().unwrap_or_else(|| "0.0".to_owned());
            match self.statics.load(&version) {
                Ok(data) => self.data = Some(data),
                Err(error) => {
                    self.retry_at = self.ticks + STATIC_RETRY_TICKS;
                    self.last_err = error.clone();
                    return PanelState::Error(error);
                }
            }
        }
        let Some(data) = &self.data else {
            return PanelState::Waiting;
        };

        let me = s
            .players
            .iter()
            .find(|p| p.summoner_name == s.active.summoner_name);
        let position = me.map_or("", |p| p.position.as_str());
        let key: RecKey = (
            me.map_or_else(String::new, |p| p.champion_name.clone()),
            if position.is_empty() {
                "None"
            } else {
                position
            }
            .to_owned(),
            s.game_mode.clone(),
            s.raw["gameData"]["mapNumber"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(if s.game_mode == "CLASSIC" { 11 } else { 0 }),
        );
        if self.rec.as_ref().map(|r| &r.0) != Some(&key) {
            let (build, skills) = if key.0.is_empty() {
                (RecommendedBuild::default(), None)
            } else {
                self.statics
                    .recommendations(&data.catalog_patch, &key.0, key.3, &key.2, &key.1)
            };
            self.rec = Some((key, build, skills));
        }
        let Some((_, build, skills)) = &self.rec else {
            return PanelState::Waiting;
        };
        let pick = self.lane_pick.lock().ok().and_then(|p| p.clone());
        let note = data
            .stale
            .then(|| format!("catálogo do patch {}", data.catalog_patch));
        PanelState::InGame(build_view(
            s,
            &data.catalog,
            build,
            skills.as_ref(),
            pick.as_deref(),
            note,
        ))
    }

    fn record(&mut self, s: &GameSnapshot) {
        let Some(dir) = &self.cfg.record_dir else {
            return;
        };
        if s.game_time < self.next_record {
            return;
        }
        self.next_record = (s.game_time / RECORD_EVERY).floor() * RECORD_EVERY + RECORD_EVERY;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let start = *self
            .game_start
            .get_or_insert(now.saturating_sub(s.game_time as u64));
        let game_dir = dir.join(start.to_string());
        // Best effort: a failed recording must never break the panel.
        if std::fs::create_dir_all(&game_dir).is_ok()
            && let Ok(json) = serde_json::to_string(&s.raw)
        {
            let _ = std::fs::write(game_dir.join(format!("{}.json", s.game_time as u64)), json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::parse_all_game_data;
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;

    const GAME: &str = include_str!("../tests/fixtures/allgamedata_classic.json");
    const ITEMS: &str = include_str!("../tests/fixtures/item_sample.json");

    struct Fake(Mutex<VecDeque<Option<String>>>);

    impl SnapshotSource for Fake {
        fn fetch(&self) -> Result<Option<GameSnapshot>, LiveError> {
            let next = self.0.lock().unwrap().pop_front().flatten();
            Ok(next.map(|json| parse_all_game_data(&json).unwrap()))
        }
    }

    fn at(time: f64) -> Option<String> {
        Some(GAME.replace("726.5", &time.to_string()))
    }

    fn worker(
        frames: Vec<Option<String>>,
        record_dir: Option<PathBuf>,
    ) -> (Worker, Arc<AtomicUsize>, tempfile::TempDir) {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let statics = StaticSource::with_fetcher(cache.path().to_path_buf(), move |url| {
            counter.fetch_add(1, Ordering::SeqCst);
            if url.ends_with("versions.json") {
                Ok(r#"["16.22.1"]"#.to_owned())
            } else if url.ends_with("item.json") {
                Ok(ITEMS.to_owned())
            } else {
                Err("offline".to_owned())
            }
        });
        let mut cfg = PollConfig::new(Duration::from_secs(1), record_dir);
        cfg.game_version = Box::new(|| Some("16.22.1".to_owned()));
        let src = Box::new(Fake(Mutex::new(frames.into())));
        (Worker::new(src, statics, cfg, Arc::default()), calls, cache)
    }

    fn collect(w: &mut Worker, ticks: usize) -> Vec<PanelState> {
        let seen = Mutex::new(Vec::new());
        for _ in 0..ticks {
            w.tick(&|s| seen.lock().unwrap().push(s));
        }
        seen.into_inner().unwrap()
    }

    #[test]
    fn emits_only_on_change() {
        let (mut w, _, _c) = worker(vec![at(100.0), at(100.0), at(100.0)], None);
        let seen = collect(&mut w, 3);
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0], PanelState::InGame(_)), "{:?}", seen[0]);
    }

    #[test]
    fn poller_goes_idle_after_game_ends() {
        let (mut w, _, _c) = worker(vec![at(100.0), None], None);
        let seen = Mutex::new(Vec::new());
        let first = w.tick(&|s| seen.lock().unwrap().push(s));
        let second = w.tick(&|s| seen.lock().unwrap().push(s));
        assert_eq!(first, Duration::from_secs(1));
        assert_eq!(second, Duration::from_secs(5));
        assert_eq!(seen.lock().unwrap().last(), Some(&PanelState::Waiting));
    }

    #[test]
    fn statics_loaded_once_per_game() {
        let (mut w, calls, _c) = worker(vec![at(100.0), at(101.0), at(102.0), at(103.0)], None);
        collect(&mut w, 1);
        let after_first = calls.load(Ordering::SeqCst);
        assert!(after_first > 0);
        collect(&mut w, 3);
        assert_eq!(calls.load(Ordering::SeqCst), after_first);
    }

    #[test]
    fn record_writes_every_10s_of_game_time() {
        let out = tempfile::tempdir().unwrap();
        let frames = [0.0, 4.0, 10.0, 12.0, 20.5].map(at).to_vec();
        let (mut w, _, _c) = worker(frames, Some(out.path().to_path_buf()));
        collect(&mut w, 5);
        let game_dir = std::fs::read_dir(out.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut names: Vec<String> = std::fs::read_dir(game_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["0.json", "10.json", "20.json"]);
    }
}
