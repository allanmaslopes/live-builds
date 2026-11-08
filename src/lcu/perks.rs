use crate::lcu::runes::RunePageDraft;
use crate::lcu::{LcuError, LcuHttp};
use serde_json::{Value, json};

const PAGES: &str = "/lol-perks/v1/pages";
const CURRENT_PAGE: &str = "/lol-perks/v1/currentpage";
const INVENTORY: &str = "/lol-perks/v1/inventory";
const APP_PAGE_NAME: &str = "live-builds";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientPage {
    pub id: u64,
    pub name: String,
    pub editable: bool,
    pub temporary: bool,
    pub current: bool,
    pub draft: RunePageDraft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Updated,
    Created,
    LimitReached,
}

pub fn list_pages(http: &dyn LcuHttp) -> Result<Vec<ClientPage>, LcuError> {
    // página malformada (ex.: sem 9 runas) é ignorada, não derruba a lista
    Ok(raw_pages(http)?
        .iter()
        .filter_map(|page| parse_page(page).ok())
        .collect())
}

fn raw_pages(http: &dyn LcuHttp) -> Result<Vec<Value>, LcuError> {
    http.get(PAGES)?
        .as_array()
        .cloned()
        .ok_or_else(|| LcuError::Other("resposta de páginas de runas não é uma lista".to_owned()))
}

/// Só o `id` da resposta importa; sem ele, relista e acha a página pelo nome.
pub fn create_page(http: &dyn LcuHttp, draft: &RunePageDraft) -> Result<ClientPage, LcuError> {
    let response = http.send("POST", PAGES, &page_body(draft))?;
    if let Ok(page) = parse_page(&response) {
        return Ok(page);
    }
    let id = match response["id"].as_u64() {
        Some(id) => id,
        None => list_pages(http)?
            .into_iter()
            .rev()
            .find(|page| page.name == draft.name)
            .map(|page| page.id)
            .ok_or_else(|| LcuError::Other("página criada não encontrada na LCU".to_owned()))?,
    };
    Ok(ClientPage {
        id,
        name: draft.name.clone(),
        editable: true,
        temporary: false,
        current: true,
        draft: draft.clone(),
    })
}

/// O chamador (editor) é responsável por pedir confirmação antes de alterar a página.
pub fn update_page(http: &dyn LcuHttp, id: u64, draft: &RunePageDraft) -> Result<(), LcuError> {
    http.send("PUT", &format!("{PAGES}/{id}"), &page_body(draft))?;
    Ok(())
}

/// O chamador (editor) é responsável por pedir confirmação antes de apagar a página.
pub fn delete_page(http: &dyn LcuHttp, id: u64) -> Result<(), LcuError> {
    http.send("DELETE", &format!("{PAGES}/{id}"), &Value::Null)?;
    Ok(())
}

pub fn set_current(http: &dyn LcuHttp, id: u64) -> Result<(), LcuError> {
    http.send("PUT", CURRENT_PAGE, &json!(id))?;
    Ok(())
}

/// Atualiza a página `live-builds` (ou cria) e a ativa. Nunca apaga nem altera outra página.
pub fn upsert_app_page(
    http: &dyn LcuHttp,
    draft: &RunePageDraft,
) -> Result<ApplyOutcome, LcuError> {
    let draft = RunePageDraft {
        name: APP_PAGE_NAME.to_owned(),
        ..draft.clone()
    };
    let mut existing = None;
    for raw in raw_pages(http)? {
        if raw["name"].as_str() != Some(APP_PAGE_NAME) {
            continue;
        }
        // não cria duplicada se a página do app existe mas não dá para ler
        let page = parse_page(&raw)?;
        if page.editable && !page.temporary {
            existing = Some(page);
            break;
        }
    }
    if let Some(page) = existing {
        update_page(http, page.id, &draft)?;
        set_current(http, page.id)?;
        return Ok(ApplyOutcome::Updated);
    }

    if limit_reached(http)? {
        return Ok(ApplyOutcome::LimitReached);
    }
    match create_page(http, &draft) {
        Ok(page) => {
            set_current(http, page.id)?;
            Ok(ApplyOutcome::Created)
        }
        // limite só se o inventory confirmar; senão (ou se o GET falhar) vale o erro do POST
        Err(error @ LcuError::Http(..)) => match limit_reached(http) {
            Ok(true) => Ok(ApplyOutcome::LimitReached),
            _ => Err(error),
        },
        Err(error) => Err(error),
    }
}

fn limit_reached(http: &dyn LcuHttp) -> Result<bool, LcuError> {
    http.get(INVENTORY)?["canAddCustomPage"]
        .as_bool()
        .map(|can_add| !can_add)
        .ok_or_else(|| LcuError::Other("inventory de runas sem canAddCustomPage".to_owned()))
}

fn page_body(draft: &RunePageDraft) -> Value {
    json!({
        "name": draft.name,
        "primaryStyleId": draft.primary_style,
        "subStyleId": draft.sub_style,
        "selectedPerkIds": draft.perks,
        "current": true,
    })
}

fn parse_page(value: &Value) -> Result<ClientPage, LcuError> {
    let missing = |field: &str| LcuError::Other(format!("página de runas sem {field}"));
    let style = |field: &str| {
        value[field]
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| missing(field))
    };
    let perks = value["selectedPerkIds"]
        .as_array()
        .ok_or_else(|| missing("selectedPerkIds"))?
        .iter()
        .map(|perk| perk.as_u64().and_then(|perk| u32::try_from(perk).ok()))
        .collect::<Option<Vec<u32>>>()
        .and_then(|perks| <[u32; 9]>::try_from(perks).ok())
        .ok_or_else(|| LcuError::Other("página de runas sem 9 runas válidas".to_owned()))?;
    let name = value["name"]
        .as_str()
        .ok_or_else(|| missing("name"))?
        .to_owned();

    Ok(ClientPage {
        id: value["id"].as_u64().ok_or_else(|| missing("id"))?,
        editable: value["isEditable"].as_bool().unwrap_or(false),
        temporary: value["isTemporary"].as_bool().unwrap_or(true),
        current: value["current"].as_bool().unwrap_or(false),
        draft: RunePageDraft {
            name: name.clone(),
            primary_style: style("primaryStyleId")?,
            sub_style: style("subStyleId")?,
            perks,
        },
        name,
    })
}
