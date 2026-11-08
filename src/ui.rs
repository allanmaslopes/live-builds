use crate::catalog::ItemCatalog;
use crate::knowledge::{KnowledgeNames, KnowledgeSource};
use crate::lcu::champ_select::{
    Action, Applier, Recommendation, SelectState, SelectionKey, parse_session, recommend, reminder,
    set_spells,
};
use crate::lcu::item_sets::upsert_app_set;
use crate::lcu::item_sets::{delete_user_set, list_sets, save_user_set};
use crate::lcu::perks::{
    ApplyOutcome, create_page, delete_page, list_pages, update_page, upsert_app_page,
};
use crate::lcu::poller::{LcuPollState, LcuPoller};
use crate::lcu::runes::RuneRules;
use crate::lcu::{LcuClient, LcuError, LcuHttp, find_lockfile};
use crate::live::LiveClient;
use crate::poller::{PanelState, PollConfig, Poller};
use crate::settings::Settings;
use crate::static_data::StaticSource;
use crate::ui_editors::{EditorAction, EditorsState};
use crate::ui_select::{AppliedItems, SelectAction, SelectScreenInput, select_screen};
use crate::view::{
    BuildView, HighEloSection, HighEloView, LaneView, MeView, PanelView, SkillView, TeamView,
};
use egui::{Color32, RichText};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MUTED: Color32 = Color32::from_gray(150);
const GOOD: Color32 = Color32::from_rgb(110, 200, 120);
const WARN: Color32 = Color32::from_rgb(240, 190, 80);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Screen {
    Select,
    Game,
    Editor,
    ClientClosed,
    ClientError,
}

pub fn choose_screen(lcu: &crate::lcu::poller::LcuPollState, live_active: bool) -> Screen {
    use crate::lcu::poller::LcuPollState;
    match lcu {
        LcuPollState::Selecting(_) => Screen::Select,
        _ if live_active => Screen::Game,
        LcuPollState::Open(_) => Screen::Editor,
        LcuPollState::ClientClosed => Screen::ClientClosed,
        LcuPollState::Error(_) => Screen::ClientError,
    }
}

#[derive(Default)]
pub struct SelectionLoadTracker {
    seen: Option<(String, Option<u32>, String)>,
    failures: u8,
    in_flight: bool,
    loaded: bool,
    retry_at: Option<Instant>,
}

impl SelectionLoadTracker {
    fn observe(&mut self, state: &SelectState) -> bool {
        let key = (
            state.session_id.clone(),
            state.champion_id,
            state.position.clone(),
        );
        if self.seen.as_ref() != Some(&key) {
            self.seen = Some(key);
            self.failures = 0;
            self.in_flight = false;
            self.loaded = false;
            self.retry_at = None;
            return true;
        }
        false
    }

    pub fn should_start(&mut self, state: &SelectState, now: Instant) -> bool {
        self.observe(state);
        let due = self.retry_at.is_none_or(|retry_at| now >= retry_at);
        let may_attempt = self.failures == 0 || (state.locked && self.failures <= 3);
        if !self.loaded && !self.in_flight && may_attempt && due {
            self.in_flight = true;
            return true;
        }
        false
    }

    pub fn failed(&mut self, now: Instant) {
        self.in_flight = false;
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + Duration::from_secs(5));
    }

    pub fn succeeded(&mut self) {
        self.in_flight = false;
        self.loaded = true;
        self.retry_at = None;
    }

    fn is_current(&self, state: &SelectState) -> bool {
        self.seen.as_ref()
            == Some(&(
                state.session_id.clone(),
                state.champion_id,
                state.position.clone(),
            ))
    }

    fn is_loading(&self, state: &SelectState) -> bool {
        self.is_current(state)
            && !self.loaded
            && (self.in_flight || self.failures == 0 || (state.locked && self.failures <= 3))
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

pub fn save_settings(settings: &mut Settings, path: &Path) -> Result<(), String> {
    settings.save(path)?;
    settings.corrupted = false;
    Ok(())
}

pub struct App {
    state: Arc<Mutex<PanelState>>,
    poller: Poller,
    lcu_state: Arc<Mutex<LcuPollState>>,
    _lcu_poller: LcuPoller,
    settings: Settings,
    editors: EditorsState,
    names: KnowledgeNames,
    catalog: Option<ItemCatalog>,
    selection: Option<SelectionUi>,
    selection_load: SelectionLoadTracker,
    ui_worker: UiWorker,
    editor_requests: EditorRequestTracker,
    patch_check_in_flight: bool,
    next_patch_check: Instant,
    applier: Applier,
    selection_error: Option<String>,
}

#[derive(Default)]
struct EditorRequestTracker {
    generation: u64,
    loading: bool,
}

impl EditorRequestTracker {
    fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        self.generation
    }

    fn complete(&mut self, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        true
    }

    fn loading(&self) -> bool {
        self.loading
    }
}

struct SelectionUi {
    key: SelectionKey,
    champion: String,
    recommendation: Recommendation,
    source_patch: Option<String>,
    source_games: Option<u32>,
    differences: Vec<String>,
    applied: AppliedItems,
    limit_reached: bool,
    applied_page: Option<crate::lcu::runes::RunePageDraft>,
    user_edited: bool,
    observed_page: Option<crate::lcu::runes::RunePageDraft>,
    observed_spells: (u32, u32),
    /// Page the client showed before our apply; a poll still reading it is stale, not a user edit.
    stale_page: Option<crate::lcu::runes::RunePageDraft>,
}

