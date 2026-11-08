use crate::catalog::ItemCatalog;
use crate::knowledge::{KnowledgeNames, KnowledgeSource};
use crate::lcu::champ_select::{
    Action, Applier, Recommendation, SelectState, SelectionKey, parse_session, recommend, reminder,
    set_spells,
};
use crate::lcu::item_sets::{create_user_set, delete_user_set, list_sets, save_user_set};
use crate::lcu::item_sets::{
    delete_global_item_set_files, list_global_item_set_files, upsert_app_set_with_exclusive,
};
use crate::lcu::perks::{
    ApplyOutcome, create_page, delete_page, list_pages, update_page, upsert_app_page,
};
use crate::lcu::poller::{LcuPollState, LcuPoller};
use crate::lcu::runes::{RunePageDraft, RuneRules};
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
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const MUTED: Color32 = Color32::from_gray(150);
const GOOD: Color32 = Color32::from_rgb(110, 200, 120);
const SELECTION_CHANGED: &str = "A seleção mudou durante a aplicação: confira runas e feitiços";
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

    /// Worker morto: nenhuma nova tentativa adianta, então para de mostrar "carregando".
    fn give_up(&mut self) {
        self.in_flight = false;
        self.failures = u8::MAX;
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
    select_autosave: SelectAutosave,
    icons: IconLoader,
    global_delete_confirmation: bool,
    confirmed_global_item_set_files: Vec<PathBuf>,
    selection_generation: u64,
}

#[derive(Default)]
struct EditorRequestTracker {
    generation: u64,
    loading: bool,
}

#[derive(Default)]
struct SelectAutosave {
    due_at: Option<Instant>,
    in_flight: bool,
    superseded: bool,
    failures: u8,
}

impl SelectAutosave {
    fn changed(&mut self, now: Instant, valid: bool) {
        self.superseded |= self.in_flight;
        self.failures = 0;
        self.due_at = valid.then_some(now + Duration::from_millis(500));
    }

    fn is_due(&self, now: Instant) -> bool {
        !self.in_flight && self.due_at.is_some_and(|due| now >= due)
    }

    fn start(&mut self, now: Instant) -> bool {
        if !self.is_due(now) {
            return false;
        }
        self.due_at = None;
        self.in_flight = true;
        self.superseded = false;
        true
    }

    fn succeeded(&mut self) -> bool {
        self.in_flight = false;
        if std::mem::take(&mut self.superseded) {
            return false;
        }
        self.failures = 0;
        true
    }

    /// Retorna `true` quando as três novas tentativas também falharam.
    fn failed(&mut self, now: Instant) -> bool {
        self.in_flight = false;
        if std::mem::take(&mut self.superseded) {
            return false;
        }
        self.failures = self.failures.saturating_add(1);
        let delay = match self.failures {
            1 => Some(Duration::from_secs(2)),
            2 => Some(Duration::from_secs(5)),
            3 => Some(Duration::from_secs(10)),
            _ => None,
        };
        if let Some(delay) = delay {
            self.due_at = Some(now + delay);
            false
        } else {
            self.due_at = None;
            true
        }
    }

    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.due_at.map(|due| due.saturating_duration_since(now))
    }

    fn can_save(rules: &RuneRules, draft: &crate::lcu::runes::RunePageDraft) -> bool {
        rules.validate(draft).is_ok()
    }
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

    fn clear(&mut self) {
        self.loading = false;
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
    edited_page: Option<crate::lcu::runes::RunePageDraft>,
    /// Índice da opção de runa que o usuário escolheu (só `choose_option` define).
    chosen: Option<usize>,
    user_edited: bool,
    observed_page: Option<crate::lcu::runes::RunePageDraft>,
    observed_spells: (u32, u32),
    /// Page the client showed before our apply; a poll still reading it is stale, not a user edit.
    stale_page: Option<crate::lcu::runes::RunePageDraft>,
    rules: Option<RuneRules>,
    global_item_set_files: Vec<PathBuf>,
}

struct IconLoader {
    commands: Sender<String>,
    results: Receiver<(String, Result<crate::ui_editors::DecodedIcon, String>)>,
    requested: HashSet<String>,
    textures: HashMap<String, egui::TextureHandle>,
}

