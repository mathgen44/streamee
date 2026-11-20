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
const COLORS: &[&str] = &["#2e86de", "#1f9d55", "#d98a1c", "#d64541", "#8e44ad", "#16a085", "#c0392b", "#f1c40f"];

struct AppState {
    engine: Arc<Mutex<Engine>>,
    config: Mutex<Config>,
    inject_status: Arc<Mutex<InjectStatus>>,
    monitor: Mutex<Sender<MonitorCmd>>,
    config_path: PathBuf,
    sounds_dir: PathBuf,
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

fn add_files(state: &AppState, paths: Vec<PathBuf>) -> AddResult {
    let mut errors = Vec::new();
    let mut added = 0;
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
        let pad = Pad {
            id: id.clone(),
            name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Son".into()),
            file,
            color: COLORS[cfg.pads.len() % COLORS.len()].into(),
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
async fn pick_sounds(app: AppHandle) -> AddResult {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Ajouter des sons")
        .add_filter("Sons", AUDIO_EXTENSIONS)
        .blocking_pick_files();
    let paths: Vec<PathBuf> = picked.unwrap_or_default().into_iter().filter_map(|p| p.into_path().ok()).collect();
    add_files(&app.state::<AppState>(), paths)
}

#[tauri::command]
async fn add_sounds(app: AppHandle, paths: Vec<String>) -> AddResult {
    add_files(&app.state::<AppState>(), paths.into_iter().map(PathBuf::from).collect())
}

#[tauri::command]
fn update_pad(app: AppHandle, state: State<AppState>, pad: Pad) -> Result<Vec<String>, String> {
    let mut cfg = state.config.lock();
    let slot = cfg.pads.iter_mut().find(|p| p.id == pad.id).ok_or("Pad introuvable")?;
    let mut pad = pad;
    pad.file = slot.file.clone(); // le fichier ne se change pas depuis l'interface
    pad.volume = pad.volume.clamp(0.0, 1.5);
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
    register_hotkeys(&app);
    Ok(())
}

#[tauri::command]
fn move_pad(state: State<AppState>, id: String, to: usize) -> Result<(), String> {
    let mut cfg = state.config.lock();
    let Some(from) = cfg.pads.iter().position(|p| p.id == id) else { return Ok(()) };
    let pad = cfg.pads.remove(from);
    let to = to.min(cfg.pads.len());
    cfg.pads.insert(to, pad);
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
    cfg.settings = settings.clone();
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

/// Lance install-apo.ps1 ou uninstall-apo.ps1 en administrateur (Windows demande la confirmation).
#[tauri::command]
fn run_apo_script(app: AppHandle, uninstall: bool) -> Result<(), String> {
    let name = if uninstall { "uninstall-apo.ps1" } else { "install-apo.ps1" };
    let script = app
        .path()
        .resolve(format!("apo/{name}"), tauri::path::BaseDirectory::Resource)
        .map_err(|e| e.to_string())?;
    if !script.exists() {
        return Err(format!("Script introuvable : {}", script.display()));
    }
    launch_elevated(&script)
}

#[cfg(windows)]
fn launch_elevated(script: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Le chemin passe par une variable d'environnement : aucun souci de guillemets ou d'espaces.
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-NoExit','-File',('\"' + $env:STREAMEE_SCRIPT + '\"'))",
        ])
        .env("STREAMEE_SCRIPT", script)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Lancement impossible : {e}"))
}
#[cfg(not(windows))]
fn launch_elevated(_script: &Path) -> Result<(), String> {
    Err("L'injection dans le micro n'existe que sous Windows.".into())
}

// ------------------------------------------------------------------ Démarrage

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
            std::fs::create_dir_all(&sounds_dir)?;
            let config_path = data_dir.join("streamee.json");
            let config = Config::load(&config_path);

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
            get_state, pick_sounds, add_sounds, update_pad, delete_pad, move_pad, play_pad, stop_pad, stop_all,
            set_settings, status, run_apo_script
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Streamee");
}
