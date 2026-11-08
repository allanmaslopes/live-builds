use std::cell::{Cell, RefCell};

use live_builds::lcu::perks::{
    ApplyOutcome, create_page, list_pages, update_page, upsert_app_page,
};
use live_builds::lcu::runes::RunePageDraft;
use live_builds::lcu::{LcuError, LcuHttp};
use serde_json::{Value, json};

const PAGES_FIXTURE: &str = include_str!("fixtures/lcu/perks-pages.json");
const INVENTORY_FIXTURE: &str = include_str!("fixtures/lcu/perks-inventory.json");

struct FakeLcu {
    pages: RefCell<Vec<Value>>,
    inventory: RefCell<Value>,
    inventory_fails: bool,
    post_error: Option<u16>,
    limit_on_post_error: bool,
    post_without_id: bool,
    next_id: Cell<u64>,
    calls: RefCell<Vec<(String, String, Value)>>,
}

impl FakeLcu {
    fn new(pages: Vec<Value>) -> Self {
        Self {
            pages: RefCell::new(pages),
            inventory: RefCell::new(serde_json::from_str(INVENTORY_FIXTURE).unwrap()),
            inventory_fails: false,
            post_error: None,
            limit_on_post_error: false,
            post_without_id: false,
            next_id: Cell::new(1000),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn set_can_add(&self, value: bool) {
        self.inventory.borrow_mut()["canAddCustomPage"] = json!(value);
    }

    fn writes(&self, method: &str) -> Vec<(String, Value)> {
        self.calls
            .borrow()
            .iter()
            .filter(|(m, _, _)| m == method)
            .map(|(_, path, body)| (path.clone(), body.clone()))
            .collect()
    }
}

fn page(id: u64, name: &str, editable: bool, temporary: bool) -> Value {
    json!({
        "id": id, "name": name, "isEditable": editable, "isTemporary": temporary,
        "current": false, "primaryStyleId": 8100, "subStyleId": 8000,
        "selectedPerkIds": [9923, 8143, 8137, 8135, 9111, 9104, 5005, 5008, 5001],
    })
}

impl LcuHttp for FakeLcu {
    fn get(&self, path: &str) -> Result<Value, LcuError> {
        self.calls
            .borrow_mut()
            .push(("GET".into(), path.into(), Value::Null));
        match path {
            "/lol-perks/v1/pages" => Ok(Value::Array(self.pages.borrow().clone())),
            "/lol-perks/v1/inventory" if self.inventory_fails => {
                Err(LcuError::Other("inventory fora do ar".into()))
            }
            "/lol-perks/v1/inventory" => Ok(self.inventory.borrow().clone()),
            _ if path.starts_with("/lol-perks/v1/pages/") => {
                let id: u64 = path.rsplit('/').next().unwrap().parse().unwrap();
                self.pages
                    .borrow()
                    .iter()
                    .find(|page| page["id"].as_u64() == Some(id))
                    .cloned()
                    .ok_or_else(|| LcuError::Http(404, "page not found".into()))
            }
            _ => Err(LcuError::Other(format!("unexpected GET: {path}"))),
        }
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
        self.calls
            .borrow_mut()
            .push((method.into(), path.into(), body.clone()));
        match (method, path) {
            ("POST", "/lol-perks/v1/pages") => {
                if let Some(status) = self.post_error {
                    if self.limit_on_post_error {
                        self.set_can_add(false);
                    }
                    return Err(LcuError::Http(status, "falhou".into()));
                }
                let id = self.next_id.get() + 1;
                self.next_id.set(id);
                let mut created = page(id, body["name"].as_str().unwrap(), true, false);
                created["selectedPerkIds"] = body["selectedPerkIds"].clone();
                self.pages.borrow_mut().push(created.clone());
                if self.post_without_id {
                    return Ok(json!({ "ok": true }));
                }
                // resposta mínima: só o id, sem runas
                Ok(json!({ "id": id }))
            }
            ("DELETE", _) => {
                let id = path.rsplit('/').next().unwrap().parse::<u64>().unwrap();
                self.pages.borrow_mut().retain(|p| p["id"] != id);
                Ok(Value::Null)
            }
            _ => Ok(Value::Null),
        }
    }
}

fn draft() -> RunePageDraft {
    RunePageDraft {
        name: "outro-nome".to_owned(),
        primary_style: 8000,
        sub_style: 8100,
        perks: [8005, 9111, 9104, 8014, 8139, 8135, 5008, 5008, 5011],
    }
}

#[test]
fn real_fixtures_are_understood() {
    let inventory: Value = serde_json::from_str(INVENTORY_FIXTURE).unwrap();
    assert_eq!(inventory["canAddCustomPage"], json!(true));

    let fake = FakeLcu::new(serde_json::from_str(PAGES_FIXTURE).unwrap());
    let pages = list_pages(&fake).unwrap();
    let blitz = pages
        .iter()
        .find(|page| page.name == "Blitz: Viego Jungle")
        .unwrap();
    assert_eq!(blitz.draft.perks.len(), 9);
    assert_eq!(blitz.draft.primary_style, 8100);
}

#[test]
fn upsert_updates_existing_app_page() {
    let fake = FakeLcu::new(vec![
        page(1, "outra", true, false),
        page(7, "live-builds", true, false),
    ]);

    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::Updated
    );
    let puts = fake.writes("PUT");
    assert_eq!(puts[0].0, "/lol-perks/v1/pages/7");
    assert_eq!(puts[0].1["selectedPerkIds"], json!(draft().perks));
    assert_eq!(puts[0].1["current"], json!(true));
    assert_eq!(puts[1], ("/lol-perks/v1/currentpage".to_owned(), json!(7)));
    assert!(fake.writes("POST").is_empty());
}

#[test]
fn upsert_creates_when_missing() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);

    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::Created
    );
    let posts = fake.writes("POST");
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].1["name"], json!("live-builds"));
    assert_eq!(posts[0].1["primaryStyleId"], json!(8000));
    assert_eq!(posts[0].1["subStyleId"], json!(8100));
    assert_eq!(posts[0].1["selectedPerkIds"], json!(draft().perks));
    assert_eq!(posts[0].1["current"], json!(true));
    assert_eq!(
        fake.writes("PUT").last().unwrap(),
        &("/lol-perks/v1/currentpage".to_owned(), json!(1001))
    );
}

