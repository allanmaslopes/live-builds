use crate::catalog::ItemCatalog;
use crate::knowledge::{
    KnowledgeNames, KnowledgePackage, select_entry, select_page, to_recommendations,
};
use crate::lcu::item_sets::{ItemSet, from_build};
use crate::lcu::runes::{RunePageDraft, RuneRules, from_knowledge, from_riot_bin};
use crate::lcu::{LcuError, LcuHttp};
use crate::settings::Settings;
use crate::static_data::StaticSource;
use serde_json::{Value, json};

const FLASH: u32 = 4;
const SMITE: u32 = 11;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    ChampSelect,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectState {
    pub phase: Phase,
    pub champion_id: Option<u32>,
    pub locked: bool,
    pub position: String,
    pub spells: (u32, u32),
    pub map_mode: (u32, String),
    pub auto_apply: bool,
}

pub fn parse_session(json: &Value, my_cell: i64) -> SelectState {
    let player = json["myTeam"].as_array().and_then(|team| {
        team.iter()
            .find(|player| player["cellId"].as_i64() == Some(my_cell))
    });
    let locked = json["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
        .any(|action| {
            action["actorCellId"].as_i64() == Some(my_cell)
                && action["type"].as_str() == Some("pick")
                && action["completed"].as_bool() == Some(true)
        });
    let champion_id = player
        .and_then(|player| player["championId"].as_u64())
        .and_then(|id| u32::try_from(id).ok())
        .filter(|id| *id != 0);
    let position = player
        .and_then(|player| player["assignedPosition"].as_str())
        .map(normalize_position)
        .unwrap_or_else(|| "ANY".to_owned());
    let spell = |field| {
        player
            .and_then(|player| player[field].as_u64())
            .and_then(|id| u32::try_from(id).ok())
            .unwrap_or(0)
    };
    let queue_id = json["queueId"].as_u64().unwrap_or_default();
    let map_mode = match queue_id {
        450 => (12, "ARAM".to_owned()),
        _ => (11, "CLASSIC".to_owned()),
    };

    SelectState {
        phase: Phase::ChampSelect,
        champion_id,
        locked,
        position,
        spells: (spell("spell1Id"), spell("spell2Id")),
        map_mode,
        auto_apply: json["isCustomGame"].as_bool() == Some(true)
            || matches!(queue_id, 400 | 420 | 430 | 440 | 450 | 490),
    }
}

fn normalize_position(position: &str) -> String {
    match position.to_ascii_lowercase().as_str() {
        "top" => "TOP",
        "jungle" => "JUNGLE",
        "middle" => "MIDDLE",
        "bottom" => "BOTTOM",
        "utility" => "UTILITY",
        _ => "ANY",
    }
    .to_owned()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    HighElo,
    Riot,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Recommendation {
    pub page: Option<RunePageDraft>,
    pub spells: (u32, u32),
    pub item_set: Option<ItemSet>,
    pub source: Source,
}

#[allow(clippy::too_many_arguments)]
pub fn recommend(
    package: Option<&KnowledgePackage>,
    previous: Option<&KnowledgePackage>,
    statics: &StaticSource,
    patch: &str,
    champion_id: u32,
    champion: &str,
    position: &str,
    map_mode: (u32, &str),
    player_keystone: Option<u32>,
    riot_bin: Option<&str>,
    rune_rules: &RuneRules,
    catalog: &ItemCatalog,
) -> Option<Recommendation> {
    let position = normalize_position(position);
    let high_elo = (map_mode.0 == 11 && map_mode.1.eq_ignore_ascii_case("CLASSIC"))
        .then(|| select_entry(package?, champion_id, &position, previous))
        .flatten();

    let (page, spells, build, source) = if let Some(selected) = high_elo {
        let selected_page = select_page(selected.entry, player_keystone);
        let page = selected_page
            .and_then(|page| from_knowledge(page, "live-builds"))
            .filter(|page| rune_rules.validate(page).is_ok())
            .or_else(|| {
                riot_bin
                    .and_then(|json| from_riot_bin(json, position.as_str(), "live-builds"))
                    .filter(|page| rune_rules.validate(page).is_ok())
            });
        let spells = selected
            .entry
            .primary
            .as_ref()
            .and_then(|primary| primary.summoner_spells.get(..2))
            .map(|spells| (spells[0], spells[1]))
            .unwrap_or_else(|| default_spells(&position));
        let (build, _) = to_recommendations(selected.entry, selected_page);
        (page, spells, build, Source::HighElo)
    } else {
        let riot_position = if map_mode.1.eq_ignore_ascii_case("CLASSIC") {
            position.as_str()
        } else {
            map_mode.1
        };
        let page = riot_bin
            .and_then(|json| from_riot_bin(json, riot_position, "live-builds"))
            .filter(|page| rune_rules.validate(page).is_ok());
        let (build, _) =
            statics.recommendations(patch, champion, map_mode.0, map_mode.1, riot_position);
        (page, default_spells(&position), build, Source::Riot)
    };
    let item_set = from_build(champion, champion_id, &position, &build, catalog);
    let item_set = (!item_set.blocks.is_empty()).then_some(item_set);

    Some(Recommendation {
        page,
        spells,
        item_set,
        source,
    })
}

fn default_spells(position: &str) -> (u32, u32) {
    let second = match position {
        "JUNGLE" => SMITE,
        "BOTTOM" => 7,
        "TOP" | "MIDDLE" | "UTILITY" | "ANY" => 14,
        _ => 14,
    };
    (FLASH, second)
}

pub fn set_spells(http: &dyn LcuHttp, first: u32, second: u32) -> Result<(), LcuError> {
    // campos spell1Id/spell2Id confirmados na leitura (docs/lcu.md §6); escrita validada na Task 8.
    http.send(
        "PATCH",
        "/lol-champ-select/v1/session/my-selection",
        &json!({"spell1Id": first, "spell2Id": second}),
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Runes,
    Spells,
    ItemSet,
}

#[derive(Default)]
pub struct Applier {
    key: Option<(u32, String, u32, String)>,
    action_states: [ActionState; 3],
    runes_user_edited: bool,
}

#[derive(Clone, Copy, Default)]
struct ActionState {
    attempts: u8,
    applied: bool,
}

impl Applier {
    pub fn on_state(
        &mut self,
        state: &SelectState,
        settings: &Settings,
        user_edited: bool,
    ) -> Vec<Action> {
        if state.phase != Phase::ChampSelect {
            *self = Self::default();
            return Vec::new();
        }
        self.runes_user_edited |= user_edited;
        if !state.locked {
            return Vec::new();
        }
        if !state.auto_apply {
            return Vec::new();
        }
        let Some(champion_id) = state.champion_id else {
            return Vec::new();
        };
        let key = (
            champion_id,
            state.position.clone(),
            state.map_mode.0,
            state.map_mode.1.clone(),
        );
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.action_states = [ActionState::default(); 3];
        }

        let mut actions = Vec::new();
        if settings.auto_runes && !self.runes_user_edited {
            self.queue(Action::Runes, &mut actions);
        }
        if settings.auto_spells {
            self.queue(Action::Spells, &mut actions);
        }
        if settings.auto_item_set {
            self.queue(Action::ItemSet, &mut actions);
        }
        actions
    }

    pub fn confirm(&mut self, action: Action, succeeded: bool) {
        if succeeded {
            self.action_states[action.index()].applied = true;
        }
    }

    fn queue(&mut self, action: Action, actions: &mut Vec<Action>) {
        let state = &mut self.action_states[action.index()];
        if !state.applied && state.attempts < 3 {
            state.attempts += 1;
            actions.push(action);
        }
    }
}

impl Action {
    const fn index(self) -> usize {
        match self {
            Self::Runes => 0,
            Self::Spells => 1,
            Self::ItemSet => 2,
        }
    }
}

pub fn reminder(
    active: &RunePageDraft,
    spells: (u32, u32),
    recommendation: &Recommendation,
    names: &KnowledgeNames,
) -> Vec<String> {
    let mut differences = Vec::new();
    if let Some(wanted) = &recommendation.page {
        if active.primary_style != wanted.primary_style {
            differences.push(format!(
                "{} → {}",
                names.style(active.primary_style),
                names.style(wanted.primary_style)
            ));
        }
        if active.sub_style != wanted.sub_style {
            differences.push(format!(
                "{} → {}",
                names.style(active.sub_style),
                names.style(wanted.sub_style)
            ));
        }
        for (current, wanted) in active.perks.iter().zip(wanted.perks) {
            if *current != wanted {
                differences.push(format!("{} → {}", names.rune(*current), names.rune(wanted)));
            }
        }
    }

    let current = [spells.0, spells.1];
    let wanted = [recommendation.spells.0, recommendation.spells.1];
    let mut missing: Vec<_> = wanted
        .into_iter()
        .filter(|spell| !current.contains(spell))
        .collect();
    let extra: Vec<_> = current
        .into_iter()
        .filter(|spell| !wanted.contains(spell))
        .collect();
    if let Some(index) = missing.iter().position(|spell| *spell == SMITE) {
        missing.remove(index);
        differences.push("falta Golpear".to_owned());
    }
    differences.extend(extra.into_iter().zip(missing).map(|(from, to)| {
        format!(
            "{} → {}",
            names.summoner_spell(from),
            names.summoner_spell(to)
        )
    }));
    differences
}

pub(crate) fn phase_from_json(value: &Value) -> Result<Phase, LcuError> {
    let phase = value
        .as_str()
        .ok_or_else(|| LcuError::Other("fase da LCU não é uma string".to_owned()))?;
    Ok(if phase == "ChampSelect" {
        Phase::ChampSelect
    } else {
        Phase::Other(phase.to_owned())
    })
}
