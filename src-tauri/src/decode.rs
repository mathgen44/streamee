//! Décodage des fichiers audio (MP3, WAV, FLAC, OGG/Vorbis, M4A/AAC, ALAC) en stéréo float32.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::mixer::Sound;

/// Au-delà, on refuse : c'est un pad, pas un lecteur de musique (10 min ≈ 230 Mo en mémoire).
const MAX_SECONDS: u64 = 600;

pub fn decode_file(path: &Path) -> Result<Sound, String> {
    let file = File::open(path).map_err(|e| format!("Impossible d'ouvrir le fichier : {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|_| "Format audio non reconnu".to_string())?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("Aucune piste audio dans ce fichier")?;
    let track_id = track.id;
    let rate = track.codec_params.sample_rate.ok_or("Fréquence d'échantillonnage inconnue")?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "Codec audio non pris en charge".to_string())?;

    let max_frames = (rate as u64 * MAX_SECONDS) as usize;
    let mut data: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("Lecture du fichier interrompue : {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue, // trame abîmée : on passe
            Err(e) => return Err(format!("Décodage impossible : {e}")),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let buf = sample_buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if buf.capacity() < decoded.capacity() * channels {
            *buf = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks(channels) {
            let (l, r) = if channels == 1 { (frame[0], frame[0]) } else { (frame[0], frame[1]) };
            data.push(l);
            data.push(r);
        }
        if data.len() / 2 > max_frames {
            return Err(format!("Son trop long (plus de {} minutes)", MAX_SECONDS / 60));
        }
    }

    if data.is_empty() {
        return Err("Le fichier ne contient aucun son".into());
    }
    Ok(Sound { rate, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WAV 16 bits mono minimal.
    fn wav_mono(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    #[test]
    fn decodes_mono_wav_to_stereo() {
        let path = std::env::temp_dir().join(format!("streamee-{}.wav", std::process::id()));
        std::fs::write(&path, wav_mono(22050, &[16384; 2205])).unwrap();
        let s = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(s.rate, 22050);
        assert_eq!(s.frames(), 2205);
        assert!((s.data[0] - 0.5).abs() < 0.01 && (s.data[1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn rejects_non_audio() {
        let path = std::env::temp_dir().join(format!("streamee-{}.mp3", std::process::id()));
        std::fs::write(&path, b"pas du son").unwrap();
        assert!(decode_file(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
