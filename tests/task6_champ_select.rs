use std::sync::Mutex;
use std::time::{Duration, Instant};

use live_builds::catalog::ItemCatalog;
use live_builds::knowledge::{KnowledgeNames, KnowledgePackage};
use live_builds::lcu::champ_select::{
    Action, Applier, Phase, Recommendation, SelectState, Source, parse_session, recommend,
    reminder, set_spells,
};
use live_builds::lcu::item_sets::{ItemBlock, ItemSet};
use live_builds::lcu::poller::{LcuPollState, poll_client};
use live_builds::lcu::runes::{RunePageDraft, RuneRules};
use live_builds::lcu::{LcuError, LcuHttp};
use live_builds::settings::Settings;
use live_builds::static_data::StaticSource;
use serde_json::{Value, json};

const LOCKED: &str = include_str!("fixtures/lcu/champ-select-session-locked.json");

fn selecting(champion_id: u32) -> SelectState {
    SelectState {
        phase: Phase::ChampSelect,
        session_id: "session-1".to_owned(),
        champion_id: Some(champion_id),
        locked: true,
        position: "JUNGLE".to_owned(),
        spells: (14, 11),
        map_mode: (11, "CLASSIC".to_owned()),
        auto_apply: true,
        active_page: None,
    }
}

fn all_auto() -> Settings {
    Settings {
        auto_runes: true,
        auto_spells: true,
        auto_item_set: true,
        exclusive_item_set: true,
        corrupted: false,
    }
}

#[test]
fn parses_locked_champion_and_position() {
    let state = parse_session(&serde_json::from_str(LOCKED).unwrap(), 0);

    assert_eq!(state.champion_id, Some(234));
    assert!(state.locked);
    assert_eq!(state.position, "JUNGLE");
    assert_eq!(state.spells, (14, 11));
    assert_eq!(state.map_mode, (11, "CLASSIC".to_owned()));
}

#[test]
fn auto_off_applies_nothing() {
    let mut applier = Applier::default();

    assert!(
        applier
            .on_state(&selecting(234), &Settings::default(), false)
            .is_empty()
    );
}

#[test]
fn auto_on_applies_once_per_lock() {
    let mut applier = Applier::default();
    let state = selecting(234);

    assert_eq!(
        applier.on_state(&state, &all_auto(), false),
        vec![Action::Runes, Action::Spells, Action::ItemSet]
    );
    applier.confirm(Action::Runes, true);
    applier.confirm(Action::Spells, true);
    applier.confirm(Action::ItemSet, true);
    assert!(applier.on_state(&state, &all_auto(), false).is_empty());
}

#[test]
fn transient_apply_failure_retries_after_five_seconds_then_succeeds() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_runes: true,
        ..Settings::default()
    };
    let state = selecting(234);
    let now = Instant::now();

    assert_eq!(
        applier.on_state_at(&state, &settings, false, now),
        vec![Action::Runes]
    );
    applier.confirm_at(Action::Runes, false, now);
    assert!(
        applier
            .on_state_at(&state, &settings, false, now + Duration::from_secs(4))
            .is_empty()
    );
    assert_eq!(
        applier.on_state_at(&state, &settings, false, now + Duration::from_secs(5)),
        vec![Action::Runes]
    );
    applier.confirm_at(Action::Runes, true, now + Duration::from_secs(5));
    assert!(
        applier
            .on_state_at(&state, &settings, false, now + Duration::from_secs(60))
            .is_empty()
    );
}

#[test]
fn champion_swap_reapplies_and_dodge_resets() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_runes: true,
        ..Settings::default()
    };

    assert_eq!(
        applier.on_state(&selecting(234), &settings, false),
        vec![Action::Runes]
    );
    assert_eq!(
        applier.on_state(&selecting(145), &settings, false),
        vec![Action::Runes]
    );
    let outside = SelectState {
        phase: Phase::Other("Lobby".to_owned()),
        session_id: String::new(),
        champion_id: None,
        locked: false,
        position: String::new(),
        spells: (0, 0),
        map_mode: (0, String::new()),
        auto_apply: false,
        active_page: None,
    };
    assert!(applier.on_state(&outside, &settings, false).is_empty());
    assert_eq!(
        applier.on_state(&selecting(234), &settings, false),
        vec![Action::Runes]
    );
}

