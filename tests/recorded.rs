//! Roda o painel sobre snapshots reais gravados na partida de teste (2026-11-06).

use live_builds::catalog::ItemCatalog;
use live_builds::live::parse_all_game_data;
use live_builds::riot_build::{ItemStage, RecommendedBuild, parse_riot_recommendations};
use live_builds::view::build_view;
use std::fs;

#[test]
fn recorded_snapshots_render_without_panic() {
    let catalog =
        ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_thresh.json"))
            .expect("catalog fixture");
    let (build, skills) = parse_riot_recommendations(
        include_str!("fixtures/thresh.bin.json"),
        11,
        "CLASSIC",
        "utility",
    );

    let mut count = 0;
    let mut visited_upgraded_boots = false;
    for entry in fs::read_dir("tests/fixtures/recorded").expect("recorded dir") {
        let path = entry.expect("entry").path();
        let json = fs::read_to_string(&path).expect("read snapshot");
        let snapshot = parse_all_game_data(&json).expect("parse snapshot");
        let view = build_view(&snapshot, &catalog, &build, skills.as_ref(), None, None);

        assert_eq!(view.me.champion, "Yasuo", "{}", path.display());
        assert_eq!(view.teams.len(), 2, "{}", path.display());
        assert_eq!(
            view.teams.iter().map(|t| t.players.len()).sum::<usize>(),
            10,
            "{}",
            path.display()
        );
        // Posições vieram preenchidas na partida real: o confronto de rota existe.
        assert!(view.lane.is_some(), "{}", path.display());
        if path.file_stem().and_then(|name| name.to_str()) == Some("sr-custom-690") {
            visited_upgraded_boots = true;
            let yasuo = snapshot
                .players
                .iter()
                .find(|player| player.summoner_name == snapshot.active.summoner_name)
                .expect("active Yasuo should exist");
            assert_eq!(yasuo.champion_name, "Yasuo");
            assert!(yasuo.items.contains(&3172));
            let boots_build = RecommendedBuild {
                starting: vec![],
                stages: vec![ItemStage {
                    items: vec![3006],
                    max_completed: None,
                }],
            };
            let boots = build_view(&snapshot, &catalog, &boots_build, None, None, None).build;
            assert!(boots.blocks[0].items[0].1);
            assert!(boots.next_item.is_none());
            assert!(!boots.affordable_now.contains(&catalog.name(3006)));
        }
        count += 1;
    }
    assert_eq!(count, 5);
    assert!(visited_upgraded_boots, "sr-custom-690 must be checked");
}
