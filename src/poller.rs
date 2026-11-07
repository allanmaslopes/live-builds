use crate::knowledge::{
    KnowledgeSource, LoadedPackage, select_entry, select_page, to_recommendations,
};
use crate::live::{GameSnapshot, LiveClient, LiveError};
use crate::riot_build::{RecommendedBuild, SkillOrder};
use crate::static_data::{StaticData, StaticSource};
use crate::view::{HighEloData, PanelView, build_view_with_knowledge};
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
    pub knowledge_url: String,
    /// Game patch, resolved once per game (the Live Client API does not expose it).
    pub game_version: Box<dyn Fn() -> Option<String> + Send>,
}

impl PollConfig {
    pub fn new(in_game: Duration, record_dir: Option<PathBuf>) -> PollConfig {
        PollConfig {
            in_game,
            idle: Duration::from_secs(5),
            record_dir,
            knowledge_url: "http://localhost:8082".to_owned(),
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
        knowledge_source: KnowledgeSource,
        on_change: impl Fn(PanelState) + Send + 'static,
        cfg: PollConfig,
    ) -> Poller {
        let lane_pick = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let mut worker = Worker::new(src, statics, knowledge_source, cfg, lane_pick.clone());
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

type RecKey = (String, String, String, u32, Option<u32>);

struct Worker {
    src: Box<dyn SnapshotSource>,
    statics: StaticSource,
    knowledge_source: KnowledgeSource,
    cfg: PollConfig,
    lane_pick: Arc<Mutex<Option<String>>>,
    last: Option<PanelState>,
    ticks: u32,
    // per game
    data: Option<StaticData>,
    knowledge: Option<LoadedPackage>,
    previous_knowledge: Option<LoadedPackage>,
    knowledge_retry_at: f64,
    previous_knowledge_attempted: bool,
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
        knowledge_source: KnowledgeSource,
        cfg: PollConfig,
        lane_pick: Arc<Mutex<Option<String>>>,
    ) -> Worker {
        Worker {
            src,
            statics,
            knowledge_source,
            cfg,
            lane_pick,
            last: None,
            ticks: 0,
            data: None,
            knowledge: None,
            previous_knowledge: None,
            knowledge_retry_at: 0.0,
            previous_knowledge_attempted: false,
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
        self.knowledge = None;
        self.previous_knowledge = None;
        self.knowledge_retry_at = 0.0;
        self.previous_knowledge_attempted = false;
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

        let map_id = s.raw["gameData"]["mapNumber"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(if s.game_mode == "CLASSIC" { 11 } else { 0 });
        let uses_high_elo = map_id == 11 && s.game_mode == "CLASSIC";
        if uses_high_elo && self.knowledge.is_none() && s.game_time >= self.knowledge_retry_at {
            self.knowledge_retry_at = s.game_time + 30.0;
            if let Some(patch) = knowledge_patch(&data.catalog_patch)
                && let Ok(loaded) = self.knowledge_source.load(&patch)
            {
                self.knowledge = Some(loaded);
                self.previous_knowledge = None;
                self.previous_knowledge_attempted = false;
                self.rec = None;
            }
        }

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
            map_id,
            s.active.keystone,
        );
        if self.rec.as_ref().map(|r| &r.0) != Some(&key) {
            let champion_id = uses_high_elo.then(|| data.champion_id(&key.0)).flatten();
            let current_missing = champion_id.is_some_and(|champion_id| {
                self.knowledge.as_ref().is_some_and(|loaded| {
                    select_entry(&loaded.package, champion_id, position, None).is_none()
                })
            });
            if current_missing && !self.previous_knowledge_attempted {
                self.previous_knowledge_attempted = true;
                if let Some(previous) = self.knowledge.as_ref().and_then(|loaded| {
                    self.knowledge_source
                        .previous_patch(&loaded.package.patch)
                        .ok()
                        .flatten()
                        .or_else(|| previous_patch(&loaded.package.patch))
                }) {
                    self.previous_knowledge = self.knowledge_source.load(&previous).ok();
                }
            }
            let selected = champion_id.and_then(|champion_id| {
                let current = &self.knowledge.as_ref()?.package;
                let previous = self
                    .previous_knowledge
                    .as_ref()
                    .map(|loaded| &loaded.package);
                select_entry(current, champion_id, position, previous)
            });
            let (build, skills) = if let Some(selected) = selected {
                let page = select_page(selected.entry, s.active.keystone);
                to_recommendations(selected.entry, page)
            } else {
                self.statics
                    .recommendations(&data.catalog_patch, &key.0, key.3, &key.2, &key.1)
            };
            self.rec = Some((key, build, skills));
        }
        let Some((rec_key, build, skills)) = &self.rec else {
            return PanelState::Waiting;
        };
        let pick = self.lane_pick.lock().ok().and_then(|p| p.clone());
        let note = if data.stale {
            Some(format!("catálogo do patch {}", data.catalog_patch))
        } else {
            self.knowledge
                .as_ref()
                .filter(|loaded| loaded.stale)
                .map(|loaded| format!("pacote de elo alto {} em cache", loaded.package.patch))
        };
        let selected = uses_high_elo
            .then(|| data.champion_id(&rec_key.0))
            .flatten()
            .and_then(|champion_id| {
                let current = &self.knowledge.as_ref()?.package;
                let previous = self
                    .previous_knowledge
                    .as_ref()
                    .map(|loaded| &loaded.package);
                select_entry(current, champion_id, position, previous)
            });
        let page = selected.and_then(|selected| select_page(selected.entry, s.active.keystone));
        let high_elo = selected.map(|selected| HighEloData {
            package: selected.package,
            entry: selected.entry,
            page,
            names: &data.knowledge_names,
            player_keystone: s.active.keystone,
        });
        let mut panel = build_view_with_knowledge(
            s,
            &data.catalog,
            build,
            skills.as_ref(),
            pick.as_deref(),
            note,
            high_elo,
        );
        if selected.is_none() {
            let has_riot_build =
                !build.starting.is_empty() || !build.stages.is_empty() || skills.is_some();
            panel.high_elo = crate::view::HighEloSection::NoData(if has_riot_build {
                "Build oficial da Riot (sem dados de elo alto)".to_owned()
            } else {
                "Sem dados para este campeão".to_owned()
            });
        }
        PanelState::InGame(panel)
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

fn knowledge_patch(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    Some(format!("{}.{}", parts.next()?, parts.next()?))
}

fn previous_patch(patch: &str) -> Option<String> {
    let (major, minor) = patch.split_once('.')?;
    let minor: u32 = minor.parse().ok()?;
    Some(format!("{major}.{}", minor.checked_sub(1)?))
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
    ) -> (
        Worker,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        tempfile::TempDir,
    ) {
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
        let knowledge_calls = Arc::new(AtomicUsize::new(0));
        let knowledge_counter = Arc::clone(&knowledge_calls);
        let knowledge = KnowledgeSource::with_fetcher(
            cfg.knowledge_url.clone(),
            cache.path().to_path_buf(),
            move |_, _| {
                knowledge_counter.fetch_add(1, Ordering::SeqCst);
                Ok(crate::knowledge::FetchResult::Fresh {
                    body: include_str!("../tests/fixtures/knowledge-16.22.json").to_owned(),
                    etag: Some("W/\"test\"".to_owned()),
                })
            },
        );
        let src = Box::new(Fake(Mutex::new(frames.into())));
        (
            Worker::new(src, statics, knowledge, cfg, Arc::default()),
            calls,
            knowledge_calls,
            cache,
        )
    }

    fn behavior_worker(
        frames: Vec<Option<String>>,
        knowledge_fetch: impl Fn(&str, Option<&str>) -> Result<crate::knowledge::FetchResult, String>
        + Send
        + Sync
        + 'static,
    ) -> (Worker, Arc<AtomicUsize>, tempfile::TempDir) {
        let cache = tempfile::tempdir().unwrap();
        let statics = StaticSource::with_fetcher(cache.path().to_path_buf(), move |url| {
            if url.ends_with("versions.json") {
                Ok(r#"["16.22.1"]"#.to_owned())
            } else if url.ends_with("item.json") {
                Ok(ITEMS.to_owned())
            } else if url.ends_with("champion.json") {
                Ok(r#"{"type":"champion","version":"16.22.1","data":{"Thresh":{"id":"Thresh","key":"1","name":"Thresh"}}}"#.to_owned())
            } else if url.contains("/characters/thresh/thresh.bin.json") {
                Ok(include_str!("../tests/fixtures/thresh.bin.json").to_owned())
            } else {
                Err("offline".to_owned())
            }
        });
        let mut cfg = PollConfig::new(Duration::from_secs(1), None);
        cfg.game_version = Box::new(|| Some("16.22.1".to_owned()));
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let knowledge = KnowledgeSource::with_fetcher(
            cfg.knowledge_url.clone(),
            cache.path().to_path_buf(),
            move |url, etag| {
                counted.fetch_add(1, Ordering::SeqCst);
                knowledge_fetch(url, etag)
            },
        );
        let src = Box::new(Fake(Mutex::new(frames.into())));
        (
            Worker::new(src, statics, knowledge, cfg, Arc::default()),
            calls,
            cache,
        )
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
        let (mut w, _, _, _c) = worker(vec![at(100.0), at(100.0), at(100.0)], None);
        let seen = collect(&mut w, 3);
        assert_eq!(seen.len(), 1);
        assert!(matches!(seen[0], PanelState::InGame(_)), "{:?}", seen[0]);
    }

    #[test]
    fn poller_goes_idle_after_game_ends() {
        let (mut w, _, _, _c) = worker(vec![at(100.0), None], None);
        let seen = Mutex::new(Vec::new());
        let first = w.tick(&|s| seen.lock().unwrap().push(s));
        let second = w.tick(&|s| seen.lock().unwrap().push(s));
        assert_eq!(first, Duration::from_secs(1));
        assert_eq!(second, Duration::from_secs(5));
        assert_eq!(seen.lock().unwrap().last(), Some(&PanelState::Waiting));
    }

    #[test]
    fn statics_loaded_once_per_game() {
        let (mut w, calls, _, _c) = worker(vec![at(100.0), at(101.0), at(102.0), at(103.0)], None);
        collect(&mut w, 1);
        let after_first = calls.load(Ordering::SeqCst);
        assert!(after_first > 0);
        collect(&mut w, 3);
        assert_eq!(calls.load(Ordering::SeqCst), after_first);
    }

    #[test]
    fn knowledge_loaded_once_per_game() {
        let (mut w, _, calls, _c) = worker(vec![at(100.0), at(101.0), at(102.0), at(103.0)], None);
        collect(&mut w, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        collect(&mut w, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn falls_back_to_riot_build_when_knowledge_missing() {
        let (mut worker, _, _cache) =
            behavior_worker(vec![at(100.0)], |_, _| Err("processor offline".to_owned()));

        let state = worker.in_game(&parse_all_game_data(GAME).unwrap());

        let PanelState::InGame(panel) = state else {
            panic!("expected in-game panel")
        };
        assert_eq!(
            worker.rec.as_ref().unwrap().1.starting,
            vec![vec![3865, 2003, 2003]]
        );
        assert_eq!(
            panel.high_elo,
            crate::view::HighEloSection::NoData(
                "Build oficial da Riot (sem dados de elo alto)".to_owned()
            )
        );
    }

    #[test]
    fn aram_uses_riot_build_not_knowledge() {
        let aram = GAME
            .replace("\"gameMode\": \"CLASSIC\"", "\"gameMode\": \"ARAM\"")
            .replace("\"mapNumber\": 11", "\"mapNumber\": 12");
        let (mut worker, calls, _cache) = behavior_worker(vec![Some(aram.clone())], |_, _| {
            Ok(crate::knowledge::FetchResult::Fresh {
                body: include_str!("../tests/fixtures/knowledge-16.22.json").to_owned(),
                etag: None,
            })
        });

        let state = worker.in_game(&parse_all_game_data(&aram).unwrap());

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            worker.rec.as_ref().unwrap().1.starting,
            vec![vec![2051], vec![6660]]
        );
        let PanelState::InGame(panel) = state else {
            panic!("expected in-game panel")
        };
        assert_eq!(
            panel.high_elo,
            crate::view::HighEloSection::NoData(
                "Build oficial da Riot (sem dados de elo alto)".to_owned()
            )
        );
    }

    #[test]
    fn retries_knowledge_after_transient_failure() {
        let attempt = Arc::new(AtomicUsize::new(0));
        let current = Arc::clone(&attempt);
        let (mut worker, calls, _cache) =
            behavior_worker(vec![at(100.0), at(129.0), at(130.0)], move |_, _| {
                if current.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("transient".to_owned())
                } else {
                    Ok(crate::knowledge::FetchResult::Fresh {
                        body: include_str!("../tests/fixtures/knowledge-16.22.json").to_owned(),
                        etag: None,
                    })
                }
            });

        collect(&mut worker, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        collect(&mut worker, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        collect(&mut worker, 1);

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(
            worker.last,
            Some(PanelState::InGame(PanelView {
                high_elo: crate::view::HighEloSection::Recommendation(_),
                ..
            }))
        ));
    }

    #[test]
    fn keystone_change_recomputes_build() {
        let first = GAME.replace("\"id\": 8439", "\"id\": 8112");
        let second = GAME.replace("\"id\": 8439", "\"id\": 8214");
        let (mut worker, _, _cache) = behavior_worker(vec![Some(first), Some(second)], |_, _| {
            Ok(crate::knowledge::FetchResult::Fresh {
                body: include_str!("../tests/fixtures/knowledge-16.22.json").to_owned(),
                etag: None,
            })
        });

        collect(&mut worker, 1);
        assert_eq!(worker.rec.as_ref().unwrap().1.stages[2].items, [3152]);
        collect(&mut worker, 1);

        assert_eq!(worker.rec.as_ref().unwrap().1.stages[2].items, [3137]);
    }

    #[test]
    fn previous_patch_from_status_crosses_season() {
        let cache = tempfile::tempdir().unwrap();
        let source = KnowledgeSource::with_fetcher(
            "http://processor".to_owned(),
            cache.path().to_path_buf(),
            |url, etag| {
                assert_eq!(url, "http://processor/processor/status");
                assert_eq!(etag, None);
                Ok(crate::knowledge::FetchResult::Fresh {
                    body: r#"{"patches":[{"patch":"17.1"},{"patch":"16.23"},{"patch":"16.24"},{"patch":"15.24"}]}"#.to_owned(),
                    etag: None,
                })
            },
        );

        assert_eq!(
            source.previous_patch("17.1").unwrap(),
            Some("16.24".to_owned())
        );
    }

    #[test]
    fn record_writes_every_10s_of_game_time() {
        let out = tempfile::tempdir().unwrap();
        let frames = [0.0, 4.0, 10.0, 12.0, 20.5].map(at).to_vec();
        let (mut w, _, _, _c) = worker(frames, Some(out.path().to_path_buf()));
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
