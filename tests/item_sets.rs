use std::cell::RefCell;
use std::collections::HashMap;

use live_builds::catalog::ItemCatalog;
use live_builds::lcu::item_sets::{
    ItemBlock, ItemSet, delete_user_set, from_build, save_user_set, upsert_app_set,
};
use live_builds::lcu::{LcuError, LcuHttp};
use live_builds::riot_build::{ItemStage, RecommendedBuild};
use serde_json::{Value, json};

struct FakeLcu {
    responses: HashMap<String, Value>,
    writes: RefCell<Vec<(String, String, Value)>>,
}

impl FakeLcu {
    fn with_sets(sets: Value) -> Self {
        Self {
            responses: HashMap::from([
                (
                    "/lol-summoner/v1/current-summoner".to_owned(),
                    json!({ "summonerId": 1 }),
                ),
                ("/lol-item-sets/v1/item-sets/1/sets".to_owned(), sets),
            ]),
            writes: RefCell::new(Vec::new()),
        }
    }
}

impl LcuHttp for FakeLcu {
    fn get(&self, path: &str) -> Result<Value, LcuError> {
        self.responses
            .get(path)
            .cloned()
            .ok_or_else(|| LcuError::Other(format!("unexpected GET: {path}")))
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
        self.writes
            .borrow_mut()
            .push((method.to_owned(), path.to_owned(), body.clone()));
        Ok(Value::Null)
    }
}

fn build() -> RecommendedBuild {
    RecommendedBuild {
        starting: vec![vec![1055, 2003]],
        stages: vec![
            ItemStage {
                items: vec![3006],
                max_completed: None,
            },
            ItemStage {
                items: vec![3031, 3094],
                max_completed: None,
            },
        ],
    }
}

fn catalog() -> ItemCatalog {
    ItemCatalog::from_ddragon_json("16.22.1", include_str!("fixtures/item_thresh.json")).unwrap()
}

fn app_set() -> ItemSet {
    from_build("Jinx", 222, "bottom", &build(), &catalog())
}

#[test]
fn builds_item_set_from_recommendation() {
    let item_set = app_set();

    assert_eq!(item_set.uid, "live-builds-222-bottom");
    assert_eq!(item_set.title, "live-builds — Jinx bottom");
    assert_eq!(item_set.champion_ids, vec![222]);
    assert_eq!(
        item_set.blocks,
        vec![
            ItemBlock {
                name: "Início".to_owned(),
                items: vec![1055, 2003],
                original: None,
            },
            ItemBlock {
                name: "Botas".to_owned(),
                items: vec![3006],
                original: None,
            },
            ItemBlock {
                name: "Itens".to_owned(),
                items: vec![3031, 3094],
                original: None,
            },
        ]
    );
}

#[test]
fn upsert_keeps_user_sets_untouched() {
    let user_one = json!({
        "uid": "user-1",
        "title": "U.GG - Jinx",
        "customField": { "keep": [1, 2, 3] },
        "blocks": []
    });
    let user_two = json!({
        "uid": "user-2",
        "title": "Manual",
        "associatedMaps": [11],
        "blocks": []
    });
    let old_app = json!({
        "uid": "live-builds-222-bottom",
        "title": "qualquer título antigo",
        "blocks": []
    });
    let fake = FakeLcu::with_sets(json!({
        "accountId": 1,
        "timestamp": 4,
        "itemSets": [user_one.clone(), user_two.clone(), old_app]
    }));

    upsert_app_set(&fake, &app_set()).unwrap();

    let writes = fake.writes.borrow();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, "PUT");
    assert_eq!(writes[0].1, "/lol-item-sets/v1/item-sets/1/sets");
    assert_eq!(writes[0].2["itemSets"][0], user_one);
    assert_eq!(writes[0].2["itemSets"][1], user_two);
    assert_eq!(writes[0].2["itemSets"].as_array().unwrap().len(), 3);
    assert_eq!(
        writes[0].2["itemSets"][2]["title"],
        "live-builds — Jinx bottom"
    );
}