#[test]
fn auto_apply_does_not_override_user_edit() {
    let mut applier = Applier::default();
    let settings = all_auto();

    assert_eq!(
        applier.on_state(&selecting(234), &settings, true),
        vec![Action::Spells, Action::ItemSet]
    );
    assert_eq!(
        applier.on_state(&selecting(145), &settings, false),
        vec![Action::Spells, Action::ItemSet]
    );
    let mut next_session = selecting(145);
    next_session.session_id = "session-2".to_owned();
    assert_eq!(
        applier.on_state(&next_session, &settings, false),
        vec![Action::Runes, Action::Spells, Action::ItemSet]
    );
}

#[test]
fn unknown_queue_never_auto_applies() {
    let mut session: Value = serde_json::from_str(LOCKED).unwrap();
    session["queueId"] = json!(1700);
    let state = parse_session(&session, 0);

    assert!(!state.auto_apply);
    assert!(
        Applier::default()
            .on_state(&state, &all_auto(), false)
            .is_empty()
    );
}

fn page(perks: [u32; 9]) -> RunePageDraft {
    RunePageDraft {
        name: "live-builds".to_owned(),
        primary_style: 8000,
        sub_style: 8100,
        perks,
    }
}

fn recommendation(page: Option<RunePageDraft>, spells: (u32, u32)) -> Recommendation {
    Recommendation {
        page,
        spells,
        item_set: Some(ItemSet {
            uid: "live-builds-234-JUNGLE".to_owned(),
            title: "live-builds — Viego JUNGLE".to_owned(),
            champion_ids: vec![234],
            blocks: vec![ItemBlock {
                name: "Itens".to_owned(),
                items: vec![3078],
                original: None,
            }],
        }),
        source: Source::HighElo,
        pages: Vec::new(),
    }
}

fn names() -> KnowledgeNames {
    KnowledgeNames {
        runes: [(8005, "Pressione o Ataque"), (8008, "Ritmo Fatal")]
            .into_iter()
            .map(|(id, name)| (id, name.to_owned()))
            .collect(),
        styles: [(8000, "Precisão"), (8100, "Dominação")]
            .into_iter()
            .map(|(id, name)| (id, name.to_owned()))
            .collect(),
        summoner_spells: [
            (4, "Flash"),
            (11, "Golpear"),
            (14, "Incendiar"),
            (3, "Exaustão"),
        ]
        .into_iter()
        .map(|(id, name)| (id, name.to_owned()))
        .collect(),
        ..KnowledgeNames::default()
    }
}

#[test]
fn reminder_lists_spell_and_rune_differences() {
    let active = page([8005, 9111, 9104, 8014, 8139, 8106, 5008, 5008, 5011]);
    let wanted = page([8008, 9111, 9104, 8014, 8139, 8106, 5008, 5008, 5011]);

    let messages = reminder(
        &active,
        (4, 14),
        &recommendation(Some(wanted), (4, 3)),
        &names(),
    );

    assert!(messages.contains(&"Incendiar → Exaustão".to_owned()));
    assert!(messages.contains(&"Pressione o Ataque → Ritmo Fatal".to_owned()));
}

#[test]
fn reminder_flags_missing_smite_in_jungle() {
    let active = page([8005, 9111, 9104, 8014, 8139, 8106, 5008, 5008, 5011]);

    assert!(
        reminder(&active, (4, 14), &recommendation(None, (4, 11)), &names(),)
            .contains(&"falta Golpear".to_owned())
    );
}

#[test]
fn settings_default_all_off_and_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/settings.json");
    let defaults = Settings::load(&path);
    assert!(!defaults.auto_runes);
    assert!(!defaults.auto_spells);
    assert!(!defaults.auto_item_set);
    assert!(defaults.exclusive_item_set);

    let wanted = Settings {
        auto_runes: true,
        auto_spells: false,
        auto_item_set: true,
        exclusive_item_set: true,
        corrupted: false,
    };
    wanted.save(&path).unwrap();

    assert_eq!(Settings::load(&path), wanted);
}

#[test]
fn settings_without_exclusive_item_set_default_to_enabled() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"auto_runes":true,"auto_spells":true,"auto_item_set":true}"#,
    )
    .unwrap();

    let loaded = Settings::load(&path);

    assert!(loaded.auto_runes);
    assert!(loaded.auto_spells);
    assert!(loaded.auto_item_set);
    assert!(loaded.exclusive_item_set);
}

#[test]
fn truncated_settings_are_reported() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path, r#"{"auto_runes": tru"#).unwrap();

    let loaded = Settings::load(&path);

    assert_eq!(
        loaded,
        Settings {
            corrupted: true,
            ..Settings::default()
        }
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        r#"{"auto_runes": tru"#
    );
}

