use crate::catalog::ItemCatalog;
use crate::lcu::{LcuError, LcuHttp};
use crate::riot_build::RecommendedBuild;
use serde_json::{Value, json};

const CURRENT_SUMMONER: &str = "/lol-summoner/v1/current-summoner";

#[derive(Debug, PartialEq, Eq)]
pub struct ItemSet {
    pub uid: String,
    pub title: String,
    pub champion_ids: Vec<u32>,
    pub blocks: Vec<ItemBlock>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ItemBlock {
    pub name: String,
    pub items: Vec<u32>,
}

pub fn from_build(
    champion: &str,
    champion_id: u32,
    position: &str,
    build: &RecommendedBuild,
    catalog: &ItemCatalog,
) -> ItemSet {
    let mut blocks = Vec::new();
    if let Some(starting) = build.starting.first() {
        blocks.push(ItemBlock {
            name: "Início".to_owned(),
            items: starting.clone(),
        });
    }
    let (boots_stages, item_stages): (Vec<_>, Vec<_>) = build
        .stages
        .iter()
        .partition(|stage| stage.items.iter().any(|&id| catalog.is_boots(id)));
    for (name, stages) in [("Botas", boots_stages), ("Itens", item_stages)] {
        let items: Vec<_> = stages
            .iter()
            .flat_map(|stage| stage.items.iter().copied())
            .collect();
        if !items.is_empty() {
            blocks.push(ItemBlock {
                name: name.to_owned(),
                items,
            });
        }
    }

    ItemSet {
        uid: format!("live-builds-{champion_id}-{position}"),
        title: format!("live-builds — {champion} {position}"),
        champion_ids: vec![champion_id],
        blocks,
    }
}

pub fn list_sets(http: &dyn LcuHttp) -> Result<Vec<ItemSet>, LcuError> {
    let response = http.get(&sets_path(http)?)?;
    response["itemSets"]
        .as_array()
        .ok_or_else(|| LcuError::Other("resposta de item sets sem itemSets".to_owned()))?
        .iter()
        .map(parse_set)
        .collect()
}

/// Troca/insere só o set do app (por `uid`); o resto do GET volta intacto no PUT.
pub fn upsert_app_set(http: &dyn LcuHttp, item_set: &ItemSet) -> Result<(), LcuError> {
    let path = sets_path(http)?;
    let mut response = http.get(&path)?;
    let sets = response["itemSets"]
        .as_array_mut()
        .ok_or_else(|| LcuError::Other("resposta de item sets sem itemSets".to_owned()))?;
    let replacement = serialize_set(item_set);

    if let Some(existing) = sets
        .iter_mut()
        .find(|set| set["uid"].as_str() == Some(item_set.uid.as_str()))
    {
        *existing = replacement;
    } else {
        sets.push(replacement);
    }

    http.send("PUT", &path, &response)?;
    Ok(())
}

fn sets_path(http: &dyn LcuHttp) -> Result<String, LcuError> {
    let summoner = http.get(CURRENT_SUMMONER)?;
    let id = summoner["summonerId"]
        .as_u64()
        .ok_or_else(|| LcuError::Other("resposta do invocador sem summonerId".to_owned()))?;
    Ok(format!("/lol-item-sets/v1/item-sets/{id}/sets"))
}

fn parse_set(value: &Value) -> Result<ItemSet, LcuError> {
    let uid = value["uid"]
        .as_str()
        .ok_or_else(|| LcuError::Other("item set sem uid".to_owned()))?
        .to_owned();
    let title = value["title"]
        .as_str()
        .ok_or_else(|| LcuError::Other("item set sem title".to_owned()))?
        .to_owned();
    let champion_ids = value["associatedChampions"]
        .as_array()
        .ok_or_else(|| LcuError::Other("item set sem associatedChampions".to_owned()))?
        .iter()
        .filter_map(|id| id.as_u64().and_then(|id| u32::try_from(id).ok()))
        .collect();
    let blocks = value["blocks"]
        .as_array()
        .ok_or_else(|| LcuError::Other("item set sem blocks".to_owned()))?
        .iter()
        .map(|block| {
            let name = block["type"]
                .as_str()
                .ok_or_else(|| LcuError::Other("bloco de item set sem type".to_owned()))?
                .to_owned();
            let items = block["items"]
                .as_array()
                .ok_or_else(|| LcuError::Other("bloco de item set sem items".to_owned()))?
                .iter()
                .filter_map(|item| {
                    item["id"]
                        .as_str()
                        .and_then(|id| id.parse().ok())
                        .or_else(|| item["id"].as_u64().and_then(|id| u32::try_from(id).ok()))
                })
                .collect();
            Ok(ItemBlock { name, items })
        })
        .collect::<Result<Vec<_>, LcuError>>()?;

    Ok(ItemSet {
        uid,
        title,
        champion_ids,
        blocks,
    })
}

fn serialize_set(item_set: &ItemSet) -> Value {
    json!({
        "associatedChampions": item_set.champion_ids,
        "associatedMaps": [],
        "blocks": item_set.blocks.iter().map(|block| json!({
            "hideIfSummonerSpell": "",
            "items": block.items.iter().map(|id| json!({
                "count": 1,
                "id": id.to_string(),
            })).collect::<Vec<_>>(),
            "showIfSummonerSpell": "",
            "type": block.name,
        })).collect::<Vec<_>>(),
        "map": "any",
        "mode": "any",
        "preferredItemSlots": [],
        "sortrank": 0,
        "startedFrom": "blank",
        "title": item_set.title,
        "type": "custom",
        "uid": item_set.uid,
    })
}