impl SelectionUi {
    fn observe_client(&mut self, state: &SelectState, names: &KnowledgeNames) {
        let stale = self.stale_page.is_some() && state.active_page == self.stale_page;
        if state.active_page == self.applied_page {
            self.stale_page = None;
        }
        if !stale
            && self.applied_page.is_some()
            && state.active_page.is_some()
            && state.active_page != self.applied_page
        {
            self.user_edited = true;
        }
        if stale
            || (state.active_page == self.observed_page && state.spells == self.observed_spells)
        {
            return;
        }
        self.observed_page = state.active_page.clone();
        self.observed_spells = state.spells;
        self.recalculate_differences(names);
    }

    fn mark_applied(&mut self, action: Action, names: &KnowledgeNames) {
        match action {
            Action::Runes => {
                self.applied.runes = true;
                self.stale_page = self.observed_page.clone();
                self.applied_page = self.recommendation.page.clone().map(|mut page| {
                    page.name = "live-builds".to_owned();
                    page
                });
                self.observed_page = self.applied_page.clone();
            }
            Action::Spells => {
                self.applied.spells = true;
                self.observed_spells = self.recommendation.spells;
            }
            Action::ItemSet => self.applied.item_set = true,
        }
        self.recalculate_differences(names);
    }

    fn recalculate_differences(&mut self, names: &KnowledgeNames) {
        self.differences = self.observed_page.as_ref().map_or_else(Vec::new, |page| {
            reminder(page, self.observed_spells, &self.recommendation, names)
        });
    }
}

struct SelectionPayload {
    catalog: ItemCatalog,
    names: KnowledgeNames,
    selection: SelectionUi,
}

struct EditorSnapshot {
    rules: RuneRules,
    pages: Vec<crate::lcu::perks::ClientPage>,
    sets: Vec<crate::lcu::item_sets::ItemSet>,
    catalog: Option<ItemCatalog>,
    names: Option<KnowledgeNames>,
}

enum UiCommand {
    LoadSelection(SelectState),
    CheckVersion {
        state: SelectState,
        current_patch: String,
    },
    Apply {
        key: SelectionKey,
        action: Action,
        recommendation: Recommendation,
    },
    Editor {
        generation: u64,
        action: EditorAction,
        settings: Settings,
        selection_session: Option<String>,
    },
}

enum UiResult {
    Selection {
        state: SelectState,
        result: Box<Result<Option<SelectionPayload>, String>>,
        is_check: bool,
    },
    Applied {
        key: SelectionKey,
        action: Action,
        result: Result<Option<bool>, String>,
    },
    Editor {
        generation: u64,
        result: Box<Result<Option<EditorSnapshot>, String>>,
        saved_settings: bool,
        edited_runes: bool,
        selection_session: Option<String>,
    },
}

struct UiWorker {
    commands: Sender<UiCommand>,
    results: Receiver<UiResult>,
}

