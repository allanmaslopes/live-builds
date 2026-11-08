use crate::catalog::ItemCatalog;
use crate::knowledge::{
    KnowledgeNames, KnowledgePackage, select_entry, select_page, to_recommendations,
};
use crate::lcu::item_sets::{ItemSet, from_build};
use crate::lcu::runes::{RunePageDraft, RuneRules, from_knowledge, from_riot_bin};
use crate::lcu::{LcuError, LcuHttp};
use crate::riot_build::RecommendedBuild;
use crate::settings::Settings;
use crate::static_data::StaticSource;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const FLASH: u32 = 4;
const SMITE: u32 = 11;
const HOVER_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    ChampSelect,
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectState {
    pub phase: Phase,
    pub session_id: String,
    pub champion_id: Option<u32>,
    pub locked: bool,
    pub position: String,
    pub spells: (u32, u32),
    pub map_mode: (u32, String),
    pub auto_apply: bool,
    pub active_page: Option<RunePageDraft>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionKey {
    pub session_id: String,
    pub champion_id: u32,
    pub position: String,
}

impl SelectState {
    pub fn selection_key(&self) -> Option<SelectionKey> {
        Some(SelectionKey {
            session_id: self.session_id.clone(),
            champion_id: self.champion_id?,
            position: self.position.clone(),
        })
    }
}

pub fn parse_session(json: &Value, my_cell: i64) -> SelectState {
    let player = json["myTeam"].as_array().and_then(|team| {
        team.iter()
            .find(|player| player["cellId"].as_i64() == Some(my_cell))
    });
    let id_of = |value: &Value| {
        value["championId"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id != 0)
    };
    let my_picks = || {
        json["actions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_array)
            .flatten()
            .filter(|action| {
                action["actorCellId"].as_i64() == Some(my_cell)
                    && action["type"].as_str() == Some("pick")
            })
    };
    let locked_pick = my_picks().find(|action| action["completed"].as_bool() == Some(true));
    let locked = locked_pick.is_some();
    // travado > pick em andamento (hover) > intenção declarada; o championId do myTeam é o último recurso.
    let champion_id = locked_pick
        .and_then(id_of)
        .or_else(|| locked.then(|| player.and_then(id_of)).flatten())
        .or_else(|| {
            my_picks()
                .find(|action| action["isInProgress"].as_bool() == Some(true))
                .and_then(id_of)
        })
        .or_else(|| {
            player
                .and_then(|player| player["championPickIntent"].as_u64())
                .and_then(|id| u32::try_from(id).ok())
                .filter(|id| *id != 0)
        })
        .or_else(|| player.and_then(id_of));
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
        session_id: json["gameId"]
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| json["id"].as_str().map(str::to_owned))
            .unwrap_or_default(),
        champion_id,
        locked,
        position,
        spells: (spell("spell1Id"), spell("spell2Id")),
        map_mode,
        auto_apply: json["isCustomGame"].as_bool() == Some(true)
            || matches!(queue_id, 400 | 420 | 430 | 440 | 450 | 490),
        active_page: None,
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

#[derive(Clone, Debug, PartialEq)]
pub struct RuneOption {
    pub draft: RunePageDraft,
    pub keystone_name: String,
    pub games: u32,
    pub win_rate: f64,
    pub low_sample: bool,
    pub source: Source,
    /// Arsenal montado com o `buildPath` desta página.
    pub item_set: Option<ItemSet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recommendation {
    /// Página recomendada ou escolhida (a que o automático e "Aplicar runas" gravam).
    pub page: Option<RunePageDraft>,
    pub spells: (u32, u32),
    pub item_set: Option<ItemSet>,
    pub source: Source,
    /// Todas as opções válidas; a de índice 0 é a recomendada.
    pub pages: Vec<RuneOption>,
}

impl Recommendation {
    pub fn can_apply(&self, action: Action) -> bool {
        action != Action::ItemSet || self.item_set.is_some()
    }

    /// Troca a página (e o arsenal que segue o `buildPath` dela) pela opção `index`.
    pub fn choose(&self, index: usize) -> Option<Self> {
        let option = self.pages.get(index)?;
        Some(Self {
            page: Some(option.draft.clone()),
            item_set: option.item_set.clone(),
            ..self.clone()
        })
    }
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
    names: &KnowledgeNames,
) -> Option<Recommendation> {
    let position = normalize_position(position);
    let high_elo = (map_mode.0 == 11 && map_mode.1.eq_ignore_ascii_case("CLASSIC"))
        .then(|| select_entry(package?, champion_id, &position, previous))
        .flatten();
    let make_set = |build: &RecommendedBuild| {
        let set = from_build(champion, champion_id, &position, build, catalog);
        (!set.blocks.is_empty()).then_some(set)
    };

    let (page, pages, spells, item_set, source) =
        if let Some(selected) = high_elo {
            let entry = selected.entry;
            let first = select_page(entry, player_keystone);
            let pages: Vec<RuneOption> =
                first
                    .into_iter()
                    .chain(entry.rune_pages.iter().filter(|rune_page| {
                        first.is_none_or(|first| !std::ptr::eq(first, *rune_page))
                    }))
                    .filter_map(|rune_page| {
                        let draft = from_knowledge(rune_page, "live-builds")
                            .filter(|draft| rune_rules.validate(draft).is_ok())?;
                        Some(RuneOption {
                            draft,
                            keystone_name: names.rune(rune_page.keystone),
                            games: rune_page.games,
                            win_rate: rune_page.win_rate,
                            low_sample: rune_page.low_sample,
                            source: Source::HighElo,
                            item_set: make_set(&to_recommendations(entry, Some(rune_page)).0),
                        })
                    })
                    .collect();
            let (page, item_set) = if let Some(best) = pages.first() {
                (Some(best.draft.clone()), best.item_set.clone())
            } else {
                let page = riot_bin
                    .and_then(|json| from_riot_bin(json, position.as_str(), "live-builds"))
                    .filter(|page| rune_rules.validate(page).is_ok());
                (page, make_set(&to_recommendations(entry, first).0))
            };
            let spells = entry
                .primary
                .as_ref()
                .and_then(|primary| primary.summoner_spells.get(..2))
                .map(|spells| (spells[0], spells[1]))
                .unwrap_or_else(|| default_spells(&position));
            (page, pages, spells, item_set, Source::HighElo)
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
            (
                page,
                Vec::new(),
                default_spells(&position),
                make_set(&build),
                Source::Riot,
            )
        };

    Some(Recommendation {
        page,
        spells,
        item_set,
        source,
        pages,
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
    key: Option<SelectionKey>,
    hover: Option<(u32, Instant)>,
    action_states: [ActionState; 3],
    runes_user_edited: bool,
    choice_made: bool,
}

#[derive(Clone, Copy, Default)]
struct ActionState {
    attempts: u8,
    applied: bool,
    in_flight: bool,
    retry_at: Option<Instant>,
}

impl Applier {
    pub fn on_state(
        &mut self,
        state: &SelectState,
        settings: &Settings,
        user_edited: bool,
    ) -> Vec<Action> {
        self.on_state_at(state, settings, user_edited, Instant::now())
    }

    pub fn on_state_at(
        &mut self,
        state: &SelectState,
        settings: &Settings,
        user_edited: bool,
        now: Instant,
    ) -> Vec<Action> {
        if state.phase != Phase::ChampSelect {
            *self = Self::default();
            return Vec::new();
        }
        if !state.auto_apply {
            return Vec::new();
        }
        let Some(key) = state.selection_key() else {
            return Vec::new();
        };
        if !state.locked {
            let since = match self.hover {
                Some((champion, since)) if champion == key.champion_id => since,
                _ => {
                    self.hover = Some((key.champion_id, now));
                    now
                }
            };
            if now.saturating_duration_since(since) < HOVER_DEBOUNCE {
                return Vec::new();
            }
        }
        if self.key.as_ref() != Some(&key) {
            if self
                .key
                .as_ref()
                .is_some_and(|previous| previous.session_id != key.session_id)
            {
                self.runes_user_edited = false;
            }
            self.key = Some(key);
            self.choice_made = false;
            self.action_states = [ActionState::default(); 3];
        }
        self.runes_user_edited |= user_edited;

        let mut actions = Vec::new();
        if settings.auto_runes && !self.runes_user_edited && !self.choice_made {
            self.queue(Action::Runes, now, &mut actions);
        }
        if settings.auto_spells {
            self.queue(Action::Spells, now, &mut actions);
        }
        if settings.auto_item_set && state.locked {
            self.queue(Action::ItemSet, now, &mut actions);
        }
        actions
    }

    /// Esquece o estado de uma ação para o automático poder refazê-la (ex.: arsenal após trocar a runa).
    pub fn reset(&mut self, action: Action) {
        let state = &mut self.action_states[action.index()];
        *state = ActionState {
            in_flight: state.in_flight,
            ..ActionState::default()
        };
    }

    /// O usuário escolheu uma opção de runa: o automático não troca mais enquanto o campeão for o mesmo.
    pub fn mark_choice(&mut self) {
        self.choice_made = true;
    }

    pub fn confirm(&mut self, action: Action, succeeded: bool) {
        self.confirm_at(action, succeeded, Instant::now());
    }

    pub fn confirm_at(&mut self, action: Action, succeeded: bool, now: Instant) {
        let state = &mut self.action_states[action.index()];
        state.in_flight = false;
        if succeeded {
            state.applied = true;
            state.retry_at = None;
        } else {
            state.retry_at = Some(now + Duration::from_secs(5));
        }
    }

    fn queue(&mut self, action: Action, now: Instant, actions: &mut Vec<Action>) {
        let state = &mut self.action_states[action.index()];
        let due = state.retry_at.is_none_or(|retry_at| now >= retry_at);
        if !state.applied && !state.in_flight && state.attempts < 3 && due {
            state.attempts += 1;
            state.in_flight = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graves_options_include_fleet_footwork_page() {
        let package: KnowledgePackage = serde_json::from_str(include_str!(
            "../../tests/fixtures/knowledge-graves-16.22.json"
        ))
        .unwrap();
        let rules =
            RuneRules::from_json(include_str!("../../tests/fixtures/lcu/perks-styles.json"))
                .unwrap();
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../../tests/fixtures/item_sample.json"),
        )
        .unwrap();
        let statics = StaticSource::with_fetcher(std::env::temp_dir(), |_| {
            Err("network must not be used".to_owned())
        });

        let recommendation = recommend(
            Some(&package),
            None,
            &statics,
            "16.22",
            104,
            "Graves",
            "JUNGLE",
            (11, "CLASSIC"),
            None,
            None,
            &rules,
            &catalog,
            &KnowledgeNames::default(),
        )
        .unwrap();

        assert_eq!(
            recommendation
                .pages
                .iter()
                .map(|option| option.draft.perks[0])
                .collect::<Vec<_>>(),
            vec![8128, 8021, 8010, 8230]
        );
        let fleet = recommendation
            .pages
            .iter()
            .find(|option| option.draft.perks[0] == 8021)
            .unwrap();
        assert_eq!(fleet.games, 69);
        assert_eq!(
            fleet
                .item_set
                .as_ref()
                .unwrap()
                .blocks
                .last()
                .unwrap()
                .items,
            vec![6697, 6676, 3036]
        );
    }
}
