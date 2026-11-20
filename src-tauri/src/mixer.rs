//! Mixeur de sons : une liste de voix lues à la fréquence de sortie, avec rééchantillonnage linéaire.
//! Deux mixeurs tournent en parallèle : un pour le micro (APO), un pour le retour casque.

use std::sync::Arc;

/// Son décodé : stéréo entrelacé, float32, à sa fréquence d'origine.
pub struct Sound {
    pub rate: u32,
    pub data: Vec<f32>,
}

impl Sound {
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }
}

struct Voice {
    pad_id: String,
    sound: Arc<Sound>,
    pos: f64,    // position dans le son, en trames source
    gain: f32,
    fade: f32,   // multiplicateur de fondu courant
    fade_step: f32, // < 0 pendant un fondu de sortie, par trame de sortie
    looped: bool,
}

impl Voice {
    fn stopping(&self) -> bool {
        self.fade_step < 0.0
    }
}

pub struct Mixer {
    rate: u32,
    voices: Vec<Voice>,
}

impl Mixer {
    pub fn new(rate: u32) -> Self {
        Self { rate: rate.max(1), voices: Vec::with_capacity(32) }
    }

    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
    }

    pub fn start(&mut self, pad_id: &str, sound: Arc<Sound>, gain: f32, looped: bool) {
        self.voices.push(Voice {
            pad_id: pad_id.to_string(),
            sound,
            pos: 0.0,
            gain,
            fade: 1.0,
            fade_step: 0.0,
            looped,
        });
    }

    /// Fondu de sortie des voix d'un pad (ou de toutes si `pad_id` vaut None).
    pub fn fade_out(&mut self, pad_id: Option<&str>, secs: f32) {
        let frames = (secs * self.rate as f32).max(1.0);
        for v in self.voices.iter_mut() {
            if pad_id.is_none_or(|id| v.pad_id == id) && !v.stopping() {
                v.fade_step = -v.fade / frames;
            }
        }
    }

    pub fn set_gain(&mut self, pad_id: &str, gain: f32) {
        for v in self.voices.iter_mut().filter(|v| v.pad_id == pad_id) {
            v.gain = gain;
        }
    }

    pub fn clear(&mut self) {
        self.voices.clear();
    }

    pub fn is_playing(&self, pad_id: &str) -> bool {
        self.voices.iter().any(|v| v.pad_id == pad_id && !v.stopping())
    }

    pub fn is_empty(&self) -> bool {
        self.voices.is_empty()
    }

    /// (pad, progression 0..1) de la voix la plus récente de chaque pad en cours.
    pub fn progress(&self) -> Vec<(String, f32)> {
        let mut out: Vec<(String, f32)> = Vec::new();
        for v in self.voices.iter().rev().filter(|v| !v.stopping()) {
            if out.iter().any(|(id, _)| *id == v.pad_id) {
                continue;
            }
            let p = (v.pos / v.sound.frames().max(1) as f64) as f32;
            out.push((v.pad_id.clone(), p.clamp(0.0, 1.0)));
        }
        out
    }

    /// Ajoute `out.len() / 2` trames stéréo dans `out` (qui doit être mis à zéro par l'appelant).
    pub fn render(&mut self, out: &mut [f32], master: f32) {
        let frames = out.len() / 2;
        let rate = self.rate as f64;
        for v in self.voices.iter_mut() {
            let len = v.sound.frames();
            if len == 0 {
                v.fade = 0.0;
                continue;
            }
            let step = v.sound.rate as f64 / rate;
            let data = &v.sound.data;
            for i in 0..frames {
                if v.pos >= len as f64 {
                    if v.looped {
                        v.pos -= len as f64;
                    } else {
                        break;
                    }
                }
                let idx = v.pos as usize;
                let frac = (v.pos - idx as f64) as f32;
                let next = if idx + 1 < len { idx + 1 } else if v.looped { 0 } else { idx };
                let g = v.gain * v.fade * master;
                let l = data[idx * 2] + (data[next * 2] - data[idx * 2]) * frac;
                let r = data[idx * 2 + 1] + (data[next * 2 + 1] - data[idx * 2 + 1]) * frac;
                out[i * 2] += l * g;
                out[i * 2 + 1] += r * g;
                v.pos += step;
                if v.fade_step != 0.0 {
                    v.fade += v.fade_step;
                    if v.fade <= 0.0 {
                        v.fade = 0.0;
                        break;
                    }
                }
            }
        }
        self.voices.retain(|v| v.fade > 0.0 && (v.looped || v.pos < v.sound.frames() as f64));
    }
}

/// Limiteur doux : évite l'écrêtage brutal quand plusieurs sons se superposent.
pub fn soft_clip(buf: &mut [f32]) {
    for s in buf.iter_mut() {
        let x = *s;
        if x.abs() > 0.9 {
            *s = x.signum() * (0.9 + 0.1 * ((x.abs() - 0.9) / 0.1).tanh());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize, value: f32) -> Arc<Sound> {
        Arc::new(Sound { rate, data: vec![value; frames * 2] })
    }

    #[test]
    fn plays_to_end_then_removes_voice() {
        let mut m = Mixer::new(48000);
        m.start("a", tone(48000, 100, 0.5), 1.0, false);
        let mut buf = vec![0.0; 2 * 150];
        m.render(&mut buf, 1.0);
        assert_eq!(buf[0], 0.5);
        assert_eq!(buf[2 * 99], 0.5);
        assert_eq!(buf[2 * 100], 0.0);
        assert!(m.is_empty());
    }

    #[test]
    fn resamples_to_output_rate() {
        // 24 kHz -> 48 kHz : 100 trames source donnent ~200 trames de sortie.
        let mut m = Mixer::new(48000);
        m.start("a", tone(24000, 100, 0.25), 1.0, false);
        let mut buf = vec![0.0; 2 * 300];
        m.render(&mut buf, 1.0);
        let played = buf.chunks(2).filter(|f| f[0] != 0.0).count();
        assert_eq!(played, 200);
    }

    #[test]
    fn fade_out_stops_voice() {
        let mut m = Mixer::new(1000);
        m.start("a", tone(1000, 10_000, 1.0), 1.0, false);
        m.fade_out(Some("a"), 0.01); // 10 trames
        assert!(!m.is_playing("a"));
        let mut buf = vec![0.0; 2 * 50];
        m.render(&mut buf, 1.0);
        assert!(buf[0] > 0.8 && buf[2 * 5] < buf[0]);
        assert_eq!(buf[2 * 20], 0.0);
        assert!(m.is_empty());
    }

    #[test]
    fn loop_wraps() {
        let mut m = Mixer::new(100);
        m.start("a", tone(100, 10, 1.0), 1.0, true);
        let mut buf = vec![0.0; 2 * 35];
        m.render(&mut buf, 1.0);
        assert!(buf.iter().all(|s| *s == 1.0));
        assert!(m.is_playing("a"));
    }

    #[test]
    fn soft_clip_bounds() {
        let mut b = vec![3.0, -3.0, 0.5];
        soft_clip(&mut b);
        assert!(b[0] <= 1.0 && b[0] > 0.9 && b[1] >= -1.0 && b[2] == 0.5);
    }
}
