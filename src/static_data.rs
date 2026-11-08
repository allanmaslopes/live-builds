use crate::catalog::ItemCatalog;
use crate::knowledge::KnowledgeNames;
use crate::riot_build::parse_riot_recommendations;
use crate::riot_build::{RecommendedBuild, SkillOrder};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct StaticData {
    pub catalog: ItemCatalog,
    pub catalog_patch: String,
    pub stale: bool,
    pub knowledge_names: KnowledgeNames,
    champion_ids: HashMap<String, u32>,
}

impl StaticData {
    pub fn champion_id(&self, champion: &str) -> Option<u32> {
        self.champion_ids
            .get(&champion.to_ascii_lowercase())
            .copied()
    }
}

type FetchFn = dyn Fn(&str) -> Result<String, String> + Send + Sync;

pub struct StaticSource {
    cache_dir: PathBuf,
    fetch: Box<FetchFn>,
}

impl StaticSource {
    pub fn new(cache_dir: PathBuf) -> StaticSource {
        Self::with_fetcher(cache_dir, |url| {
            let mut response = ureq::get(url)
                .config()
                .timeout_global(Some(Duration::from_secs(10)))
                .build()
                .call()
                .map_err(|error| error.to_string())?;
            response
                .body_mut()
                .read_to_string()
                .map_err(|error| error.to_string())
        })
    }

    pub fn with_fetcher(
        cache_dir: PathBuf,
        f: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    ) -> StaticSource {
        StaticSource {
            cache_dir,
            fetch: Box::new(f),
        }
    }

    pub fn load(&self, game_version: &str) -> Result<StaticData, String> {
        let requested = short_version(game_version)
            .ok_or_else(|| format!("Invalid game version: {game_version}"))?;
        if let Some(catalog) = self.cached_catalog(Some(requested)) {
            return Ok(self.with_metadata(catalog, false));
        }

        let remote = (|| {
            let json = (self.fetch)("https://ddragon.leagueoflegends.com/api/versions.json")?;
            let versions: Vec<String> =
                serde_json::from_str(&json).map_err(|error| error.to_string())?;
            let patch = versions
                .into_iter()
                .filter(|version| {
                    patch_version(version)
                        .is_some_and(|(major, minor, _)| (major, minor) == requested)
                })
                .max_by_key(|version| patch_version(version))
                .ok_or_else(|| {
                    format!("No Data Dragon patch for {}.{}", requested.0, requested.1)
                })?;
            let json = (self.fetch)(&format!(
                "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/item.json"
            ))?;
            let catalog =
                ItemCatalog::from_ddragon_json(&patch, &json).map_err(|error| error.to_string())?;
            // Champion metadata must not prevent the item panel from working offline.
            let _ = self.cache_champions(&patch);
            write_atomic(&self.cache_dir.join(&patch).join("item.json"), &json)?;
            Ok::<_, String>(catalog)
        })();

        match remote {
            Ok(catalog) => Ok(self.with_metadata(catalog, false)),
            Err(error) => self
                .cached_catalog(None)
                .map(|catalog| self.with_metadata(catalog, true))
                .ok_or(error),
        }
    }

    pub fn load_exact(&self, patch: &str) -> Result<StaticData, String> {
        let item_path = self.cache_dir.join(patch).join("item.json");
        if let Ok(json) = fs::read_to_string(&item_path)
            && let Ok(catalog) = ItemCatalog::from_ddragon_json(patch, &json)
        {
            return Ok(self.with_metadata(catalog, false));
        }
        let json = (self.fetch)(&format!(
            "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/item.json"
        ))?;
        let catalog =
            ItemCatalog::from_ddragon_json(patch, &json).map_err(|error| error.to_string())?;
        write_atomic(&item_path, &json)?;
        Ok(self.with_metadata(catalog, false))
    }