impl UiWorker {
    fn spawn(
        ctx: egui::Context,
        cache_dir: PathBuf,
        knowledge_url: String,
        lockfile: Option<PathBuf>,
        settings_path: PathBuf,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        thread::spawn(move || {
            let statics = StaticSource::new(cache_dir.clone());
            let knowledge = KnowledgeSource::new(knowledge_url, cache_dir.clone());
            while let Ok(command) = command_rx.recv() {
                let result = match command {
                    UiCommand::LoadSelection(state) => UiResult::Selection {
                        result: Box::new(
                            load_selection(
                                &state,
                                &cache_dir,
                                &statics,
                                &knowledge,
                                lockfile.as_deref(),
                            )
                            .map(Some),
                        ),
                        state,
                        is_check: false,
                    },
                    UiCommand::CheckVersion {
                        state,
                        current_patch,
                    } => {
                        let latest = crate::poller::latest_ddragon_version();
                        let changed = latest.as_deref().is_some_and(|latest| {
                            compare_patch_versions(latest, &current_patch) != Ordering::Equal
                        });
                        UiResult::Selection {
                            result: Box::new(if changed {
                                load_selection(
                                    &state,
                                    &cache_dir,
                                    &statics,
                                    &knowledge,
                                    lockfile.as_deref(),
                                )
                                .map(Some)
                            } else {
                                Ok(None)
                            }),
                            state,
                            is_check: true,
                        }
                    }
                    UiCommand::Apply {
                        key,
                        action,
                        recommendation,
                    } => {
                        let result = connect_lcu(lockfile.as_deref())
                            .map_err(|error| error.to_string())
                            .and_then(|http| {
                                apply_recommendation(&http, &key, action, &recommendation)
                            });
                        UiResult::Applied {
                            key,
                            action,
                            result,
                        }
                    }
                    UiCommand::Editor {
                        generation,
                        action,
                        mut settings,
                        selection_session,
                    } => {
                        let saved_settings = matches!(&action, EditorAction::SaveSettings);
                        let edited_runes = matches!(
                            &action,
                            EditorAction::CreatePage(_) | EditorAction::SavePage(_, _)
                        );
                        let result = process_editor_command(
                            lockfile.as_deref(),
                            &statics,
                            &settings_path,
                            &mut settings,
                            action,
                        );
                        UiResult::Editor {
                            generation,
                            result: Box::new(result),
                            saved_settings,
                            edited_runes,
                            selection_session,
                        }
                    }
                };
                if result_tx.send(result).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            commands: command_tx,
            results: result_rx,
        }
    }

    fn send(&self, command: UiCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "worker da interface foi encerrado; tente novamente".to_owned())
    }
}

fn connect_lcu(lockfile: Option<&Path>) -> Result<LcuClient, LcuError> {
    let path = find_lockfile(lockfile).ok_or(LcuError::ClientClosed)?;
    LcuClient::connect(&path)
}

fn load_selection(
    state: &SelectState,
    cache_dir: &Path,
    statics: &StaticSource,
    knowledge: &KnowledgeSource,
    lockfile: Option<&Path>,
) -> Result<SelectionPayload, String> {
    let champion_id = state
        .champion_id
        .ok_or_else(|| "campeão ainda não selecionado".to_owned())?;
    let version = resolve_selection_version(crate::poller::latest_ddragon_version(), cache_dir)
        .ok_or_else(|| {
            "não foi possível descobrir a versão atual nem achar cache válido".to_owned()
        })?;
    let data = statics.load_exact(&version)?;
    let patch = data.catalog_patch.clone();
    let (champion, alias) = load_champion(cache_dir, &patch, champion_id)
        .ok_or_else(|| format!("campeão {champion_id} ausente no Data Dragon {patch}"))?;
    let http = connect_lcu(lockfile).map_err(|error| error.to_string())?;
    let styles = http
        .get("/lol-perks/v1/styles")
        .map_err(|error| error.to_string())?;
    let rules = RuneRules::from_json(&styles.to_string())?;
    let knowledge_patch = patch.split('.').take(2).collect::<Vec<_>>().join(".");
    let package = knowledge.load(&knowledge_patch).ok();
    let riot_bin = fs::read_to_string(
        cache_dir
            .join(&patch)
            .join(format!("champion-{alias}.json")),
    )
    .ok();
    let recommendation = recommend(
        package.as_ref().map(|loaded| &loaded.package),
        None,
        statics,
        &patch,
        champion_id,
        &champion,
        &state.position,
        (state.map_mode.0, &state.map_mode.1),
        None,
        riot_bin.as_deref(),
        &rules,
        &data.catalog,
    )
    .ok_or_else(|| "nenhuma recomendação disponível".to_owned())?;
    let selected = package.as_ref().and_then(|loaded| {
        crate::knowledge::select_entry(&loaded.package, champion_id, &state.position, None)
    });
    let active = list_pages(&http)
        .ok()
        .and_then(|pages| pages.into_iter().find(|page| page.current));
    let differences = active.as_ref().map_or_else(Vec::new, |page| {
        reminder(
            &page.draft,
            state.spells,
            &recommendation,
            &data.knowledge_names,
        )
    });
    Ok(SelectionPayload {
        catalog: data.catalog,
        names: data.knowledge_names,
        selection: SelectionUi {
            key: state
                .selection_key()
                .ok_or_else(|| "seleção sem chave".to_owned())?,
            champion,
            source_patch: selected.map(|selected| selected.package.patch.clone()),
            source_games: selected.map(|selected| selected.entry.games),
            recommendation,
            differences,
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            user_edited: false,
            observed_page: active.map(|page| page.draft),
            observed_spells: state.spells,
            stale_page: None,
        },
    })
}

fn resolve_selection_version(discovered: Option<String>, cache_dir: &Path) -> Option<String> {
    if discovered.is_some() {
        return discovered;
    }
    fs::read_dir(cache_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let patch = entry.file_name().to_str()?.to_owned();
            let item_json = fs::read_to_string(entry.path().join("item.json")).ok()?;
            ItemCatalog::from_ddragon_json(&patch, &item_json).ok()?;
            let knowledge_patch = patch.split('.').take(2).collect::<Vec<_>>().join(".");
            let package_json =
                fs::read_to_string(cache_dir.join(format!("knowledge-{knowledge_patch}.json")))
                    .ok()?;
            let package: crate::knowledge::KnowledgePackage =
                serde_json::from_str(&package_json).ok()?;
            (package.patch == knowledge_patch).then_some(patch)
        })
        .max_by(|left, right| compare_patch_versions(left, right))
}

fn apply_recommendation(
    http: &dyn LcuHttp,
    key: &SelectionKey,
    action: Action,
    recommendation: &Recommendation,
) -> Result<Option<bool>, String> {
    let session = http
        .get("/lol-champ-select/v1/session")
        .map_err(|error| error.to_string())?;
    let my_cell = session["localPlayerCellId"]
        .as_i64()
        .ok_or_else(|| "sessão sem localPlayerCellId".to_owned())?;
    if parse_session(&session, my_cell).selection_key().as_ref() != Some(key) {
        return Ok(None);
    }
    let applied = match action {
        Action::Runes => {
            let page = recommendation
                .page
                .as_ref()
                .ok_or_else(|| "sem recomendação de runas".to_owned())?;
            upsert_app_page(http, page)
                .map(|outcome| outcome != ApplyOutcome::LimitReached)
                .map_err(|error| error.to_string())
        }
        Action::Spells => set_spells(http, recommendation.spells.0, recommendation.spells.1)
            .map(|_| true)
            .map_err(|error| error.to_string()),
        Action::ItemSet => {
            let set = recommendation
                .item_set
                .as_ref()
                .ok_or_else(|| "sem recomendação de arsenal".to_owned())?;
            upsert_app_set(http, set)
                .map(|_| true)
                .map_err(|error| error.to_string())
        }
    }?;
    Ok(Some(applied))
}

fn process_editor_command(
    lockfile: Option<&Path>,
    statics: &StaticSource,
    settings_path: &Path,
    settings: &mut Settings,
    action: EditorAction,
) -> Result<Option<EditorSnapshot>, String> {
    if matches!(action, EditorAction::SaveSettings) {
        return save_settings(settings, settings_path).map(|_| None);
    }
    let http = connect_lcu(lockfile).map_err(|error| error.to_string())?;
    match action {
        EditorAction::Refresh => {}
        EditorAction::CreatePage(draft) => {
            create_page(&http, &draft).map_err(|error| error.to_string())?;
        }
        EditorAction::SavePage(id, draft) => {
            update_page(&http, id, &draft).map_err(|error| error.to_string())?;
        }
        EditorAction::DeletePage(id) => {
            delete_page(&http, id).map_err(|error| error.to_string())?;
        }
        EditorAction::CreateSet(set) | EditorAction::SaveSet(set) => {
            save_user_set(&http, &set).map_err(|error| error.to_string())?;
        }
        EditorAction::DeleteSet(uid) => {
            delete_user_set(&http, &uid).map_err(|error| error.to_string())?;
        }
        EditorAction::SaveSettings => unreachable!(),
    }
    load_editor_snapshot(&http, statics).map(Some)
}

