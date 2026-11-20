//! Streamee — interface légère pour streamers.
//! Première fonction : un pad de sons qui partent directement dans le micro.

mod config;
mod decode;
mod engine;
mod inject;
mod mixer;
mod monitor;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use config::{Config, Pad, Settings};
use engine::Engine;
use inject::InjectStatus;
use monitor::MonitorCmd;

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "oga", "m4a", "aac", "mp4", "aif", "aiff", "caf"];
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];
const COLORS: &[&str] = &["#2e86de", "#1f9d55", "#d98a1c", "#d64541", "#8e44ad", "#16a085", "#c0392b", "#f1c40f"];

struct AppState {
    engine: Arc<Mutex<Engine>>,
    config: Mutex<Config>,
    inject_status: Arc<Mutex<InjectStatus>>,
    monitor: Mutex<Sender<MonitorCmd>>,
    config_path: PathBuf,
    sounds_dir: PathBuf,
    images_dir: PathBuf,
    /// Pads dont le son n'a pas pu être chargé, avec la raison.
    load_errors: Mutex<HashMap<String, String>>,
    /// Raccourcis refusés au dernier enregistrement (déjà pris par un autre logiciel, etc.).
    hotkey_errors: Mutex<Vec<String>>,
    running: Arc<AtomicBool>,
}

impl AppState {
    fn save(&self, cfg: &Config) -> Result<(), String> {
        cfg.save(&self.config_path)
    }
}

fn new_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    format!("{t:x}{:x}", N.fetch_add(1, Ordering::Relaxed))
}

fn apply_settings(engine: &mut Engine, s: &Settings) {
    engine.inject_volume = s.inject_volume.clamp(0.0, 2.0);
    engine.monitor_volume = s.monitor_volume.clamp(0.0, 2.0);
    engine.monitor_enabled = s.monitor_enabled;
}

fn load_sound(state: &AppState, pad: &Pad) {
    match decode::decode_file(&state.sounds_dir.join(&pad.file)) {
        Ok(sound) => {
            state.engine.lock().insert_sound(&pad.id, Arc::new(sound));
            state.load_errors.lock().remove(&pad.id);
        }
        Err(e) => {
            state.load_errors.lock().insert(pad.id.clone(), e);
        }
    }
}

fn trigger_pad(state: &AppState, id: &str) {
    let pad = state.config.lock().pad(id).cloned();
    if let Some(pad) = pad {
        state.engine.lock().trigger(&pad);
    }
}

/// (Ré)enregistre tous les raccourcis globaux. Renvoie les raccourcis refusés.
fn register_hotkeys(app: &AppHandle) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let cfg = app.state::<AppState>().config.lock().clone();
    let mut errors = Vec::new();
    let mut taken: Vec<String> = Vec::new();

    let mut bind = |hotkey: &str, action: Option<String>| {
        let key = hotkey.to_lowercase();
        if taken.contains(&key) {
            errors.push(format!("{hotkey} : utilisé deux fois"));
            return;
        }
        let res = gs.on_shortcut(hotkey, move |app, _sc, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            let state = app.state::<AppState>();
            match &action {
                Some(id) => trigger_pad(&state, id),
                None => state.engine.lock().stop(None),
            }
        });
        match res {
            Ok(()) => taken.push(key),
            Err(e) => errors.push(format!("{hotkey} : {e}")),
        }
    };


    if let Some(h) = cfg.settings.stop_all_hotkey.as_deref().filter(|h| !h.is_empty()) {
        bind(h, None);
    }
    for pad in &cfg.pads {
        if let Some(h) = pad.hotkey.as_deref().filter(|h| !h.is_empty()) {
            bind(h, Some(pad.id.clone()));
        }
    }
    *app.state::<AppState>().hotkey_errors.lock() = errors.clone();
    errors
}

// ------------------------------------------------------------------ Commandes

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateDto {
    config: Config,
    devices: Vec<String>,
    durations: HashMap<String, f64>,
    /// Dossier des images des pads (l'interface en fait des URL asset://).
    images_dir: String,
    load_errors: HashMap<String, String>,
    hotkey_errors: Vec<String>,
}