#[test]
fn upsert_does_not_duplicate() {
    let fake = FakeLcu::with_sets(json!({
        "accountId": 1,
        "itemSets": [{
            "uid": "live-builds-222-bottom",
            "title": "live-builds — Jinx bottom",
            "blocks": []
        }]
    }));

    upsert_app_set(&fake, &app_set()).unwrap();

    let writes = fake.writes.borrow();
    let sets = writes[0].2["itemSets"].as_array().unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0]["title"], "live-builds — Jinx bottom");
}

#[test]
fn upsert_preserves_unknown_fields_of_user_sets() {
    let original: Value =
        serde_json::from_str(include_str!("fixtures/lcu/item-sets.json")).unwrap();
    let fake = FakeLcu::with_sets(original.clone());

    upsert_app_set(&fake, &app_set()).unwrap();

    let writes = fake.writes.borrow();
    let sent = &writes[0].2;
    let original_sets = original["itemSets"].as_array().unwrap();
    let sent_sets = sent["itemSets"].as_array().unwrap();
    assert_eq!(sent_sets.len(), original_sets.len() + 1);
    for (before, after) in original_sets.iter().zip(sent_sets) {
        assert_eq!(before, after);
    }
    assert_eq!(sent_sets.last().unwrap()["uid"], "live-builds-222-bottom");
    for key in ["accountId", "timestamp"] {
        assert_eq!(sent[key], original[key], "{key}");
    }
    assert_eq!(
        sent.as_object().unwrap().len(),
        original.as_object().unwrap().len()
    );
}

#[test]
fn user_set_with_same_title_is_not_overwritten() {
    let user = json!({
        "uid": "user-1",
        "title": "live-builds — Jinx bottom",
        "extra": true,
        "blocks": []
    });
    let fake = FakeLcu::with_sets(json!({ "accountId": 1, "itemSets": [user.clone()] }));

    upsert_app_set(&fake, &app_set()).unwrap();

    let writes = fake.writes.borrow();
    let sets = writes[0].2["itemSets"].as_array().unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0], user);
    assert_eq!(sets[1]["uid"], "live-builds-222-bottom");
}

#[test]
fn boots_block_follows_catalog_not_position() {
    // ordem real do pacote: item1 -> botas -> item2 -> item3
    let build = RecommendedBuild {
        starting: vec![],
        stages: [vec![3190], vec![3006], vec![1042], vec![3031]]
            .into_iter()
            .map(|items| ItemStage {
                items,
                max_completed: None,
            })
            .collect(),
    };

    let item_set = from_build("Jinx", 222, "bottom", &build, &catalog());

    assert_eq!(
        item_set.blocks,
        vec![
            ItemBlock {
                name: "Botas".to_owned(),
                items: vec![3006],
                original: None,
            },
            ItemBlock {
                name: "Itens".to_owned(),
                items: vec![3190, 1042, 3031],
                original: None,
            },
        ]
    );
}

#[test]
fn save_user_set_preserves_other_sets() {
    let untouched = json!({"uid":"other", "title":"Outro", "unknown":{"keep":true}});
    let target = json!({"uid":"mine", "title":"Antigo", "unknown":"old"});
    let fake = FakeLcu::with_sets(json!({
        "accountId": 1, "timestamp": 99, "itemSets": [untouched.clone(), target]
    }));
    let edited = ItemSet {
        uid: "mine".to_owned(),
        title: "Novo".to_owned(),
        champion_ids: vec![],
        blocks: vec![ItemBlock {
            name: "Itens".to_owned(),
            items: vec![3031],
            original: None,
        }],
    };

    save_user_set(&fake, &edited).unwrap();

    let sent = &fake.writes.borrow()[0].2;
    assert_eq!(sent["accountId"], 1);
    assert_eq!(sent["timestamp"], 99);
    assert_eq!(sent["itemSets"][0], untouched);
    assert_eq!(sent["itemSets"].as_array().unwrap().len(), 2);
    assert_eq!(sent["itemSets"][1]["uid"], "mine");
    assert_eq!(sent["itemSets"][1]["title"], "Novo");
}