    fn with_metadata(&self, catalog: ItemCatalog, stale: bool) -> StaticData {
        let patch = catalog.patch.clone();
        let _ = self.cache_champions(&patch);
        let _ = self.cache_runes(&patch);
        let _ = self.cache_summoner_spells(&patch);
        let champions = fs::read_to_string(self.cache_dir.join(&patch).join("champion.json"))
            .ok()
            .and_then(|json| parse_champions(&patch, &json));
        let champion_ids = champions.map_or_else(HashMap::new, |champions| {
            champions
                .data
                .into_values()
                .filter_map(|champion| {
                    let key = champion.key?.parse().ok()?;
                    Some((champion.name.to_ascii_lowercase(), key))
                })
                .collect()
        });
        let mut knowledge_names =
            fs::read_to_string(self.cache_dir.join(&patch).join("runesReforged.json"))
                .ok()
                .and_then(|json| parse_runes(&json))
                .unwrap_or_default();
        knowledge_names.summoner_spells =
            fs::read_to_string(self.cache_dir.join(&patch).join("summoner.json"))
                .ok()
                .and_then(|json| parse_summoner_spells(&patch, &json))
                .unwrap_or_default();
        StaticData {
            catalog_patch: patch,
            catalog,
            stale,
            knowledge_names,
            champion_ids,
        }
    }

    pub fn recommendations(
        &self,
        patch: &str,
        champion_alias: &str,
        map_id: u32,
        mode: &str,
        position: &str,
    ) -> (RecommendedBuild, Option<SkillOrder>) {
        let Some((major, minor, _)) = patch_version(patch) else {
            return (RecommendedBuild::default(), None);
        };
        let alias = fs::read_to_string(self.cache_dir.join(patch).join("champion.json"))
            .ok()
            .and_then(|json| parse_champions(patch, &json))
            .and_then(|champions| {
                champions
                    .data
                    .into_values()
                    .find(|champion| {
                        champion.id.eq_ignore_ascii_case(champion_alias)
                            || champion.name.eq_ignore_ascii_case(champion_alias)
                    })
                    .map(|champion| champion.id)
            })
            .unwrap_or_else(|| champion_alias.to_owned())
            .to_ascii_lowercase();
        if !valid_champion_alias(&alias) {
            return (RecommendedBuild::default(), None);
        }
        let short_patch = format!("{major}.{minor}");
        let path = self
            .cache_dir
            .join(patch)
            .join(format!("champion-{alias}.json"));
        if let Ok(json) = fs::read_to_string(&path)
            && valid_bin(&json)
        {
            return parse_riot_recommendations(&json, map_id, mode, position);
        }
        for version in [short_patch.as_str(), "latest"] {
            let url = format!(
                "https://raw.communitydragon.org/{version}/game/data/characters/{alias}/{alias}.bin.json"
            );
            if let Ok(json) = (self.fetch)(&url)
                && valid_bin(&json)
            {
                let _ = write_atomic(&path, &json);
                return parse_riot_recommendations(&json, map_id, mode, position);
            }
        }
        (RecommendedBuild::default(), None)
    }

    fn cache_champions(&self, patch: &str) -> Result<(), String> {
        let path = self.cache_dir.join(patch).join("champion.json");
        if let Ok(json) = fs::read_to_string(&path)
            && parse_champions(patch, &json).is_some()
        {
            return Ok(());
        }
        let json = (self.fetch)(&format!(
            "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/champion.json"
        ))?;
        parse_champions(patch, &json)
            .ok_or_else(|| "Invalid Data Dragon champion catalog".to_owned())?;
        write_atomic(&path, &json)
    }

    fn cache_runes(&self, patch: &str) -> Result<(), String> {
        let path = self.cache_dir.join(patch).join("runesReforged.json");
        if let Ok(json) = fs::read_to_string(&path)
            && parse_runes(&json).is_some()
        {
            return Ok(());
        }
        let json = (self.fetch)(&format!(
            "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/runesReforged.json"
        ))?;
        parse_runes(&json).ok_or_else(|| "Invalid Data Dragon rune catalog".to_owned())?;
        write_atomic(&path, &json)
    }

    fn cache_summoner_spells(&self, patch: &str) -> Result<(), String> {
        let path = self.cache_dir.join(patch).join("summoner.json");
        if let Ok(json) = fs::read_to_string(&path)
            && parse_summoner_spells(patch, &json).is_some()
        {
            return Ok(());
        }
        let json = (self.fetch)(&format!(
            "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/summoner.json"
        ))?;
        parse_summoner_spells(patch, &json)
            .ok_or_else(|| "Invalid Data Dragon summoner spell catalog".to_owned())?;
        write_atomic(&path, &json)
    }

