use live_builds::knowledge::KnowledgeNames;
use live_builds::lcu::champ_select::{Recommendation, Source};
use live_builds::lcu::item_sets::{ItemBlock, ItemSet};
use live_builds::lcu::perks::ClientPage;
use live_builds::lcu::poller::LcuPollState;
use live_builds::lcu::runes::{RunePageDraft, RuneRules};
use live_builds::settings::Settings;
use live_builds::ui::{
    Screen, SelectionLoadTracker, choose_screen, compare_patch_versions, save_settings,
};
use live_builds::ui_editors::{DeleteConfirmation, EditorsState, new_item_set, rune_options};
use live_builds::ui_select::{AppliedItems, SelectScreenInput, select_screen};
use std::time::{Duration, Instant};

fn recommendation() -> Recommendation {
    Recommendation {
        page: Some(RunePageDraft {
            name: "live-builds".to_owned(),
            primary_style: 8000,
            sub_style: 8100,
            perks: [8005, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
        }),
        spells: (4, 7),
        item_set: Some(ItemSet {
            uid: "live-builds-145-BOTTOM".to_owned(),
            title: "live-builds — Kai'Sa BOTTOM".to_owned(),
            champion_ids: vec![145],
            blocks: vec![ItemBlock {
                name: "Início".to_owned(),
                items: vec![1055, 2003],
                original: None,
            }],
        }),
        source: Source::HighElo,
        pages: Vec::new(),
    }
}

fn names() -> KnowledgeNames {
    KnowledgeNames {
        runes: [
            (8005, "Pressione o Ataque"),
            (8009, "Presença de Espírito"),
            (9103, "Lenda: Espontaneidade"),
            (3035, "Até a Morte"),
            (8139, "Gosto de Sangue"),
            (8135, "Caça Suprema"),
            (5005, "Velocidade de Ataque"),
            (5008, "Força Adaptativa"),
            (5011, "Vida"),
        ]
        .into_iter()
        .map(|(id, name)| (id, name.to_owned()))
        .collect(),
        styles: [(8000, "Precisão"), (8100, "Dominação")]
            .into_iter()
            .map(|(id, name)| (id, name.to_owned()))
            .collect(),
        summoner_spells: [(4, "Flash"), (7, "Curar")]
            .into_iter()
            .map(|(id, name)| (id, name.to_owned()))
            .collect(),
    }
}

#[test]
fn select_screen_shows_source_and_reminder() {
    let recommendation = recommendation();
    let view = select_screen(SelectScreenInput {
        champion: "Kai'Sa",
        position: "BOTTOM",
        recommendation: Some(&recommendation),
        source_patch: Some("16.22"),
        source_games: Some(321),
        names: &names(),
        item_names: &|id| match id {
            1055 => "Lâmina de Doran".to_owned(),
            _ => "Poção de Vida".to_owned(),
        },
        differences: &["Curar → Barreira".to_owned()],
        applied: AppliedItems {
            runes: true,
            spells: false,
            item_set: true,
        },
        chosen: None,
        limit_reached: true,
        settings_corrupted: true,
        loading: false,
        error: None,
    });

    assert_eq!(view.champion_position, "Kai'Sa · BOTTOM");
    assert_eq!(view.source, "elo alto 16.22 (321 partidas)");
    assert!(
        view.runes
            .iter()
            .any(|line| line.contains("Pressione o Ataque"))
    );
    assert_eq!(view.spells, "Flash · Curar");
    assert!(
        view.item_set
            .iter()
            .any(|line| line.contains("Lâmina de Doran"))
    );
    assert_eq!(
        view.reminder_title.as_deref(),
        Some("Confira feitiços e runas")
    );
    assert_eq!(view.differences, vec!["Curar → Barreira"]);
    assert_eq!(view.applied, vec!["✓ Runas", "✓ Arsenal"]);
    assert!(view.limit_message.is_some());
    assert!(view.settings_warning.is_some());
}

#[test]
fn editor_offers_only_valid_runes_per_slot() {
    let rules = RuneRules::from_json(include_str!("fixtures/lcu/perks-styles.json")).unwrap();
    let draft = recommendation().page.unwrap();

    assert_eq!(
        rune_options(&rules, &draft, 0),
        vec![8005, 8008, 8021, 8010]
    );
    assert!(rune_options(&rules, &draft, 4).contains(&8139));
    assert!(!rune_options(&rules, &draft, 4).contains(&8009));
    assert_eq!(rune_options(&rules, &draft, 6), vec![5008, 5005, 5007]);
}

#[test]
fn delete_requires_confirmation() {
    let mut confirmation = DeleteConfirmation::<u64>::default();
    assert_eq!(confirmation.confirm(), None);

    confirmation.request_page(42, "Minha página");
    assert_eq!(
        confirmation.message(),
        Some("Apagar a página Minha página? Não dá para desfazer.")
    );
    confirmation.cancel();
    assert_eq!(confirmation.confirm(), None);

    confirmation.request_page(42, "Minha página");
    assert_eq!(confirmation.confirm(), Some(42));
    assert_eq!(confirmation.confirm(), None);
}

#[test]
fn screen_switches_select_game_editor() {
    let selecting = LcuPollState::Selecting(live_builds::lcu::champ_select::SelectState {
        phase: live_builds::lcu::champ_select::Phase::ChampSelect,
        session_id: "session-1".to_owned(),
        champion_id: Some(145),
        locked: true,
        position: "BOTTOM".to_owned(),
        spells: (4, 7),
        map_mode: (11, "CLASSIC".to_owned()),
        auto_apply: true,
        active_page: None,
    });
    assert_eq!(choose_screen(&selecting, false), Screen::Select);
    assert_eq!(
        choose_screen(
            &LcuPollState::Open(live_builds::lcu::champ_select::Phase::Other(
                "InProgress".to_owned()
            )),
            true
        ),
        Screen::Game
    );
    assert_eq!(
        choose_screen(
            &LcuPollState::Open(live_builds::lcu::champ_select::Phase::Other(
                "Lobby".to_owned()
            )),
            false
        ),
        Screen::Editor
    );
    assert_eq!(
        choose_screen(&LcuPollState::ClientClosed, false),
        Screen::ClientClosed
    );
    assert_eq!(
        choose_screen(
            &LcuPollState::Error("401 sem credenciais".to_owned()),
            false
        ),
        Screen::ClientError
    );
}

#[test]
fn new_item_set_uid_uses_timestamp_and_skips_collisions() {
    let sets = [
        ItemSet {
            uid: "live-builds-user-123".to_owned(),
            title: String::new(),
            champion_ids: vec![],
            blocks: vec![],
        },
        ItemSet {
            uid: "live-builds-user-123-2".to_owned(),
            title: String::new(),
            champion_ids: vec![],
            blocks: vec![],
        },
    ];

    assert_eq!(new_item_set(&sets, 123).uid, "live-builds-user-123-1");
}

#[test]
fn patch_versions_sort_numerically() {
    assert!(compare_patch_versions("16.10.1", "16.9.1").is_gt());
    assert!(compare_patch_versions("17.1", "16.24").is_gt());
    assert!(compare_patch_versions("16.10.2", "16.10.1").is_gt());
}

#[test]
fn selection_failure_retries_then_success_stops_retries() {
    let state = live_builds::lcu::champ_select::SelectState {
        phase: live_builds::lcu::champ_select::Phase::ChampSelect,
        session_id: "session-1".to_owned(),
        champion_id: Some(145),
        locked: true,
        position: "BOTTOM".to_owned(),
        spells: (4, 7),
        map_mode: (11, "CLASSIC".to_owned()),
        auto_apply: true,
        active_page: None,
    };
    let now = Instant::now();
    let mut tracker = SelectionLoadTracker::default();

    assert!(tracker.should_start(&state, now));
    tracker.failed(now);
    assert!(!tracker.should_start(&state, now + Duration::from_secs(4)));
    assert!(tracker.should_start(&state, now + Duration::from_secs(5)));
    tracker.succeeded();
    assert!(!tracker.should_start(&state, now + Duration::from_secs(60)));
}

#[test]
fn selection_poll_updates_do_not_reload_recommendation() {
    let mut state = live_builds::lcu::champ_select::SelectState {
        phase: live_builds::lcu::champ_select::Phase::ChampSelect,
        session_id: "session-1".to_owned(),
        champion_id: Some(145),
        locked: true,
        position: "BOTTOM".to_owned(),
        spells: (4, 7),
        map_mode: (11, "CLASSIC".to_owned()),
        auto_apply: true,
        active_page: None,
    };
    let now = Instant::now();
    let mut tracker = SelectionLoadTracker::default();
    assert!(tracker.should_start(&state, now));
    tracker.succeeded();

    state.spells = (4, 21);
    state.active_page = recommendation().page;

    assert!(!tracker.should_start(&state, now + Duration::from_secs(1)));
}

#[test]
fn editor_changes_only_local_drafts_until_save() {
    let page = ClientPage {
        id: 42,
        name: "Original".to_owned(),
        editable: true,
        temporary: false,
        current: true,
        draft: recommendation().page.unwrap(),
    };
    let set = recommendation().item_set.unwrap();
    let mut editors = EditorsState {
        pages: vec![page],
        sets: vec![set],
        ..EditorsState::default()
    };

    editors.edit_page(0);
    editors.page_draft.as_mut().unwrap().draft.name = "Rascunho".to_owned();
    editors.edit_set(0);
    editors.set_draft.as_mut().unwrap().set.blocks.clear();

    assert_eq!(editors.pages[0].draft.name, "live-builds");
    assert_eq!(editors.sets[0].blocks.len(), 1);
}

#[test]
fn corrupted_settings_clear_only_after_successful_save() {
    let mut settings = Settings {
        corrupted: true,
        ..Settings::default()
    };
    let directory = tempfile::tempdir().unwrap();
    let file_parent = directory.path().join("not-a-directory");
    std::fs::write(&file_parent, "occupied").unwrap();
    let impossible = file_parent.join("settings.json");
    assert!(save_settings(&mut settings, &impossible).is_err());
    assert!(settings.corrupted);

    assert!(save_settings(&mut settings, &directory.path().join("settings.json")).is_ok());
    assert!(!settings.corrupted);
}

fn client_page(id: u64, name: &str) -> ClientPage {
    let mut draft = recommendation().page.unwrap();
    draft.name = name.to_owned();
    ClientPage {
        id,
        name: name.to_owned(),
        editable: true,
        temporary: false,
        current: false,
        draft,
    }
}

fn editors_with_two_pages() -> EditorsState {
    let mut editors = EditorsState {
        pages: vec![client_page(1, "A"), client_page(2, "B")],
        sets: vec![recommendation().item_set.unwrap()],
        ..EditorsState::default()
    };
    editors.edit_page(0);
    editors
}

#[test]
fn dirty_page_asks_before_switching_or_creating() {
    let mut editors = editors_with_two_pages();
    editors.page_draft.as_mut().unwrap().draft.name = "Suja".to_owned();

    editors.open_page(1);
    assert!(editors.discard.is_some());
    assert_eq!(editors.page_draft.as_ref().unwrap().draft.name, "Suja");

    editors.confirm_discard();
    assert_eq!(editors.page_draft.as_ref().unwrap().id, Some(2));

    editors.page_draft.as_mut().unwrap().draft.name = "Suja".to_owned();
    editors.request_new_page(client_page(0, "Nova").draft);
    assert!(editors.discard.is_some());
    editors.discard = None;
    assert_eq!(editors.page_draft.as_ref().unwrap().id, Some(2));
}

#[test]
fn clean_page_switches_without_asking() {
    let mut editors = editors_with_two_pages();

    editors.open_page(1);

    assert!(editors.discard.is_none());
    assert_eq!(editors.page_draft.as_ref().unwrap().id, Some(2));
}

#[test]
fn delete_confirmation_is_cancelled_by_other_item_create_and_missing_draft() {
    let mut editors = editors_with_two_pages();
    editors.page_delete.request_page(1, "A");
    editors.open_page(1);
    assert!(editors.page_delete.message().is_none());

    editors.page_delete.request_page(2, "B");
    editors.request_new_page(client_page(0, "Nova").draft);
    assert!(editors.page_delete.message().is_none());

    editors.page_delete.request_page(2, "B");
    editors.page_draft = None;
    editors.sync_confirmations();
    assert!(editors.page_delete.message().is_none());

    editors.edit_set(0);
    editors.set_delete.request_item_set("uid".to_owned(), "S");
    editors.request_new_set(recommendation().item_set.unwrap());
    assert!(editors.set_delete.message().is_none());
}

#[test]
fn refresh_keeps_dirty_draft_but_save_replaces_it() {
    let mut editors = editors_with_two_pages();
    editors.page_draft.as_mut().unwrap().draft.name = "Suja".to_owned();
    let pages = editors.pages.clone();
    let sets = editors.sets.clone();

    editors.replace_lists(pages.clone(), sets.clone(), false, false);
    assert_eq!(editors.page_draft.as_ref().unwrap().draft.name, "Suja");

    editors.replace_lists(pages, sets, true, false);
    assert_eq!(editors.page_draft.as_ref().unwrap().draft.name, "A");
}

#[test]
fn options_mark_chosen_by_index_and_hide_empty_stats() {
    let mut rec = recommendation();
    let option = |keystone: u32, games: u32| live_builds::lcu::champ_select::RuneOption {
        draft: RunePageDraft {
            perks: [keystone, 8009, 9103, 3035, 8139, 8135, 5005, 5008, 5011],
            ..rec.page.clone().unwrap()
        },
        keystone_name: keystone.to_string(),
        games,
        win_rate: 0.5,
        low_sample: false,
        source: Source::HighElo,
        item_set: None,
    };
    rec.pages = vec![option(8005, 12), option(8010, 0)];
    let view = |chosen| {
        select_screen(SelectScreenInput {
            champion: "Kai'Sa",
            position: "BOTTOM",
            recommendation: Some(&rec),
            source_patch: None,
            source_games: None,
            names: &names(),
            item_names: &|id| id.to_string(),
            differences: &[],
            applied: AppliedItems::default(),
            chosen,
            limit_reached: false,
            settings_corrupted: false,
            loading: false,
            error: None,
        })
    };

    let none = view(None);
    assert!(none.options.iter().all(|option| !option.chosen));
    let second = view(Some(1));
    assert_eq!(
        second.options.iter().map(|o| o.chosen).collect::<Vec<_>>(),
        [false, true]
    );
    assert_eq!(second.options[0].detail, "12 partidas · 50.0% vitória");
    assert_eq!(second.options[1].detail, "");
}
