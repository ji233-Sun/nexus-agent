use std::{
    io::Cursor,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::Command;

const MAX_SECONDS: u32 = 60;
const MAX_SAMPLE_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct Recording {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
}

pub(super) fn capture(commands: &mpsc::Receiver<Command>) -> anyhow::Result<Option<Recording>> {
    #[cfg(target_os = "macos")]
    super::native::authorize_microphone(commands)?;
    let device = cpal::default_host()
        .default_input_device()
        .context("no input device available")?;
    let config = device
        .default_input_config()
        .context("input device has no supported default format")?;
    let channels = config.channels();
    let sample_rate = config.sample_rate().0;
    if channels == 0 {
        bail!("audio input has no channels");
    }
    if sample_rate == 0 {
        bail!("audio input has an invalid sample rate");
    }
    let format_limit = (sample_rate as usize)
        .checked_mul(channels as usize)
        .and_then(|value| value.checked_mul(MAX_SECONDS as usize))
        .context("audio input format is too large")?;
    let limit = format_limit.min(MAX_SAMPLE_BYTES / std::mem::size_of::<f32>()) / channels as usize
        * channels as usize;
    if limit < channels as usize {
        bail!("audio input channel configuration exceeds the capture memory limit");
    }
    let samples = Arc::new(Mutex::new(Vec::with_capacity(limit.min(1_000_000))));
    let captured = Arc::clone(&samples);
    let (full_tx, full_rx) = mpsc::sync_channel(1);
    let errors = Arc::new(Mutex::new(None::<String>));
    let stream_errors = Arc::clone(&errors);
    let err_fn = move |error: cpal::StreamError| {
        *stream_errors.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.to_string())
    };
    let stream_config = config.config();

    macro_rules! stream {
        ($ty:ty, $convert:expr) => {{
            let full_tx = full_tx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[$ty], _| {
                    let mut output = captured.lock().unwrap_or_else(|e| e.into_inner());
                    let remaining = limit.saturating_sub(output.len());
                    output.extend(data.iter().take(remaining).map($convert));
                    if output.len() >= limit {
                        let _ = full_tx.try_send(());
                    }
                },
                err_fn,
                None,
            )?
        }};
    }
    let stream = match config.sample_format() {
        cpal::SampleFormat::I8 => stream!(i8, |&x| x as f32 / i8::MAX as f32),
        cpal::SampleFormat::I16 => stream!(i16, |&x| x as f32 / i16::MAX as f32),
        cpal::SampleFormat::I24 => stream!(cpal::I24, |&x| x.inner() as f32 / 8_388_607.0),
        cpal::SampleFormat::I32 => stream!(i32, |&x| x as f32 / i32::MAX as f32),
        cpal::SampleFormat::I64 => stream!(i64, |&x| x as f32 / i64::MAX as f32),
        cpal::SampleFormat::U8 => stream!(u8, |&x| x as f32 / u8::MAX as f32 * 2.0 - 1.0),
        cpal::SampleFormat::U16 => stream!(u16, |&x| x as f32 / u16::MAX as f32 * 2.0 - 1.0),
        cpal::SampleFormat::U32 => stream!(u32, |&x| x as f32 / u32::MAX as f32 * 2.0 - 1.0),
        cpal::SampleFormat::U64 => stream!(u64, |&x| x as f32 / u64::MAX as f32 * 2.0 - 1.0),
        cpal::SampleFormat::F32 => stream!(f32, |&x| x),
        cpal::SampleFormat::F64 => stream!(f64, |&x| x as f32),
        format => bail!("unsupported input sample format: {format:?}"),
    };
    stream.play().context("failed to start input device")?;
    let deadline = Instant::now() + Duration::from_secs(MAX_SECONDS.into());
    let cancelled = loop {
        if full_rx.try_recv().is_ok() {
            break false;
        }
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Stop) => break false,
            Ok(Command::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => break true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some(error) = errors.lock().unwrap_or_else(|e| e.into_inner()).take() {
            bail!("input device error: {error}");
        }
        if Instant::now() >= deadline {
            break false;
        }
    };
    drop(stream); // Releases the device before encoding or uploading.
    if cancelled {
        return Ok(None);
    }
    let samples = Arc::try_unwrap(samples)
        .map_err(|_| anyhow::anyhow!("audio callback did not stop"))?
        .into_inner()
        .unwrap_or_else(|e| e.into_inner());
    Ok(Some(Recording {
        samples,
        channels,
        sample_rate,
    }))
}

pub(super) fn wav_mono(recording: &Recording) -> anyhow::Result<Vec<u8>> {
    if recording.channels == 0 || recording.sample_rate == 0 {
        bail!("invalid recording format");
    }
    if recording.samples.is_empty() {
        bail!("recording contains no audio");
    }
    let mono: Vec<i16> = recording
        .samples
        .chunks(recording.channels as usize)
        .map(|frame| {
            let value = frame.iter().copied().sum::<f32>() / frame.len() as f32;
            (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
        })
        .collect();
    let data_len = (mono.len() * 2) as u32;
    let mut wav = Cursor::new(Vec::with_capacity(44 + data_len as usize));
    use std::io::Write;
    wav.write_all(b"RIFF")?;
    wav.write_all(&(36 + data_len).to_le_bytes())?;
    wav.write_all(b"WAVEfmt ")?;
    wav.write_all(&16u32.to_le_bytes())?;
    wav.write_all(&1u16.to_le_bytes())?;
    wav.write_all(&1u16.to_le_bytes())?;
    wav.write_all(&recording.sample_rate.to_le_bytes())?;
    wav.write_all(&(recording.sample_rate * 2).to_le_bytes())?;
    wav.write_all(&2u16.to_le_bytes())?;
    wav.write_all(&16u16.to_le_bytes())?;
    wav.write_all(b"data")?;
    wav.write_all(&data_len.to_le_bytes())?;
    for sample in mono {
        wav.write_all(&sample.to_le_bytes())?;
    }
    Ok(wav.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wav_is_mono_and_averages_channels() {
        let wav = wav_mono(&Recording {
            samples: vec![1.0, -1.0, 0.5, 0.5],
            channels: 2,
            sample_rate: 8_000,
        })
        .unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[22..24], &1u16.to_le_bytes());
        assert_eq!(&wav[24..28], &8_000u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(i16::from_le_bytes(wav[44..46].try_into().unwrap()), 0);
        assert_eq!(i16::from_le_bytes(wav[46..48].try_into().unwrap()), 16384);
    }

    #[test]
    fn empty_recording_is_rejected() {
        let error = wav_mono(&Recording {
            samples: Vec::new(),
            channels: 1,
            sample_rate: 8_000,
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "recording contains no audio");
    }
}
