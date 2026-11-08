use std::sync::Mutex;
use std::time::Duration;

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
        champion_id: Some(champion_id),
        locked: true,
        position: "JUNGLE".to_owned(),
        spells: (14, 11),
        map_mode: (11, "CLASSIC".to_owned()),
        auto_apply: true,
    }
}

fn all_auto() -> Settings {
    Settings {
        auto_runes: true,
        auto_spells: true,
        auto_item_set: true,
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
fn failed_apply_retries_then_succeeds() {
    let mut applier = Applier::default();
    let settings = Settings {
        auto_runes: true,
        ..Settings::default()
    };
    let state = selecting(234);

    for succeeded in [false, false, true] {
        assert_eq!(
            applier.on_state(&state, &settings, false),
            vec![Action::Runes]
        );
        applier.confirm(Action::Runes, succeeded);
    }
    assert!(applier.on_state(&state, &settings, false).is_empty());
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
        champion_id: None,
        locked: false,
        position: String::new(),
        spells: (0, 0),
        map_mode: (0, String::new()),
        auto_apply: false,
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
            }],
        }),
        source: Source::HighElo,
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
    assert_eq!(Settings::load(&path), Settings::default());

    let wanted = Settings {
        auto_runes: true,
        auto_spells: false,
        auto_item_set: true,
        corrupted: false,
    };
    wanted.save(&path).unwrap();

    assert_eq!(Settings::load(&path), wanted);
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
    ]);
    let select_poll = poll_client(&selecting).unwrap();
    assert!(matches!(select_poll.state, LcuPollState::Selecting(_)));
    assert_eq!(select_poll.next, Duration::from_secs(1));
    assert_eq!(selecting.calls.lock().unwrap().len(), 2);
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
    )
    .unwrap();

    assert!(rec.page.is_none());
}
