//! Envoi des sons vers l'APO posé sur le micro, via le tampon partagé
//! C:\ProgramData\Streamee\inject.shm. Disposition : apo/src/shared.h (à garder identique).

use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use memmap2::MmapMut;
use parking_lot::Mutex;

use crate::engine::Engine;

pub const SHM_PATH: &str = r"C:\ProgramData\Streamee\inject.shm";
const MAGIC: u32 = 0x4D52_5453;
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 4096;
pub const RING_FRAMES: usize = 65536;
const SHM_BYTES: usize = HEADER_BYTES + RING_FRAMES * 2 * 4;

/// Avance de l'écriture sur la lecture de l'APO : petit = moins de retard, trop petit = coupures.
const LEAD_MS: u32 = 30;
/// Au-delà, on considère que plus aucun logiciel n'écoute le micro.
const HEARTBEAT_TIMEOUT_MS: u64 = 400;
const CHUNK_FRAMES: usize = 256;

/// Vue typée sur le fichier projeté.
pub struct Shm {
    map: MmapMut,
}

impl Shm {
    pub fn open(path: &Path) -> Option<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
        if file.metadata().ok()?.len() < SHM_BYTES as u64 {
            return None;
        }
        // SAFETY : le fichier est partagé avec l'APO, qui n'y accède que par des opérations
        // atomiques sur l'en-tête et en lecture seule sur l'anneau ; on fait de même.
        let map = unsafe { MmapMut::map_mut(&file).ok()? };
        let shm = Self { map };
        if shm.u32_at(0).load(Ordering::Acquire) != MAGIC || shm.u32_at(4).load(Ordering::Acquire) != VERSION {
            return None;
        }
        Some(shm)
    }

    fn u32_at(&self, offset: usize) -> &AtomicU32 {
        // SAFETY : offset aligné sur 4 dans une projection alignée sur une page.
        unsafe { &*(self.map.as_ptr().add(offset) as *const AtomicU32) }
    }
    fn i64_at(&self, offset: usize) -> &AtomicI64 {
        // SAFETY : offset aligné sur 8 dans une projection alignée sur une page.
        unsafe { &*(self.map.as_ptr().add(offset) as *const AtomicI64) }
    }

    pub fn apo_rate(&self) -> u32 { self.u32_at(8).load(Ordering::Acquire) }
    pub fn apo_channels(&self) -> u32 { self.u32_at(12).load(Ordering::Acquire) }
    pub fn heartbeat(&self) -> u64 { self.i64_at(16).load(Ordering::Acquire) as u64 }
    pub fn read_frame(&self) -> i64 { self.i64_at(24).load(Ordering::Acquire) }
    pub fn write_frame(&self) -> i64 { self.i64_at(32).load(Ordering::Acquire) }
    pub fn publish_write_frame(&self, f: i64) { self.i64_at(32).store(f, Ordering::Release) }

    /// Copie des trames stéréo dans l'anneau à partir de la trame absolue `at`.
    pub fn write_ring(&mut self, at: i64, frames: &[f32]) {
        let ring = &mut self.map[HEADER_BYTES..];
        let n = frames.len() / 2;
        let mut idx = (at as usize) & (RING_FRAMES - 1);
        for f in 0..n {
            let off = idx * 8;
            ring[off..off + 4].copy_from_slice(&frames[f * 2].to_le_bytes());
            ring[off + 4..off + 8].copy_from_slice(&frames[f * 2 + 1].to_le_bytes());
            idx = (idx + 1) & (RING_FRAMES - 1);
        }
    }

    #[cfg(test)]
    fn ring_frame(&self, at: i64) -> (f32, f32) {
        let off = HEADER_BYTES + ((at as usize) & (RING_FRAMES - 1)) * 8;
        let b = &self.map[off..off + 8];
        (f32::from_le_bytes(b[0..4].try_into().unwrap()), f32::from_le_bytes(b[4..8].try_into().unwrap()))
    }
}

/// État de l'injection, lu par l'interface.
#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectStatus {
    /// Le tampon partagé existe (script d'installation passé).
    pub ready: bool,
    /// Un logiciel utilise le micro en ce moment : les sons y partent.
    pub active: bool,
    pub rate: u32,
    pub channels: u32,
}

#[cfg(windows)]
pub fn now_ms() -> u64 {
    // Même horloge que GetTickCount64() côté APO (commune à tous les processus).
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}
#[cfg(not(windows))]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