#[derive(Default)]
struct FakeHttp {
    responses: Mutex<Vec<Value>>,
    calls: Mutex<Vec<(String, String, Value)>>,
}

impl FakeHttp {
    fn responding(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            calls: Mutex::default(),
        }
    }
}

impl LcuHttp for FakeHttp {
    fn get(&self, path: &str) -> Result<Value, LcuError> {
        self.calls
            .lock()
            .unwrap()
            .push(("GET".to_owned(), path.to_owned(), Value::Null));
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| LcuError::Other("sem resposta".to_owned()))
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), path.to_owned(), body.clone()));
        Ok(Value::Null)
    }
}

#[test]
fn set_spells_patches_my_selection() {
    let http = FakeHttp::default();

    set_spells(&http, 4, 11).unwrap();

    assert_eq!(
        *http.calls.lock().unwrap(),
        vec![(
            "PATCH".to_owned(),
            "/lol-champ-select/v1/session/my-selection".to_owned(),
            json!({"spell1Id": 4, "spell2Id": 11}),
        )]
    );
}

#[test]
fn poller_only_queries_session_during_champ_select() {
    let lobby = FakeHttp::responding(vec![json!("Lobby")]);
    let lobby_poll = poll_client(&lobby).unwrap();
    assert_eq!(
        lobby_poll.state,
        LcuPollState::Open(Phase::Other("Lobby".to_owned()))
    );
    assert_eq!(lobby_poll.next, Duration::from_secs(5));
    assert_eq!(lobby.calls.lock().unwrap().len(), 1);

    let selecting = FakeHttp::responding(vec![
        json!("ChampSelect"),
        serde_json::from_str(LOCKED).unwrap(),
        serde_json::from_str(include_str!("fixtures/lcu/perks-currentpage.json")).unwrap(),
    ]);
    let select_poll = poll_client(&selecting).unwrap();
    assert!(matches!(select_poll.state, LcuPollState::Selecting(_)));
    assert_eq!(select_poll.next, Duration::from_secs(1));
    assert_eq!(selecting.calls.lock().unwrap().len(), 3);
    assert_eq!(
        selecting.calls.lock().unwrap()[2].1,
        "/lol-perks/v1/currentpage"
    );
}

#[test]
fn no_riot_runes_still_returns_spells_and_item_set() {
    // Patch 16.22 commonly has no ChampionRuneRecommendationsContext.
    let cache = tempfile::tempdir().unwrap();
    let statics = StaticSource::with_fetcher(cache.path().to_path_buf(), |url| {
        if url.contains("/thresh/thresh.bin.json") {
            Ok(include_str!("fixtures/thresh.bin.json").to_owned())
        } else {
            Err("offline".to_owned())
        }
    });
    let catalog =
        ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_sample.json"))
            .unwrap();
    let rules = RuneRules::from_json(include_str!("fixtures/lcu/perks-styles.json")).unwrap();

    let rec = recommend(
        None,
        None,
        &statics,
        "16.22.1",
        412,
        "Thresh",
        "UTILITY",
        (11, "CLASSIC"),
        None,
        Some(include_str!("fixtures/thresh.bin.json")),
        &rules,
        &catalog,
        &names(),
    )
    .unwrap();

    assert!(rec.page.is_none());
    assert_eq!(rec.spells, (4, 14));
    assert!(!rec.item_set.unwrap().blocks.is_empty());
    assert_eq!(rec.source, Source::Riot);
}