#[tauri::command]
fn get_state(state: State<AppState>) -> StateDto {
    let config = state.config.lock().clone();
    let engine = state.engine.lock();
    let durations = config.pads.iter().filter_map(|p| engine.duration(&p.id).map(|d| (p.id.clone(), d))).collect();
    drop(engine);
    StateDto {
        config,
        devices: monitor::output_devices(),
        durations,
        images_dir: state.images_dir.to_string_lossy().to_string(),
        load_errors: state.load_errors.lock().clone(),
        hotkey_errors: state.hotkey_errors.lock().clone(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddResult {
    added: usize,
    errors: Vec<String>,
}

fn is_ext(path: &Path, list: &[&str]) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    list.contains(&ext.as_str())
}

/// Ajoute des sons, à partir de la case `slot` (puis dans les cases libres suivantes).
fn add_files(state: &AppState, paths: Vec<PathBuf>, slot: Option<u32>) -> AddResult {
    let mut errors = Vec::new();
    let mut added = 0;
    let mut next_slot = slot.unwrap_or(0);
    for path in paths {
        let label = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
            errors.push(format!("{label} : ce n'est pas un fichier audio"));
            continue;
        }
        let sound = match decode::decode_file(&path) {
            Ok(s) => s,
            Err(e) => {
                errors.push(format!("{label} : {e}"));
                continue;
            }
        };
        let id = new_id();
        let file = format!("{id}.{ext}");
        // On garde une copie : le pad marche encore si l'original est déplacé ou supprimé.
        if let Err(e) = std::fs::copy(&path, state.sounds_dir.join(&file)) {
            errors.push(format!("{label} : copie impossible ({e})"));
            continue;
        }
        let mut cfg = state.config.lock();
        let at = cfg.free_slot(next_slot);
        next_slot = at + 1;
        let pad = Pad {
            id: id.clone(),
            name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Son".into()),
            file,
            color: COLORS[cfg.pads.len() % COLORS.len()].into(),
            slot: Some(at),
            ..Pad::default()
        };
        cfg.pads.push(pad);
        if let Err(e) = state.save(&cfg) {
            errors.push(e);
        }
        drop(cfg);
        state.engine.lock().insert_sound(&id, Arc::new(sound));
        added += 1;
    }
    AddResult { added, errors }
}

#[tauri::command]
async fn pick_sounds(app: AppHandle, slot: Option<u32>) -> AddResult {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Ajouter des sons")
        .add_filter("Sons", AUDIO_EXTENSIONS)
        .blocking_pick_files();
    let paths: Vec<PathBuf> = picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect();
    add_files(&app.state::<AppState>(), paths, slot)
}

#[tauri::command]
async fn add_sounds(app: AppHandle, paths: Vec<String>, slot: Option<u32>) -> AddResult {
    add_files(&app.state::<AppState>(), paths.into_iter().map(PathBuf::from).collect(), slot)
}

/// Copie une image dans le dossier de Streamee et la pose sur le pad (remplace l'ancienne).
fn apply_pad_image(state: &AppState, id: &str, path: Option<&Path>) -> Result<(), String> {
    let new_file = match path {
        Some(path) => {
            if !is_ext(path, IMAGE_EXTENSIONS) {
                return Err("Ce n'est pas une image (PNG, JPG, GIF, WebP, SVG…)".into());
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
            // Nom unique : le navigateur ne garde pas en cache l'ancienne image.
            let file = format!("{id}-{}.{ext}", new_id());
            std::fs::copy(path, state.images_dir.join(&file)).map_err(|e| format!("Copie de l'image impossible : {e}"))?;
            Some(file)
        }
        None => None,
    };
    let mut cfg = state.config.lock();
    let Some(pad) = cfg.pads.iter_mut().find(|p| p.id == id) else {
        if let Some(f) = new_file {
            let _ = std::fs::remove_file(state.images_dir.join(f));
        }
        return Err("Pad introuvable".into());
    };
    let old = std::mem::replace(&mut pad.image, new_file);
    state.save(&cfg)?;
    if let Some(old) = old {
        let _ = std::fs::remove_file(state.images_dir.join(old));
    }
    Ok(())
}

#[tauri::command]
fn set_pad_image(state: State<AppState>, id: String, path: Option<String>) -> Result<(), String> {
    apply_pad_image(&state, &id, path.as_deref().map(Path::new))
}

#[tauri::command]
async fn pick_pad_image(app: AppHandle, id: String) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Image du pad")
        .add_filter("Images", IMAGE_EXTENSIONS)
        .blocking_pick_file();
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    apply_pad_image(&app.state::<AppState>(), &id, Some(&path))?;
    Ok(true)
}

#[tauri::command]
fn update_pad(app: AppHandle, state: State<AppState>, pad: Pad) -> Result<Vec<String>, String> {
    let mut cfg = state.config.lock();
    let slot = cfg.pads.iter_mut().find(|p| p.id == pad.id).ok_or("Pad introuvable")?;
    let mut pad = pad;
    // Fichier, case et image ont leurs propres commandes.
    pad.file = slot.file.clone();
    pad.slot = slot.slot;
    pad.image = slot.image.clone();
    pad.volume = pad.volume.clamp(0.0, 1.5);
    pad.icon = pad.icon.map(|i| i.trim().chars().take(8).collect::<String>()).filter(|i| !i.is_empty());
    *slot = pad.clone();
    state.save(&cfg)?;
    drop(cfg);
    state.engine.lock().set_pad_volume(&pad.id, pad.volume);
    Ok(register_hotkeys(&app))
}

#[tauri::command]
fn delete_pad(app: AppHandle, state: State<AppState>, id: String) -> Result<(), String> {
    let mut cfg = state.config.lock();
    let Some(idx) = cfg.pads.iter().position(|p| p.id == id) else { return Ok(()) };
    let pad = cfg.pads.remove(idx);
    state.save(&cfg)?;
    drop(cfg);
    state.engine.lock().remove_sound(&id);
    let _ = std::fs::remove_file(state.sounds_dir.join(&pad.file));
    if let Some(img) = &pad.image {
        let _ = std::fs::remove_file(state.images_dir.join(img));
    }
    register_hotkeys(&app);
    Ok(())
}

#[tauri::command]
fn move_pad(state: State<AppState>, id: String, slot: u32) -> Result<(), String> {
    let mut cfg = state.config.lock();
    cfg.move_pad(&id, slot);
    state.save(&cfg)
}

#[tauri::command]
fn play_pad(state: State<AppState>, id: String) {
    trigger_pad(&state, &id);
}

#[tauri::command]
fn stop_pad(state: State<AppState>, id: String) {
    state.engine.lock().stop(Some(&id));
}

#[tauri::command]
fn stop_all(state: State<AppState>) {
    state.engine.lock().stop(None);
}

#[tauri::command]
fn set_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> Result<Vec<String>, String> {
    let mut cfg = state.config.lock();
    let device_changed = cfg.settings.monitor_device != settings.monitor_device;
    let hotkey_changed = cfg.settings.stop_all_hotkey != settings.stop_all_hotkey;
    cfg.settings = settings;
    cfg.normalize(); // les lignes ne descendent pas sous le dernier pad
    let settings = cfg.settings.clone();
    state.save(&cfg)?;
    drop(cfg);
    apply_settings(&mut state.engine.lock(), &settings);
    if device_changed {
        let _ = state.monitor.lock().send(MonitorCmd::SetDevice(settings.monitor_device));
    }
    Ok(if hotkey_changed { register_hotkeys(&app) } else { Vec::new() })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusDto {
    playing: Vec<(String, f32)>,
    inject: InjectStatus,
    /// Micro sur lequel le filtre Streamee est posé (None = pas installé).
    apo_endpoint: Option<String>,
}

#[tauri::command]
fn status(state: State<AppState>) -> StatusDto {
    StatusDto {
        playing: state.engine.lock().progress(),
        inject: state.inject_status.lock().clone(),
        apo_endpoint: apo_endpoint(),
    }
}

#[cfg(windows)]
fn apo_endpoint() -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Streamee")
        .and_then(|k| k.get_value::<String, _>("EndpointName"))
        .ok()
}
#[cfg(not(windows))]
fn apo_endpoint() -> Option<String> {
    None
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureDevice {
    id: String,
    name: String,
}

/// Micros actifs, lus là où Windows les range (lisible sans droits administrateur).
/// Même logique que Get-CaptureEndpoints dans install-apo.ps1.
#[cfg(windows)]
#[tauri::command]
fn capture_devices() -> Vec<CaptureDevice> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    const ROOT: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Capture";
    const DESC: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
    const IFACE: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";
    let Ok(root) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(ROOT, KEY_READ) else {
        return Vec::new();
    };
    let mut out: Vec<CaptureDevice> = root
        .enum_keys()
        .flatten()
        .filter_map(|id| {
            let ep = root.open_subkey_with_flags(&id, KEY_READ).ok()?;
            if ep.get_value::<u32, _>("DeviceState").ok()? != 1 {
                return None; // débranché ou désactivé
            }
            let props = ep.open_subkey_with_flags("Properties", KEY_READ).ok()?;
            let desc: String = props.get_value(DESC).unwrap_or_else(|_| "Micro".into());
            let name = match props.get_value::<String, _>(IFACE) {
                Ok(iface) if !iface.is_empty() => format!("{desc} ({iface})"),
                _ => desc,
            };
            Some(CaptureDevice { id, name })
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}
#[cfg(not(windows))]
#[tauri::command]
fn capture_devices() -> Vec<CaptureDevice> {
    Vec::new()
}

/// Identifiant de micro Windows : « {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx} ».
fn is_endpoint_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && b[1..37].iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Lance install-apo.ps1 ou uninstall-apo.ps1 en administrateur (Windows demande la confirmation).
#[tauri::command]
fn run_apo_script(app: AppHandle, uninstall: bool, endpoint: Option<String>) -> Result<(), String> {
    let name = if uninstall { "uninstall-apo.ps1" } else { "install-apo.ps1" };
    let script = app
        .path()
        .resolve(format!("apo/{name}"), tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;
    if !script.exists() {
        return Err(format!("Script introuvable : {}", script.display()));
    }
    // Tauri renvoie parfois « \\?\D:\... » : PowerShell 5.1 gère mal ce format, on l'enlève.
    let script = match script.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => script,
    };
    let endpoint = endpoint.filter(|_| !uninstall);
    if let Some(id) = &endpoint {
        if !is_endpoint_id(id) {
            return Err(format!("Identifiant de micro invalide : {id}"));
        }
    }
    launch_elevated(&script, endpoint.as_deref())
}

#[cfg(windows)]
fn launch_elevated(script: &Path, endpoint: Option<&str>) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Les valeurs passent par des variables d'environnement : aucun souci de guillemets ou d'espaces.
    // -FromApp : la fenêtre se ferme seule si tout va bien, et reste ouverte sur l'erreur sinon.
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "$a = @('-NoProfile','-ExecutionPolicy','Bypass','-File',('\"' + $env:STREAMEE_SCRIPT + '\"'),'-FromApp'); \
             if ($env:STREAMEE_ENDPOINT) { $a += @('-EndpointId', $env:STREAMEE_ENDPOINT) }; \
             Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList $a",
        ])
        .env("STREAMEE_SCRIPT", script)
        .env("STREAMEE_ENDPOINT", endpoint.unwrap_or(""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Lancement impossible : {e}"))
}
#[cfg(not(windows))]
fn launch_elevated(_script: &Path, _endpoint: Option<&str>) -> Result<(), String> {
    Err("L'injection dans le micro n'existe que sous Windows.".into())
}

// ------------------------------------------------------------------ Démarrage

/// Supprime les images qui ne sont plus utilisées par aucun pad.
fn clean_images(dir: &Path, cfg: &Config) {
    let used: std::collections::HashSet<&str> = cfg.pads.iter().filter_map(|p| p.image.as_deref()).collect();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if !used.contains(entry.file_name().to_string_lossy().as_ref()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let sounds_dir = data_dir.join("sounds");
            let images_dir = data_dir.join("images");
            std::fs::create_dir_all(&sounds_dir)?;
            std::fs::create_dir_all(&images_dir)?;
            let config_path = data_dir.join("streamee.json");
            let mut config = Config::load(&config_path);
            config.normalize(); // anciennes configs sans cases : les pads gardent leur ordre
            let _ = config.save(&config_path);
            clean_images(&images_dir, &config);
            // L'interface affiche les images par le protocole asset://, limité à ce dossier.
            let _ = app.asset_protocol_scope().allow_directory(&images_dir, false);

            let mut engine = Engine::new();
            apply_settings(&mut engine, &config.settings);
            let engine = Arc::new(Mutex::new(engine));
            let inject_status = Arc::new(Mutex::new(InjectStatus::default()));
            let running = Arc::new(AtomicBool::new(true));

            let monitor = monitor::spawn(engine.clone(), config.settings.monitor_device.clone());
            inject::spawn_writer(engine.clone(), inject_status.clone(), running.clone());

            let pads = config.pads.clone();
            app.manage(AppState {
                engine,
                config: Mutex::new(config),
                inject_status,
                monitor: Mutex::new(monitor),
                config_path,
                sounds_dir,
                images_dir,
                load_errors: Mutex::new(HashMap::new()),
                hotkey_errors: Mutex::new(Vec::new()),
                running,
            });
            register_hotkeys(app.handle());

            // Les sons se chargent en arrière-plan : la fenêtre s'ouvre tout de suite.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let state = handle.state::<AppState>();
                for pad in &pads {
                    load_sound(&state, pad);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                window.state::<AppState>().running.store(false, Ordering::Relaxed);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state, pick_sounds, add_sounds, set_pad_image, pick_pad_image, update_pad, delete_pad, move_pad, play_pad, stop_pad, stop_all,
            set_settings, status, capture_devices, run_apo_script
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Streamee");
}

#[cfg(test)]
mod tests {
    use super::is_endpoint_id;

    #[test]
    fn endpoint_id_is_validated() {
        assert!(is_endpoint_id("{0a1b2c3d-4e5f-6789-abcd-ef0123456789}"));
        assert!(!is_endpoint_id("0a1b2c3d-4e5f-6789-abcd-ef0123456789"));
        assert!(!is_endpoint_id("{0a1b2c3d-4e5f-6789-abcd-ef012345678'}"));
        assert!(!is_endpoint_id("{0a1b2c3d-4e5f-6789-abcd-ef0123456789}; rm"));
    }
}
