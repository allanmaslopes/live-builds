use crate::knowledge::RunePage;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunePageDraft {
    pub name: String,
    pub primary_style: u32,
    pub sub_style: u32,
    pub perks: [u32; 9],
}

#[derive(Debug)]
pub struct RuneRules {
    styles: HashMap<u32, StyleRule>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StyleRule {
    id: u32,
    allowed_sub_styles: Vec<u32>,
    slots: Vec<SlotRule>,
}

#[derive(Debug, Deserialize)]
struct SlotRule {
    perks: Vec<u32>,
}

impl RuneRules {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let styles: Vec<StyleRule> =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        Ok(Self {
            styles: styles.into_iter().map(|style| (style.id, style)).collect(),
        })
    }

    pub fn validate(&self, draft: &RunePageDraft) -> Result<(), String> {
        let primary = self
            .styles
            .get(&draft.primary_style)
            .ok_or_else(|| "Unknown primary rune style".to_owned())?;
        let secondary = self
            .styles
            .get(&draft.sub_style)
            .ok_or_else(|| "Unknown secondary rune style".to_owned())?;
        if draft.primary_style == draft.sub_style
            || !primary.allowed_sub_styles.contains(&draft.sub_style)
        {
            return Err("Secondary rune style must differ from primary".to_owned());
        }

        for (slot, perk) in draft.perks[..4].iter().enumerate() {
            if !primary
                .slots
                .get(slot)
                .is_some_and(|rule| rule.perks.contains(perk))
            {
                return Err(format!("Primary rune {perk} is outside slot {slot}"));
            }
        }

        let mut secondary_slots = Vec::with_capacity(2);
        for perk in &draft.perks[4..6] {
            let slot = secondary
                .slots
                .get(1..4)
                .ok_or_else(|| "Secondary rune style has fewer than 4 slots".to_owned())?
                .iter()
                .position(|rule| rule.perks.contains(perk))
                .ok_or_else(|| format!("Secondary rune {perk} is outside its style"))?
                + 1;
            if secondary_slots.contains(&slot) {
                return Err("Secondary runes must use different slots".to_owned());
            }
            secondary_slots.push(slot);
        }

        for (index, perk) in draft.perks[6..].iter().enumerate() {
            let slot = index + 4;
            if !primary
                .slots
                .get(slot)
                .is_some_and(|rule| rule.perks.contains(perk))
            {
                return Err(format!("Stat shard {perk} is outside slot {index}"));
            }
        }
        Ok(())
    }
}

pub fn from_knowledge(page: &RunePage, name: &str) -> Option<RunePageDraft> {
    let perks: [u32; 6] = page.perks.as_slice().try_into().ok()?;
    let shards = page.stat_perks.as_ref().map_or([5008, 5008, 5011], |stat| {
        [stat.offense, stat.flex, stat.defense]
    });
    Some(RunePageDraft {
        name: name.to_owned(),
        primary_style: page.primary_style,
        sub_style: page.sub_style,
        perks: [
            perks[0], perks[1], perks[2], perks[3], perks[4], perks[5], shards[0], shards[1],
            shards[2],
        ],
    })
}

pub fn from_riot_bin(bin_json: &str, position: &str, name: &str) -> Option<RunePageDraft> {
    let root: Value = serde_json::from_str(bin_json).ok()?;
    find_recommendation(&root, position).map(|recommendation| RunePageDraft {
        name: name.to_owned(),
        primary_style: number(recommendation, &["primaryStyleId", "primaryStyle"]).unwrap(),
        sub_style: number(recommendation, &["subStyleId", "subStyle"]).unwrap(),
        perks: perk_ids(recommendation).unwrap(),
    })
}

fn find_recommendation<'a>(value: &'a Value, position: &str) -> Option<&'a Value> {
    match value {
        Value::Object(object) => {
            let has_page = number(value, &["primaryStyleId", "primaryStyle"]).is_some()
                && number(value, &["subStyleId", "subStyle"]).is_some()
                && perk_ids(value).is_some();
            let matches_position = ["position", "Position", "mPosition"]
                .iter()
                .filter_map(|key| object.get(*key)?.as_str())
                .any(|candidate| candidate.eq_ignore_ascii_case(position));
            if has_page && matches_position {
                return Some(value);
            }
            object
                .values()
                .find_map(|child| find_recommendation(child, position))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|child| find_recommendation(child, position)),
        _ => None,
    }
}

fn number(value: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter()
        .find_map(|key| value.get(*key)?.as_u64()?.try_into().ok())
}

fn perk_ids(value: &Value) -> Option<[u32; 9]> {
    ["selectedPerkIds", "perks", "perkIds"]
        .iter()
        .find_map(|key| {
            value
                .get(*key)?
                .as_array()?
                .iter()
                .map(|perk| perk.as_u64()?.try_into().ok())
                .collect::<Option<Vec<u32>>>()?
                .try_into()
                .ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::{KnowledgePackage, RunePage};

    const STYLES: &str = include_str!("../../tests/fixtures/lcu/perks-styles.json");
    const KNOWLEDGE: &str = include_str!("../../tests/fixtures/knowledge-16.22.json");
    const RIOT_BIN: &str =
        include_str!("../../tests/fixtures/kaisa-rune-recommendations-16.22.json");

    fn rules() -> RuneRules {
        RuneRules::from_json(STYLES).unwrap()
    }

    fn kaisa_page() -> RunePage {
        let package: KnowledgePackage = serde_json::from_str(KNOWLEDGE).unwrap();
        package
            .entries
            .into_iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap()
            .rune_pages
            .into_iter()
            .next()
            .unwrap()
    }

    fn valid_draft() -> RunePageDraft {
        from_knowledge(&kaisa_page(), "live-builds").unwrap()
    }

    #[test]
    fn validates_real_package_page() {
        assert!(rules().validate(&valid_draft()).is_ok());
    }

    #[test]
    fn rejects_rune_outside_slot() {
        let mut draft = valid_draft();
        draft.perks[1] = 9103;

        assert!(rules().validate(&draft).is_err());
    }

    #[test]
    fn rejects_secondary_same_as_primary() {
        let mut draft = valid_draft();
        draft.sub_style = draft.primary_style;
        draft.perks[4..6].copy_from_slice(&[8009, 9103]);

        assert!(rules().validate(&draft).is_err());
    }

    #[test]
    fn rejects_bad_shard_slot() {
        let mut draft = valid_draft();
        draft.perks[6] = 5011;

        assert!(rules().validate(&draft).is_err());
    }

    #[test]
    fn invalid_package_page_falls_back() {
        let mut draft = valid_draft();
        draft.perks[3] = 999_999;

        assert!(rules().validate(&draft).is_err());
    }

    #[test]
    fn short_style_slots_are_an_error() {
        let draft = valid_draft();
        let mut styles: Value = serde_json::from_str(STYLES).unwrap();
        for style in styles.as_array_mut().unwrap() {
            if style["id"] == draft.sub_style {
                style["slots"].as_array_mut().unwrap().truncate(2);
            }
        }
        let rules = RuneRules::from_json(&styles.to_string()).unwrap();

        assert!(rules.validate(&draft).is_err());
    }

    #[test]
    fn parses_riot_rune_recommendation() {
        assert!(from_riot_bin(RIOT_BIN, "bottom", "live-builds").is_none());
    }
}
