use super::LiveError;
use serde_json::Value;

#[derive(Debug)]
pub struct GameSnapshot {
    pub game_mode: String,
    pub game_time: f64,
    pub active: ActivePlayer,
    pub players: Vec<PlayerInfo>,
    pub raw: serde_json::Value,
}

#[derive(Debug)]
pub struct ActivePlayer {
    pub summoner_name: String,
    pub level: u32,
    pub current_gold: f64,
    pub abilities: AbilityLevels,
    pub rune_ids: Vec<u32>,
}

#[derive(Debug)]
pub struct AbilityLevels {
    pub q: u32,
    pub w: u32,
    pub e: u32,
    pub r: u32,
}

#[derive(Debug)]
pub struct PlayerInfo {
    pub summoner_name: String,
    pub champion_name: String,
    pub team: String,
    pub position: String,
    pub level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub creep_score: u32,
    pub items: Vec<u32>,
    pub has_smite: bool,
    pub subteam: Option<u32>,
}

pub fn parse_all_game_data(json: &str) -> Result<GameSnapshot, LiveError> {
    let raw: Value = serde_json::from_str(json).map_err(LiveError::Json)?;
    let active = raw
        .get("activePlayer")
        .filter(|value| value.is_object())
        .ok_or(LiveError::NoActivePlayer)?;
    let active_name = player_name(active);
    let ability_level = |key: &str| integer(&active["abilities"][key]["abilityLevel"]);
    let players = raw["allPlayers"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|player| {
            let is_active = ["summonerName", "riotId"].iter().any(|key| {
                active[*key]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .is_some_and(|name| {
                        ["summonerName", "riotId"]
                            .iter()
                            .any(|key| player[*key].as_str() == Some(name))
                    })
            });
            PlayerInfo {
                summoner_name: if is_active {
                    active_name.clone()
                } else {
                    player_name(player)
                },
                champion_name: string(&player["championName"]),
                team: string(&player["team"]),
                position: string(&player["position"]),
                level: integer(&player["level"]),
                kills: integer(&player["scores"]["kills"]),
                deaths: integer(&player["scores"]["deaths"]),
                assists: integer(&player["scores"]["assists"]),
                creep_score: integer(&player["scores"]["creepScore"]),
                items: player["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| integer(&item["itemID"]))
                    .filter(|id| *id != 0)
                    .collect(),
                has_smite: ["summonerSpellOne", "summonerSpellTwo"].iter().any(|key| {
                    let spell = &player["summonerSpells"][*key];
                    spell["displayName"]
                        .as_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case("Smite"))
                        || ["rawDisplayName", "rawDescription"].iter().any(|key| {
                            spell[*key]
                                .as_str()
                                .is_some_and(|name| name.contains("SummonerSmite"))
                        })
                }),
                subteam: player["subteam"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok()),
            }
        })
        .collect();
    let active = ActivePlayer {
        summoner_name: active_name,
        level: integer(&active["level"]),
        current_gold: active["currentGold"].as_f64().unwrap_or(0.0),
        abilities: AbilityLevels {
            q: ability_level("Q"),
            w: ability_level("W"),
            e: ability_level("E"),
            r: ability_level("R"),
        },
        rune_ids: active["fullRunes"]["generalRunes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|rune| integer(&rune["id"]))
            .collect(),
    };

    Ok(GameSnapshot {
        game_mode: string(&raw["gameData"]["gameMode"]),
        game_time: raw["gameData"]["gameTime"].as_f64().unwrap_or(0.0),
        active,
        players,
        raw,
    })
}

fn string(value: &Value) -> String {
    value.as_str().map_or_else(String::new, str::to_owned)
}

fn integer(value: &Value) -> u32 {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

fn player_name(player: &Value) -> String {
    player["summonerName"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or_else(|| player["riotId"].as_str())
        .map_or_else(String::new, str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const CLASSIC: &str = include_str!("../../tests/fixtures/allgamedata_classic.json");
    const ARENA: &str = include_str!("../../tests/fixtures/allgamedata_arena.json");

    #[test]
    fn parses_classic_snapshot() {
        let snapshot = parse_all_game_data(CLASSIC).unwrap();

        assert_eq!(snapshot.game_mode, "CLASSIC");
        assert_eq!(snapshot.game_time, 726.5);
        assert_eq!(snapshot.active.summoner_name, "FixturePlayer");
        assert_eq!(snapshot.active.level, 6);
        assert_eq!(snapshot.active.current_gold, 1245.5);
        assert_eq!(snapshot.active.abilities.q, 2);
        assert_eq!(snapshot.active.abilities.w, 1);
        assert_eq!(snapshot.active.abilities.e, 2);
        assert_eq!(snapshot.active.abilities.r, 0);
        assert_eq!(snapshot.players.len(), 10);
        let expected_items = [
            vec![3850, 1001, 2003],
            vec![1036, 1055],
            vec![1101, 1001],
            vec![1056, 1026],
            vec![1055, 1036],
            vec![1054, 1028],
            vec![1102, 1001],
            vec![1056, 1052],
            vec![1055, 1042],
            vec![3850, 1001],
        ];
        for (player, items) in snapshot.players.iter().zip(expected_items) {
            assert_eq!(player.items, items, "items for {}", player.summoner_name);
        }
        let me = snapshot
            .players
            .iter()
            .find(|player| player.summoner_name == snapshot.active.summoner_name)
            .unwrap();
        assert_eq!(me.champion_name, "Thresh");
        assert_eq!(me.team, "ORDER");
        assert_eq!(me.position, "UTILITY");
        assert_eq!(
            (me.kills, me.deaths, me.assists, me.creep_score),
            (1, 2, 5, 24)
        );
        assert!(!me.has_smite);
        assert!(snapshot.players[2].has_smite);
        assert!(snapshot.players[6].has_smite);
        assert_eq!(snapshot.raw["gameData"]["mapNumber"], 11);
        assert!(snapshot.raw["_fixture_note"].is_string());
    }

    #[test]
    fn parses_active_rune_ids() {
        let with =
            r#"{"activePlayer":{"summonerName":"A","fullRunes":{"generalRunes":[{"id":8304}]}}}"#;
        let without = r#"{"activePlayer":{"summonerName":"A"}}"#;

        assert_eq!(parse_all_game_data(with).unwrap().active.rune_ids, [8304]);
        assert!(
            parse_all_game_data(without)
                .unwrap()
                .active
                .rune_ids
                .is_empty()
        );
    }

    #[test]
    fn missing_fields_default() {
        let snapshot =
            parse_all_game_data(r#"{"activePlayer":{},"allPlayers":[{}],"gameData":{}}"#).unwrap();

        assert_eq!(snapshot.game_mode, "");
        assert_eq!(snapshot.game_time, 0.0);
        assert_eq!(snapshot.active.summoner_name, "");
        assert_eq!(snapshot.active.level, 0);
        assert_eq!(snapshot.active.current_gold, 0.0);
        assert_eq!(snapshot.active.abilities.q, 0);
        assert_eq!(snapshot.active.abilities.w, 0);
        assert_eq!(snapshot.active.abilities.e, 0);
        assert_eq!(snapshot.active.abilities.r, 0);
        let player = &snapshot.players[0];
        assert_eq!(player.summoner_name, "");
        assert_eq!(player.champion_name, "");
        assert_eq!(player.team, "");
        assert_eq!(player.position, "");
        assert_eq!(player.level, 0);
        assert_eq!(
            (
                player.kills,
                player.deaths,
                player.assists,
                player.creep_score
            ),
            (0, 0, 0, 0)
        );
        assert!(player.items.is_empty());
        assert!(!player.has_smite);
        assert_eq!(player.subteam, None);
    }

    #[test]
    fn parses_arena_snapshot() {
        let snapshot = parse_all_game_data(ARENA).unwrap();

        assert_eq!(snapshot.game_mode, "CHERRY");
        assert_eq!(snapshot.players.len(), 8);
        assert!(
            snapshot
                .players
                .iter()
                .all(|player| player.subteam.is_some())
        );
        let subteams: HashSet<_> = snapshot
            .players
            .iter()
            .map(|player| player.subteam)
            .collect();
        assert_eq!(subteams.len(), 4);
        assert_eq!(snapshot.players[0].subteam, Some(1));
        assert_eq!(snapshot.players[2].subteam, Some(2));
        assert_eq!(snapshot.players[4].subteam, Some(3));
        assert_eq!(snapshot.players[6].subteam, Some(4));
        assert!(
            snapshot
                .players
                .iter()
                .all(|player| player.position.is_empty())
        );
    }

    #[test]
    fn loading_screen_signals_no_active_player() {
        let result = parse_all_game_data(r#"{"allPlayers":[],"gameData":{"gameMode":"CLASSIC"}}"#);

        assert!(matches!(result, Err(LiveError::NoActivePlayer)));
    }

    #[test]
    fn riot_id_is_used_when_summoner_name_is_absent() {
        let snapshot = parse_all_game_data(
            r#"{"activePlayer":{"riotId":"FixturePlayer#BR1"},"allPlayers":[{"riotId":"FixturePlayer#BR1","championName":"Thresh"}]}"#,
        )
        .unwrap();

        assert_eq!(snapshot.active.summoner_name, "FixturePlayer#BR1");
        assert_eq!(
            snapshot.players[0].summoner_name,
            snapshot.active.summoner_name
        );
        assert_eq!(snapshot.players[0].champion_name, "Thresh");
    }
}