/// Calcule une passe d'écriture. Renvoie la nouvelle position d'écriture.
/// Séparé de la boucle pour être testable sans APO.
pub fn pump(shm: &mut Shm, engine: &Mutex<Engine>, mut write_at: i64, scratch: &mut Vec<f32>) -> i64 {
    let read_at = shm.read_frame();
    if write_at < read_at {
        write_at = read_at; // l'APO nous a doublés (pause, saccade) : on se recale
    }
    let lead = (shm.apo_rate().max(8000) * LEAD_MS / 1000) as i64;
    let target = read_at + lead;
    while write_at < target {
        let n = ((target - write_at) as usize).min(CHUNK_FRAMES);
        scratch.clear();
        scratch.resize(n * 2, 0.0);
        engine.lock().render_inject(scratch);
        shm.write_ring(write_at, scratch);
        write_at += n as i64;
        shm.publish_write_frame(write_at);
    }
    write_at
}

/// Fil d'envoi vers le micro. Tourne tant que `running` est vrai.
pub fn spawn_writer(engine: Arc<Mutex<Engine>>, status: Arc<Mutex<InjectStatus>>, running: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("streamee-inject".into())
        .spawn(move || {
            #[cfg(windows)]
            unsafe {
                // Minuterie à 1 ms, sinon sleep() arrondit à ~15,6 ms.
                windows_sys::Win32::Media::timeBeginPeriod(1);
            }
            let mut shm: Option<Shm> = None;
            let mut was_active = false;
            let mut write_at: i64 = 0;
            let mut rate = 0u32;
            let mut scratch = Vec::with_capacity(CHUNK_FRAMES * 2);
            let mut retry_in = 0u32;

            while running.load(Ordering::Relaxed) {
                if shm.is_none() {
                    if retry_in == 0 {
                        shm = Shm::open(Path::new(SHM_PATH));
                        retry_in = 50; // ~1 s
                    } else {
                        retry_in -= 1;
                    }
                    if shm.is_none() {
                        *status.lock() = InjectStatus::default();
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                }
                let s = shm.as_mut().unwrap();
                let hb = s.heartbeat();
                let active = hb != 0 && now_ms().saturating_sub(hb) < HEARTBEAT_TIMEOUT_MS;
                {
                    let mut st = status.lock();
                    st.ready = true;
                    st.active = active;
                    st.rate = s.apo_rate();
                    st.channels = s.apo_channels();
                }
                if !active {
                    if was_active {
                        engine.lock().clear_inject();
                    }
                    was_active = false;
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                let r = s.apo_rate();
                if !was_active || r != rate {
                    rate = r;
                    engine.lock().set_inject_rate(rate);
                    write_at = s.write_frame().max(s.read_frame());
                    was_active = true;
                }
                write_at = pump(s, &engine, write_at, &mut scratch);
                thread::sleep(Duration::from_millis(2));
            }
        })
        .expect("fil d'injection");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::Sound;

    fn make_shm(dir: &Path) -> Shm {
        let path = dir.join("inject.shm");
        let mut bytes = vec![0u8; SHM_BYTES];
        bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&VERSION.to_le_bytes());
        bytes[8..12].copy_from_slice(&48000u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        Shm::open(&path).unwrap()
    }

    #[test]
    fn rejects_missing_or_bad_file() {
        let dir = tempdir();
        assert!(Shm::open(&dir.join("absent")).is_none());
        std::fs::write(dir.join("bad"), vec![0u8; SHM_BYTES]).unwrap();
        assert!(Shm::open(&dir.join("bad")).is_none());
    }

    #[test]
    fn pump_keeps_lead_and_writes_sound() {
        let dir = tempdir();
        let mut shm = make_shm(&dir);
        let engine = Mutex::new(Engine::new());
        {
            let mut e = engine.lock();
            e.set_inject_rate(48000);
            e.insert_sound("pad", Arc::new(Sound { rate: 48000, data: vec![0.5; 48000 * 2] }));
            e.trigger_for_test("pad");
        }
        let mut scratch = Vec::new();
        // L'APO est à la trame 1000 : on doit écrire jusqu'à 1000 + 30 ms.
        shm.i64_at(24).store(1000, Ordering::Release);
        let w = pump(&mut shm, &engine, 0, &mut scratch);
        assert_eq!(w, 1000 + 1440);
        assert_eq!(shm.write_frame(), w);
        let (l, r) = shm.ring_frame(1000);
        assert!(l > 0.3 && r > 0.3, "le son doit être dans l'anneau ({l}, {r})");
        // Rien de plus tant que l'APO n'avance pas.
        assert_eq!(pump(&mut shm, &engine, w, &mut scratch), w);
        // Anneau : écriture au-delà de la fin qui revient au début.
        shm.i64_at(24).store(RING_FRAMES as i64 - 10, Ordering::Release);
        let w2 = pump(&mut shm, &engine, w, &mut scratch);
        assert_eq!(w2, RING_FRAMES as i64 - 10 + 1440);
        assert!(shm.ring_frame(RING_FRAMES as i64 + 5).0 > 0.3);
    }

    fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("streamee-test-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn rand_suffix() -> u64 {
        use std::sync::atomic::AtomicU64;
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}