#[test]
fn upsert_creates_when_post_response_has_no_id() {
    let mut fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.post_without_id = true;

    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::Created
    );
    assert_eq!(
        fake.writes("PUT").last().unwrap(),
        &("/lol-perks/v1/currentpage".to_owned(), json!(1001))
    );
}

#[test]
fn missing_app_page_is_recreated() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    upsert_app_page(&fake, &draft()).unwrap();
    let created = fake.pages.borrow().last().unwrap()["id"].as_u64().unwrap();
    fake.pages.borrow_mut().retain(|p| p["id"] != created);

    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::Created
    );
    assert_eq!(fake.writes("POST").len(), 2);
    assert_eq!(list_pages(&fake).unwrap().len(), 2);
}

#[test]
fn limit_reached_never_deletes() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.set_can_add(false);
    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::LimitReached
    );
    assert!(fake.writes("POST").is_empty());
    assert!(fake.writes("DELETE").is_empty());

    // status de limite suposto; trocar pelo real capturado na Task 8
    let mut fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.post_error = Some(400);
    fake.limit_on_post_error = true;
    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::LimitReached
    );
    assert!(fake.writes("DELETE").is_empty());
}

#[test]
fn invalid_page_error_is_not_limit() {
    let mut fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.post_error = Some(400);

    assert!(matches!(
        upsert_app_page(&fake, &draft()),
        Err(LcuError::Http(400, _))
    ));
    assert!(fake.writes("DELETE").is_empty());
}

#[test]
fn post_error_survives_inventory_failure() {
    let mut fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.post_error = Some(500);
    // inventory ok antes do POST; falha só na reconsulta depois do erro
    struct FailAfterPost<'a>(&'a FakeLcu);
    impl LcuHttp for FailAfterPost<'_> {
        fn get(&self, path: &str) -> Result<Value, LcuError> {
            if path.ends_with("inventory") && !self.0.writes("POST").is_empty() {
                return Err(LcuError::Other("inventory fora do ar".into()));
            }
            self.0.get(path)
        }
        fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, LcuError> {
            self.0.send(method, path, body)
        }
    }
    fake.inventory_fails = false;

    assert!(matches!(
        upsert_app_page(&FailAfterPost(&fake), &draft()),
        Err(LcuError::Http(500, _))
    ));
}

