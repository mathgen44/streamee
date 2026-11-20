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
    /// Case de la grille (ligne * colonnes + colonne). None = à placer dans la première case libre.
    pub slot: Option<u32>,
    /// Icône affichée sur le pad (un emoji).
    pub icon: Option<String>,
    /// Image affichée sur le pad : nom du fichier dans le dossier « images » de Streamee.
    pub image: Option<String>,
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
            slot: None,
            icon: None,
            image: None,
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
    pub rows: u32,
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
            rows: 3,
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

    pub fn capacity(&self) -> u32 {
        self.settings.columns * self.settings.rows
    }

    pub fn is_free(&self, slot: u32) -> bool {
        !self.pads.iter().any(|p| p.slot == Some(slot))
    }

    /// Première case libre à partir de `from` (puis depuis le début). Ajoute une ligne si tout est pris.
    pub fn free_slot(&mut self, from: u32) -> u32 {
        let cap = self.capacity();
        if let Some(s) = (from..cap).chain(0..from.min(cap)).find(|&s| self.is_free(s)) {
            return s;
        }
        self.settings.rows += 1;
        cap
    }

    /// Remet la grille en ordre : bornes des réglages, une case par pad, assez de lignes pour tous.
    pub fn normalize(&mut self) {
        self.settings.columns = self.settings.columns.clamp(1, 12);
        self.settings.rows = self.settings.rows.clamp(1, 30);
        let mut seen = std::collections::HashSet::new();
        for p in &mut self.pads {
            if let Some(s) = p.slot {
                if !seen.insert(s) {
                    p.slot = None; // case déjà prise : on le replace plus loin
                }
            }
        }
        for i in 0..self.pads.len() {
            if self.pads[i].slot.is_none() {
                let s = self.free_slot(0);
                self.pads[i].slot = Some(s);
            }
        }
        let cols = self.settings.columns;
        let needed = self.pads.iter().filter_map(|p| p.slot).max().map_or(1, |m| m / cols + 1);
        self.settings.rows = self.settings.rows.max(needed);
    }

    /// Déplace un pad sur une case ; s'il y en a déjà un, les deux échangent leur place.
    pub fn move_pad(&mut self, id: &str, slot: u32) {
        let Some(from) = self.pad(id).and_then(|p| p.slot) else { return };
        if let Some(other) = self.pads.iter_mut().find(|p| p.slot == Some(slot)) {
            other.slot = Some(from);
        }
        if let Some(p) = self.pads.iter_mut().find(|p| p.id == id) {
            p.slot = Some(slot);
        }
        self.normalize();
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

    fn pad(id: &str, slot: Option<u32>) -> Pad {
        Pad { id: id.into(), slot, ..Pad::default() }
    }

    #[test]
    fn normalize_places_pads_and_grows_rows() {
        let mut c = Config::default(); // 4 x 3
        c.pads = vec![pad("a", None), pad("b", Some(0)), pad("c", Some(0)), pad("d", Some(13))];
        c.normalize();
        let slot = |id: &str| c.pad(id).unwrap().slot.unwrap();
        assert_eq!(slot("b"), 0);
        assert_eq!(slot("d"), 13);
        assert_eq!((slot("a"), slot("c")), (1, 2)); // sans case, puis doublon
        assert_eq!(c.settings.rows, 4); // la case 13 est sur la 4e ligne
    }

    #[test]
    fn move_swaps_and_free_slot_adds_row() {
        let mut c = Config::default();
        c.settings.columns = 2;
        c.settings.rows = 1;
        c.pads = vec![pad("a", Some(0)), pad("b", Some(1))];
        c.move_pad("a", 1);
        assert_eq!((c.pad("a").unwrap().slot, c.pad("b").unwrap().slot), (Some(1), Some(0)));
        assert_eq!(c.free_slot(0), 2);
        assert_eq!(c.settings.rows, 2);
    }
}