#[test]
fn aram_uses_riot_recommendation() {
    let package: KnowledgePackage =
        serde_json::from_str(include_str!("fixtures/knowledge-16.22.json")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let statics =
        StaticSource::with_fetcher(cache.path().to_path_buf(), |_| Err("offline".to_owned()));
    let catalog =
        ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_sample.json"))
            .unwrap();
    let rules = RuneRules::from_json(include_str!("fixtures/lcu/perks-styles.json")).unwrap();

    let rec = recommend(
        Some(&package),
        None,
        &statics,
        "16.22.1",
        145,
        "KaiSa",
        "BOTTOM",
        (12, "ARAM"),
        None,
        None,
        &rules,
        &catalog,
        &names(),
    )
    .unwrap();

    assert_eq!(rec.source, Source::Riot);
    assert!(rec.item_set.is_none());
}

#[test]
fn invalid_package_page_falls_back() {
    let mut package: KnowledgePackage =
        serde_json::from_str(include_str!("fixtures/knowledge-16.22.json")).unwrap();
    let entry = package
        .entries
        .iter_mut()
        .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
        .unwrap();
    entry.rune_pages[0].perks[0] = 999_999;
    let cache = tempfile::tempdir().unwrap();
    let statics = StaticSource::with_fetcher(cache.path().to_path_buf(), |_| Err("offline".into()));
    let catalog =
        ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_sample.json"))
            .unwrap();
    let rules = RuneRules::from_json(include_str!("fixtures/lcu/perks-styles.json")).unwrap();

    let rec = recommend(
        Some(&package),
        None,
        &statics,
        "16.22.1",
        145,
        "KaiSa",
        "BOTTOM",
        (11, "CLASSIC"),
        None,
        None,
        &rules,
        &catalog,
        &names(),
    )
    .unwrap();

    assert!(rec.page.is_none());
}

fn pick_session(pick: Value, intent: u32) -> Value {
    json!({
        "gameId": 10,
        "localPlayerCellId": 0,
        "queueId": 420,
        "myTeam": [{
            "cellId": 0,
            "championId": 0,
            "championPickIntent": intent,
            "assignedPosition": "jungle",
            "spell1Id": 4,
            "spell2Id": 14
        }],
        "actions": [[pick]]
    })
}

fn hovering(champion_id: u32) -> SelectState {
    SelectState {
        locked: false,
        ..selecting(champion_id)
    }
}

#[test]
fn hover_loads_recommendation_before_lock() {
    let session = pick_session(
        json!({"actorCellId": 0, "type": "pick", "championId": 234,
               "isInProgress": true, "completed": false}),
        0,
    );

    let state = parse_session(&session, 0);

    assert_eq!(state.champion_id, Some(234));
    assert!(!state.locked);
    assert_eq!(state.selection_key().unwrap().champion_id, 234);
    let mut tracker = live_builds::ui::SelectionLoadTracker::default();
    assert!(tracker.should_start(&state, Instant::now()));
}

#[test]
fn intent_used_when_no_pick_in_progress() {
    let idle = json!({"actorCellId": 0, "type": "pick", "championId": 0,
                      "isInProgress": false, "completed": false});
    assert_eq!(
        parse_session(&pick_session(idle.clone(), 145), 0).champion_id,
        Some(145)
    );
    assert_eq!(parse_session(&pick_session(idle, 0), 0).champion_id, None);

    let hovered = json!({"actorCellId": 0, "type": "pick", "championId": 234,
                         "isInProgress": true, "completed": false});
    assert_eq!(
        parse_session(&pick_session(hovered, 145), 0).champion_id,
        Some(234)
    );
    let locked = json!({"actorCellId": 0, "type": "pick", "championId": 99,
                        "isInProgress": false, "completed": true});
    let state = parse_session(&pick_session(locked, 145), 0);
    assert_eq!((state.champion_id, state.locked), (Some(99), true));
}

#[test]
fn hover_change_reapplies_after_debounce() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_runes: true,
        auto_spells: true,
        ..Settings::default()
    };
    let now = Instant::now();
    let second = Duration::from_secs(1);

    assert!(
        applier
            .on_state_at(&hovering(234), &settings, false, now)
            .is_empty()
    );
    assert!(
        applier
            .on_state_at(&hovering(234), &settings, false, now + second / 2)
            .is_empty()
    );
    assert_eq!(
        applier.on_state_at(&hovering(234), &settings, false, now + second),
        vec![Action::Runes, Action::Spells]
    );
    applier.confirm_at(Action::Runes, true, now + second);
    applier.confirm_at(Action::Spells, true, now + second);

    let later = now + second * 2;
    assert!(
        applier
            .on_state_at(&hovering(145), &settings, false, later)
            .is_empty()
    );
    assert_eq!(
        applier.on_state_at(&hovering(145), &settings, false, later + second),
        vec![Action::Runes, Action::Spells]
    );
}

#[test]
fn item_set_waits_for_lock() {
    let mut applier = Applier::default();
    let settings = all_auto();
    let now = Instant::now();
    let second = Duration::from_secs(1);

    assert!(
        applier
            .on_state_at(&hovering(234), &settings, false, now)
            .is_empty()
    );
    assert_eq!(
        applier.on_state_at(&hovering(234), &settings, false, now + second),
        vec![Action::Runes, Action::Spells]
    );
    applier.confirm_at(Action::Runes, true, now + second);
    applier.confirm_at(Action::Spells, true, now + second);

    assert_eq!(
        applier.on_state_at(&selecting(234), &settings, false, now + second * 2),
        vec![Action::ItemSet]
    );
}

