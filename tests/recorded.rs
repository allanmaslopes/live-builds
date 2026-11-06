//! Roda o painel sobre snapshots reais gravados na partida de teste (2026-11-06).

use live_builds::catalog::ItemCatalog;
use live_builds::live::parse_all_game_data;
use live_builds::riot_build::parse_riot_recommendations;
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
        count += 1;
    }
    assert_eq!(count, 5);
}
