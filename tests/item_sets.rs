use std::cell::RefCell;
use std::collections::HashMap;

use live_builds::catalog::ItemCatalog;
use live_builds::lcu::item_sets::{ItemBlock, ItemSet, from_build, upsert_app_set};
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
            },
            ItemBlock {
                name: "Botas".to_owned(),
                items: vec![3006],
            },
            ItemBlock {
                name: "Itens".to_owned(),
                items: vec![3031, 3094],
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
            },
            ItemBlock {
                name: "Itens".to_owned(),
                items: vec![3190, 1042, 3031],
            },
        ]
    );
}
