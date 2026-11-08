use live_builds::lcu::{LcuClient, LcuError, LcuHttp, find_lockfile};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::{env, fs};

fn anonymize(value: &mut Value) {
    let mut set_number = 0;
    fn visit(value: &mut Value, set_number: &mut u32) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    match key.as_str() {
                        "summonerId" | "accountId" | "obfuscatedSummonerId" | "gameId" => {
                            *value = json!(1)
                        }
                        "puuid" | "obfuscatedPuuid" => *value = json!("1"),
                        // Ids de chat/sessão: identificam a partida real.
                        "channelClaim" | "multiUserChatId" | "multiUserChatPassword" => {
                            *value = json!("00000000-0000-0000-0000-000000000000")
                        }
                        // Só ids no formato UUID; ids de item nos arsenais também são texto ("6676").
                        "id" if value.as_str().is_some_and(|id| {
                            id.len() == 36 && id.chars().filter(|c| *c == '-').count() == 4
                        }) =>
                        {
                            *value = json!("00000000-0000-0000-0000-000000000000")
                        }
                        "uid" => {
                            *set_number += 1;
                            *value = json!(format!("set-{set_number}"));
                        }
                        "timestamp"
                        | "lastModified"
                        | "profileIconId"
                        | "summonerLevel"
                        | "xpSinceLastLevel"
                        | "xpUntilNextLevel"
                        | "percentCompleteForNextLevel" => *value = json!(0),
                        "tagLine" => *value = json!("BR1"),
                        "gameName" | "displayName" | "internalName" | "summonerName" => {
                            *value = json!("JogadorX")
                        }
                        _ => visit(value, set_number),
                    }
                }
            }
            Value::Array(values) => values.iter_mut().for_each(|value| visit(value, set_number)),
            _ => {}
        }
    }
    visit(value, &mut set_number);
}

fn arguments() -> (Option<PathBuf>, Option<String>, bool) {
    let (mut lockfile, mut suffix, mut reanonymize) = (None, None, false);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lcu-lockfile" => lockfile = args.next().map(PathBuf::from),
            "--suffix" => suffix = args.next(),
            "--reanonymize" => reanonymize = true,
            _ => {}
        }
    }
    (lockfile, suffix, reanonymize)
}

fn write_result(output: &Path, filename: &str, value: &Value) -> Result<usize, String> {
    let mut value = value.clone();
    anonymize(&mut value);
    let bytes = serde_json::to_vec_pretty(&value).map_err(|error| error.to_string())?;
    let destination = output.join(filename);
    let temporary = output.join(format!(".{filename}.tmp"));
    fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
    Ok(bytes.len())
}

fn probe(
    client: &LcuClient,
    output: &Path,
    path: &str,
    filename: &str,
) -> Result<Option<Value>, String> {
    let (status, value, result) = match client.get(path) {
        Ok(value) => (200, value.clone(), Some(value)),
        Err(LcuError::Http(status, message)) => (status, json!({"error": message}), None),
        Err(error) => return Err(format!("{path}: {error}")),
    };
    let bytes = write_result(output, filename, &value)?;
    println!("{path} -> HTTP {status}, {bytes} bytes");
    Ok(result)
}

fn reanonymize(output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(output)? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let value = serde_json::from_slice(&fs::read(&path)?)?;
            write_result(
                output,
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("nome invalido")?,
                &value,
            )?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (explicit, suffix, do_reanonymize) = arguments();
    let output = Path::new("tests/fixtures/lcu");
    fs::create_dir_all(output)?;
    if do_reanonymize {
        return reanonymize(output);
    }
    let lockfile = find_lockfile(explicit.as_deref()).ok_or("lockfile da LCU nao encontrado")?;
    let client = LcuClient::connect(&lockfile)?;
    probe(
        &client,
        output,
        "/lol-gameflow/v1/gameflow-phase",
        "gameflow-phase.json",
    )?;
    let summoner = probe(
        &client,
        output,
        "/lol-summoner/v1/current-summoner",
        "current-summoner.json",
    )?;
    let filename = suffix
        .map(|suffix| format!("champ-select-session-{suffix}.json"))
        .unwrap_or_else(|| "champ-select-session.json".to_owned());
    probe(&client, output, "/lol-champ-select/v1/session", &filename)?;
    for (path, filename) in [
        ("/lol-perks/v1/pages", "perks-pages.json"),
        ("/lol-perks/v1/currentpage", "perks-currentpage.json"),
        ("/lol-perks/v1/styles", "perks-styles.json"),
        (
            "/lol-perks/v1/customizationlimits",
            "perks-customizationlimits.json",
        ),
        ("/lol-perks/v1/inventory", "perks-inventory.json"),
    ] {
        probe(&client, output, path, filename)?;
    }
    if let Some(id) = summoner
        .as_ref()
        .and_then(|value| value.get("summonerId"))
        .and_then(|id| {
            id.as_u64()
                .map(|id| id.to_string())
                .or_else(|| id.as_str().map(str::to_owned))
        })
    {
        println!("/lol-item-sets/v1/item-sets/{{summonerId}}/sets -> consultando");
        probe(
            &client,
            output,
            &format!("/lol-item-sets/v1/item-sets/{id}/sets"),
            "item-sets.json",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures_have_no_identity_fields() {
        fn check(value: &Value) {
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        match key.as_str() {
                            "summonerId" | "accountId" => {
                                assert!(value == &json!(1) || value == &json!("1"))
                            }
                            "uid" => {
                                assert!(value.as_str().is_some_and(|id| id.starts_with("set-")))
                            }
                            "timestamp"
                            | "lastModified"
                            | "profileIconId"
                            | "summonerLevel"
                            | "xpSinceLastLevel"
                            | "xpUntilNextLevel"
                            | "percentCompleteForNextLevel" => assert_eq!(value, &json!(0)),
                            "puuid" => assert_eq!(value, &json!("1")),
                            "gameName" | "displayName" | "internalName" | "summonerName" => {
                                assert_eq!(value, &json!("JogadorX"))
                            }
                            "tagLine" => assert_eq!(value, &json!("BR1")),
                            _ => check(value),
                        }
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        check(value);
                    }
                }
                _ => {}
            }
        }
        for entry in fs::read_dir("tests/fixtures/lcu").unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "json") {
                check(&serde_json::from_slice(&fs::read(path).unwrap()).unwrap());
            }
        }
    }
    #[test]
    fn anonymizes_item_set_fields() {
        let mut value = json!({"uid":"x", "timestamp":2, "profileIconId":3});
        anonymize(&mut value);
        assert_eq!(
            value,
            json!({"uid":"set-1", "timestamp":0, "profileIconId":0})
        );
    }
}