fn load_editor_snapshot(
    http: &dyn LcuHttp,
    statics: &StaticSource,
) -> Result<EditorSnapshot, String> {
    let styles = http
        .get("/lol-perks/v1/styles")
        .map_err(|error| error.to_string())?;
    let rules = RuneRules::from_json(&styles.to_string())?;
    let pages = list_pages(http).map_err(|error| error.to_string())?;
    let sets = list_sets(http).map_err(|error| error.to_string())?;
    let data = crate::poller::latest_ddragon_version()
        .and_then(|version| statics.load_exact(&version).ok());
    let (catalog, names) = data.map_or((None, None), |data| {
        (Some(data.catalog), Some(data.knowledge_names))
    });
    Ok(EditorSnapshot {
        rules,
        pages,
        sets,
        catalog,
        names,
    })
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cache_dir: PathBuf,
        cfg: PollConfig,
        lcu_lockfile: Option<PathBuf>,
    ) -> App {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        cc.egui_ctx.style_mut(|style| {
            for (text_style, size) in [
                (egui::TextStyle::Body, 16.0),
                (egui::TextStyle::Button, 16.0),
                (egui::TextStyle::Heading, 22.0),
            ] {
                if let Some(font) = style.text_styles.get_mut(&text_style) {
                    font.size = size;
                }
            }
        });
        let knowledge_url = cfg.knowledge_url.clone();
        let state = Arc::new(Mutex::new(PanelState::Waiting));
        let shared = state.clone();
        let ctx = cc.egui_ctx.clone();
        // Repaint only when the poller reports a new state; egui is idle otherwise.
        let poller = Poller::spawn(
            Box::new(LiveClient::new()),
            StaticSource::new(cache_dir.clone()),
            KnowledgeSource::new(cfg.knowledge_url.clone(), cache_dir.clone()),
            move |new| {
                if let Ok(mut guard) = shared.lock() {
                    *guard = new;
                }
                ctx.request_repaint();
            },
            cfg,
        );
        let lcu_state = Arc::new(Mutex::new(LcuPollState::ClientClosed));
        let lcu_shared = Arc::clone(&lcu_state);
        let lcu_ctx = cc.egui_ctx.clone();
        let lcu_poller = LcuPoller::spawn(lcu_lockfile.clone(), move |new| {
            if let Ok(mut guard) = lcu_shared.lock() {
                *guard = new;
            }
            lcu_ctx.request_repaint();
        });
        let settings_path = cache_dir
            .parent()
            .unwrap_or(&cache_dir)
            .join("settings.json");
        let settings = Settings::load(&settings_path);
        let ui_worker = UiWorker::spawn(
            cc.egui_ctx.clone(),
            cache_dir,
            knowledge_url,
            lcu_lockfile,
            settings_path,
        );
        App {
            state,
            poller,
            lcu_state,
            _lcu_poller: lcu_poller,
            settings,
            editors: EditorsState::default(),
            names: KnowledgeNames::default(),
            catalog: None,
            selection: None,
            selection_load: SelectionLoadTracker::default(),
            ui_worker,
            editor_requests: EditorRequestTracker::default(),
            patch_check_in_flight: false,
            next_patch_check: Instant::now(),
            applier: Applier::default(),
            selection_error: None,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let state = self
            .state
            .lock()
            .map_or(PanelState::Waiting, |guard| guard.clone());
        let lcu = self
            .lcu_state
            .lock()
            .map_or(LcuPollState::ClientClosed, |guard| guard.clone());
        let live_active = matches!(state, PanelState::InGame(_));
        let screen = choose_screen(&lcu, live_active);
        if screen != Screen::Select {
            self.reset_selection();
        }
        self.drain_worker();
        match screen {
            Screen::Game => {
                if let PanelState::InGame(view) = state
                    && let Some(pick) = self.panel(ctx, &view)
                {
                    self.poller.set_lane_pick(Some(pick));
                }
            }
            Screen::Select => self.select(ctx, &lcu),
            Screen::Editor => {
                self.editor(ctx);
            }
            Screen::ClientClosed => {
                centered(ctx, RichText::new("Cliente do LoL fechado").heading())
            }
            Screen::ClientError => {
                let message = match &lcu {
                    LcuPollState::Error(message) => message.as_str(),
                    _ => "Erro desconhecido da LCU",
                };
                centered(
                    ctx,
                    RichText::new(format!("Erro no cliente do LoL: {message}")).heading(),
                )
            }
        }
    }
}

impl App {
    fn reset_selection(&mut self) {
        self.selection_load.reset();
        self.selection = None;
        self.patch_check_in_flight = false;
        self.applier = Applier::default();
        self.selection_error = None;
    }

