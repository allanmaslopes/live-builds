pub struct ItemStage {
    pub items: Vec<u32>,
    pub max_completed: Option<u32>,
}

#[derive(Default)]
pub struct RecommendedBuild {
    pub starting: Vec<Vec<u32>>,
    pub stages: Vec<ItemStage>,
}

pub struct SkillOrder {
    pub priority: Vec<u8>,
    pub early: Vec<u8>,
}

pub fn parse_riot_recommendations(
    bin_json: &str,
    map_id: u32,
    mode: &str,
    position: &str,
) -> (RecommendedBuild, Option<SkillOrder>) {
    let Ok(Value::Object(document)) = serde_json::from_str(bin_json) else {
        return (RecommendedBuild::default(), None);
    };
    let mut build = RecommendedBuild::default();
    if let Some(set) = document
        .values()
        .find(|entry| entry["__type"] == "ItemRecommendationOverrideSet")
    {
        let overrides = array(&set["mOverrides"]);
        let selected = overrides
            .iter()
            .find(|entry| {
                array(&entry["mOverrideContexts"]).iter().any(|context| {
                    context["mMapID"].as_u64() == Some(u64::from(map_id))
                        && context["mModeNameStringId"].as_str() == Some(mode)
                })
            })
            .or_else(|| {
                overrides
                    .iter()
                    .find(|entry| array(&entry["mOverrideContexts"]).is_empty())
            });
        if let Some(selected) = selected {
            build.starting = array(&selected["StartingItemBundles"])
                .iter()
                .map(|bundle| item_ids(&bundle["items"]))
                .filter(|items| !items.is_empty())
                .collect();
            build.stages = array(&selected["mRecItemRanges"])
                .iter()
                .filter_map(|range| {
                    let items = item_ids(&range["items"]);
                    (!items.is_empty()).then(|| ItemStage {
                        items,
                        max_completed: range["MaxCompletedItems"]
                            .as_u64()
                            .and_then(|value| u32::try_from(value).ok()),
                    })
                })
                .collect();
        }
    }

    let skills = document
        .values()
        .find(|entry| entry["__type"] == "RecSpellRankUpInfoList")
        .and_then(|list| {
            let infos = array(&list["RecSpellRankUpInfos"]);
            let matches_map = |entry: &&Value| entry["MapId"].as_u64() == Some(u64::from(map_id));
            infos
                .iter()
                .filter(matches_map)
                .find(|entry| entry["Position"].as_str() == Some(position))
                .or_else(|| {
                    infos
                        .iter()
                        .filter(matches_map)
                        .find(|entry| entry["IsDefaultRecommendation"].as_bool() == Some(true))
                })
                .or_else(|| {
                    infos
                        .iter()
                        .filter(matches_map)
                        .find(|entry| entry["Position"].as_str() == Some("None"))
                })
        })
        .and_then(|entry| {
            let priority = skill_indexes(&entry["mDefaultPriority"])?;
            if priority.is_empty() {
                return None;
            }
            let early = if entry["mEarlyLevelOverrides"].is_null() {
                Vec::new()
            } else {
                skill_indexes(&entry["mEarlyLevelOverrides"])?
            };
            Some(SkillOrder { priority, early })
        });

    (build, skills)
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn item_ids(value: &Value) -> Vec<u32> {
    array(value)
        .iter()
        .filter_map(|item| item.as_str()?.strip_prefix("Items/")?.parse().ok())
        .collect()
}

fn skill_indexes(value: &Value) -> Option<Vec<u8>> {
    value
        .as_array()?
        .iter()
        .map(|value| {
            let index = value.as_u64()?;
            (index <= 3).then_some(index as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_riot_recommendations;

    const THRESH: &str = include_str!("../tests/fixtures/thresh.bin.json");

    #[test]
    fn parses_thresh_classic_build() {
        let (build, _) = parse_riot_recommendations(THRESH, 11, "CLASSIC", "utility");

        assert_eq!(build.starting, vec![vec![3865, 2003, 2003]]);
        assert_eq!(build.stages.len(), 4);
        assert_eq!(build.stages[0].items, vec![3190, 2065]);
        assert_eq!(build.stages[0].max_completed, Some(0));
        assert_eq!(build.stages[1].items, vec![3047, 3111, 3117]);
        assert_eq!(build.stages[1].max_completed, None);
        assert!(
            build
                .stages
                .iter()
                .any(|stage| stage.max_completed == Some(3) && stage.items.contains(&3109))
        );
    }

    #[test]
    fn parses_thresh_support_skill_order() {
        let (_, skills) = parse_riot_recommendations(THRESH, 11, "CLASSIC", "utility");
        let skills = skills.expect("Thresh has a support skill order on Summoner's Rift");

        assert_eq!(skills.priority, vec![3, 0, 2, 1]);
        assert_eq!(skills.early, vec![0, 2, 1, 0]);
    }

    #[test]
    fn skill_order_falls_back_to_default_position() {
        let (_, skills) = parse_riot_recommendations(THRESH, 11, "CLASSIC", "top");
        let skills = skills.expect("unknown position uses the map's default recommendation");

        assert_eq!(skills.priority, vec![3, 0, 2, 1]);
        assert_eq!(skills.early, vec![0, 2, 1, 0]);

        let (_, skills) = parse_riot_recommendations(THRESH, 12, "ARAM", "utility");
        let skills = skills.expect("ARAM falls back to the map's None position");

        assert_eq!(skills.priority, vec![3, 0, 2, 1]);
        assert_eq!(skills.early, vec![0, 1, 2, 0]);
    }
}
use serde_json::Value;
