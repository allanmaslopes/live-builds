use crate::catalog::{BOOTS, ItemCatalog, MAGICAL_FOOTWEAR, MAGICAL_FOOTWEAR_RUNE};
use crate::knowledge::{
    KnowledgeEntry, KnowledgeNames, KnowledgePackage, RunePage, selected_full_build_path,
};
use crate::live::{GameSnapshot, PlayerInfo};
use crate::riot_build::{RecommendedBuild, SkillOrder};
use std::collections::HashMap;

/// Um item é "completo" quando custa pelo menos isto (botas e componentes ficam abaixo).
// shortcut: heurística por preço; trocar por `into` vazio do item.json quando o catálogo ler esse campo.
const COMPLETE_ITEM_COST: u32 = 2000;
const ULT_LEVELS: [u32; 3] = [6, 11, 16];
const SKILL_LETTERS: [char; 4] = ['Q', 'W', 'E', 'R'];

#[derive(Clone, Debug, PartialEq)]
pub struct PanelView {
    pub high_elo: HighEloSection,
    pub me: MeView,
    pub build: BuildView,
    pub skills: SkillView,
    pub teams: Vec<TeamView>,
    pub lane: Option<LaneView>,
    pub catalog_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HighEloSection {
    Recommendation(HighEloView),
    NoData(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HighEloView {
    pub header: String,
    pub low_sample: bool,
    pub keystone: String,
    pub keystone_source: String,
    pub primary_style: String,
    pub primary_runes: usize,
    pub sub_style: String,
    pub sub_runes: usize,
    pub stat_perks: Vec<String>,
    pub summoner_spells: Vec<String>,
}

pub struct HighEloData<'a> {
    pub package: &'a KnowledgePackage,
    pub entry: &'a KnowledgeEntry,
    pub page: Option<&'a RunePage>,
    pub names: &'a KnowledgeNames,
    pub player_keystone: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeView {
    pub champion: String,
    pub level: u32,
    pub xp_fraction: Option<f32>,
    pub gold: u32,
    pub cs: u32,
    pub kda: (u32, u32, u32),
    pub items: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildView {
    pub blocks: Vec<BlockView>,
    pub next_item: Option<NextItem>,
    pub affordable_now: Vec<String>,
    pub empty_message: Option<String>,
    pub free_boots_line: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockView {
    pub name: String,
    pub items: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NextItem {
    pub name: String,
    pub missing_gold: u32,
    pub next_component: Option<(String, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillView {
    pub q: u32,
    pub w: u32,
    pub e: u32,
    pub r: u32,
    pub points_available: u32,
    pub ult_hint: bool,
    pub next_skill: Option<char>,
    pub priority_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeamView {
    pub label: String,
    pub items_value: u32,
    pub players: Vec<PlayerRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRow {
    pub champion: String,
    pub level: u32,
    pub kda: (u32, u32, u32),
    pub items_value: u32,
    pub is_me: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaneView {
    pub opponent: String,
    pub level_diff: i32,
    pub items_value_diff: i32,
}

pub fn build_view(
    s: &GameSnapshot,
    c: &ItemCatalog,
    b: &RecommendedBuild,
    skills: Option<&SkillOrder>,
    lane_pick: Option<&str>,
    catalog_note: Option<String>,
) -> PanelView {
    let me_info = s
        .players
        .iter()
        .find(|p| p.summoner_name == s.active.summoner_name);
    let owned: &[u32] = me_info.map_or(&[], |p| p.items.as_slice());
    let gold = s.active.current_gold.max(0.0) as u32;

    let me = MeView {
        champion: me_info.map_or_else(String::new, |p| p.champion_name.clone()),
        level: s.active.level,
        xp_fraction: None, // a API local não informa XP
        gold,
        cs: me_info.map_or(0, |p| p.creep_score),
        kda: me_info.map_or((0, 0, 0), kda),
        items: owned.iter().map(|&id| c.name(id)).collect(),
    };

    PanelView {
        high_elo: HighEloSection::NoData("Sem dados para este campeão".to_owned()),
        me,
        build: build_section(c, b, owned, gold, &s.active.rune_ids),
        skills: skill_section(s, skills),
        teams: team_section(s, c, me_info),
        lane: me_info.and_then(|me| lane_section(s, c, me, lane_pick)),
        catalog_note,
    }
}

pub fn build_view_with_knowledge(
    s: &GameSnapshot,
    c: &ItemCatalog,
    b: &RecommendedBuild,
    skills: Option<&SkillOrder>,
    lane_pick: Option<&str>,
    catalog_note: Option<String>,
    high_elo: Option<HighEloData<'_>>,
) -> PanelView {
    let mut panel = build_view(s, c, b, skills, lane_pick, catalog_note);
    if let Some(data) = high_elo {
        let page = data.page;
        let primary = data.entry.primary.as_ref();
        let keystone = page
            .map(|page| page.keystone)
            .or_else(|| primary.map(|primary| primary.keystone));
        let games_label = if data.entry.games == 1 {
            "partida"
        } else {
            "partidas"
        };
        panel.high_elo = HighEloSection::Recommendation(HighEloView {
            header: format!(
                "{} · {} · elo alto {} ({} {}, {:.0}%)",
                panel.me.champion,
                data.entry.position,
                data.package.patch,
                data.entry.games,
                games_label,
                data.entry.win_rate * 100.0
            ),
            low_sample: data.entry.low_sample
                || page.is_some_and(|page| page.low_sample)
                || page.is_some_and(|page| page.build_path.is_some() && page.build_path_low_sample)
                || selected_full_build_path(data.entry, page).is_some_and(|path| path.low_sample)
                || primary.is_some_and(|primary| primary.summoner_spells_low_sample),
            keystone: keystone.map_or_else(String::new, |id| data.names.rune(id)),
            keystone_source: if page.is_some_and(|page| data.player_keystone == Some(page.keystone))
            {
                "sua".to_owned()
            } else {
                "recomendada".to_owned()
            },
            primary_style: page.map_or_else(String::new, |page| {
                format!(
                    "{}: {}",
                    data.names.style(page.primary_style),
                    page.perks
                        .iter()
                        .take(4)
                        .map(|&id| data.names.rune(id))
                        .collect::<Vec<_>>()
                        .join(" · ")
                )
            }),
            primary_runes: page.map_or(0, |page| page.perks.len().min(4)),
            sub_style: page.map_or_else(String::new, |page| {
                format!(
                    "{}: {}",
                    data.names.style(page.sub_style),
                    page.perks
                        .iter()
                        .skip(4)
                        .take(2)
                        .map(|&id| data.names.rune(id))
                        .collect::<Vec<_>>()
                        .join(" · ")
                )
            }),
            sub_runes: page.map_or(0, |page| page.perks.len().saturating_sub(4)),
            stat_perks: page
                .and_then(|page| page.stat_perks.as_ref())
                .map(|stats| {
                    [stats.offense, stats.flex, stats.defense]
                        .into_iter()
                        .map(|id| data.names.rune(id))
                        .collect()
                })
                .unwrap_or_default(),
            summoner_spells: primary
                .into_iter()
                .flat_map(|primary| primary.summoner_spells.iter())
                .map(|&id| data.names.summoner_spell(id))
                .collect(),
        });
    }
    panel
}

fn kda(p: &PlayerInfo) -> (u32, u32, u32) {
    (p.kills, p.deaths, p.assists)
}

fn items_value(c: &ItemCatalog, items: &[u32]) -> u32 {
    items
        .iter()
        .map(|&id| c.get(id).map_or(0, |i| i.total_cost))
        .sum()
}

fn build_section(
    c: &ItemCatalog,
    b: &RecommendedBuild,
    owned: &[u32],
    gold: u32,
    runes: &[u32],
) -> BuildView {
    // Calçados Mágicos dá as Levemente Mágicas: sem bota ainda, 1001 não é compra.
    let free_boots = runes.contains(&MAGICAL_FOOTWEAR_RUNE)
        && c.get(MAGICAL_FOOTWEAR).is_some()
        && !owned.iter().any(|&id| c.is_boots(id));
    // 1001 como item do bloco; como componente, só some se o pai está em `2422.into`.
    let hidden = |id: u32| free_boots && id == BOOTS;
    // Para custos, as botas grátis já contam como possuídas.
    let cost_owned: Vec<u32> = owned
        .iter()
        .copied()
        .chain(free_boots.then_some(MAGICAL_FOOTWEAR))
        .collect();
    let owned_cost = cost_owned.as_slice();

    // Cada item do inventário cobre uma ocorrência no bloco.
    let block = |name: String, ids: &[u32]| {
        let mut left = owned.to_vec();
        BlockView {
            name,
            items: ids
                .iter()
                .map(|&id| {
                    // ID exato antes de evolução: [3172, 3006] cobre o bloco [3006, 3172] inteiro.
                    let hit = left
                        .iter()
                        .position(|&o| o == id)
                        .or_else(|| left.iter().position(|&o| c.is_or_evolves_from(o, id)));
                    if let Some(i) = hit {
                        left.swap_remove(i);
                    }
                    (c.name(id), hit.is_some())
                })
                .collect(),
        }
    };
    let mut blocks = Vec::new();
    if let Some(start) = b.starting.first() {
        blocks.push(block("Início".to_owned(), start));
    }
    for (i, stage) in b.stages.iter().enumerate() {
        blocks.push(block(format!("Fase {}", i + 1), &stage.items));
    }

    let completed = owned
        .iter()
        .filter(|&&id| {
            c.get(id)
                .is_some_and(|i| i.total_cost >= COMPLETE_ITEM_COST)
        })
        .count() as u32;
    // Primeiro estágio ainda válido (max_completed) em que não tenho nenhuma alternativa.
    let next_id = b
        .stages
        .iter()
        .filter(|st| st.max_completed.is_none_or(|n| completed <= n))
        .filter(|st| !st.items.iter().any(|&id| c.owns(id, owned)))
        .find_map(|st| {
            st.items
                .iter()
                .copied()
                .find(|&id| !hidden(id) && !c.owns(id, owned) && c.get(id).is_some())
        });

    let mut affordable_ids: Vec<u32> = b
        .starting
        .first()
        .into_iter()
        .flatten()
        .chain(b.stages.iter().flat_map(|st| st.items.iter()))
        .filter(|&&id| !c.owns(id, owned) && !hidden(id))
        .copied()
        .collect();
    let next_item = next_id.and_then(|id| {
        let item = c.get(id)?;
        let mut left = owned_counts(owned_cost);
        let next_component = item
            .from
            .iter()
            .filter(|&&comp| !c.take_component(&mut left, comp, id))
            .filter_map(|&comp| c.get(comp))
            .min_by_key(|comp| (comp.total_cost, comp.id));
        let remaining_owned: Vec<_> = left
            .into_iter()
            .flat_map(|(id, count)| std::iter::repeat_n(id, count as usize))
            .collect();
        let next_component = next_component.map(|comp| {
            let missing = c
                .remaining_cost(comp.id, &remaining_owned)
                .saturating_sub(gold);
            (comp.name.clone(), missing)
        });
        affordable_ids.extend(
            item.from
                .iter()
                .copied()
                .filter(|&comp| !(free_boots && c.magical_footwear_for(comp, item.id).is_some())),
        );
        Some(NextItem {
            name: item.name.clone(),
            missing_gold: c.remaining_cost(id, owned_cost).saturating_sub(gold),
            next_component,
        })
    });

    let mut affordable_now: Vec<String> = Vec::new();
    for id in affordable_ids {
        let Some(item) = c.get(id) else { continue };
        if item.purchasable
            && !owned.contains(&id)
            && c.remaining_cost(id, owned_cost) <= gold
            && !affordable_now.contains(&item.name)
        {
            affordable_now.push(item.name.clone());
        }
    }

    let empty = b.starting.is_empty() && b.stages.is_empty();
    BuildView {
        blocks,
        next_item,
        affordable_now,
        empty_message: empty.then(|| "Sem build recomendada para este campeão".to_owned()),
        free_boots_line: free_boots.then(|| "Botas grátis pela runa (Calçados Mágicos)".to_owned()),
    }
}

fn owned_counts(owned: &[u32]) -> HashMap<u32, u32> {
    let mut counts = HashMap::new();
    for &id in owned {
        *counts.entry(id).or_insert(0) += 1;
    }
    counts
}

fn skill_section(s: &GameSnapshot, order: Option<&SkillOrder>) -> SkillView {
    let a = &s.active.abilities;
    let levels = [a.q, a.w, a.e, a.r];
    let spent: u32 = levels.iter().sum();
    let level = s.active.level;
    let points_available = level.saturating_sub(spent);
    let ult_hint = points_available > 0 && ULT_LEVELS.contains(&level);

    // Q/W/E: no máximo 5 pontos e no máximo ceil(nível / 2).
    let can_raise = |i: usize| i < 3 && levels[i] < 5 && levels[i] < level.div_ceil(2);
    let next_skill = order.filter(|_| points_available > 0).and_then(|o| {
        if ult_hint {
            return Some('R');
        }
        o.early
            .get(spent as usize)
            .map(|&i| usize::from(i))
            .filter(|&i| can_raise(i))
            .or_else(|| {
                o.priority
                    .iter()
                    .map(|&i| usize::from(i))
                    .find(|&i| can_raise(i))
            })
            .map(|i| SKILL_LETTERS[i])
    });
    let priority_label = order.and_then(|o| {
        let letters: Vec<String> = o
            .priority
            .iter()
            .filter_map(|&i| SKILL_LETTERS.get(usize::from(i)))
            .map(char::to_string)
            .collect();
        (!letters.is_empty()).then(|| letters.join(" > "))
    });

    SkillView {
        q: a.q,
        w: a.w,
        e: a.e,
        r: a.r,
        points_available,
        ult_hint,
        next_skill,
        priority_label,
    }
}

fn same_side(a: &PlayerInfo, b: &PlayerInfo) -> bool {
    a.team == b.team && a.subteam == b.subteam
}

fn team_section(s: &GameSnapshot, c: &ItemCatalog, me: Option<&PlayerInfo>) -> Vec<TeamView> {
    let mut groups: Vec<(&PlayerInfo, TeamView)> = Vec::new();
    for p in &s.players {
        let row = PlayerRow {
            champion: p.champion_name.clone(),
            level: p.level,
            kda: kda(p),
            items_value: items_value(c, &p.items),
            is_me: me.is_some_and(|m| m.summoner_name == p.summoner_name),
        };
        match groups.iter_mut().find(|(first, _)| same_side(first, p)) {
            Some((_, team)) => {
                team.items_value += row.items_value;
                team.players.push(row);
            }
            None => groups.push((
                p,
                TeamView {
                    label: team_label(p),
                    items_value: row.items_value,
                    players: vec![row],
                },
            )),
        }
    }
    // Ordenação estável: o meu time primeiro, os outros na ordem em que aparecem.
    groups.sort_by_key(|(first, _)| !me.is_some_and(|m| same_side(m, first)));
    groups.into_iter().map(|(_, team)| team).collect()
}

fn team_label(p: &PlayerInfo) -> String {
    let side = match p.team.as_str() {
        "ORDER" => "Azul",
        "CHAOS" => "Vermelho",
        other => other,
    };
    match p.subteam {
        Some(n) => format!("{side} · Equipe {n}"),
        None => side.to_owned(),
    }
}

fn lane_section(
    s: &GameSnapshot,
    c: &ItemCatalog,
    me: &PlayerInfo,
    lane_pick: Option<&str>,
) -> Option<LaneView> {
    let enemies = || s.players.iter().filter(|p| !same_side(p, me));
    let has_position = |p: &PlayerInfo| {
        !me.position.is_empty()
            && !me.position.eq_ignore_ascii_case("none")
            && p.position.eq_ignore_ascii_case(&me.position)
    };
    let is_pick = |p: &PlayerInfo| {
        lane_pick.is_some_and(|pick| {
            !pick.is_empty()
                && (p.champion_name.eq_ignore_ascii_case(pick)
                    || p.summoner_name.eq_ignore_ascii_case(pick))
        })
    };
    let opponent = enemies()
        .find(|p| has_position(p))
        .or_else(|| enemies().find(|p| is_pick(p)))
        .or_else(|| {
            if me.has_smite {
                enemies().find(|p| p.has_smite)
            } else {
                None
            }
        })?;
    let diff = |a: u32, b: u32| i32::try_from(i64::from(a) - i64::from(b)).unwrap_or(0);
    Some(LaneView {
        opponent: opponent.champion_name.clone(),
        level_diff: diff(me.level, opponent.level),
        items_value_diff: diff(items_value(c, &me.items), items_value(c, &opponent.items)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgePackage, select_page};
    use crate::live::parse_all_game_data;
    use crate::riot_build::{ItemStage, parse_riot_recommendations};

    fn snapshot(json: &str) -> GameSnapshot {
        parse_all_game_data(json).unwrap()
    }

    fn classic() -> GameSnapshot {
        snapshot(include_str!("../tests/fixtures/allgamedata_classic.json"))
    }

    fn catalog() -> ItemCatalog {
        ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../tests/fixtures/item_thresh.json"),
        )
        .unwrap()
    }

    fn thresh() -> (RecommendedBuild, SkillOrder) {
        let (build, skills) = parse_riot_recommendations(
            include_str!("../tests/fixtures/thresh.bin.json"),
            11,
            "CLASSIC",
            "utility",
        );
        (build, skills.unwrap())
    }

    fn set_skills(s: &mut GameSnapshot, level: u32, q: u32, w: u32, e: u32, r: u32) {
        s.active.level = level;
        let a = &mut s.active.abilities;
        (a.q, a.w, a.e, a.r) = (q, w, e, r);
    }

    fn set_items(s: &mut GameSnapshot, gold: f64, items: &[u32]) {
        s.active.current_gold = gold;
        s.players[0].items = items.to_vec();
    }

    fn rune_names() -> KnowledgeNames {
        KnowledgeNames {
            runes: HashMap::from([
                (8008, "Ritmo Fatal".to_owned()),
                (8009, "Presença de Espírito".to_owned()),
                (9103, "Lenda: Linhagem".to_owned()),
                (8017, "Dilacerar".to_owned()),
                (8313, "Tônico Triplo".to_owned()),
                (8347, "Perspicácia Cósmica".to_owned()),
            ]),
            styles: HashMap::from([
                (8000, "Precisão".to_owned()),
                (8300, "Inspiração".to_owned()),
            ]),
            summoner_spells: HashMap::from([(4, "Flash".to_owned()), (12, "Teleporte".to_owned())]),
        }
    }

    #[test]
    fn panel_shows_high_elo_header_and_low_sample_warning() {
        let package: KnowledgePackage =
            serde_json::from_str(include_str!("../tests/fixtures/knowledge-16.22.json")).unwrap();
        let entry = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 1 && entry.position == "ANY")
            .unwrap();
        let page = select_page(entry, None);
        let names = KnowledgeNames::default();
        let (build, skills) = crate::knowledge::to_recommendations(entry, page);

        let panel = build_view_with_knowledge(
            &classic(),
            &catalog(),
            &build,
            skills.as_ref(),
            None,
            None,
            Some(HighEloData {
                package: &package,
                entry,
                page,
                names: &names,
                player_keystone: None,
            }),
        );

        let HighEloSection::Recommendation(high_elo) = panel.high_elo else {
            panic!("expected high elo recommendation")
        };
        assert_eq!(
            high_elo.header,
            "Thresh · ANY · elo alto 16.22 (18 partidas, 50%)"
        );
        assert!(high_elo.low_sample);
    }

    #[test]
    fn panel_without_package_shows_no_data_line() {
        let panel = build_view(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
        );

        assert_eq!(
            panel.high_elo,
            HighEloSection::NoData("Sem dados para este campeão".to_owned())
        );
    }

    #[test]
    fn panel_lists_every_rune_name() {
        let package: KnowledgePackage =
            serde_json::from_str(include_str!("../tests/fixtures/knowledge-16.22.json")).unwrap();
        let entry = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap();
        let page = select_page(entry, None);
        let names = rune_names();
        let panel = build_view_with_knowledge(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
            Some(HighEloData {
                package: &package,
                entry,
                page,
                names: &names,
                player_keystone: None,
            }),
        );

        let HighEloSection::Recommendation(view) = panel.high_elo else {
            panic!("expected high elo recommendation")
        };
        assert_eq!(
            view.primary_style,
            "Precisão: Ritmo Fatal · Presença de Espírito · Lenda: Linhagem · Dilacerar"
        );
        assert_eq!(
            view.sub_style,
            "Inspiração: Tônico Triplo · Perspicácia Cósmica"
        );
        assert_eq!(
            view.stat_perks,
            ["Velocidade de ataque", "Força adaptativa", "Vida"]
        );
    }

    #[test]
    fn panel_warns_when_page_or_spells_have_low_sample() {
        let package: KnowledgePackage =
            serde_json::from_str(include_str!("../tests/fixtures/knowledge-16.22.json")).unwrap();
        let mut entry = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap()
            .clone();
        entry.low_sample = false;
        entry.rune_pages[0].low_sample = true;
        let names = KnowledgeNames::default();
        let panel = build_view_with_knowledge(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
            Some(HighEloData {
                package: &package,
                entry: &entry,
                page: Some(&entry.rune_pages[0]),
                names: &names,
                player_keystone: None,
            }),
        );
        let HighEloSection::Recommendation(view) = panel.high_elo else {
            panic!("expected high elo recommendation")
        };
        assert!(view.low_sample);

        entry.rune_pages[0].low_sample = false;
        entry.primary.as_mut().unwrap().summoner_spells_low_sample = true;
        let panel = build_view_with_knowledge(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
            Some(HighEloData {
                package: &package,
                entry: &entry,
                page: Some(&entry.rune_pages[0]),
                names: &names,
                player_keystone: None,
            }),
        );
        let HighEloSection::Recommendation(view) = panel.high_elo else {
            panic!("expected high elo recommendation")
        };
        assert!(view.low_sample);
    }

    #[test]
    fn full_build_path_low_sample_shows_warning() {
        let package: KnowledgePackage =
            serde_json::from_str(include_str!("../tests/fixtures/knowledge-16.22.json")).unwrap();
        let mut entry = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap()
            .clone();
        entry.low_sample = false;
        entry.rune_pages[0].low_sample = false;
        entry.primary.as_mut().unwrap().summoner_spells_low_sample = false;
        entry.rune_pages[0].build_path_low_sample = false;
        entry.rune_pages[0]
            .full_build_path
            .as_mut()
            .unwrap()
            .low_sample = true;
        let names = KnowledgeNames::default();
        let panel = build_view_with_knowledge(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
            Some(HighEloData {
                package: &package,
                entry: &entry,
                page: Some(&entry.rune_pages[0]),
                names: &names,
                player_keystone: None,
            }),
        );

        let HighEloSection::Recommendation(view) = panel.high_elo else {
            panic!("expected high elo recommendation")
        };
        assert!(view.low_sample);
    }

    #[test]
    fn next_skill_follows_riot_order() {
        let (build, skills) = thresh();
        let (c, mut s) = (catalog(), classic());
        let mut next = |level, q, w, e, r| {
            set_skills(&mut s, level, q, w, e, r);
            build_view(&s, &c, &build, Some(&skills), None, None)
                .skills
                .next_skill
        };
        assert_eq!(next(1, 0, 0, 0, 0), Some('Q'));
        assert_eq!(next(2, 1, 0, 0, 0), Some('E'));
        assert_eq!(next(6, 2, 1, 2, 0), Some('R'));
        assert_eq!(next(7, 2, 1, 2, 1), Some('Q'));
        assert_eq!(next(7, 2, 1, 2, 2), None); // sem pontos
    }

    #[test]
    fn skill_points_and_ult_hint() {
        let (build, skills) = thresh();
        let (c, mut s) = (catalog(), classic());
        set_skills(&mut s, 6, 2, 1, 2, 0);
        let v = build_view(&s, &c, &build, Some(&skills), None, None).skills;
        assert_eq!(v.points_available, 1);
        assert!(v.ult_hint);
        assert_eq!(v.priority_label.as_deref(), Some("R > Q > E > W"));

        set_skills(&mut s, 7, 2, 1, 2, 1);
        let v = build_view(&s, &c, &build, None, None, None).skills;
        assert_eq!((v.points_available, v.ult_hint), (1, false));
        assert_eq!((v.next_skill, v.priority_label), (None, None));
    }

    #[test]
    fn next_item_skips_owned_and_starter() {
        let (build, _) = thresh();
        let (c, mut s) = (catalog(), classic());
        // só inicial/botas/poção: continua no 1º estágio (Medalhão dos Solari de Ferro)
        set_items(&mut s, 0.0, &[3865, 2003, 1001]);
        let v = build_view(&s, &c, &build, None, None, None).build;
        assert_eq!(v.next_item.unwrap().name, "Medalhão dos Solari de Ferro");
        // já tenho um item completo do 1º estágio (max_completed 0): vai para o seguinte
        set_items(&mut s, 0.0, &[3190]);
        let v = build_view(&s, &c, &build, None, None, None).build;
        assert_eq!(v.next_item.unwrap().name, "Botas Galvanizadas de Aço");
        assert!(v.blocks[1].items[0].1 && !v.blocks[1].items[1].1);
        // tenho uma das alternativas de botas: pula o estágio todo
        set_items(&mut s, 0.0, &[3190, 3111]);
        let v = build_view(&s, &c, &build, None, None, None).build;
        assert_ne!(v.next_item.unwrap().name, "Botas Galvanizadas de Aço");
    }

    #[test]
    fn upgraded_boots_mark_boots_stage_done() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![vec![1001, 1042]],
            stages: vec![ItemStage {
                items: vec![3006, 3047],
                max_completed: None,
            }],
        };
        set_items(&mut s, 5000.0, &[3172]);

        let v = build_view(&s, &c, &build, None, None, None).build;

        assert_eq!(
            v.blocks[0].items,
            vec![(c.name(1001), false), (c.name(1042), false)]
        );
        assert_eq!(
            v.blocks[1].items,
            vec![(c.name(3006), true), (c.name(3047), false)]
        );
        assert!(v.next_item.is_none());
        assert!(v.affordable_now.contains(&c.name(1001)));
        assert!(v.affordable_now.contains(&c.name(1042)));
        assert!(!v.affordable_now.contains(&c.name(3006)));
    }

    #[test]
    fn boots_bought_early_do_not_stay_as_next_item() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![
                ItemStage {
                    items: vec![3031],
                    max_completed: Some(0),
                },
                ItemStage {
                    items: vec![3006],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3190],
                    max_completed: None,
                },
            ],
        };
        set_items(&mut s, 5000.0, &[3172]);
        let early = build_view(&s, &c, &build, None, None, None).build;
        assert_eq!(early.next_item.unwrap().name, c.name(3031));
        assert!(early.blocks[1].items[0].1);

        set_items(&mut s, 5000.0, &[3172, 3031]);
        let later = build_view(&s, &c, &build, None, None, None).build;
        assert_eq!(later.next_item.unwrap().name, c.name(3190));
        assert!(later.blocks[1].items[0].1);
        assert!(!later.affordable_now.contains(&c.name(3006)));
    }

    #[test]
    fn missing_gold_and_next_component() {
        let (build, _) = thresh();
        let (c, mut s) = (catalog(), classic());
        // 3190 = 2200 (Gema Ardente 800 + Couraça de Pano 300 + Manto 400 + 700 de receita)
        set_items(&mut s, 1000.0, &[]);
        let n = build_view(&s, &c, &build, None, None, None)
            .build
            .next_item
            .unwrap();
        assert_eq!(n.missing_gold, 1200);
        assert_eq!(n.next_component, Some(("Couraça de Pano".to_owned(), 0)));
        // com a Couraça de Pano, o mais barato que falta é o Manto Anula-Magia
        set_items(&mut s, 100.0, &[1029]);
        let n = build_view(&s, &c, &build, None, None, None)
            .build
            .next_item
            .unwrap();
        assert_eq!(n.missing_gold, 2200 - 300 - 100);
        assert_eq!(
            n.next_component,
            Some(("Manto Anula-Magia".to_owned(), 300))
        );
        // ouro de sobra: falta 0
        set_items(&mut s, 9000.0, &[]);
        let n = build_view(&s, &c, &build, None, None, None)
            .build
            .next_item
            .unwrap();
        assert_eq!(n.missing_gold, 0);
    }

    #[test]
    fn incorporated_components_do_not_count_as_owned_items() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![ItemStage {
                items: vec![3076],
                max_completed: None,
            }],
        };
        set_items(&mut s, 300.0, &[1031]);
        let view = build_view(&s, &c, &build, None, None, None);
        assert!(view.build.affordable_now.contains(&c.name(1029)));
        let next = view.build.next_item.unwrap();
        assert_eq!(next.next_component, Some((c.name(1029), 0)));
        assert_eq!(next.missing_gold, 500);

        set_items(&mut s, 0.0, &[1031, 1029]);

        let next = build_view(&s, &c, &build, None, None, None)
            .build
            .next_item
            .unwrap();

        assert_eq!(next.next_component, Some((c.name(1029), 300)));
    }

    #[test]
    fn consumed_component_is_not_owned() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![vec![1018]],
            stages: vec![],
        };
        set_items(&mut s, 600.0, &[3031]);

        let view = build_view(&s, &c, &build, None, None, None).build;

        assert_eq!(view.blocks[0].items, vec![(c.name(1018), false)]);
        assert!(view.affordable_now.contains(&c.name(1018)));
    }

    #[test]
    fn one_owned_item_covers_one_occurrence() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![ItemStage {
                items: vec![3006, 3006],
                max_completed: None,
            }],
        };
        set_items(&mut s, 0.0, &[3172]);

        let view = build_view(&s, &c, &build, None, None, None).build;

        assert_eq!(
            view.blocks[0].items,
            vec![(c.name(3006), true), (c.name(3006), false)]
        );
    }