impl IconLoader {
    fn spawn(ctx: egui::Context, cache_dir: PathBuf) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<String>();
        let (result_tx, result_rx) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(icon) = command_rx.recv() {
                let result = load_rune_icon(&cache_dir, &icon);
                if result_tx.send((icon, result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            commands: command_tx,
            results: result_rx,
            requested: HashSet::new(),
            textures: HashMap::new(),
        }
    }

    fn prepare(&mut self, names: &KnowledgeNames) {
        let paths = names
            .style_icons
            .values()
            .chain(names.rune_icons.values())
            .map(String::as_str)
            .chain(
                [5001, 5005, 5007, 5008, 5010, 5011, 5013]
                    .into_iter()
                    .filter_map(|id| names.rune_icon(id)),
            )
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for path in paths {
            if self.requested.insert(path.clone()) && self.commands.send(path).is_err() {
                break;
            }
        }
    }

    fn prepare_items(&mut self, build: &BuildView, patch: &str) {
        for id in build
            .items
            .iter()
            .map(|item| item.id)
            .chain(build.affordable_now_ids.iter().copied())
        {
            let key = item_icon_key(patch, id);
            if self.requested.insert(key.clone()) && self.commands.send(key).is_err() {
                break;
            }
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        while let Ok((path, result)) = self.results.try_recv() {
            if let Ok(decoded) = result {
                let image = egui::ColorImage::from_rgba_unmultiplied(decoded.size, &decoded.rgba);
                let texture = ctx.load_texture(path.clone(), image, egui::TextureOptions::LINEAR);
                self.textures.insert(path, texture);
            }
        }
    }
}

const MAX_ICON_BYTES: usize = 512 * 1024;
const MAX_ICON_DIMENSION: usize = 256;

fn read_icon_bytes(mut reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((MAX_ICON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ICON_BYTES {
        return Err("icon larger than 512 KB".to_owned());
    }
    Ok(bytes)
}

fn decode_rune_icon(bytes: &[u8]) -> Result<crate::ui_editors::DecodedIcon, String> {
    let decoded = crate::ui_editors::decode_icon(bytes)?;
    if decoded
        .size
        .iter()
        .any(|dimension| *dimension > MAX_ICON_DIMENSION)
    {
        return Err("icon larger than 256x256".to_owned());
    }
    Ok(decoded)
}

fn load_rune_icon_with<F>(
    cache_dir: &Path,
    icon: &str,
    download: F,
) -> Result<crate::ui_editors::DecodedIcon, String>
where
    F: FnOnce() -> Result<Vec<u8>, String>,
{
    let path = crate::ui_editors::icon_cache_path(cache_dir, icon);
    if let Ok(bytes) = fs::read(&path)
        && let Ok(decoded) = decode_rune_icon(&bytes)
    {
        return Ok(decoded);
    }
    if path.exists() {
        fs::remove_file(&path).map_err(|error| error.to_string())?;
    }

    let bytes = download()?;
    let decoded = decode_rune_icon(&bytes)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("png.tmp");
    fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, &path).map_err(|error| error.to_string())?;
    Ok(decoded)
}

fn load_rune_icon(cache_dir: &Path, icon: &str) -> Result<crate::ui_editors::DecodedIcon, String> {
    load_rune_icon_with(cache_dir, icon, || {
        let url = icon
            .strip_prefix("item/")
            .and_then(|item| {
                let (patch, id) = item.split_once('/')?;
                id.parse::<u32>().ok()?;
                Some(format!(
                    "https://ddragon.leagueoflegends.com/cdn/{patch}/img/item/{id}.png"
                ))
            })
            .unwrap_or_else(|| format!("https://ddragon.leagueoflegends.com/cdn/img/{icon}"));
        let mut response = ureq::get(&url)
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .map_err(|error| error.to_string())?;
        read_icon_bytes(response.body_mut().as_reader())
    })
}

fn item_icon_key(patch: &str, id: u32) -> String {
    format!("item/{patch}/{id}")
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

    /// O usuário escolheu uma opção de runa: vale como edição dele (o automático não troca mais).
    fn choose_option(&mut self, index: usize, names: &KnowledgeNames) -> bool {
        let Some(chosen) = self.recommendation.choose(index) else {
            return false;
        };
        self.recommendation = chosen;
        self.edited_page = None;
        self.chosen = Some(index);
        self.applied.item_set = false;
        self.recalculate_differences(names);
        true
    }

    /// Recarga com a mesma chave (troca de patch) não pode esquecer o que já foi aplicado.
    fn inherit(&mut self, old: SelectionUi) {
        self.applied = old.applied;
        self.applied_page = old.applied_page;
        self.edited_page = old.edited_page;
        self.user_edited = old.user_edited;
        self.stale_page = old.stale_page;
        if let Some(index) = old.chosen
            && let Some(chosen) = self.recommendation.choose(index)
        {
            self.recommendation = chosen;
            self.chosen = Some(index);
        }
        if let Some(page) = self.edited_page.clone() {
            self.recommendation.page = Some(page);
        }
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
        exclusive_item_set: bool,
    },
    SaveSelectionRunes {
        key: SelectionKey,
        draft: RunePageDraft,
    },
    DeleteGlobalItemSets {
        key: SelectionKey,
        generation: u64,
        files: Vec<PathBuf>,
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
        selection_changed: bool,
    },
    SelectionRunesSaved {
        key: SelectionKey,
        result: Result<Option<bool>, String>,
        selection_changed: bool,
    },
    Editor {
        generation: u64,
        result: Box<Result<Option<EditorSnapshot>, String>>,
        saved_settings: bool,
        edited_runes: bool,
        reset_page: bool,
        reset_set: bool,
        selection_session: Option<String>,
    },
    GlobalItemSetsDeleted {
        key: SelectionKey,
        generation: u64,
        result: Result<(), String>,
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
                        exclusive_item_set,
                    } => {
                        let checked = connect_lcu(lockfile.as_deref())
                            .map_err(|error| error.to_string())
                            .and_then(|http| {
                                if exclusive_item_set {
                                    apply_and_verify_with_exclusive(
                                        &http,
                                        &key,
                                        action,
                                        &recommendation,
                                        true,
                                    )
                                } else {
                                    apply_and_verify(&http, &key, action, &recommendation)
                                }
                            });
                        let (result, selection_changed) = match checked {
                            Ok((applied, changed)) => (Ok(applied), changed),
                            Err(error) => (Err(error), false),
                        };
                        UiResult::Applied {
                            key,
                            action,
                            result,
                            selection_changed,
                        }
                    }
                    UiCommand::SaveSelectionRunes { key, draft } => {
                        let checked = connect_lcu(lockfile.as_deref())
                            .map_err(|error| error.to_string())
                            .and_then(|http| {
                                if session_key(&http)?.as_ref() != Some(&key) {
                                    return Ok((None, true));
                                }
                                upsert_app_page(&http, &draft)
                                    .map(|outcome| {
                                        (Some(outcome != ApplyOutcome::LimitReached), false)
                                    })
                                    .map_err(|error| error.to_string())
                            });
                        let (result, selection_changed) = match checked {
                            Ok((applied, changed)) => (Ok(applied), changed),
                            Err(error) => (Err(error), false),
                        };
                        UiResult::SelectionRunesSaved {
                            key,
                            result,
                            selection_changed,
                        }
                    }
                    UiCommand::DeleteGlobalItemSets {
                        key,
                        generation,
                        files,
                    } => {
                        let result = connect_lcu(lockfile.as_deref())
                            .map_err(|error| error.to_string())
                            .and_then(|http| {
                                if session_key(&http)?.as_ref() != Some(&key) {
                                    return Err(SELECTION_CHANGED.to_owned());
                                }
                                find_lockfile(lockfile.as_deref())
                                    .ok_or_else(|| "cliente do LoL fechado".to_owned())
                                    .and_then(|path| {
                                        delete_global_item_set_files(&path, &files)
                                            .map_err(|error| error.to_string())
                                    })
                            });
                        UiResult::GlobalItemSetsDeleted {
                            key,
                            generation,
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
                        let reset_page = matches!(
                            &action,
                            EditorAction::CreatePage(_)
                                | EditorAction::SavePage(_, _)
                                | EditorAction::DeletePage(_)
                        );
                        let reset_set = matches!(
                            &action,
                            EditorAction::CreateSet(_)
                                | EditorAction::SaveSet(_)
                                | EditorAction::DeleteSet(_)
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
                            reset_page,
                            reset_set,
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
        &data.knowledge_names,
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
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: active.map(|page| page.draft),
            observed_spells: state.spells,
            stale_page: None,
            rules: Some(rules),
            global_item_set_files: find_lockfile(lockfile)
                .and_then(|path| list_global_item_set_files(&path).ok())
                .unwrap_or_default(),
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

/// Aplica e relê a sessão; `true` no fim = a seleção mudou durante a aplicação (sem reverter).
// shortcut: a LCU não tem transação; só vemos mudança até esta releitura e, se ela ocorrer depois,
// o próximo poll da seleção mostra a diferença.
fn apply_and_verify(
    http: &dyn LcuHttp,
    key: &SelectionKey,
    action: Action,
    recommendation: &Recommendation,
) -> Result<(Option<bool>, bool), String> {
    let applied = apply_recommendation(http, key, action, recommendation)?;
    let changed = applied.is_some() && session_key(http)?.as_ref() != Some(key);
    Ok((applied, changed))
}

fn apply_and_verify_with_exclusive(
    http: &dyn LcuHttp,
    key: &SelectionKey,
    action: Action,
    recommendation: &Recommendation,
    exclusive_item_set: bool,
) -> Result<(Option<bool>, bool), String> {
    let applied =
        apply_recommendation_with_exclusive(http, key, action, recommendation, exclusive_item_set)?;
    let changed = applied.is_some() && session_key(http)?.as_ref() != Some(key);
    Ok((applied, changed))
}

fn session_key(http: &dyn LcuHttp) -> Result<Option<SelectionKey>, String> {
    let session = http
        .get("/lol-champ-select/v1/session")
        .map_err(|error| error.to_string())?;
    let my_cell = session["localPlayerCellId"]
        .as_i64()
        .ok_or_else(|| "sessão sem localPlayerCellId".to_owned())?;
    Ok(parse_session(&session, my_cell).selection_key())
}

fn apply_recommendation(
    http: &dyn LcuHttp,
    key: &SelectionKey,
    action: Action,
    recommendation: &Recommendation,
) -> Result<Option<bool>, String> {
    apply_recommendation_with_exclusive(http, key, action, recommendation, false)
}

fn apply_recommendation_with_exclusive(
    http: &dyn LcuHttp,
    key: &SelectionKey,
    action: Action,
    recommendation: &Recommendation,
    exclusive_item_set: bool,
) -> Result<Option<bool>, String> {
    if session_key(http)?.as_ref() != Some(key) {
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
            upsert_app_set_with_exclusive(http, set, exclusive_item_set)
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
        EditorAction::CreateSet(set) => {
            create_user_set(&http, &set).map_err(|error| error.to_string())?;
        }
        EditorAction::SaveSet(set) => {
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
        let icons = IconLoader::spawn(cc.egui_ctx.clone(), cache_dir.join("icons"));
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
            select_autosave: SelectAutosave::default(),
            icons,
            global_delete_confirmation: false,
            confirmed_global_item_set_files: Vec::new(),
            selection_generation: 0,
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
        self.icons.drain(ctx);
        if (screen == Screen::Select && self.selection.is_some())
            || (screen == Screen::Editor && self.editors.tab == crate::ui_editors::EditorTab::Runes)
        {
            self.icons.prepare(&self.names);
        }
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
        self.select_autosave = SelectAutosave::default();
        self.global_delete_confirmation = false;
        self.confirmed_global_item_set_files.clear();
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
        if self.select_autosave.is_due(now) {
            let pending = self.selection.as_ref().and_then(|selection| {
                selection
                    .recommendation
                    .page
                    .as_ref()
                    .filter(|draft| {
                        selection
                            .rules
                            .as_ref()
                            .is_some_and(|rules| SelectAutosave::can_save(rules, draft))
                    })
                    .map(|draft| (selection.key.clone(), draft.clone()))
            });
            if let Some((key, draft)) = pending
                && self.select_autosave.start(now)
                && let Err(error) = self
                    .ui_worker
                    .send(UiCommand::SaveSelectionRunes { key, draft })
                && self.select_autosave.failed(now)
            {
                self.selection_error = Some(format!(
                    "Não foi possível salvar as runas após 3 tentativas: {error}"
                ));
            }
        }
        if let Some(remaining) = self.select_autosave.remaining(now) {
            ctx.request_repaint_after(remaining);
        }
        for action in actions {
            let wanted = self
                .selection
                .as_ref()
                .is_some_and(|data| data.recommendation.can_apply(action));
            if wanted {
                self.apply_selection(action);
            } else {
                self.applier.confirm(action, true);
            }
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
            chosen: selection.and_then(|data| data.chosen),
            limit_reached: selection.is_some_and(|data| data.limit_reached),
            settings_corrupted: self.settings.corrupted,
            global_item_set_files: &selection.map_or_else(Vec::new, |data| {
                data.global_item_set_files
                    .iter()
                    .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
                    .collect()
            }),
            global_delete_confirmation: self.global_delete_confirmation,
            loading: self.selection_load.is_loading(state),
            error: self.selection_error.as_deref(),
        });
        let select_action = if let Some(selection) = self.selection.as_mut() {
            crate::ui_select::draw(
                ctx,
                &view,
                selection.rules.as_ref(),
                selection.recommendation.page.as_mut(),
                &self.names,
                &self.icons.textures,
            )
        } else {
            crate::ui_select::draw(ctx, &view, None, None, &self.names, &self.icons.textures)
        };
        match select_action {
            Some(SelectAction::ApplyRunes) => self.apply_selection(Action::Runes),
            Some(SelectAction::ApplySpells) => self.apply_selection(Action::Spells),
            Some(SelectAction::ApplyItemSet) => self.apply_selection(Action::ItemSet),
            Some(SelectAction::ChooseOption(index)) => self.choose_option(index),
            Some(SelectAction::EditRunes(draft)) => self.edit_selection_runes(draft, now),
            Some(SelectAction::RequestDeleteGlobalItemSets) => {
                self.global_delete_confirmation = true;
                self.confirmed_global_item_set_files = self
                    .selection
                    .as_ref()
                    .map(|selection| selection.global_item_set_files.clone())
                    .unwrap_or_default();
            }
            Some(SelectAction::CancelDeleteGlobalItemSets) => {
                self.global_delete_confirmation = false;
                self.confirmed_global_item_set_files.clear();
            }
            Some(SelectAction::ConfirmDeleteGlobalItemSets) => {
                self.global_delete_confirmation = false;
                let Some(selection) = self.selection.as_ref() else {
                    return;
                };
                let command = UiCommand::DeleteGlobalItemSets {
                    key: selection.key.clone(),
                    generation: self.selection_generation,
                    files: std::mem::take(&mut self.confirmed_global_item_set_files),
                };
                if let Err(error) = self.ui_worker.send(command) {
                    self.selection_error = Some(error);
                }
            }
            None => {}
        }
    }

    fn edit_selection_runes(&mut self, draft: RunePageDraft, now: Instant) {
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        if let Some(page) = selection.recommendation.page.as_mut() {
            *page = draft;
            selection.edited_page = Some(page.clone());
            selection.user_edited = true;
            self.applier.mark_choice();
            self.selection_error = selection
                .rules
                .as_ref()
                .and_then(|rules| rules.validate(page).err());
            self.select_autosave
                .changed(now, self.selection_error.is_none());
        }
    }

    fn choose_option(&mut self, index: usize) {
        if self
            .selection
            .as_mut()
            .is_some_and(|selection| selection.choose_option(index, &self.names))
        {
            self.applier.mark_choice();
            // o arsenal segue a página escolhida; o automático o refaz se estiver ligado
            self.applier.reset(Action::ItemSet);
            self.apply_selection(Action::Runes);
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
        if action == Action::Runes
            && self.selection.as_ref().is_some_and(|selection| {
                let Some(rules) = selection.rules.as_ref() else {
                    return true;
                };
                selection
                    .recommendation
                    .page
                    .as_ref()
                    .is_none_or(|draft| rules.validate(draft).is_err())
            })
        {
            self.selection_error = Some("A página de runas é inválida".to_owned());
            return;
        }
        if let Err(error) = self.ui_worker.send(UiCommand::Apply {
            key,
            action,
            recommendation,
            exclusive_item_set: self.settings.exclusive_item_set,
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
            &self.icons.textures,
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
        loop {
            let result = match self.ui_worker.results.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let message = "worker da interface foi encerrado; reinicie o app".to_owned();
                    self.selection_load.give_up();
                    self.editor_requests.clear();
                    self.patch_check_in_flight = false;
                    self.editors.status = Some(message.clone());
                    self.selection_error = Some(message);
                    break;
                }
            };
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
                            let mut fresh = payload.selection;
                            if let Some(old) = self.selection.take()
                                && old.key == fresh.key
                            {
                                fresh.inherit(old);
                            }
                            self.selection = Some(fresh);
                            self.selection_generation = self.selection_generation.wrapping_add(1);
                            self.selection_load.succeeded();
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Ok(None) => {
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Err(_) if is_check => {
                            self.next_patch_check = Instant::now() + Duration::from_secs(60);
                        }
                        Err(error) => {
                            self.selection_error = Some(error);
                            self.selection_load.failed(Instant::now());
                        }
                    }
                }
                UiResult::Applied {
                    key,
                    action,
                    result,
                    selection_changed,
                } => {
                    if selection_changed {
                        self.selection_error = Some(SELECTION_CHANGED.to_owned());
                    }
                    if self.selection.as_ref().map(|selection| &selection.key) != Some(&key) {
                        continue;
                    }
                    let succeeded = result.as_ref().is_ok_and(|applied| *applied == Some(true));
                    if let Err(error) = &result {
                        self.selection_error = Some(error.clone());
                    } else if result.as_ref().is_ok_and(Option::is_some) && !selection_changed {
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
                UiResult::SelectionRunesSaved {
                    key,
                    result,
                    selection_changed,
                } => {
                    if self.selection.as_ref().map(|selection| &selection.key) != Some(&key) {
                        continue;
                    }
                    if selection_changed {
                        self.select_autosave.succeeded();
                        self.selection_error = Some(SELECTION_CHANGED.to_owned());
                        continue;
                    }
                    match result {
                        Ok(Some(true)) => {
                            if self.select_autosave.succeeded() {
                                self.selection_error = None;
                                if let Some(selection) = &mut self.selection {
                                    selection.mark_applied(Action::Runes, &self.names);
                                }
                            }
                        }
                        Ok(Some(false)) => {
                            let message = "Limite de páginas de runas atingido".to_owned();
                            if self.select_autosave.failed(Instant::now()) {
                                self.selection_error = Some(format!(
                                    "Não foi possível salvar as runas após 3 tentativas: {message}"
                                ));
                            }
                        }
                        Ok(None) => {
                            self.select_autosave.succeeded();
                        }
                        Err(error) => {
                            if self.select_autosave.failed(Instant::now()) {
                                self.selection_error = Some(format!(
                                    "Não foi possível salvar as runas após 3 tentativas: {error}"
                                ));
                            }
                        }
                    }
                }
                UiResult::Editor {
                    generation,
                    result,
                    saved_settings,
                    edited_runes,
                    reset_page,
                    reset_set,
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
                                if let Some(catalog) = snapshot.catalog {
                                    self.catalog = Some(catalog);
                                }
                                if let Some(names) = snapshot.names {
                                    self.names = names;
                                }
                                self.editors.replace_lists(
                                    snapshot.pages,
                                    snapshot.sets,
                                    reset_page,
                                    reset_set,
                                );
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
                UiResult::GlobalItemSetsDeleted {
                    key,
                    generation,
                    result,
                } => {
                    if generation != self.selection_generation
                        || self.selection.as_ref().map(|selection| &selection.key) != Some(&key)
                    {
                        continue;
                    }
                    match result {
                        Ok(()) => {
                            if let Some(selection) = &mut self.selection {
                                selection.global_item_set_files.clear();
                            }
                        }
                        Err(error) => self.selection_error = Some(error),
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
    use std::io::Cursor;

    #[test]
    fn icon_download_rejects_data_above_limit() {
        let bytes = vec![0; 512 * 1024 + 1];

        assert!(read_icon_bytes(Cursor::new(bytes)).is_err());
    }

    #[test]
    fn invalid_cached_icon_is_recovered_from_download() {
        const PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15,
            0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        let cache = std::env::temp_dir().join(format!("live-builds-icon-{}", std::process::id()));
        let path = crate::ui_editors::icon_cache_path(&cache, "invalid.png");
        fs::create_dir_all(&cache).unwrap();
        fs::write(&path, b"invalid").unwrap();

        let decoded = load_rune_icon_with(&cache, "invalid.png", || Ok(PNG.to_vec())).unwrap();

        assert_eq!(decoded.size, [1, 1]);
        assert_eq!(fs::read(path).unwrap(), PNG);
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn select_edit_autosaves_after_debounce() {
        let mut save = SelectAutosave::default();
        let start = Instant::now();
        save.changed(start, true);

        assert!(!save.is_due(start + Duration::from_millis(499)));
        assert!(save.is_due(start + Duration::from_millis(500)));
    }

    #[test]
    fn rapid_edits_write_once() {
        let mut save = SelectAutosave::default();
        let start = Instant::now();
        save.changed(start, true);
        save.changed(start + Duration::from_millis(300), true);

        assert!(!save.is_due(start + Duration::from_millis(500)));
        assert!(save.is_due(start + Duration::from_millis(800)));
        assert!(save.start(start + Duration::from_millis(800)));
        save.succeeded();
        assert!(!save.is_due(start + Duration::from_secs(2)));
    }

    #[test]
    fn invalid_edit_is_not_saved() {
        let rules =
            RuneRules::from_json(include_str!("../tests/fixtures/lcu/perks-styles.json")).unwrap();
        let mut draft = crate::ui_editors::new_page(&rules).unwrap();
        draft.perks[0] = 999_999;

        assert!(!SelectAutosave::can_save(&rules, &draft));
    }

    #[test]
    fn invalid_edit_does_not_keep_repainting() {
        let mut save = SelectAutosave::default();
        let start = Instant::now();
        save.changed(start, true);
        assert!(save.start(start + Duration::from_millis(500)));
        save.changed(start + Duration::from_millis(600), false);
        assert!(!save.failed(start + Duration::from_millis(700)));

        assert_eq!(save.remaining(start + Duration::from_secs(1)), None);
        assert!(!save.is_due(start + Duration::from_secs(1)));
    }

    #[test]
    fn autosave_retries_after_lcu_failure() {
        let mut save = SelectAutosave::default();
        let start = Instant::now();
        save.changed(start, true);

        assert!(save.start(start + Duration::from_millis(500)));
        assert!(!save.failed(start + Duration::from_millis(500)));
        assert!(!save.start(start + Duration::from_millis(2499)));
        assert!(save.start(start + Duration::from_millis(2500)));

        assert!(!save.failed(start + Duration::from_millis(2500)));
        assert!(save.start(start + Duration::from_millis(7500)));
        assert!(!save.failed(start + Duration::from_millis(7500)));
        assert!(save.start(start + Duration::from_millis(17_500)));

        assert!(save.failed(start + Duration::from_millis(17_500)));
        assert_eq!(save.remaining(start + Duration::from_secs(30)), None);
    }

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
            pages: Vec::new(),
        };

        let applied = apply_recommendation(&http, &key, Action::Spells, &recommendation).unwrap();

        assert_eq!(applied, None);
        assert!(http.writes.borrow().is_empty());
    }

    #[test]
    fn ui_default_item_set_apply_does_not_enable_exclusive_mode() {
        struct ItemSetHttp {
            responses: RefCell<Vec<Value>>,
            writes: RefCell<Vec<Value>>,
        }

        impl LcuHttp for ItemSetHttp {
            fn get(&self, _path: &str) -> Result<Value, LcuError> {
                Ok(self.responses.borrow_mut().remove(0))
            }

            fn send(&self, _method: &str, _path: &str, body: &Value) -> Result<Value, LcuError> {
                self.writes.borrow_mut().push(body.clone());
                Ok(Value::Null)
            }
        }

        let key = SelectionKey {
            session_id: "10".to_owned(),
            champion_id: 234,
            position: "JUNGLE".to_owned(),
        };
        let session = json!({
            "gameId": 10, "localPlayerCellId": 0, "queueId": 420,
            "myTeam": [{"cellId": 0, "championId": 234, "assignedPosition": "jungle",
                        "spell1Id": 4, "spell2Id": 14}], "actions": []
        });
        let other_app = json!({
            "uid": "other-app-234", "associatedChampions": [234], "blocks": []
        });
        let http = ItemSetHttp {
            responses: RefCell::new(vec![
                session,
                json!({"summonerId": 7}),
                json!({"itemSets": [other_app.clone()]}),
            ]),
            writes: RefCell::new(Vec::new()),
        };
        let recommendation = Recommendation {
            page: None,
            spells: (4, 11),
            item_set: Some(crate::lcu::item_sets::ItemSet {
                uid: "live-builds-234-JUNGLE".to_owned(),
                title: "live-builds — Viego JUNGLE".to_owned(),
                champion_ids: vec![234],
                blocks: Vec::new(),
            }),
            source: Source::Riot,
            pages: Vec::new(),
        };

        let applied = apply_recommendation(&http, &key, Action::ItemSet, &recommendation).unwrap();

        assert_eq!(applied, Some(true));
        assert!(
            http.writes.borrow()[0]["itemSets"]
                .as_array()
                .unwrap()
                .contains(&other_app)
        );
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
            pages: Vec::new(),
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
            pages: Vec::new(),
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
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: Some(wanted.clone()),
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
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
                pages: Vec::new(),
            },
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: Some(previous.clone()),
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
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

    struct ChangingHttp {
        reads: Cell<u8>,
    }

    impl LcuHttp for ChangingHttp {
        fn get(&self, _path: &str) -> Result<Value, LcuError> {
            let read = self.reads.get() + 1;
            self.reads.set(read);
            Ok(json!({
                "gameId": 10,
                "localPlayerCellId": 0,
                "queueId": 420,
                "myTeam": [{
                    "cellId": 0,
                    "championId": if read == 1 { 234 } else { 145 },
                    "assignedPosition": "jungle",
                    "spell1Id": 4,
                    "spell2Id": 14
                }],
                "actions": []
            }))
        }

        fn send(&self, _method: &str, _path: &str, _body: &Value) -> Result<Value, LcuError> {
            Ok(Value::Null)
        }
    }

    #[test]
    fn selection_change_after_write_is_reported() {
        let http = ChangingHttp {
            reads: Cell::new(0),
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
            pages: Vec::new(),
        };

        let (applied, changed) =
            apply_and_verify(&http, &key, Action::Spells, &recommendation).unwrap();

        assert_eq!(applied, Some(true));
        assert!(changed);
    }

    #[test]
    fn reload_with_same_key_keeps_applied_state() {
        let page = RunePageDraft {
            name: "live-builds".to_owned(),
            primary_style: 8000,
            sub_style: 8100,
            perks: [8005, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
        };
        let make = || SelectionUi {
            key: SelectionKey {
                session_id: "10".to_owned(),
                champion_id: 234,
                position: "JUNGLE".to_owned(),
            },
            champion: "Viego".to_owned(),
            recommendation: Recommendation {
                page: None,
                spells: (4, 11),
                item_set: None,
                source: Source::Riot,
                pages: Vec::new(),
            },
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: None,
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
        };
        let mut old = make();
        old.applied.runes = true;
        old.applied_page = Some(page.clone());
        old.user_edited = true;
        let mut fresh = make();

        fresh.inherit(old);

        assert!(fresh.applied.runes && fresh.user_edited);
        assert_eq!(fresh.applied_page, Some(page));
    }

    #[test]
    fn edited_page_survives_reload() {
        let page = RunePageDraft {
            name: "live-builds".to_owned(),
            primary_style: 8000,
            sub_style: 8100,
            perks: [8008, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
        };
        let make = |page: RunePageDraft| SelectionUi {
            key: SelectionKey {
                session_id: "10".to_owned(),
                champion_id: 234,
                position: "JUNGLE".to_owned(),
            },
            champion: "Viego".to_owned(),
            recommendation: Recommendation {
                page: Some(page),
                spells: (4, 11),
                item_set: None,
                source: Source::Riot,
                pages: Vec::new(),
            },
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: None,
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
        };
        let mut old = make(page.clone());
        old.user_edited = true;
        old.edited_page = Some(page.clone());
        let mut recommended = page.clone();
        recommended.perks[0] = 8005;
        let mut fresh = make(recommended);

        fresh.inherit(old);

        assert_eq!(fresh.recommendation.page, Some(page));
    }

    struct RecordingHttp {
        writes: RefCell<Vec<(String, String, Value)>>,
    }

    impl LcuHttp for RecordingHttp {
        fn get(&self, path: &str) -> Result<Value, LcuError> {
            Ok(match path {
                "/lol-champ-select/v1/session" => json!({
                    "gameId": 10, "localPlayerCellId": 0, "queueId": 420,
                    "myTeam": [{"cellId": 0, "championId": 234,
                                "assignedPosition": "jungle", "spell1Id": 4, "spell2Id": 14}],
                    "actions": []
                }),
                "/lol-perks/v1/pages" => json!([{
                    "id": 7, "name": "live-builds", "isEditable": true, "isTemporary": false,
                    "current": true, "primaryStyleId": 8000, "subStyleId": 8100,
                    "selectedPerkIds": [8005, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011]
                }]),
                other => panic!("GET inesperado: {other}"),
            })
        }

        fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
            self.writes
                .borrow_mut()
                .push((method.to_owned(), path.to_owned(), body.clone()));
            Ok(Value::Null)
        }
    }

    #[test]
    fn choosing_option_writes_app_page_and_stops_auto() {
        let option = |keystone: u32| crate::lcu::champ_select::RuneOption {
            draft: RunePageDraft {
                name: "live-builds".to_owned(),
                primary_style: 8000,
                sub_style: 8100,
                perks: [keystone, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
            },
            keystone_name: keystone.to_string(),
            games: 10,
            win_rate: 0.5,
            low_sample: false,
            source: Source::HighElo,
            item_set: None,
        };
        let recommendation = Recommendation {
            page: Some(option(8005).draft),
            spells: (4, 11),
            item_set: None,
            source: Source::HighElo,
            pages: vec![option(8005), option(8010)],
        };
        let key = SelectionKey {
            session_id: "10".to_owned(),
            champion_id: 234,
            position: "JUNGLE".to_owned(),
        };
        let mut selection = SelectionUi {
            key: key.clone(),
            champion: "Viego".to_owned(),
            recommendation,
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            edited_page: None,
            user_edited: false,
            chosen: None,
            observed_page: None,
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
        };

        let names = KnowledgeNames::default();
        assert!(selection.choose_option(1, &names));
        assert!(!selection.choose_option(5, &names));
        assert_eq!(selection.chosen, Some(1));
        let http = RecordingHttp {
            writes: RefCell::new(Vec::new()),
        };
        let applied =
            apply_recommendation(&http, &key, Action::Runes, &selection.recommendation).unwrap();

        assert_eq!(applied, Some(true));
        let writes = http.writes.borrow();
        assert!(writes.iter().all(|(method, _, _)| method != "DELETE"));
        let (_, path, body) = writes
            .iter()
            .find(|(method, _, _)| method == "PUT")
            .unwrap();
        assert_eq!(path, "/lol-perks/v1/pages/7");
        assert_eq!(body["selectedPerkIds"][0], 8010);
        assert_eq!(body["name"], "live-builds");
        assert!(!selection.user_edited);
        let state = SelectState {
            phase: crate::lcu::champ_select::Phase::ChampSelect,
            session_id: "10".to_owned(),
            champion_id: Some(234),
            locked: true,
            position: "JUNGLE".to_owned(),
            spells: (4, 11),
            map_mode: (11, "CLASSIC".to_owned()),
            auto_apply: true,
            active_page: None,
        };
        let auto = Settings {
            auto_runes: true,
            ..Settings::default()
        };
        let mut applier = Applier::default();
        applier.on_state(&state, &auto, false);
        applier.mark_choice();
        applier.reset(Action::Runes);
        assert!(
            !applier
                .on_state(&state, &auto, selection.user_edited)
                .contains(&Action::Runes)
        );
    }

    #[test]
    fn reload_reapplies_only_explicit_choice() {
        let option = |keystone: u32| crate::lcu::champ_select::RuneOption {
            draft: RunePageDraft {
                name: "live-builds".to_owned(),
                primary_style: 8000,
                sub_style: 8100,
                perks: [keystone, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
            },
            keystone_name: keystone.to_string(),
            games: 10,
            win_rate: 0.5,
            low_sample: false,
            source: Source::HighElo,
            item_set: None,
        };
        let make = |page: u32| SelectionUi {
            key: SelectionKey {
                session_id: "10".to_owned(),
                champion_id: 234,
                position: "JUNGLE".to_owned(),
            },
            champion: "Viego".to_owned(),
            recommendation: Recommendation {
                page: Some(option(page).draft),
                spells: (4, 11),
                item_set: None,
                source: Source::HighElo,
                pages: vec![option(8005), option(8010)],
            },
            source_patch: None,
            source_games: None,
            differences: Vec::new(),
            applied: AppliedItems::default(),
            limit_reached: false,
            applied_page: None,
            edited_page: None,
            user_edited: true,
            chosen: None,
            observed_page: None,
            observed_spells: (4, 11),
            stale_page: None,
            rules: None,
            global_item_set_files: Vec::new(),
        };
        let names = KnowledgeNames::default();

        let mut unchosen_old = make(8010);
        let mut fresh = make(8005);
        fresh.inherit(unchosen_old);
        assert_eq!(fresh.chosen, None);
        assert_eq!(fresh.recommendation.page, Some(option(8005).draft));

        unchosen_old = make(8005);
        assert!(unchosen_old.choose_option(1, &names));
        let mut fresh = make(8005);
        fresh.inherit(unchosen_old);
        assert_eq!(fresh.chosen, Some(1));
        assert_eq!(fresh.recommendation.page, Some(option(8010).draft));
    }

    #[test]
    fn dead_worker_stops_selection_loading() {
        let state = SelectState {
            phase: crate::lcu::champ_select::Phase::ChampSelect,
            session_id: "10".to_owned(),
            champion_id: Some(234),
            locked: true,
            position: "JUNGLE".to_owned(),
            spells: (4, 11),
            map_mode: (11, "CLASSIC".to_owned()),
            auto_apply: true,
            active_page: None,
        };
        let now = Instant::now();
        let mut tracker = SelectionLoadTracker::default();
        assert!(tracker.should_start(&state, now));

        tracker.give_up();

        assert!(!tracker.is_loading(&state));
        assert!(!tracker.should_start(&state, now + Duration::from_secs(60)));
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
    fn panel(&mut self, ctx: &egui::Context, view: &PanelView) -> Option<String> {
        if let Some(catalog) = self.catalog.as_ref() {
            self.icons.prepare_items(&view.build, &catalog.patch);
        }
        egui::TopBottomPanel::top("high_elo").show(ctx, |ui| {
            high_elo(ui, &view.high_elo);
        });
        egui::TopBottomPanel::bottom("lane").show(ctx, |ui| {
            lane(ui, view.lane.as_ref());
        });
        egui::CentralPanel::default()
            .show(ctx, |ui| {
                let mut picked = None;
                if let Some(note) = &view.catalog_note {
                    ui.label(RichText::new(note).color(WARN));
                }
                build(
                    ui,
                    &view.build,
                    view.me.gold,
                    self.catalog
                        .as_ref()
                        .map(|catalog| catalog.patch.as_str())
                        .unwrap_or_default(),
                    &self.icons.textures,
                );
                ui.separator();
                skill_grid(ui, &view.skills);
                ui.separator();
                ui.columns(2, |columns| {
                    me(&mut columns[0], &view.me, &view.skills);
                    picked = teams(&mut columns[1], &view.teams, view.lane.as_ref());
                });
                picked
            })
            .inner
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
    ui.label(RichText::new(format!("{} · level {}", me.champion, me.level)).strong());
    ui.label(format!(
        "Gold {} · CS {} · KDA {}/{}/{}",
        me.gold, me.cs, me.kda.0, me.kda.1, me.kda.2
    ));
    if skills.points_available > 0 {
        let next = skills
            .next_skill
            .map_or(String::new(), |skill| format!(" -> {skill}"));
        ui.label(
            RichText::new(format!("{} skill point(s){next}", skills.points_available)).color(WARN),
        );
    }
}

fn build(
    ui: &mut egui::Ui,
    build: &BuildView,
    gold: u32,
    patch: &str,
    icons: &HashMap<String, egui::TextureHandle>,
) {
    ui.label(RichText::new(format!("Build · gold {gold}")).strong());
    ui.horizontal_wrapped(|ui| {
        for item in &build.items {
            let stroke = if item.next {
                egui::Stroke::new(2.0_f32, WARN)
            } else {
                egui::Stroke::NONE
            };
            egui::Frame::new().stroke(stroke).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    if let Some(icon) = icons.get(&item_icon_key(patch, item.id)) {
                        let tint = if item.owned {
                            Color32::from_gray(110)
                        } else {
                            Color32::WHITE
                        };
                        ui.add(
                            egui::Image::new(icon)
                                .fit_to_exact_size(egui::vec2(48.0, 48.0))
                                .tint(tint),
                        );
                    } else {
                        ui.allocate_space(egui::vec2(48.0, 48.0));
                    }
                    let suffix = if item.owned { " ✓" } else { "" };
                    let color = if item.owned { MUTED } else { Color32::WHITE };
                    ui.label(
                        RichText::new(format!("{}{}", short_item_name(&item.name), suffix))
                            .small()
                            .color(color),
                    );
                    if item.next {
                        if let Some(component) = &item.next_component {
                            ui.label(
                                RichText::new(short_item_name(component))
                                    .small()
                                    .color(WARN),
                            );
                        }
                        ui.label(
                            RichText::new(format!("{} gold", item.missing_gold))
                                .small()
                                .color(WARN),
                        );
                    }
                });
            });
        }
    });
    if let Some(line) = &build.free_boots_line {
        ui.label(RichText::new(line).small().color(MUTED));
    }
    if !build.affordable_now.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Dá para comprar agora:").small().color(GOOD));
            for (name, id) in build.affordable_now.iter().zip(&build.affordable_now_ids) {
                if let Some(icon) = icons.get(&item_icon_key(patch, *id)) {
                    ui.add(
                        egui::Image::new(icon)
                            .fit_to_exact_size(egui::vec2(24.0, 24.0))
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text(name);
                }
            }
        });
    }
    if let Some(message) = &build.empty_message {
        ui.label(message);
    }
}

fn short_item_name(name: &str) -> String {
    const LIMIT: usize = 14;
    let mut result = name.chars().take(LIMIT).collect::<String>();
    if name.chars().nth(LIMIT).is_some() {
        result.push('…');
    }
    result
}

fn skill_grid(ui: &mut egui::Ui, skills: &SkillView) {
    ui.label(RichText::new("Skills").strong());
    egui::Grid::new("skill_grid")
        .spacing(egui::vec2(2.0, 2.0))
        .show(ui, |ui| {
            ui.label("");
            for level in 1..=18 {
                ui.label(RichText::new(level.to_string()).small());
            }
            ui.end_row();
            for (skill, letter) in ['Q', 'W', 'E', 'R'].into_iter().enumerate() {
                ui.label(letter.to_string());
                for level in 0..18 {
                    let current = skills.current_level == (level + 1) as u32;
                    let next = skills.next_point == Some((skill, level));
                    let fill = if next {
                        WARN
                    } else if skills.grid[skill][level] {
                        GOOD
                    } else {
                        Color32::from_gray(45)
                    };
                    let stroke = if current {
                        egui::Stroke::new(1.5_f32, Color32::WHITE)
                    } else {
                        egui::Stroke::NONE
                    };
                    egui::Frame::new().fill(fill).stroke(stroke).show(ui, |ui| {
                        ui.allocate_space(egui::vec2(14.0, 14.0));
                    });
                }
                ui.end_row();
            }
        });
    if let Some(priority) = &skills.priority_label {
        ui.label(
            RichText::new(format!("Priority: {priority}"))
                .small()
                .color(MUTED),
        );
    }
}

#[allow(dead_code)]
fn old_me(ui: &mut egui::Ui, me: &MeView, skills: &SkillView) {
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

#[allow(dead_code)]
fn old_build(ui: &mut egui::Ui, build: &BuildView, gold: u32) {
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