    fn select(&mut self, ctx: &egui::Context, lcu: &LcuPollState) {
        let LcuPollState::Selecting(state) = lcu else {
            return;
        };
        let now = Instant::now();
        if state.champion_id.is_none() {
            if self.selection_load.observe(state) {
                self.selection = None;
            }
        } else if self.selection_load.should_start(state, now) {
            self.selection = None;
            if let Err(error) = self.ui_worker.send(UiCommand::LoadSelection(state.clone())) {
                self.selection_load.failed(now);
                self.selection_error = Some(error);
            }
        }
        if self.selection.is_none() {
            ctx.request_repaint_after(Duration::from_secs(5));
        } else {
            ctx.request_repaint_after(
                Duration::from_secs(1).min(self.next_patch_check.saturating_duration_since(now)),
            );
            if !self.patch_check_in_flight && now >= self.next_patch_check {
                let current_patch = self
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.patch.clone())
                    .unwrap_or_default();
                self.patch_check_in_flight = true;
                if let Err(error) = self.ui_worker.send(UiCommand::CheckVersion {
                    state: state.clone(),
                    current_patch,
                }) {
                    self.patch_check_in_flight = false;
                    self.next_patch_check = now + Duration::from_secs(60);
                    self.selection_error = Some(error);
                }
            }
        }
        let actions = self.selection.as_mut().map_or_else(Vec::new, |selection| {
            selection.observe_client(state, &self.names);
            self.applier
                .on_state_at(state, &self.settings, selection.user_edited, now)
        });
        for action in actions {
            self.apply_selection(action);
        }
        let fallback = state.champion_id.map_or_else(
            || "Escolha um campeão".to_owned(),
            |id| format!("Campeão {id}"),
        );
        let selection = self.selection.as_ref();
        let view = select_screen(SelectScreenInput {
            champion: selection.map_or(fallback.as_str(), |data| data.champion.as_str()),
            position: &state.position,
            recommendation: selection.map(|data| &data.recommendation),
            source_patch: selection.and_then(|data| data.source_patch.as_deref()),
            source_games: selection.and_then(|data| data.source_games),
            names: &self.names,
            item_names: &|id| {
                self.catalog
                    .as_ref()
                    .map_or_else(|| id.to_string(), |catalog| catalog.name(id))
            },
            differences: selection.map_or(&[], |data| data.differences.as_slice()),
            applied: selection.map_or_else(AppliedItems::default, |data| data.applied),
            limit_reached: selection.is_some_and(|data| data.limit_reached),
            settings_corrupted: self.settings.corrupted,
            loading: self.selection_load.is_loading(state),
            error: self.selection_error.as_deref(),
        });
        if let Some(action) = crate::ui_select::draw(ctx, &view) {
            self.apply_selection(match action {
                SelectAction::ApplyRunes => Action::Runes,
                SelectAction::ApplySpells => Action::Spells,
                SelectAction::ApplyItemSet => Action::ItemSet,
            });
        }
    }

    fn apply_selection(&mut self, action: Action) {
        let Some(recommendation) = self
            .selection
            .as_ref()
            .map(|data| data.recommendation.clone())
        else {
            return;
        };
        let Some(key) = self
            .selection
            .as_ref()
            .map(|selection| selection.key.clone())
        else {
            return;
        };
        if let Err(error) = self.ui_worker.send(UiCommand::Apply {
            key,
            action,
            recommendation,
        }) {
            self.applier.confirm_at(action, false, Instant::now());
            self.selection_error = Some(error);
        }
    }

    fn editor(&mut self, ctx: &egui::Context) {
        if !self.editors.loaded && !self.editor_requests.loading() {
            self.process_editor_action(EditorAction::Refresh);
        }
        let actions = crate::ui_editors::draw(
            ctx,
            &mut self.editors,
            &mut self.settings,
            &self.names,
            self.catalog.as_ref(),
            !self.editor_requests.loading(),
        );
        for action in actions {
            self.process_editor_action(action);
        }
    }

    fn process_editor_action(&mut self, action: EditorAction) {
        let generation = self.editor_requests.begin();
        let selection_session = self.lcu_state.lock().ok().and_then(|state| match &*state {
            LcuPollState::Selecting(state) => Some(state.session_id.clone()),
            _ => None,
        });
        if let Err(error) = self.ui_worker.send(UiCommand::Editor {
            generation,
            action,
            settings: self.settings.clone(),
            selection_session,
        }) {
            self.editor_requests.complete(generation);
            self.editors.status = Some(error);
        }
    }

    fn drain_worker(&mut self) {
        while let Ok(result) = self.ui_worker.results.try_recv() {
            match result {
                UiResult::Selection {
                    state,
                    result,
                    is_check,
                } => {
                    if !self.selection_load.is_current(&state) {
                        continue;
                    }
                    self.patch_check_in_flight = false;
                    match *result {
                        Ok(Some(payload)) => {
                            self.selection_error = None;
                            self.catalog = Some(payload.catalog);
                            self.names = payload.names;
                            self.selection = Some(payload.selection);
                            self.selection_load.succeeded();
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Ok(None) => {
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Err(_) if is_check => {
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Err(_) => self.selection_load.failed(Instant::now()),
                    }
                }
                UiResult::Applied {
                    key,
                    action,
                    result,
                } => {
                    if self.selection.as_ref().map(|selection| &selection.key) != Some(&key) {
                        continue;
                    }
                    let succeeded = result.as_ref().is_ok_and(|applied| *applied == Some(true));
                    if let Err(error) = &result {
                        self.selection_error = Some(error.clone());
                    } else if result.as_ref().is_ok_and(Option::is_some) {
                        self.selection_error = None;
                    }
                    if let Some(selection) = &mut self.selection {
                        match result {
                            Ok(Some(true)) => selection.mark_applied(action, &self.names),
                            Ok(Some(false)) => selection.limit_reached = true,
                            Ok(None) | Err(_) => {}
                        }
                    }
                    self.applier.confirm(action, succeeded);
                }
                UiResult::Editor {
                    generation,
                    result,
                    saved_settings,
                    edited_runes,
                    selection_session,
                } => {
                    if !self.editor_requests.complete(generation) {
                        continue;
                    }
                    match *result {
                        Ok(snapshot) => {
                            if edited_runes
                                && selection_session.as_deref()
                                    == self
                                        .selection
                                        .as_ref()
                                        .map(|selection| selection.key.session_id.as_str())
                                && let Some(selection) = &mut self.selection
                            {
                                selection.user_edited = true;
                            }
                            if saved_settings {
                                self.settings.corrupted = false;
                            }
                            if let Some(snapshot) = snapshot {
                                self.editors.rules = Some(snapshot.rules);
                                self.editors.pages = snapshot.pages;
                                self.editors.sets = snapshot.sets;
                                if let Some(catalog) = snapshot.catalog {
                                    self.catalog = Some(catalog);
                                }
                                if let Some(names) = snapshot.names {
                                    self.names = names;
                                }
                                let page_index = self
                                    .editors
                                    .selected_page
                                    .filter(|index| *index < self.editors.pages.len())
                                    .or_else(|| (!self.editors.pages.is_empty()).then_some(0));
                                let set_index = self
                                    .editors
                                    .selected_set
                                    .filter(|index| *index < self.editors.sets.len())
                                    .or_else(|| (!self.editors.sets.is_empty()).then_some(0));
                                self.editors.page_draft = None;
                                self.editors.set_draft = None;
                                if let Some(index) = page_index {
                                    self.editors.edit_page(index);
                                }
                                if let Some(index) = set_index {
                                    self.editors.edit_set(index);
                                }
                                self.editors.loaded = true;
                            }
                            self.editors.status = Some("Salvo".to_owned());
                        }
                        Err(error) => {
                            self.editors.loaded = true;
                            self.editors.status = Some(format!("Não foi possível salvar: {error}"));
                        }
                    }
                }
            }
        }
    }
}

pub fn compare_patch_versions(left: &str, right: &str) -> Ordering {
    let components = |version: &str| {
        version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    components(left).cmp(&components(right))
}

fn load_champion(cache_dir: &Path, patch: &str, champion_id: u32) -> Option<(String, String)> {
    let json = fs::read_to_string(cache_dir.join(patch).join("champion.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    value["data"].as_object()?.values().find_map(|champion| {
        (champion["key"].as_str()?.parse::<u32>().ok()? == champion_id).then(|| {
            (
                champion["name"].as_str().unwrap_or("Campeão").to_owned(),
                champion["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
            )
        })
    })
}

fn centered(ctx: &egui::Context, text: RichText) {
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.centered_and_justified(|ui| ui.label(text));
    });
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::lcu::champ_select::{SelectionKey, Source};
    use crate::lcu::runes::RunePageDraft;
    use crate::settings::Settings;
    use serde_json::{Value, json};
    use std::cell::{Cell, RefCell};

    struct SwapHttp {
        writes: RefCell<Vec<String>>,
    }

    impl LcuHttp for SwapHttp {
        fn get(&self, path: &str) -> Result<Value, LcuError> {
            assert_eq!(path, "/lol-champ-select/v1/session");
            Ok(json!({
                "gameId": 10,
                "localPlayerCellId": 0,
                "queueId": 420,
                "myTeam": [{
                    "cellId": 0,
                    "championId": 145,
                    "assignedPosition": "bottom",
                    "spell1Id": 4,
                    "spell2Id": 7
                }],
                "actions": []
            }))
        }

        fn send(&self, method: &str, path: &str, _body: &Value) -> Result<Value, LcuError> {
            self.writes.borrow_mut().push(format!("{method} {path}"));
            Ok(Value::Null)
        }
    }

    #[test]
    fn champion_swap_discards_in_flight_apply() {
        let http = SwapHttp {
            writes: RefCell::new(Vec::new()),
        };
        let key = SelectionKey {
            session_id: "10".to_owned(),
            champion_id: 234,
            position: "JUNGLE".to_owned(),
        };
        let recommendation = Recommendation {
            page: None,
            spells: (4, 11),
            item_set: None,
            source: Source::Riot,
        };

        let applied = apply_recommendation(&http, &key, Action::Spells, &recommendation).unwrap();

        assert_eq!(applied, None);
        assert!(http.writes.borrow().is_empty());
    }

    struct TransientHttp {
        attempts: Cell<u8>,
    }

    impl LcuHttp for TransientHttp {
        fn get(&self, path: &str) -> Result<Value, LcuError> {
            assert_eq!(path, "/lol-champ-select/v1/session");
            Ok(json!({
                "gameId": 10,
                "localPlayerCellId": 0,
                "queueId": 420,
                "myTeam": [{
                    "cellId": 0,
                    "championId": 234,
                    "assignedPosition": "jungle",
                    "spell1Id": 4,
                    "spell2Id": 14
                }],
                "actions": [[{
                    "actorCellId": 0,
                    "type": "pick",
                    "completed": true
                }]]
            }))
        }

        fn send(&self, _method: &str, _path: &str, _body: &Value) -> Result<Value, LcuError> {
            let attempt = self.attempts.get() + 1;
            self.attempts.set(attempt);
            if attempt == 1 {
                Err(LcuError::Other("falha transitória".to_owned()))
            } else {
                Ok(Value::Null)
            }
        }
    }

    #[test]
    fn worker_transient_failure_succeeds_on_second_attempt() {
        let http = TransientHttp {
            attempts: Cell::new(0),
        };
        let state = parse_session(&http.get("/lol-champ-select/v1/session").unwrap(), 0);
        let key = state.selection_key().unwrap();
        let recommendation = Recommendation {
            page: None,
            spells: (4, 11),
            item_set: None,
            source: Source::Riot,
        };
        let settings = Settings {
            auto_spells: true,
            ..Settings::default()
        };
        let now = Instant::now();
        let mut applier = Applier::default();

        assert_eq!(
            applier.on_state_at(&state, &settings, false, now),
            vec![Action::Spells]
        );
        assert!(apply_recommendation(&http, &key, Action::Spells, &recommendation).is_err());
        applier.confirm_at(Action::Spells, false, now);
        assert!(
            applier
                .on_state_at(&state, &settings, false, now + Duration::from_secs(4))
                .is_empty()
        );
        assert_eq!(
            applier.on_state_at(&state, &settings, false, now + Duration::from_secs(5)),
            vec![Action::Spells]
        );
        assert_eq!(
            apply_recommendation(&http, &key, Action::Spells, &recommendation).unwrap(),
            Some(true)
        );
        applier.confirm_at(Action::Spells, true, now + Duration::from_secs(5));
        assert_eq!(http.attempts.get(), 2);
    }

    #[test]
    fn automatic_runes_stop_after_active_page_edit() {
        let wanted = RunePageDraft {
            name: "live-builds".to_owned(),
            primary_style: 8000,
            sub_style: 8100,
            perks: [8005, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
        };
        let recommendation = Recommendation {
            page: Some(wanted.clone()),
            spells: (4, 11),
            item_set: None,
            source: Source::Riot,
        };
        let key = SelectionKey {
            session_id: "10".to_owned(),
            champion_id: 234,
            position: "JUNGLE".to_owned(),
        };
        let mut selection = SelectionUi {
            key,
            champion: "Viego".to_owned(),
            recommendation,
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: Some(wanted.clone()),
            user_edited: false,
            observed_page: Some(wanted.clone()),
            observed_spells: (4, 11),
            stale_page: None,
        };
        let mut edited = wanted;
        edited.perks[0] = 8008;
        let mut state = SelectState {
            phase: crate::lcu::champ_select::Phase::ChampSelect,
            session_id: "10".to_owned(),
            champion_id: Some(234),
            locked: true,
            position: "JUNGLE".to_owned(),
            spells: (4, 11),
            map_mode: (11, "CLASSIC".to_owned()),
            auto_apply: true,
            active_page: Some(edited),
        };

        selection.observe_client(&state, &KnowledgeNames::default());
        let actions = Applier::default().on_state(
            &state,
            &Settings {
                auto_runes: true,
                ..Settings::default()
            },
            selection.user_edited,
        );

        assert!(selection.user_edited);
        assert!(!actions.contains(&Action::Runes));
        state.active_page = None;
    }

    #[test]
    fn stale_poll_immediately_after_apply_is_not_a_user_edit() {
        let wanted = RunePageDraft {
            name: "live-builds".to_owned(),
            primary_style: 8000,
            sub_style: 8100,
            perks: [8005, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
        };
        let mut previous = wanted.clone();
        previous.perks[0] = 8008;
        let mut selection = SelectionUi {
            key: SelectionKey {
                session_id: "10".to_owned(),
                champion_id: 234,
                position: "JUNGLE".to_owned(),
            },
            champion: "Viego".to_owned(),
            recommendation: Recommendation {
                page: Some(wanted),
                spells: (4, 11),
                item_set: None,
                source: Source::Riot,
            },
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            user_edited: false,
            observed_page: Some(previous.clone()),
            observed_spells: (4, 11),
            stale_page: None,
        };
        let state = SelectState {
            phase: crate::lcu::champ_select::Phase::ChampSelect,
            session_id: "10".to_owned(),
            champion_id: Some(234),
            locked: true,
            position: "JUNGLE".to_owned(),
            spells: (4, 11),
            map_mode: (11, "CLASSIC".to_owned()),
            auto_apply: true,
            active_page: Some(previous),
        };

        selection.mark_applied(Action::Runes, &KnowledgeNames::default());
        selection.observe_client(&state, &KnowledgeNames::default());

        assert!(!selection.user_edited);
    }

    #[test]
    fn old_editor_generation_does_not_finish_current_request() {
        let mut tracker = EditorRequestTracker::default();
        let old = tracker.begin();
        let current = tracker.begin();

        assert!(!tracker.complete(old));
        assert!(tracker.loading());
        assert!(tracker.complete(current));
        assert!(!tracker.loading());
    }

    #[test]
    fn version_discovery_failure_uses_latest_valid_cache() {
        let cache = tempfile::tempdir().unwrap();
        for patch in ["16.21.1", "16.22.1"] {
            let directory = cache.path().join(patch);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("item.json"),
                include_str!("../tests/fixtures/item_sample.json").replace("16.22.1", patch),
            )
            .unwrap();
            let short = patch.rsplit_once('.').unwrap().0;
            std::fs::write(
                cache.path().join(format!("knowledge-{short}.json")),
                include_str!("../tests/fixtures/knowledge-16.22.json").replace("16.22", short),
            )
            .unwrap();
        }
        std::fs::create_dir_all(cache.path().join("16.23.1")).unwrap();
        std::fs::write(cache.path().join("16.23.1/item.json"), "inválido").unwrap();

        let resolved = resolve_selection_version(None, cache.path()).unwrap();

        assert_eq!(resolved, "16.22.1");
    }
}

impl App {
    /// Draws layout B; returns the enemy champion the user clicked, if any.
    fn panel(&self, ctx: &egui::Context, view: &PanelView) -> Option<String> {
        egui::TopBottomPanel::top("high_elo").show(ctx, |ui| high_elo(ui, &view.high_elo));
        egui::TopBottomPanel::bottom("lane").show(ctx, |ui| lane(ui, view.lane.as_ref()));
        egui::SidePanel::left("me")
            .exact_width(330.0)
            .resizable(false)
            .show(ctx, |ui| me(ui, &view.me, &view.skills));
        let picked = egui::SidePanel::right("teams")
            .exact_width(420.0)
            .resizable(false)
            .show(ctx, |ui| teams(ui, &view.teams, view.lane.as_ref()))
            .inner;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(note) = &view.catalog_note {
                ui.label(RichText::new(note).color(WARN));
            }
            build(ui, &view.build, view.me.gold);
        });
        picked
    }
}

fn high_elo(ui: &mut egui::Ui, section: &HighEloSection) {
    let HighEloSection::Recommendation(view) = section else {
        let HighEloSection::NoData(message) = section else {
            unreachable!()
        };
        ui.label(message);
        return;
    };
    high_elo_recommendation(ui, view);
}

fn high_elo_recommendation(ui: &mut egui::Ui, view: &HighEloView) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&view.header).strong());
        if view.low_sample {
            ui.label(RichText::new("⚠ poucas partidas").color(WARN));
        }
    });
    ui.label(format!(
        "RUNAS  {} ({})",
        view.keystone, view.keystone_source
    ));
    ui.label(format!("{} · {}", view.primary_style, view.sub_style));
    if !view.stat_perks.is_empty() {
        ui.label(format!("Fragmentos: {}", view.stat_perks.join(" · ")));
    }
    if !view.summoner_spells.is_empty() {
        ui.label(format!("FEITIÇOS  {}", view.summoner_spells.join(" · ")));
    }
}

