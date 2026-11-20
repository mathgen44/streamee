//! Réglages et pads, enregistrés dans streamee.json (dossier de données de l'application).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PadMode {
    /// Un nouvel appui relance le son du début.
    #[default]
    Restart,
    /// Un nouvel appui arrête le son (fondu court).
    Toggle,
    /// Un nouvel appui lance une lecture de plus, par-dessus.
    Overlap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pad {
    pub id: String,
    pub name: String,
    /// Nom du fichier dans le dossier « sounds » de Streamee.
    pub file: String,
    pub color: String,
    /// 0 à 1,5 (au-delà de 1 : amplification).
    pub volume: f32,
    pub mode: PadMode,
    pub looped: bool,
    /// Raccourci global, ex. « Ctrl+Alt+Digit1 » ; actif même quand le jeu a le focus.
    pub hotkey: Option<String>,
}

impl Default for Pad {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            file: String::new(),
            color: "#2e86de".into(),
            volume: 1.0,
            mode: PadMode::Restart,
            looped: false,
            hotkey: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Volume des sons envoyés dans le micro.
    pub inject_volume: f32,
    /// Volume du retour dans le casque.
    pub monitor_volume: f32,
    pub monitor_enabled: bool,
    /// Sortie du retour (None = sortie par défaut de Windows).
    pub monitor_device: Option<String>,
    /// Raccourci global qui coupe tous les sons.
    pub stop_all_hotkey: Option<String>,
    pub columns: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            inject_volume: 0.8,
            monitor_volume: 0.6,
            monitor_enabled: true,
            monitor_device: None,
            stop_all_hotkey: Some("Ctrl+Alt+Backspace".into()),
            columns: 4,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub pads: Vec<Pad>,
    pub settings: Settings,
}

impl Config {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Écriture atomique (fichier temporaire puis renommage) : pas de config à moitié écrite.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp: PathBuf = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| format!("Enregistrement impossible : {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("Enregistrement impossible : {e}"))
    }

    pub fn pad(&self, id: &str) -> Option<&Pad> {
        self.pads.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let c: Config = serde_json::from_str(r#"{"pads":[{"id":"a","name":"Air horn","mode":"toggle"}]}"#).unwrap();
        assert_eq!(c.pads[0].mode, PadMode::Toggle);
        assert_eq!(c.pads[0].volume, 1.0);
        assert_eq!(c.settings.columns, 4);
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.pads[0].name, "Air horn");
    }
}