#[test]
fn missing_can_add_flag_is_error() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    fake.inventory
        .borrow_mut()
        .as_object_mut()
        .unwrap()
        .remove("canAddCustomPage");

    assert!(upsert_app_page(&fake, &draft()).is_err());
    assert!(fake.writes("POST").is_empty());
}

#[test]
fn malformed_page_is_skipped_not_fatal() {
    let mut broken = page(2, "quebrada", true, false);
    broken["selectedPerkIds"] = json!([1, 2, 3, 4, 5, 6, 7, 8]);
    let fake = FakeLcu::new(vec![
        page(1, "a", true, false),
        broken,
        page(3, "b", true, false),
    ]);

    let ids: Vec<u64> = list_pages(&fake).unwrap().iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![1, 3]);
}

#[test]
fn unreadable_app_page_is_error_not_duplicate() {
    let mut broken = page(2, "live-builds", true, false);
    broken["selectedPerkIds"] = json!([1, 2, 3, 4, 5, 6, 7, 8]);
    let fake = FakeLcu::new(vec![page(1, "outra", true, false), broken]);

    assert!(upsert_app_page(&fake, &draft()).is_err());
    assert!(fake.writes("POST").is_empty());
    assert!(fake.writes("DELETE").is_empty());
}

#[test]
fn missing_is_temporary_counts_as_temporary() {
    let mut no_flag = page(2, "live-builds", true, false);
    no_flag.as_object_mut().unwrap().remove("isTemporary");
    let fake = FakeLcu::new(vec![no_flag]);

    assert_eq!(
        upsert_app_page(&fake, &draft()).unwrap(),
        ApplyOutcome::Created
    );
    assert!(
        fake.writes("PUT")
            .iter()
            .all(|(path, _)| !path.ends_with("/pages/2"))
    );
}

#[test]
fn never_touches_other_pages() {
    let fake = FakeLcu::new(vec![
        page(1, "outra", true, false),
        page(2, "live-builds", false, false),
        page(3, "live-builds", true, true),
        page(7, "live-builds", true, false),
    ]);

    upsert_app_page(&fake, &draft()).unwrap();

    let puts: Vec<_> = fake
        .writes("PUT")
        .into_iter()
        .filter(|(path, _)| path.starts_with("/lol-perks/v1/pages/"))
        .collect();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].0, "/lol-perks/v1/pages/7");
    assert!(fake.writes("DELETE").is_empty());
    assert!(fake.writes("POST").is_empty());
}

#[test]
fn editor_create_and_save_do_not_activate_the_page() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);

    create_page(&fake, &draft()).unwrap();
    update_page(&fake, 1, &draft()).unwrap();

    assert_eq!(fake.writes("POST")[0].1["current"], json!(false));
    assert_eq!(fake.writes("PUT")[0].1["current"], json!(false));
    assert!(
        fake.writes("PUT")
            .iter()
            .all(|(path, _)| path != "/lol-perks/v1/currentpage")
    );
}

#[test]
fn editor_refuses_reserved_page_name() {
    let fake = FakeLcu::new(vec![page(1, "outra", true, false)]);
    let mut reserved = draft();
    reserved.name = " Live-Builds ".to_owned();

    let created = create_page(&fake, &reserved).unwrap_err().to_string();
    let renamed = update_page(&fake, 1, &reserved).unwrap_err().to_string();

    assert_eq!(created, "Nome reservado para o live-builds");
    assert_eq!(renamed, "Nome reservado para o live-builds");
    assert!(fake.writes("POST").is_empty() && fake.writes("PUT").is_empty());
}

#[test]
fn editor_can_save_the_app_page_itself() {
    let fake = FakeLcu::new(vec![page(7, "live-builds", true, false)]);
    let mut edited = draft();
    edited.name = "live-builds".to_owned();

    update_page(&fake, 7, &edited).unwrap();

    assert_eq!(fake.writes("PUT").len(), 1);
}