    #[test]
    fn exact_match_before_evolution_in_block() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![ItemStage {
                items: vec![3006, 3172],
                max_completed: None,
            }],
        };
        set_items(&mut s, 0.0, &[3172, 3006]);

        let view = build_view(&s, &c, &build, None, None, None).build;

        assert_eq!(
            view.blocks[0].items,
            vec![(c.name(3006), true), (c.name(3172), true)]
        );
    }

    #[test]
    fn owned_counts_preserves_exact_item_multiplicity() {
        let counts = owned_counts(&[3172, 3172]);

        assert_eq!(counts.get(&3172), Some(&2));
        assert!(!counts.contains_key(&3006));
        assert!(!counts.contains_key(&1001));
        assert!(!counts.contains_key(&1042));
    }

    #[test]
    fn affordable_now_respects_gold() {
        let (build, _) = thresh();
        let (c, mut s) = (catalog(), classic());
        set_items(&mut s, 1245.0, &[1001, 2003]);
        let a = build_view(&s, &c, &build, None, None, None)
            .build
            .affordable_now;
        // Botas (já tenho 1001): 1200 - 300 = 900 e 1250 - 300 = 950 cabem
        assert!(a.contains(&"Botas Galvanizadas de Aço".to_owned()));
        assert!(a.contains(&"Passos de Mercúrio".to_owned()));
        assert!(a.contains(&"Atlas Mundial".to_owned()));
        // componente do próximo item (Medalhão) que cabe
        assert!(a.contains(&"Couraça de Pano".to_owned()));
        // não cabem / não compráveis / já possuídos
        assert!(!a.contains(&"Medalhão dos Solari de Ferro".to_owned()));
        assert!(!a.contains(&"Botas da Mobilidade".to_owned())); // purchasable = false
        assert!(!a.contains(&"Poção de Vida".to_owned())); // já possuo
        // sem ouro, nada
        set_items(&mut s, 0.0, &[]);
        let a = build_view(&s, &c, &build, None, None, None)
            .build
            .affordable_now;
        assert!(a.is_empty());
    }

    #[test]
    fn team_values_and_my_team_first() {
        let (c, mut s) = (catalog(), classic());
        let v = build_view(&s, &c, &RecommendedBuild::default(), None, None, None);
        assert_eq!(v.teams.len(), 2);
        assert_eq!(v.teams[0].label, "Azul");
        assert_eq!(v.teams[0].players.len(), 5);
        assert!(v.teams[0].players[0].is_me);
        assert_eq!(v.teams[0].players[0].items_value, 350); // 3850 desconhecido + 300 + 50
        assert_eq!(
            (v.teams[0].items_value, v.teams[1].items_value),
            (650, 1650)
        );

        // meu time passa a ser o CHAOS: ele vem primeiro
        for p in &mut s.players {
            p.team = if p.team == "ORDER" { "CHAOS" } else { "ORDER" }.to_owned();
        }
        let v = build_view(&s, &c, &RecommendedBuild::default(), None, None, None);
        assert_eq!(v.teams[0].label, "Vermelho");
        assert!(v.teams[0].players.iter().any(|p| p.is_me));
        assert_eq!(v.teams[0].items_value, 650);
    }

    #[test]
    fn lane_by_position_then_pick_then_smite() {
        let (c, mut s) = (catalog(), classic());
        let b = RecommendedBuild::default();
        // posição igual
        let lane = build_view(&s, &c, &b, None, Some("Jinx"), None)
            .lane
            .unwrap();
        assert_eq!(lane.opponent, "Leona");
        assert_eq!((lane.level_diff, lane.items_value_diff), (0, 50));
        // sem posição: usa o campeão escolhido
        s.players[0].position = String::new();
        let lane = build_view(&s, &c, &b, None, Some("jinx"), None)
            .lane
            .unwrap();
        assert_eq!(lane.opponent, "Jinx");
        assert_eq!(lane.level_diff, -2);
        // sem posição nem escolha: Smite contra Smite
        assert!(build_view(&s, &c, &b, None, None, None).lane.is_none());
        s.players[0].has_smite = true;
        let lane = build_view(&s, &c, &b, None, None, None).lane.unwrap();
        assert_eq!(lane.opponent, "Vi");
        // escolha que não é inimigo (aliado) não vale
        s.players[0].has_smite = false;
        assert!(
            build_view(&s, &c, &b, None, Some("Garen"), None)
                .lane
                .is_none()
        );
    }

    #[test]
    fn view_tolerates_unknown_items() {
        let (_, skills) = thresh();
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![vec![888_888]],
            stages: vec![
                ItemStage {
                    items: vec![888_889],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![888_890, 3190],
                    max_completed: None,
                },
            ],
        };
        set_items(&mut s, 99_999.0, &[999_999]);
        let v = build_view(
            &s,
            &c,
            &build,
            Some(&skills),
            None,
            Some("catálogo do patch 16.21".into()),
        );
        assert_eq!(v.me.items, vec!["Item 999999"]);
        assert_eq!(
            v.build.blocks[0].items,
            vec![("Item 888888".to_owned(), false)]
        );
        // desconhecido nunca é próximo item nem comprável
        assert_eq!(
            v.build.next_item.unwrap().name,
            "Medalhão dos Solari de Ferro"
        );
        assert!(
            v.build
                .affordable_now
                .iter()
                .all(|n| !n.starts_with("Item "))
        );
        assert_eq!(v.catalog_note.as_deref(), Some("catálogo do patch 16.21"));
    }

    #[test]
    fn view_handles_arena_snapshot() {
        let (build, skills) = thresh();
        let s = snapshot(include_str!("../tests/fixtures/allgamedata_arena.json"));
        let v = build_view(&s, &catalog(), &build, Some(&skills), None, None);
        assert_eq!(v.teams.len(), 4);
        assert!(v.teams[0].players.iter().any(|p| p.is_me));
        assert_eq!(v.teams[0].players.len(), 2);
        assert!(v.lane.is_none());
    }

    #[test]
    fn empty_build_shows_message() {
        let v = build_view(
            &classic(),
            &catalog(),
            &RecommendedBuild::default(),
            None,
            None,
            None,
        );
        assert!(v.build.empty_message.is_some());
        assert!(v.build.blocks.is_empty() && v.build.next_item.is_none());
        assert_eq!(v.me.champion, "Thresh");
        assert_eq!(v.teams.len(), 2);
    }

    #[test]
    fn same_snapshot_gives_equal_view() {
        let (build, skills) = thresh();
        let (c, s) = (catalog(), classic());
        let a = build_view(&s, &c, &build, Some(&skills), Some("Jinx"), None);
        let b = build_view(&s, &c, &build, Some(&skills), Some("Jinx"), None);
        assert_eq!(a, b);
    }

    fn with_magical_footwear_rune(s: &mut GameSnapshot) {
        s.active.rune_ids = vec![8304];
    }

    #[test]
    fn magical_footwear_hides_boots_purchase() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![
                ItemStage {
                    items: vec![1001],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3006],
                    max_completed: None,
                },
            ],
        };
        set_items(&mut s, 5000.0, &[]);
        with_magical_footwear_rune(&mut s);

        let v = build_view(&s, &c, &build, None, None, None).build;

        let boots = c.name(1001);
        let n = v.next_item.unwrap();
        assert_eq!(n.name, c.name(3006));
        assert_ne!(n.next_component.unwrap().0, boots);
        assert!(!v.affordable_now.contains(&boots));
        assert!(v.free_boots_line.is_some());

        // sem a runa, a sugestão de Botas continua
        s.active.rune_ids.clear();
        let v = build_view(&s, &c, &build, None, None, None).build;
        assert_eq!(v.next_item.unwrap().name, c.name(1001));
        assert!(v.free_boots_line.is_none());

        // já tenho bota: a linha some
        with_magical_footwear_rune(&mut s);
        set_items(&mut s, 5000.0, &[2422]);
        assert!(
            build_view(&s, &c, &build, None, None, None)
                .build
                .free_boots_line
                .is_none()
        );
    }

    #[test]
    fn magical_boots_next_component_is_not_boots() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![ItemStage {
                items: vec![3006],
                max_completed: None,
            }],
        };
        set_items(&mut s, 0.0, &[2422]);

        let n = build_view(&s, &c, &build, None, None, None)
            .build
            .next_item
            .unwrap();

        assert_eq!(n.missing_gold, c.get(3006).unwrap().total_cost - 300);
        assert_ne!(n.next_component.unwrap().0, c.name(1001));
    }

    #[test]
    fn boots_still_suggested_for_target_outside_magical_into() {
        let (c, mut s) = (catalog(), classic());
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![ItemStage {
                items: vec![3010],
                max_completed: None,
            }],
        };
        set_items(&mut s, 5000.0, &[]);
        with_magical_footwear_rune(&mut s);

        let v = build_view(&s, &c, &build, None, None, None).build;

        let n = v.next_item.unwrap();
        assert_eq!(n.name, c.name(3010));
        assert_eq!(n.next_component.unwrap().0, c.name(1001));
        assert!(v.affordable_now.contains(&c.name(1001)));
    }
}