fn me(ui: &mut egui::Ui, me: &MeView, skills: &SkillView) {
    ui.heading(format!("{} · nível {}", me.champion, me.level));
    if let Some(fraction) = me.xp_fraction {
        ui.add(egui::ProgressBar::new(fraction));
    }
    ui.label(format!("Ouro: {}   CS: {}", me.gold, me.cs));
    ui.label(format!("KDA: {}/{}/{}", me.kda.0, me.kda.1, me.kda.2));
    ui.separator();
    ui.label(RichText::new("Itens").color(MUTED));
    if me.items.is_empty() {
        ui.label("—");
    }
    for item in &me.items {
        ui.label(item);
    }
    ui.separator();
    ui.label(RichText::new("Skills").color(MUTED));
    ui.label(format!(
        "Q {}   W {}   E {}   R {}",
        skills.q, skills.w, skills.e, skills.r
    ));
    if skills.points_available > 0 {
        let next = skills
            .next_skill
            .map_or(String::new(), |c| format!(" → {c}"));
        ui.label(
            RichText::new(format!(
                "{} ponto(s) de skill{next}",
                skills.points_available
            ))
            .color(WARN),
        );
    }
    if skills.ult_hint {
        ui.label(RichText::new("Ultimate disponível para evoluir").color(WARN));
    }
    if let Some(label) = &skills.priority_label {
        ui.label(format!("Prioridade: {label}"));
    }
}