#[test]
fn editing_set_preserves_its_own_unknown_fields() {
    let mut original: Value =
        serde_json::from_str(include_str!("fixtures/lcu/item-sets.json")).unwrap();
    original["itemSets"][0]["blocks"][0]["items"][0]["count"] = json!(7);
    original["itemSets"][0]["unknownRoot"] = json!({"keep": true});
    original["itemSets"][0]["blocks"][0]["unknownBlock"] = json!("keep");
    original["itemSets"][0]["blocks"][0]["items"][0]["unknownItem"] = json!(42);
    let fake = FakeLcu::with_sets(original.clone());
    let mut edited = live_builds::lcu::item_sets::list_sets(&fake).unwrap()[0].clone();
    edited.title = "Evelynn editada".to_owned();
    edited.champion_ids = vec![28, 99];
    edited.blocks[0].name = "Começo".to_owned();

    save_user_set(&fake, &edited).unwrap();

    let sent = &fake.writes.borrow()[0].2["itemSets"][0];
    assert_eq!(sent["title"], "Evelynn editada");
    assert_eq!(sent["associatedChampions"], json!([28, 99]));
    assert_eq!(sent["blocks"][0]["type"], "Começo");
    assert_eq!(
        sent["associatedMaps"],
        original["itemSets"][0]["associatedMaps"]
    );
    assert_eq!(
        sent["preferredItemSlots"],
        original["itemSets"][0]["preferredItemSlots"]
    );
    assert_eq!(sent["unknownRoot"], json!({"keep": true}));
    assert_eq!(sent["blocks"][0]["unknownBlock"], "keep");
    assert_eq!(sent["blocks"][0]["items"][0]["count"], 7);
    assert_eq!(sent["blocks"][0]["items"][0]["unknownItem"], 42);
}

#[test]
fn removing_first_block_keeps_second_block_conditions() {
    let first = json!({
        "type": "Primeiro",
        "items": [{"id": "1055", "count": 1}],
        "showIfSummonerSpell": "",
        "hideIfSummonerSpell": ""
    });
    let second = json!({
        "type": "Segundo",
        "items": [{"id": "3031", "count": 1}],
        "showIfSummonerSpell": "SummonerFlash",
        "hideIfSummonerSpell": "SummonerSmite"
    });
    let fake = FakeLcu::with_sets(json!({
        "accountId": 1,
        "itemSets": [{
            "uid": "mine",
            "title": "Meu arsenal",
            "associatedChampions": [],
            "blocks": [first, second]
        }]
    }));
    let mut edited = live_builds::lcu::item_sets::list_sets(&fake).unwrap()[0].clone();
    edited.blocks.remove(0);

    save_user_set(&fake, &edited).unwrap();

    let block = &fake.writes.borrow()[0].2["itemSets"][0]["blocks"][0];
    assert_eq!(block["type"], "Segundo");
    assert_eq!(block["showIfSummonerSpell"], "SummonerFlash");
    assert_eq!(block["hideIfSummonerSpell"], "SummonerSmite");
}

#[test]
fn delete_user_set_preserves_other_sets() {
    let first = json!({"uid":"first", "unknown":[1,2]});
    let target = json!({"uid":"mine", "title":"Apagar"});
    let last = json!({"uid":"last", "custom":true});
    let fake = FakeLcu::with_sets(json!({
        "accountId": 1, "timestamp": 99,
        "itemSets": [first.clone(), target, last.clone()]
    }));

    delete_user_set(&fake, "mine").unwrap();

    let sent = &fake.writes.borrow()[0].2;
    assert_eq!(sent["accountId"], 1);
    assert_eq!(sent["timestamp"], 99);
    assert_eq!(sent["itemSets"], json!([first, last]));
}