fn five_page_package() -> KnowledgePackage {
    use live_builds::knowledge::SequenceStats;
    let mut package: KnowledgePackage =
        serde_json::from_str(include_str!("fixtures/knowledge-16.22.json")).unwrap();
    let entry = package
        .entries
        .iter_mut()
        .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
        .unwrap();
    let base = entry.rune_pages[0].clone();
    let variant = |keystone: u32| {
        let mut page = base.clone();
        page.keystone = keystone;
        page.perks[0] = keystone;
        page.games = 7;
        page.win_rate = 0.4;
        page.low_sample = true;
        page.build_path = Some(SequenceStats {
            sequence: vec![3302, 3124, 6672],
        });
        page
    };
    let mut invalid = base.clone();
    invalid.perks[0] = 999_999;
    entry.rune_pages = vec![
        base.clone(),
        invalid,
        variant(8005),
        variant(8021),
        variant(8010),
    ];
    package
}

fn recommend_kaisa(package: &KnowledgePackage, player_keystone: Option<u32>) -> Recommendation {
    let cache = tempfile::tempdir().unwrap();
    let statics = StaticSource::with_fetcher(cache.path().to_path_buf(), |_| Err("offline".into()));
    let catalog =
        ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_sample.json"))
            .unwrap();
    let rules = RuneRules::from_json(include_str!("fixtures/lcu/perks-styles.json")).unwrap();
    recommend(
        Some(package),
        None,
        &statics,
        "16.22.1",
        145,
        "KaiSa",
        "BOTTOM",
        (11, "CLASSIC"),
        player_keystone,
        None,
        &rules,
        &catalog,
        &names(),
    )
    .unwrap()
}

#[test]
fn recommendation_lists_all_valid_pages() {
    let package = five_page_package();

    let rec = recommend_kaisa(&package, None);

    let keystones: Vec<_> = rec.pages.iter().map(|o| o.keystone_name.as_str()).collect();
    assert_eq!(
        keystones,
        ["Ritmo Fatal", "Pressione o Ataque", "8021", "8010"]
    );
    assert_eq!((rec.pages[0].games, rec.pages[0].win_rate), (12, 0.5));
    assert!(!rec.pages[0].low_sample);
    assert!(rec.pages[1].low_sample);
    assert_eq!(rec.pages[0].source, Source::HighElo);
    assert_eq!(rec.page.as_ref(), Some(&rec.pages[0].draft));

    let rec = recommend_kaisa(&package, Some(8021));
    assert_eq!(rec.pages[0].draft.perks[0], 8021);
    assert_eq!(rec.pages.len(), 4);
}

#[test]
fn chosen_option_drives_item_set() {
    let rec = recommend_kaisa(&five_page_package(), None);

    let chosen = rec.choose(1).unwrap();

    assert_eq!(chosen.page.as_ref(), Some(&rec.pages[1].draft));
    assert_eq!(chosen.item_set, rec.pages[1].item_set);
    assert_ne!(chosen.item_set, rec.item_set);
    assert!(
        chosen
            .item_set
            .unwrap()
            .blocks
            .iter()
            .any(|b| b.items.first() == Some(&3302))
    );
    assert!(rec.choose(3).is_some());
    assert!(rec.choose(4).is_none());
}

#[test]
fn manual_choice_does_not_disable_auto_for_next_champion() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_runes: true,
        ..Settings::default()
    };

    assert_eq!(
        applier.on_state(&selecting(234), &settings, false),
        vec![Action::Runes]
    );
    applier.confirm(Action::Runes, true);
    applier.mark_choice();
    applier.reset(Action::Runes);
    assert!(
        applier
            .on_state(&selecting(234), &settings, false)
            .is_empty()
    );

    assert_eq!(
        applier.on_state(&selecting(145), &settings, false),
        vec![Action::Runes]
    );
}

#[test]
fn reset_keeps_in_flight_action() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_item_set: true,
        ..Settings::default()
    };
    assert_eq!(
        applier.on_state(&selecting(234), &settings, false),
        vec![Action::ItemSet]
    );

    applier.reset(Action::ItemSet);

    assert!(
        applier
            .on_state(&selecting(234), &settings, false)
            .is_empty()
    );
}

#[test]
fn recommendation_without_item_set_cannot_apply_it() {
    let mut rec = recommendation(None, (4, 11));
    assert!(rec.can_apply(Action::ItemSet));
    rec.item_set = None;
    assert!(!rec.can_apply(Action::ItemSet));
    assert!(rec.can_apply(Action::Spells));
}
