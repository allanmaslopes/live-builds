use crate::riot_build::ItemStage;
use crate::riot_build::{RecommendedBuild, SkillOrder};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePackage {
    pub format_version: u32,
    pub patch: String,
    pub entries: Vec<KnowledgeEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeEntry {
    pub patch: String,
    pub champion_id: u32,
    pub position: String,
    pub games: u32,
    pub wins: u32,
    pub win_rate: f64,
    pub low_sample: bool,
    pub starting: Vec<SequenceStats>,
    pub boots: Vec<SequenceStats>,
    pub build_paths: Vec<SequenceStats>,
    #[serde(default)]
    pub full_build_paths: Vec<FullBuildPath>,
    pub skill_orders: Vec<SequenceStats>,
    pub skill_priority: String,
    pub rune_pages: Vec<RunePage>,
    pub primary: Option<PrimaryRecommendation>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceStats {
    pub sequence: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullBuildPath {
    pub sequence: Vec<u32>,
    #[serde(default)]
    pub low_sample: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunePage {
    pub keystone: u32,
    pub primary_style: u32,
    pub sub_style: u32,
    pub perks: Vec<u32>,
    pub stat_perks: Option<StatPerks>,
    #[serde(default)]
    pub games: u32,
    #[serde(default)]
    pub win_rate: f64,
    pub low_sample: bool,
    pub build_path: Option<SequenceStats>,
    #[serde(default)]
    pub build_path_low_sample: bool,
    #[serde(default)]
    pub full_build_path: Option<FullBuildPath>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StatPerks {
    pub offense: u32,
    pub flex: u32,
    pub defense: u32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryRecommendation {
    pub keystone: u32,
    pub summoner_spells: Vec<u32>,
    pub summoner_spells_low_sample: bool,
}

#[derive(Clone, Copy)]
pub struct Selected<'a> {
    pub package: &'a KnowledgePackage,
    pub entry: &'a KnowledgeEntry,
}

pub struct LoadedPackage {
    pub package: KnowledgePackage,
    pub stale: bool,
}

#[derive(Clone, Debug, Default)]
pub struct KnowledgeNames {
    pub runes: HashMap<u32, String>,
    pub styles: HashMap<u32, String>,
    pub rune_icons: HashMap<u32, String>,
    pub style_icons: HashMap<u32, String>,
    pub summoner_spells: HashMap<u32, String>,
}

impl KnowledgeNames {
    pub fn rune(&self, id: u32) -> String {
        self.runes
            .get(&id)
            .cloned()
            .or_else(|| stat_perk_name(id).map(str::to_owned))
            .unwrap_or_else(|| id.to_string())
    }

    pub fn style(&self, id: u32) -> String {
        self.styles
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    pub fn rune_icon(&self, id: u32) -> Option<&str> {
        self.rune_icons
            .get(&id)
            .map(String::as_str)
            .or_else(|| stat_perk_icon(id))
    }

    pub fn style_icon(&self, id: u32) -> Option<&str> {
        self.style_icons.get(&id).map(String::as_str)
    }

    pub fn summoner_spell(&self, id: u32) -> String {
        self.summoner_spells
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
}

fn stat_perk_icon(id: u32) -> Option<&'static str> {
    match id {
        5001 => Some("perk-images/StatMods/StatModsHealthScalingIcon.png"),
        5005 => Some("perk-images/StatMods/StatModsAttackSpeedIcon.png"),
        5007 => Some("perk-images/StatMods/StatModsCDRScalingIcon.png"),
        5008 => Some("perk-images/StatMods/StatModsAdaptiveForceIcon.png"),
        5010 => Some("perk-images/StatMods/StatModsMovementSpeedIcon.png"),
        5011 => Some("perk-images/StatMods/StatModsHealthPlusIcon.png"),
        5013 => Some("perk-images/StatMods/StatModsTenacityIcon.png"),
        _ => None,
    }
}

pub fn stat_perk_name(id: u32) -> Option<&'static str> {
    match id {
        5001 => Some("Vida escalável"),
        5005 => Some("Velocidade de ataque"),
        5007 => Some("Aceleração de habilidade"),
        5008 => Some("Força adaptativa"),
        5010 => Some("Velocidade de movimento"),
        5011 => Some("Vida"),
        5013 => Some("Tenacidade e Lentidão"),
        _ => None,
    }
}

#[cfg(test)]
mod icon_tests {
    use super::KnowledgeNames;

    #[test]
    fn shard_icon_paths_match_ddragon() {
        assert_eq!(
            KnowledgeNames::default().rune_icon(5008),
            Some("perk-images/StatMods/StatModsAdaptiveForceIcon.png")
        );
        assert_eq!(
            KnowledgeNames::default().rune_icon(5013),
            Some("perk-images/StatMods/StatModsTenacityIcon.png")
        );
    }
}

#[derive(Debug)]
pub enum FetchResult {
    Fresh { body: String, etag: Option<String> },
    NotModified,
}

type FetchFn = dyn Fn(&str, Option<&str>) -> Result<FetchResult, String> + Send + Sync;

pub struct KnowledgeSource {
    base_url: String,
    cache_dir: PathBuf,
    fetch: Box<FetchFn>,
}

impl KnowledgeSource {
    pub fn new(base_url: String, cache_dir: PathBuf) -> KnowledgeSource {
        Self::with_fetcher(base_url, cache_dir, |url, etag| {
            let mut request = ureq::get(url)
                .config()
                .timeout_global(Some(Duration::from_secs(10)))
                .build();
            if let Some(etag) = etag {
                request = request.header("If-None-Match", etag);
            }
            let mut response = request.call().map_err(|error| error.to_string())?;
            if response.status().as_u16() == 304 {
                return Ok(FetchResult::NotModified);
            }
            let etag = response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|error| error.to_string())?;
            Ok(FetchResult::Fresh { body, etag })
        })
    }

    pub fn with_fetcher(
        base_url: String,
        cache_dir: PathBuf,
        fetch: impl Fn(&str, Option<&str>) -> Result<FetchResult, String> + Send + Sync + 'static,
    ) -> KnowledgeSource {
        KnowledgeSource {
            base_url: base_url.trim_end_matches('/').to_owned(),
            cache_dir,
            fetch: Box::new(fetch),
        }
    }

    pub fn load(&self, patch: &str) -> Result<LoadedPackage, String> {
        let json_path = self.cache_dir.join(format!("knowledge-{patch}.json"));
        let etag_path = self.cache_dir.join(format!("knowledge-{patch}.etag"));
        let etag = fs::read_to_string(&etag_path).ok();
        let url = format!("{}/processor/knowledge/{patch}", self.base_url);
        match (self.fetch)(&url, etag.as_deref()) {
            Ok(FetchResult::Fresh { body, etag }) => {
                store_fresh(&json_path, &etag_path, patch, body, etag).or_else(|error| {
                    read_cached(&json_path, patch)
                        .map(|package| LoadedPackage {
                            package,
                            stale: true,
                        })
                        .map_err(|_| error)
                })
            }
            Ok(FetchResult::NotModified) => match read_cached(&json_path, patch) {
                Ok(package) => Ok(LoadedPackage {
                    package,
                    stale: false,
                }),
                Err(_) => {
                    let _ = fs::remove_file(&etag_path);
                    match (self.fetch)(&url, None)? {
                        FetchResult::Fresh { body, etag } => {
                            store_fresh(&json_path, &etag_path, patch, body, etag)
                        }
                        FetchResult::NotModified => {
                            Err("Processor returned 304 without an ETag".to_owned())
                        }
                    }
                }
            },
            Err(error) => read_cached(&json_path, patch)
                .map(|package| LoadedPackage {
                    package,
                    stale: true,
                })
                .map_err(|_| error),
        }
    }

    pub fn previous_patch(&self, current: &str) -> Result<Option<String>, String> {
        let current = patch_number(current)
            .ok_or_else(|| format!("Invalid current knowledge patch: {current}"))?;
        let url = format!("{}/processor/status", self.base_url);
        let body = match (self.fetch)(&url, None)? {
            FetchResult::Fresh { body, .. } => body,
            FetchResult::NotModified => {
                return Err("Processor status unexpectedly returned 304".to_owned());
            }
        };
        let status: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| error.to_string())?;
        Ok(status["patches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry["patch"].as_str())
            .filter_map(|patch| Some((patch_number(patch)?, patch)))
            .filter(|(number, _)| *number < current)
            .max_by_key(|(number, _)| *number)
            .map(|(_, patch)| patch.to_owned()))
    }
}

fn patch_number(patch: &str) -> Option<(u32, u32)> {
    let (major, minor) = patch.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn store_fresh(
    json_path: &Path,
    etag_path: &Path,
    patch: &str,
    body: String,
    etag: Option<String>,
) -> Result<LoadedPackage, String> {
    let package = parse_package(&body, patch)?;
    write_atomic(json_path, &body)?;
    if let Some(etag) = etag {
        write_atomic(etag_path, &etag)?;
    } else {
        let _ = fs::remove_file(etag_path);
    }
    Ok(LoadedPackage {
        package,
        stale: false,
    })
}

fn parse_package(json: &str, patch: &str) -> Result<KnowledgePackage, String> {
    let package: KnowledgePackage =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    if package.format_version != 2 || package.patch != patch {
        return Err(format!("Invalid knowledge package for patch {patch}"));
    }
    Ok(package)
}

fn read_cached(path: &Path, patch: &str) -> Result<KnowledgePackage, String> {
    let json = fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse_package(&json, patch)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Cache path has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

pub fn select_entry<'a>(
    pkg: &'a KnowledgePackage,
    champion_id: u32,
    position: &str,
    prev_pkg: Option<&'a KnowledgePackage>,
) -> Option<Selected<'a>> {
    let find = |package: &'a KnowledgePackage, wanted: &str| {
        package.entries.iter().find(|entry| {
            entry.champion_id == champion_id && entry.position.eq_ignore_ascii_case(wanted)
        })
    };
    find(pkg, position)
        .or_else(|| find(pkg, "ANY"))
        .map(|entry| Selected {
            package: pkg,
            entry,
        })
        .or_else(|| {
            let package = prev_pkg?;
            find(package, position)
                .or_else(|| find(package, "ANY"))
                .map(|entry| Selected { package, entry })
        })
}

pub fn select_page(entry: &KnowledgeEntry, player_keystone: Option<u32>) -> Option<&RunePage> {
    player_keystone
        .and_then(|keystone| {
            entry
                .rune_pages
                .iter()
                .find(|page| page.keystone == keystone)
        })
        .or_else(|| {
            let keystone = entry.primary.as_ref()?.keystone;
            entry
                .rune_pages
                .iter()
                .find(|page| page.keystone == keystone)
        })
}

pub fn to_recommendations(
    entry: &KnowledgeEntry,
    page: Option<&RunePage>,
) -> (RecommendedBuild, Option<SkillOrder>) {
    let starting = entry
        .starting
        .first()
        .map(|sequence| vec![sequence.sequence.clone()])
        .unwrap_or_default();
    let path = page
        .and_then(|page| page.build_path.as_ref())
        .or_else(|| entry.build_paths.first());
    let mut stages = Vec::new();
    if let Some(item) = path.and_then(|path| path.sequence.first()) {
        stages.push(stage(*item));
    }
    if let Some(item) = entry.boots.first().and_then(|boots| boots.sequence.first()) {
        stages.push(stage(*item));
    }
    if let Some(path) = path {
        stages.extend(path.sequence.iter().skip(1).take(2).copied().map(stage));
    }
    let mut completed = path.map_or(0, |path| path.sequence.len().min(3));
    if let Some(full_path) = selected_full_build_path(entry, page) {
        for &item in &full_path.sequence {
            if completed == 6 {
                break;
            }
            if !stages.iter().any(|stage| stage.items.contains(&item)) {
                stages.push(stage(item));
                completed += 1;
            }
        }
    }
    let build = RecommendedBuild { starting, stages };

    let priority: Vec<u8> = entry
        .skill_priority
        .split('>')
        .filter_map(|skill| skill_index(skill.trim()))
        .collect();
    let early: Vec<u8> = entry
        .skill_orders
        .first()
        .into_iter()
        .flat_map(|order| order.sequence.iter())
        .filter_map(|skill| u8::try_from(skill.saturating_sub(1)).ok())
        .filter(|skill| *skill < 4)
        .collect();
    let skills = (!priority.is_empty()).then_some(SkillOrder { priority, early });
    (build, skills)
}

pub fn selected_full_build_path<'a>(
    entry: &'a KnowledgeEntry,
    page: Option<&'a RunePage>,
) -> Option<&'a FullBuildPath> {
    page.and_then(|page| page.full_build_path.as_ref())
        .or_else(|| entry.full_build_paths.first())
}

fn stage(item: u32) -> ItemStage {
    ItemStage {
        items: vec![item],
        max_completed: None,
    }
}

fn skill_index(skill: &str) -> Option<u8> {
    match skill {
        "Q" => Some(0),
        "W" => Some(1),
        "E" => Some(2),
        "R" => Some(3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};

    const PACKAGE: &str = include_str!("../tests/fixtures/knowledge-16.22.json");

    fn package() -> KnowledgePackage {
        serde_json::from_str(PACKAGE).unwrap()
    }

    #[test]
    fn uses_cache_on_304() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("knowledge-16.22.json"), PACKAGE).unwrap();
        fs::write(cache.path().join("knowledge-16.22.etag"), "W/\"cached\"").unwrap();
        let seen = Arc::new(Mutex::new(None));
        let called = Arc::clone(&seen);
        let source = KnowledgeSource::with_fetcher(
            "http://processor".to_owned(),
            cache.path().to_path_buf(),
            move |url, etag| {
                *called.lock().unwrap() = Some((url.to_owned(), etag.map(str::to_owned)));
                Ok(FetchResult::NotModified)
            },
        );

        let loaded = source.load("16.22").unwrap();

        assert_eq!(loaded.package.patch, "16.22");
        assert!(!loaded.stale);
        assert_eq!(
            *seen.lock().unwrap(),
            Some((
                "http://processor/processor/knowledge/16.22".to_owned(),
                Some("W/\"cached\"".to_owned())
            ))
        );
    }

    #[test]
    fn uses_cache_when_offline_and_marks_stale() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("knowledge-16.22.json"), PACKAGE).unwrap();
        let source = KnowledgeSource::with_fetcher(
            "http://processor/".to_owned(),
            cache.path().to_path_buf(),
            |_, _| Err("offline".to_owned()),
        );

        let loaded = source.load("16.22").unwrap();

        assert_eq!(loaded.package.entries.len(), 5);
        assert!(loaded.stale);
    }

    #[test]
    fn invalid_fresh_response_keeps_valid_cache() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("knowledge-16.22.json"), PACKAGE).unwrap();
        let source = KnowledgeSource::with_fetcher(
            "http://processor".to_owned(),
            cache.path().to_path_buf(),
            |_, _| {
                Ok(FetchResult::Fresh {
                    body: "not json".to_owned(),
                    etag: Some("W/\"invalid\"".to_owned()),
                })
            },
        );

        let loaded = source.load("16.22").unwrap();

        assert_eq!(loaded.package.entries.len(), 5);
        assert!(loaded.stale);
        assert_eq!(
            fs::read_to_string(cache.path().join("knowledge-16.22.json")).unwrap(),
            PACKAGE
        );
    }

    #[test]
    fn second_fresh_download_replaces_cache() {
        let cache = tempfile::tempdir().unwrap();
        let second = PACKAGE.replacen("\"games\": 110", "\"games\": 111", 1);
        let responses = Arc::new(Mutex::new(vec![second.clone(), PACKAGE.to_owned()]));
        let queued = Arc::clone(&responses);
        let source = KnowledgeSource::with_fetcher(
            "http://processor".to_owned(),
            cache.path().to_path_buf(),
            move |_, _| {
                Ok(FetchResult::Fresh {
                    body: queued.lock().unwrap().pop().unwrap(),
                    etag: None,
                })
            },
        );

        assert_eq!(source.load("16.22").unwrap().package.entries[0].games, 110);
        assert_eq!(source.load("16.22").unwrap().package.entries[0].games, 111);
        let cached = fs::read_to_string(cache.path().join("knowledge-16.22.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<KnowledgePackage>(&cached)
                .unwrap()
                .entries[0]
                .games,
            111
        );
    }

    #[test]
    fn refetches_without_etag_when_cache_corrupt() {
        let cache = tempfile::tempdir().unwrap();
        fs::write(cache.path().join("knowledge-16.22.json"), "not json").unwrap();
        fs::write(cache.path().join("knowledge-16.22.etag"), "W/\"cached\"").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::clone(&seen);
        let source = KnowledgeSource::with_fetcher(
            "http://processor".to_owned(),
            cache.path().to_path_buf(),
            move |_, etag| {
                calls.lock().unwrap().push(etag.map(str::to_owned));
                if etag.is_some() {
                    Ok(FetchResult::NotModified)
                } else {
                    Ok(FetchResult::Fresh {
                        body: PACKAGE.to_owned(),
                        etag: None,
                    })
                }
            },
        );

        let loaded = source.load("16.22").unwrap();

        assert_eq!(loaded.package.patch, "16.22");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![Some("W/\"cached\"".to_owned()), None]
        );
        assert!(!cache.path().join("knowledge-16.22.etag").exists());
    }

    #[test]
    fn select_entry_prefers_position_then_any_then_previous_patch() {
        let full = package();
        let bottom = full
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap()
            .clone();
        let any = full
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "ANY")
            .unwrap()
            .clone();
        let previous = KnowledgePackage {
            format_version: 2,
            patch: "16.21".to_owned(),
            entries: vec![bottom.clone(), any.clone()],
        };
        let current_position = KnowledgePackage {
            format_version: 2,
            patch: "16.22".to_owned(),
            entries: vec![any.clone(), bottom],
        };
        assert_eq!(
            select_entry(&current_position, 145, "BOTTOM", Some(&previous))
                .unwrap()
                .entry
                .position,
            "BOTTOM"
        );

        let current_any = KnowledgePackage {
            format_version: 2,
            patch: "16.22".to_owned(),
            entries: vec![any],
        };
        assert_eq!(
            select_entry(&current_any, 145, "BOTTOM", Some(&previous))
                .unwrap()
                .package
                .patch,
            "16.22"
        );

        let empty = KnowledgePackage {
            format_version: 2,
            patch: "16.22".to_owned(),
            entries: vec![],
        };
        let selected = select_entry(&empty, 145, "BOTTOM", Some(&previous)).unwrap();
        assert_eq!(selected.package.patch, "16.21");
        assert_eq!(selected.entry.position, "BOTTOM");
        assert!(select_entry(&empty, 999, "BOTTOM", Some(&previous)).is_none());
    }

    #[test]
    fn select_page_matches_player_keystone() {
        let pkg = package();
        let entry = pkg
            .entries
            .iter()
            .find(|entry| entry.champion_id == 1)
            .unwrap();

        assert_eq!(select_page(entry, Some(8229)).unwrap().keystone, 8229);
    }

    #[test]
    fn select_page_falls_back_to_primary() {
        let pkg = package();
        let entry = pkg
            .entries
            .iter()
            .find(|entry| entry.champion_id == 1)
            .unwrap();

        assert_eq!(select_page(entry, Some(9999)).unwrap().keystone, 8112);
        assert_eq!(select_page(entry, None).unwrap().keystone, 8112);
    }

    #[test]
    fn converts_entry_to_recommended_build() {
        let pkg = package();
        let entry = pkg
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap();
        let page = select_page(entry, None);

        let (build, skills) = to_recommendations(entry, page);

        assert_eq!(build.starting, vec![vec![1086, 2003, 2003]]);
        assert_eq!(
            build
                .stages
                .iter()
                .map(|stage| stage.items.as_slice())
                .collect::<Vec<_>>(),
            vec![
                &[6672][..],
                &[1001][..],
                &[3124][..],
                &[3302][..],
                &[3006][..],
                &[3032][..],
                &[3036][..]
            ]
        );
        assert!(
            build
                .stages
                .iter()
                .all(|stage| stage.max_completed.is_none())
        );
        let skills = skills.unwrap();
        assert_eq!(skills.priority, vec![3, 0, 2, 1]);
        assert_eq!(skills.early, vec![0, 1, 2, 0, 0, 3, 0, 2, 0]);
    }

    #[test]
    fn full_build_extends_stages_to_six_items() {
        let pkg = package();
        let entry = pkg
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap();

        let (build, _) = to_recommendations(entry, select_page(entry, None));

        assert_eq!(
            build
                .stages
                .iter()
                .flat_map(|stage| stage.items.iter())
                .copied()
                .collect::<Vec<_>>(),
            vec![6672, 1001, 3124, 3302, 3006, 3032, 3036]
        );
    }

    #[test]
    fn full_build_skips_items_already_in_path() {
        let mut document: serde_json::Value = serde_json::from_str(PACKAGE).unwrap();
        let entry = document["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["championId"] == 145 && entry["position"] == "BOTTOM")
            .unwrap();
        entry["runePages"][0]["fullBuildPath"]["sequence"] =
            serde_json::json!([3124, 6672, 3302, 3032, 3036, 3032]);
        let package: KnowledgePackage = serde_json::from_value(document).unwrap();
        let entry = package
            .entries
            .iter()
            .find(|entry| entry.position == "BOTTOM")
            .unwrap();

        let (build, _) = to_recommendations(entry, select_page(entry, None));

        assert_eq!(
            build
                .stages
                .iter()
                .flat_map(|stage| stage.items.iter())
                .copied()
                .collect::<Vec<_>>(),
            vec![6672, 1001, 3124, 3302, 3032, 3036]
        );
    }

    #[test]
    fn old_package_without_full_build_still_works() {
        let mut document: serde_json::Value = serde_json::from_str(PACKAGE).unwrap();
        for entry in document["entries"].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("fullBuildPaths");
            for page in entry["runePages"].as_array_mut().unwrap() {
                page.as_object_mut().unwrap().remove("fullBuildPath");
            }
        }
        let package: KnowledgePackage = serde_json::from_value(document).unwrap();
        let entry = package
            .entries
            .iter()
            .find(|entry| entry.champion_id == 145 && entry.position == "BOTTOM")
            .unwrap();

        let (build, _) = to_recommendations(entry, select_page(entry, None));

        assert_eq!(
            build
                .stages
                .iter()
                .flat_map(|stage| stage.items.iter())
                .copied()
                .collect::<Vec<_>>(),
            vec![6672, 1001, 3124, 3302]
        );
    }
}
