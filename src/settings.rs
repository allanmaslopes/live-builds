use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub auto_runes: bool,
    #[serde(default)]
    pub auto_spells: bool,
    #[serde(default)]
    pub auto_item_set: bool,
    #[serde(skip)]
    pub corrupted: bool,
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|_| Self {
                corrupted: true,
                ..Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self {
                corrupted: true,
                ..Self::default()
            },
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("falha ao criar pasta das configurações: {error}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("falha ao serializar configurações: {error}"))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, json)
            .map_err(|error| format!("falha ao salvar configurações: {error}"))?;
        if let Err(error) = fs::rename(&temporary, path) {
            if !path.exists() {
                let _ = fs::remove_file(&temporary);
                return Err(format!("falha ao salvar configurações: {error}"));
            }
            fs::remove_file(path)
                .map_err(|error| format!("falha ao substituir configurações: {error}"))?;
            fs::rename(&temporary, path)
                .map_err(|error| format!("falha ao salvar configurações: {error}"))?;
        }
        Ok(())
    }
}
