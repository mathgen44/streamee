//! Moteur audio : sons chargés, déclenchement des pads, mixeurs micro et retour casque.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{Pad, PadMode};
use crate::mixer::{soft_clip, Mixer, Sound};

const RETRIGGER_FADE: f32 = 0.015;
const STOP_FADE: f32 = 0.12;

pub struct Engine {
    sounds: HashMap<String, Arc<Sound>>,
    inject: Mixer,
    monitor: Mixer,
    pub inject_volume: f32,
    pub monitor_volume: f32,
    pub monitor_enabled: bool,
    /// Vrai si le retour casque tourne (sinon la progression est lue côté micro).
    monitor_running: bool,
    /// Vrai si un logiciel écoute le micro (l'APO tourne).
    inject_active: bool,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            sounds: HashMap::new(),
            inject: Mixer::new(48000),
            monitor: Mixer::new(48000),
            inject_volume: 1.0,
            monitor_volume: 0.8,
            monitor_enabled: true,
            monitor_running: false,
            inject_active: false,
        }
    }

    pub fn insert_sound(&mut self, pad_id: &str, sound: Arc<Sound>) {
        self.sounds.insert(pad_id.to_string(), sound);
    }

    pub fn remove_sound(&mut self, pad_id: &str) {
        self.sounds.remove(pad_id);
        self.inject.fade_out(Some(pad_id), RETRIGGER_FADE);
        self.monitor.fade_out(Some(pad_id), RETRIGGER_FADE);
    }

    pub fn duration(&self, pad_id: &str) -> Option<f64> {
        self.sounds.get(pad_id).map(|s| s.duration_secs())
    }

    fn is_playing(&self, pad_id: &str) -> bool {
        self.monitor.is_playing(pad_id) || self.inject.is_playing(pad_id)
    }

    /// Appui sur un pad, selon son comportement.
    pub fn trigger(&mut self, pad: &Pad) {
        let Some(sound) = self.sounds.get(&pad.id).cloned() else { return };
        match pad.mode {
            PadMode::Restart => self.fade(Some(&pad.id), RETRIGGER_FADE),
            PadMode::Toggle if self.is_playing(&pad.id) => {
                self.fade(Some(&pad.id), STOP_FADE);
                return;
            }
            PadMode::Toggle | PadMode::Overlap => {}
        }
        // On ne lance une voix que là où quelqu'un l'écoute, sinon elle ne finirait jamais.
        if self.inject_active {
            self.inject.start(&pad.id, sound.clone(), pad.volume, pad.looped);
        }
        if self.monitor_running {
            self.monitor.start(&pad.id, sound, pad.volume, pad.looped);
        }
    }

    #[cfg(test)]
    pub fn trigger_for_test(&mut self, pad_id: &str) {
        let pad = Pad { id: pad_id.into(), ..Pad::default() };
        self.trigger(&pad);
    }

    pub fn set_pad_volume(&mut self, pad_id: &str, volume: f32) {
        self.inject.set_gain(pad_id, volume);
        self.monitor.set_gain(pad_id, volume);
    }

    pub fn stop(&mut self, pad_id: Option<&str>) {
        self.fade(pad_id, STOP_FADE);
    }

    fn fade(&mut self, pad_id: Option<&str>, secs: f32) {
        self.inject.fade_out(pad_id, secs);
        self.monitor.fade_out(pad_id, secs);
    }

    /// L'APO tourne à cette fréquence : les sons peuvent partir dans le micro.
    pub fn set_inject_rate(&mut self, rate: u32) {
        self.inject.set_rate(rate);
        self.inject_active = true;
    }
    pub fn set_monitor_running(&mut self, rate: Option<u32>) {
        match rate {
            Some(r) => {
                self.monitor.set_rate(r);
                self.monitor_running = true;
            }
            None => {
                self.monitor.clear();
                self.monitor_running = false;
            }
        }
    }

    /// Plus aucun logiciel n'écoute le micro : on oublie ce qui devait y partir.
    pub fn clear_inject(&mut self) {
        self.inject.clear();
        self.inject_active = false;
    }

    pub fn render_inject(&mut self, out: &mut [f32]) {
        self.inject.render(out, self.inject_volume);
        soft_clip(out);
    }

    pub fn render_monitor(&mut self, out: &mut [f32]) {
        let gain = if self.monitor_enabled { self.monitor_volume } else { 0.0 };
        self.monitor.render(out, gain);
        soft_clip(out);
    }

    pub fn progress(&self) -> Vec<(String, f32)> {
        if self.monitor_running || self.inject.is_empty() {
            self.monitor.progress()
        } else {
            self.inject.progress()
        }
    }
}