    fn cached_catalog(&self, requested: Option<(u32, u32)>) -> Option<ItemCatalog> {
        fs::read_dir(&self.cache_dir)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let patch = name.to_str()?;
                let (major, minor, _) = patch_version(patch)?;
                if requested.is_some_and(|requested| requested != (major, minor)) {
                    return None;
                }
                let json = fs::read_to_string(entry.path().join("item.json")).ok()?;
                ItemCatalog::from_ddragon_json(patch, &json).ok()
            })
            .max_by_key(|catalog| patch_version(&catalog.patch))
    }
}

#[derive(Deserialize)]
struct DDragonChampions {
    #[serde(rename = "type")]
    kind: String,
    version: String,
    data: HashMap<String, DDragonChampion>,
}

#[derive(Deserialize)]
struct DDragonChampion {
    id: String,
    #[serde(default)]
    key: Option<String>,
    name: String,
}

#[derive(Deserialize)]
struct DDragonRuneStyle {
    id: u32,
    name: String,
    #[serde(default)]
    icon: String,
    slots: Vec<DDragonRuneSlot>,
}

#[derive(Deserialize)]
struct DDragonRuneSlot {
    runes: Vec<DDragonRune>,
}

#[derive(Deserialize)]
struct DDragonRune {
    id: u32,
    name: String,
    #[serde(default)]
    icon: String,
}

#[derive(Deserialize)]
struct DDragonSummonerSpells {
    #[serde(rename = "type")]
    kind: String,
    version: String,
    data: HashMap<String, DDragonSummonerSpell>,
}

#[derive(Deserialize)]
struct DDragonSummonerSpell {
    key: String,
    name: String,
}

fn parse_champions(patch: &str, json: &str) -> Option<DDragonChampions> {
    let champions: DDragonChampions = serde_json::from_str(json).ok()?;
    (champions.kind == "champion"
        && champions.version == patch
        && !champions.data.is_empty()
        && champions.data.iter().all(|(key, champion)| {
            key == &champion.id
                && valid_champion_alias(&champion.id)
                && !champion.name.trim().is_empty()
        }))
    .then_some(champions)
}

fn parse_runes(json: &str) -> Option<KnowledgeNames> {
    let styles: Vec<DDragonRuneStyle> = serde_json::from_str(json).ok()?;
    if styles.is_empty() {
        return None;
    }
    let mut names = KnowledgeNames::default();
    for style in styles {
        if !style.icon.is_empty() {
            names.style_icons.insert(style.id, style.icon);
        }
        names.styles.insert(style.id, style.name);
        for rune in style.slots.into_iter().flat_map(|slot| slot.runes) {
            if !rune.icon.is_empty() {
                names.rune_icons.insert(rune.id, rune.icon);
            }
            names.runes.insert(rune.id, rune.name);
        }
    }
    Some(names)
}

fn parse_summoner_spells(patch: &str, json: &str) -> Option<HashMap<u32, String>> {
    let spells: DDragonSummonerSpells = serde_json::from_str(json).ok()?;
    if spells.kind != "summoner" || spells.version != patch || spells.data.is_empty() {
        return None;
    }
    spells
        .data
        .into_values()
        .map(|spell| Some((spell.key.parse().ok()?, spell.name)))
        .collect()
}

fn valid_champion_alias(alias: &str) -> bool {
    !alias.is_empty() && alias.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn short_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn patch_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    let result = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(result)
}

fn valid_bin(json: &str) -> bool {
    matches!(
        serde_json::from_str::<serde_json::Value>(json),
        Ok(serde_json::Value::Object(_))
    )
}