fn build(ui: &mut egui::Ui, build: &BuildView, gold: u32) {
    ui.heading("Build recomendada");
    ui.label(RichText::new(format!("Ouro agora: {gold}")).color(GOOD));
    if let Some(next) = &build.next_item {
        ui.label(format!(
            "Próximo item: {} (faltam {} de ouro)",
            next.name, next.missing_gold
        ));
        if let Some((name, cost)) = &next.next_component {
            ui.label(format!("Próximo componente: {name} ({cost})"));
        }
    }
    if let Some(line) = &build.free_boots_line {
        ui.label(line);
    }
    if !build.affordable_now.is_empty() {
        ui.label(format!(
            "Dá para comprar já: {}",
            build.affordable_now.join(", ")
        ));
    }
    ui.separator();
    if let Some(message) = &build.empty_message {
        ui.label(message);
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for block in &build.blocks {
            ui.label(RichText::new(&block.name).strong());
            for (name, owned) in &block.items {
                if *owned {
                    ui.label(RichText::new(format!("✓ {name}")).color(GOOD));
                } else {
                    ui.label(format!("• {name}"));
                }
            }
            ui.add_space(6.0);
        }
    });
}

/// Returns the champion of the enemy row clicked this frame.
fn teams(ui: &mut egui::Ui, teams: &[TeamView], lane: Option<&LaneView>) -> Option<String> {
    let mut picked = None;
    for (index, team) in teams.iter().enumerate() {
        ui.label(RichText::new(format!("{} · itens {}", team.label, team.items_value)).strong());
        for player in &team.players {
            let text = format!(
                "{}  nv {}  {}/{}/{}  {}",
                player.champion,
                player.level,
                player.kda.0,
                player.kda.1,
                player.kda.2,
                player.items_value
            );
            if index == 0 {
                let text = RichText::new(text);
                ui.label(if player.is_me { text.color(GOOD) } else { text });
            } else {
                let selected = lane.is_some_and(|l| l.opponent == player.champion);
                if ui.selectable_label(selected, text).clicked() {
                    picked = Some(player.champion.clone());
                }
            }
        }
        ui.add_space(8.0);
    }
    if let [first, second] = teams {
        let diff = i64::from(first.items_value) - i64::from(second.items_value);
        ui.label(RichText::new(format!("Diferença de itens: {diff:+}")).color(MUTED));
    }
    picked
}

fn lane(ui: &mut egui::Ui, lane: Option<&LaneView>) {
    match lane {
        Some(l) => ui.label(format!(
            "Rota vs {}: nível {:+}, itens {:+}",
            l.opponent, l.level_diff, l.items_value_diff
        )),
        None => ui.label(RichText::new("Rota: clique num inimigo para comparar").color(MUTED)),
    };
}
