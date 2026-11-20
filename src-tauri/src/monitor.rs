//! Retour casque : pour que le streamer entende aussi ses sons.

use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use parking_lot::Mutex;

use crate::engine::Engine;

pub enum MonitorCmd {
    /// Changer de sortie (None = sortie par défaut).
    SetDevice(Option<String>),
    /// La sortie a disparu (casque débranché) : on réessaie.
    Rebuild,
}

pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// Lance le fil qui possède le flux audio (un `cpal::Stream` ne peut pas changer de fil).
pub fn spawn(engine: Arc<Mutex<Engine>>, device: Option<String>) -> Sender<MonitorCmd> {
    let (tx, rx) = channel::<MonitorCmd>();
    let tx_err = tx.clone();
    thread::Builder::new()
        .name("streamee-monitor".into())
        .spawn(move || {
            let mut wanted = device;
            loop {
                let stream = match build(&engine, wanted.as_deref(), tx_err.clone()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("retour casque indisponible : {e}");
                        engine.lock().set_monitor_running(None);
                        None
                    }
                };
                // Attend une commande ; sans flux, réessaie toutes les 3 s (casque rebranché).
                let cmd = if stream.is_some() {
                    rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
                } else {
                    rx.recv_timeout(Duration::from_secs(3))
                };
                drop(stream);
                engine.lock().set_monitor_running(None);
                match cmd {
                    Ok(MonitorCmd::SetDevice(d)) => wanted = d,
                    Ok(MonitorCmd::Rebuild) | Err(RecvTimeoutError::Timeout) => {
                        thread::sleep(Duration::from_millis(300));
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("fil du retour casque");
    tx
}

fn build(engine: &Arc<Mutex<Engine>>, name: Option<&str>, tx: Sender<MonitorCmd>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|x| x == n).unwrap_or(false))
            .or_else(|| host.default_output_device()),
        None => host.default_output_device(),
    }
    .ok_or("aucune sortie audio")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        cpal::SampleFormat::F32 => run::<f32>(&device, &config, engine.clone(), tx),
        cpal::SampleFormat::I16 => run::<i16>(&device, &config, engine.clone(), tx),
        cpal::SampleFormat::U16 => run::<u16>(&device, &config, engine.clone(), tx),
        cpal::SampleFormat::I32 => run::<i32>(&device, &config, engine.clone(), tx),
        other => return Err(format!("format de sortie non géré : {other}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    engine.lock().set_monitor_running(Some(config.sample_rate.0));
    Ok(stream)
}

fn run<T>(device: &cpal::Device, config: &cpal::StreamConfig, engine: Arc<Mutex<Engine>>, tx: Sender<MonitorCmd>)
    -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut stereo: Vec<f32> = Vec::with_capacity(4096);
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                let frames = out.len() / channels;
                stereo.clear();
                stereo.resize(frames * 2, 0.0);
                engine.lock().render_monitor(&mut stereo);
                for (f, frame) in out.chunks_mut(channels).enumerate() {
                    let (l, r) = (stereo[f * 2], stereo[f * 2 + 1]);
                    for (c, s) in frame.iter_mut().enumerate() {
                        let v = match (channels, c) {
                            (1, _) => 0.5 * (l + r),
                            (_, 0) => l,
                            (_, 1) => r,
                            _ => 0.0,
                        };
                        *s = T::from_sample(v);
                    }
                }
            },
            move |err| {
                eprintln!("erreur du retour casque : {err}");
                let _ = tx.send(MonitorCmd::Rebuild);
            },
            None,
        )
        .map_err(|e| e.to_string())
}
