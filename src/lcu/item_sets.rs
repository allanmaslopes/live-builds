use crate::catalog::ItemCatalog;
use crate::lcu::{LcuError, LcuHttp};
use crate::riot_build::RecommendedBuild;
use serde_json::{Value, json};

const CURRENT_SUMMONER: &str = "/lol-summoner/v1/current-summoner";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemSet {
    pub uid: String,
    pub title: String,
    pub champion_ids: Vec<u32>,
    pub blocks: Vec<ItemBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemBlock {
    pub name: String,
    pub items: Vec<u32>,
    pub original: Option<Value>,
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
            original: None,
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
                original: None,
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
    if let Some(existing) = sets
        .iter_mut()
        .find(|set| set["uid"].as_str() == Some(item_set.uid.as_str()))
    {
        merge_set(existing, item_set);
        existing["sortrank"] = json!(2_000_000_000);
        existing["priority"] = json!(true);
    } else {
        let mut app_set = serialize_set(item_set);
        app_set["sortrank"] = json!(2_000_000_000);
        app_set["priority"] = json!(true);
        sets.push(app_set);
    }

    http.send("PUT", &path, &response)?;
    Ok(())
}

/// Salva somente o arsenal indicado e devolve todo o restante do documento sem alterações.
pub fn save_user_set(http: &dyn LcuHttp, item_set: &ItemSet) -> Result<(), LcuError> {
    let path = sets_path(http)?;
    let mut response = http.get(&path)?;
    let sets = response["itemSets"]
        .as_array_mut()
        .ok_or_else(|| LcuError::Other("resposta de item sets sem itemSets".to_owned()))?;
    if let Some(existing) = sets
        .iter_mut()
        .find(|set| set["uid"].as_str() == Some(item_set.uid.as_str()))
    {
        merge_set(existing, item_set);
    } else {
        sets.push(serialize_set(item_set));
    }
    http.send("PUT", &path, &response)?;
    Ok(())
}

/// Só insere: se o `uid` já existe, troca o sufixo; nunca mescla nem sobrescreve.
pub fn create_user_set(http: &dyn LcuHttp, item_set: &ItemSet) -> Result<(), LcuError> {
    let path = sets_path(http)?;
    let mut response = http.get(&path)?;
    let sets = response["itemSets"]
        .as_array_mut()
        .ok_or_else(|| LcuError::Other("resposta de item sets sem itemSets".to_owned()))?;
    let taken = |uid: &str| sets.iter().any(|set| set["uid"].as_str() == Some(uid));
    let mut new_set = item_set.clone();
    let mut suffix = 1;
    while taken(&new_set.uid) {
        suffix += 1;
        new_set.uid = format!("{}-{suffix}", item_set.uid);
    }
    sets.push(serialize_set(&new_set));
    http.send("PUT", &path, &response)?;
    Ok(())
}

/// Apaga somente o arsenal indicado e preserva o envelope e os outros sets do GET.
pub fn delete_user_set(http: &dyn LcuHttp, uid: &str) -> Result<(), LcuError> {
    let path = sets_path(http)?;
    let mut response = http.get(&path)?;
    let sets = response["itemSets"]
        .as_array_mut()
        .ok_or_else(|| LcuError::Other("resposta de item sets sem itemSets".to_owned()))?;
    sets.retain(|set| set["uid"].as_str() != Some(uid));
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
            Ok(ItemBlock {
                name,
                items,
                original: Some(block.clone()),
            })
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

fn merge_set(existing: &mut Value, item_set: &ItemSet) {
    existing["title"] = json!(item_set.title);
    existing["associatedChampions"] = json!(item_set.champion_ids);
    existing["blocks"] = Value::Array(
        item_set
            .blocks
            .iter()
            .map(|block| {
                let mut value = block.original.clone().unwrap_or_else(|| {
                    json!({
                        "hideIfSummonerSpell": "",
                        "showIfSummonerSpell": ""
                    })
                });
                let old_items = value["items"].as_array().cloned().unwrap_or_default();
                let mut used = vec![false; old_items.len()];
                let kept = old_items.iter().filter(|item| item_id(item).is_none());
                let items = block
                    .items
                    .iter()
                    .map(|id| {
                        old_items
                            .iter()
                            .enumerate()
                            .find(|(item_index, item)| {
                                !used[*item_index] && item_id(item) == Some(*id)
                            })
                            .map(|(item_index, item)| {
                                used[item_index] = true;
                                item.clone()
                            })
                            .unwrap_or_else(|| json!({"count": 1, "id": id.to_string()}))
                    })
                    // shortcut: ids não numéricos (o editor não os mostra) vão ao fim do bloco
                    .chain(kept.cloned())
                    .collect();
                value["type"] = json!(block.name);
                value["items"] = Value::Array(items);
                value
            })
            .collect(),
    );
}

fn item_id(item: &Value) -> Option<u32> {
    item["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .or_else(|| item["id"].as_u64().and_then(|id| u32::try_from(id).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcu::LcuHttp;
    use crate::riot_build::ItemStage;
    use std::cell::RefCell;

    struct RecordingHttp {
        responses: RefCell<Vec<Value>>,
        put_body: RefCell<Option<Value>>,
    }

    impl RecordingHttp {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: RefCell::new(responses),
                put_body: RefCell::new(None),
            }
        }
    }

    impl LcuHttp for RecordingHttp {
        fn get(&self, _path: &str) -> Result<Value, LcuError> {
            Ok(self.responses.borrow_mut().remove(0))
        }

        fn send(&self, method: &str, _path: &str, body: &Value) -> Result<Value, LcuError> {
            assert_eq!(method, "PUT");
            *self.put_body.borrow_mut() = Some(body.clone());
            Ok(Value::Null)
        }
    }

    #[test]
    fn item_set_contains_full_build() {
        let catalog = ItemCatalog::from_ddragon_json(
            "16.22.1",
            include_str!("../../tests/fixtures/item_thresh.json"),
        )
        .unwrap();
        let build = RecommendedBuild {
            starting: vec![],
            stages: vec![
                ItemStage {
                    items: vec![6672],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![1001],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3124],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3302],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3006],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3032],
                    max_completed: None,
                },
                ItemStage {
                    items: vec![3036],
                    max_completed: None,
                },
            ],
        };

        let item_set = from_build("Kai'Sa", 145, "BOTTOM", &build, &catalog);

        assert_eq!(
            item_set
                .blocks
                .iter()
                .find(|block| block.name == "Itens")
                .unwrap()
                .items,
            vec![6672, 3124, 3302, 3032, 3036]
        );
    }

    #[test]
    fn app_set_is_sent_first_in_shop() {
        let http = RecordingHttp::new(vec![
            json!({"summonerId": 7}),
            json!({
                "itemSets": [
                    {
                        "uid": "live-builds-145-BOTTOM",
                        "title": "live-builds — Kai'Sa BOTTOM",
                        "associatedChampions": [145],
                        "blocks": [],
                        "sortrank": 4999,
                        "priority": false
                    },
                    {
                        "uid": "live-builds-user-1",
                        "title": "Meu arsenal",
                        "associatedChampions": [],
                        "blocks": [],
                        "sortrank": 0,
                        "priority": false
                    }
                ]
            }),
        ]);
        let app_set = ItemSet {
            uid: "live-builds-145-BOTTOM".to_owned(),
            title: "live-builds — Kai'Sa BOTTOM".to_owned(),
            champion_ids: vec![145],
            blocks: vec![],
        };

        upsert_app_set(&http, &app_set).unwrap();

        let body = http.put_body.into_inner().unwrap();
        let sets = body["itemSets"].as_array().unwrap();
        assert_eq!(sets[0]["sortrank"], 2_000_000_000);
        assert_eq!(sets[0]["priority"], true);
        assert_eq!(sets[1]["sortrank"], 0);
        assert_eq!(sets[1]["priority"], false);
    }
}