fn write_atomic(path: &Path, json: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Cache path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, json).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{StaticSource, parse_runes};
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    const VERSIONS_URL: &str = "https://ddragon.leagueoflegends.com/api/versions.json";
    const THRESH: &str = include_str!("../tests/fixtures/thresh.bin.json");

    #[test]
    fn parses_rune_icon_paths() {
        let names = parse_runes(
            r#"[{"id":8000,"name":"Precisão","icon":"perk-images/Styles/Precision/Precision.png","slots":[{"runes":[{"id":8008,"name":"Ritmo Fatal","icon":"perk-images/Styles/Precision/LethalTempo/LethalTempo2.png"}]}]}]"#,
        )
        .unwrap();

        assert_eq!(
            names.style_icon(8000),
            Some("perk-images/Styles/Precision/Precision.png")
        );
        assert_eq!(
            names.rune_icon(8008),
            Some("perk-images/Styles/Precision/LethalTempo/LethalTempo2.png")
        );
    }

    fn fixture_source(
        cache_dir: &Path,
        patch: &str,
        calls: Arc<Mutex<Vec<String>>>,
    ) -> StaticSource {
        let patch = patch.to_owned();
        StaticSource::with_fetcher(cache_dir.to_path_buf(), move |url| {
            calls.lock().unwrap().push(url.to_owned());
            if url == VERSIONS_URL {
                return Ok(
                    serde_json::json!(["16.23.1", patch, "16.22.2", "16.22.1", "16.21.1"])
                        .to_string(),
                );
            }
            if url
                == format!("https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/item.json")
            {
                return Ok(
                    include_str!("../tests/fixtures/item_sample.json").replace("16.22.1", &patch)
                );
            }
            if url
                == format!(
                    "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/champion.json"
                )
            {
                return Ok(serde_json::json!({
                    "type": "champion",
                    "format": "standAloneComplex",
                    "version": patch,
                    "data": {
                        "MonkeyKing": {"id": "MonkeyKing", "name": "Wukong"},
                        "Thresh": {
                            "id": "Thresh",
                            "key": "412",
                            "name": "Thresh",
                            "title": "o Guardião das Correntes",
                            "tags": ["Support", "Tank"],
                            "partype": "Mana",
                            "image": {
                                "full": "Thresh.png", "sprite": "champion4.png",
                                "group": "champion", "x": 0, "y": 0, "w": 48, "h": 48
                            },
                            "info": {"attack": 5, "defense": 6, "magic": 6, "difficulty": 7},
                            "stats": {
                                "hp": 604, "hpperlevel": 120, "mp": 274, "mpperlevel": 44,
                                "movespeed": 330, "armor": 33, "armorperlevel": 0,
                                "spellblock": 30, "spellblockperlevel": 1.55,
                                "attackrange": 450, "hpregen": 7, "hpregenperlevel": 0.55,
                                "mpregen": 6, "mpregenperlevel": 0.8,
                                "crit": 0, "critperlevel": 0, "attackdamage": 56,
                                "attackdamageperlevel": 2.2, "attackspeedperlevel": 3.5,
                                "attackspeed": 0.625
                            }
                        }
                    }
                })
                .to_string());
            }
            if url
                == format!(
                    "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/runesReforged.json"
                )
            {
                return Ok(include_str!("../tests/fixtures/runesReforged-16.22.1.json").to_owned());
            }
            if url
                == format!(
                    "https://ddragon.leagueoflegends.com/cdn/{patch}/data/pt_BR/summoner.json"
                )
            {
                return Ok(include_str!("../tests/fixtures/summoner-16.22.1.json")
                    .replace("16.22.1", &patch));
            }
            if url
                == "https://raw.communitydragon.org/16.22/game/data/characters/thresh/thresh.bin.json"
                || url
                    == "https://raw.communitydragon.org/16.22/game/data/characters/monkeyking/monkeyking.bin.json"
            {
                return Ok(THRESH.to_owned());
            }
            Err(format!("404: {url}"))
        })
    }

    #[test]
    fn maps_game_version_to_patches() {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = fixture_source(cache.path(), "16.22.10", Arc::clone(&calls));

        let data = source.load("16.22.712.1234").unwrap();
        assert_eq!(data.catalog_patch, "16.22.10");
        assert_eq!(data.catalog.patch, "16.22.10");
        assert_eq!(data.catalog.name(3071), "Cutelo Negro");
        assert!(!data.stale);

        let (build, skills) =
            source.recommendations(&data.catalog_patch, "Thresh", 11, "CLASSIC", "utility");
        assert_eq!(build.starting, vec![vec![3865, 2003, 2003]]);
        assert_eq!(skills.unwrap().early, vec![0, 2, 1, 0]);

        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(|url| url == VERSIONS_URL));
        assert!(
            calls.iter().any(|url| url
                == "https://ddragon.leagueoflegends.com/cdn/16.22.10/data/pt_BR/item.json")
        );
        assert!(calls.iter().any(|url| url
            == "https://raw.communitydragon.org/16.22/game/data/characters/thresh/thresh.bin.json"));
    }

    #[test]
    fn loads_knowledge_names_and_champion_id() {
        let cache = tempfile::tempdir().unwrap();
        let source = fixture_source(cache.path(), "16.22.2", Arc::new(Mutex::new(Vec::new())));

        let data = source.load("16.22.712.1234").unwrap();

        assert_eq!(data.champion_id("Thresh"), Some(412));
        assert_eq!(data.knowledge_names.style(8000), "Precisão");
        assert_eq!(data.knowledge_names.rune(8008), "Ritmo Fatal");
        assert_eq!(data.knowledge_names.summoner_spell(4), "Flash");
        assert_eq!(data.knowledge_names.summoner_spell(21), "Barreira");
        assert!(cache.path().join("16.22.2/runesReforged.json").is_file());
        assert!(cache.path().join("16.22.2/summoner.json").is_file());
    }

    #[test]
    fn caches_catalog_on_disk() {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = fixture_source(cache.path(), "16.22.2", Arc::clone(&calls));
        let first = source.load("16.22.712.1234").unwrap();
        let item_path = cache.path().join("16.22.2/item.json");
        let cached_json = fs::read_to_string(&item_path).unwrap();
        let cached: serde_json::Value = serde_json::from_str(&cached_json).unwrap();

        assert_eq!(cached["data"]["3071"]["gold"]["total"], 3000);
        assert_eq!(first.catalog_patch, "16.22.2");
        assert_eq!(first.catalog.name(3071), "Cutelo Negro");
        assert!(!first.stale);
        assert!(!calls.lock().unwrap().is_empty());

        calls.lock().unwrap().clear();
        let source = fixture_source(cache.path(), "16.22.2", Arc::clone(&calls));
        let second = source.load("16.22.712.1234").unwrap();

        assert_eq!(second.catalog_patch, first.catalog_patch);
        assert_eq!(second.catalog.patch, first.catalog.patch);
        assert_eq!(second.catalog.name(3071), "Cutelo Negro");
        assert!(!second.stale);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn exact_load_replaces_older_micro_patch_cache() {
        let cache = tempfile::tempdir().unwrap();
        let old = cache.path().join("16.22.1");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join("item.json"),
            include_str!("../tests/fixtures/item_sample.json"),
        )
        .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = fixture_source(cache.path(), "16.22.2", Arc::clone(&calls));

        let loaded = source.load_exact("16.22.2").unwrap();

        assert_eq!(loaded.catalog_patch, "16.22.2");
        assert!(cache.path().join("16.22.2/item.json").is_file());
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|url| url.contains("/16.22.2/"))
        );
    }

    #[test]
    fn resolves_champion_name_to_official_id_from_cached_champions() {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = fixture_source(cache.path(), "16.22.2", Arc::clone(&calls));
        let data = source.load("16.22.712.1234").unwrap();
        let champion_path = cache.path().join("16.22.2/champion.json");
        let champions: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(champion_path).unwrap()).unwrap();
        assert_eq!(champions["data"]["MonkeyKing"]["name"], "Wukong");

        let (build, _) =
            source.recommendations(&data.catalog_patch, "Wukong", 11, "CLASSIC", "utility");
        assert_eq!(build.starting, vec![vec![3865, 2003, 2003]]);
        assert!(
            cache
                .path()
                .join("16.22.2/champion-monkeyking.json")
                .is_file()
        );
        assert!(!cache.path().join("16.22/champion-monkeyking.json").exists());
        assert!(calls.lock().unwrap().iter().any(|url| url
            == "https://raw.communitydragon.org/16.22/game/data/characters/monkeyking/monkeyking.bin.json"));

        calls.lock().unwrap().clear();
        let (cached_build, _) =
            source.recommendations(&data.catalog_patch, "MonkeyKing", 11, "CLASSIC", "utility");
        assert_eq!(cached_build.starting, build.starting);
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_champion_data_does_not_block_or_pollute_item_cache() {
        for champion_json in [
            None,
            Some("{}"),
            Some(r#"{"type":"champion","version":"16.22.2","data":{}}"#),
            Some(
                r#"{"type":"champion","version":"16.22.2","data":{"MonkeyKing":{"id":"../MonkeyKing","name":"Wukong"}}}"#,
            ),
        ] {
            let cache = tempfile::tempdir().unwrap();
            let source = StaticSource::with_fetcher(cache.path().to_path_buf(), move |url| {
                if url == VERSIONS_URL {
                    Ok(r#"["16.22.2"]"#.to_owned())
                } else if url.ends_with("/item.json") {
                    Ok(include_str!("../tests/fixtures/item_sample.json").to_owned())
                } else {
                    champion_json
                        .map(str::to_owned)
                        .ok_or_else(|| "404".to_owned())
                }
            });

            let data = source.load("16.22.712.1234").unwrap();
            assert_eq!(data.catalog.name(3071), "Cutelo Negro");
            assert!(!data.stale);
            assert!(cache.path().join("16.22.2/item.json").is_file());
            assert!(!cache.path().join("16.22.2/champion.json").exists());
        }
    }

    #[test]
    fn falls_back_to_cached_patch_when_remote_missing() {
        let cache = tempfile::tempdir().unwrap();
        for patch in ["16.21.1", "16.22.2"] {
            let source = fixture_source(cache.path(), patch, Arc::new(Mutex::new(Vec::new())));
            source
                .load(if patch == "16.21.1" {
                    "16.21.700.1234"
                } else {
                    "16.22.712.1234"
                })
                .unwrap();
        }
        let cached_json = fs::read_to_string(cache.path().join("16.22.2/item.json")).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fetch_calls = Arc::clone(&calls);
        let source = StaticSource::with_fetcher(cache.path().to_path_buf(), move |url| {
            fetch_calls.lock().unwrap().push(url.to_owned());
            if url == VERSIONS_URL {
                Ok(r#"["16.23.1","16.22.2","16.21.1"]"#.to_owned())
            } else {
                Err(format!("404: {url}"))
            }
        });

        let data = source.load("16.23.800.1234").unwrap();

        assert!(data.stale);
        assert_eq!(data.catalog_patch, "16.22.2");
        assert_eq!(data.catalog.patch, "16.22.2");
        assert_eq!(data.catalog.name(3071), "Cutelo Negro");
        assert_eq!(data.catalog.get(3071).unwrap().total_cost, 3000);
        assert_eq!(
            fs::read_to_string(cache.path().join("16.22.2/item.json")).unwrap(),
            cached_json
        );
        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(
            |url| url == "https://ddragon.leagueoflegends.com/cdn/16.23.1/data/pt_BR/item.json"
        ));
        assert!(!calls.iter().any(|url| url.contains("/cdn/16.22.2/")));
    }

    #[test]
    fn missing_build_is_empty_not_error() {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fetch_calls = Arc::clone(&calls);
        let source = StaticSource::with_fetcher(cache.path().to_path_buf(), move |url| {
            fetch_calls.lock().unwrap().push(url.to_owned());
            Err(format!("404: {url}"))
        });

        let (build, skills) =
            source.recommendations("16.22.1", "NewChampion", 11, "CLASSIC", "utility");

        assert!(build.starting.is_empty());
        assert!(build.stages.is_empty());
        assert!(skills.is_none());
        let calls = calls.lock().unwrap();
        assert!(calls.iter().any(|url| url
            == "https://raw.communitydragon.org/16.22/game/data/characters/newchampion/newchampion.bin.json"));
        assert!(calls.iter().any(|url| url
            == "https://raw.communitydragon.org/latest/game/data/characters/newchampion/newchampion.bin.json"));
    }
}
