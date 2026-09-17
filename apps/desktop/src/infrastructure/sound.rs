//! Task completion chime, played through the default output device.
//!
//! Playback is best effort: a missing device or a failing stream is silently
//! ignored, since a notification sound must never disturb the task flow.

#[cfg(not(test))]
use std::{f32::consts::TAU, time::Duration};

#[cfg(not(test))]
pub(crate) fn play_task_complete() {
    // Detached thread: the presenter must not wait on device setup or playback.
    std::thread::Builder::new()
        .name("task-complete-sound".into())
        .spawn(play)
        .ok();
}

// Keep the test build silent on machines with real audio devices.
#[cfg(test)]
pub(crate) fn play_task_complete() {}

#[cfg(not(test))]
fn play() {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    const TONE_HZ: f32 = 880.0;
    const FADE_IN_SECS: f32 = 0.005;
    const DECAY_PER_SEC: f32 = 8.0;
    const PEAK: f32 = 0.25;
    const DURATION: Duration = Duration::from_millis(350);

    let Some(device) = cpal::default_host().default_output_device() else {
        return;
    };
    let Ok(config) = device.default_output_config() else {
        return;
    };
    let sample_rate = config.sample_rate().0 as f32;
    let total = (sample_rate * DURATION.as_secs_f32()) as usize;
    let samples: Vec<f32> = (0..total)
        .map(|index| {
            let time = index as f32 / sample_rate;
            // Fade in avoids a click; exponential decay reads as a chime.
            let envelope = (time / FADE_IN_SECS).min(1.0) * (-time * DECAY_PER_SEC).exp();
            (TAU * TONE_HZ * time).sin() * envelope * PEAK
        })
        .collect();
    let stream_config = config.config();
    let channels = usize::from(stream_config.channels);
    if channels == 0 {
        return;
    }
    let i24_silence = cpal::I24::new(0);

    macro_rules! output {
        ($ty:ty, $convert:expr, $silence:expr) => {{
            let mut position = 0usize;
            device.build_output_stream(
                &stream_config,
                move |data: &mut [$ty], _| {
                    for frame in data.chunks_mut(channels) {
                        let sample = match samples.get(position) {
                            Some(&sample) => {
                                position += 1;
                                $convert(sample)
                            }
                            None => $silence, // silence once the chime ends
                        };
                        for slot in frame.iter_mut() {
                            *slot = sample;
                        }
                    }
                },
                |_: cpal::StreamError| {},
                None,
            )
        }};
    }
    let Ok(stream) = (match config.sample_format() {
        cpal::SampleFormat::I8 => output!(
            i8,
            |sample: f32| (sample.clamp(-1.0, 1.0) * i8::MAX as f32).round() as i8,
            0i8
        ),
        cpal::SampleFormat::I16 => output!(
            i16,
            |sample: f32| (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16,
            0i16
        ),
        cpal::SampleFormat::I24 => {
            let Some(i24_silence) = i24_silence else {
                return;
            };
            output!(
                cpal::I24,
                move |sample: f32| {
                    cpal::I24::new((sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32)
                        .unwrap_or(i24_silence)
                },
                i24_silence
            )
        }
        cpal::SampleFormat::I32 => output!(
            i32,
            |sample: f32| (sample.clamp(-1.0, 1.0) * i32::MAX as f32).round() as i32,
            0i32
        ),
        cpal::SampleFormat::I64 => output!(
            i64,
            |sample: f32| (sample.clamp(-1.0, 1.0) as f64 * i64::MAX as f64).round() as i64,
            0i64
        ),
        cpal::SampleFormat::U8 => output!(
            u8,
            |sample: f32| (((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u8::MAX as f32).round()) as u8,
            u8::MAX / 2 + 1
        ),
        cpal::SampleFormat::U16 => output!(
            u16,
            |sample: f32| {
                (((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32).round()) as u16
            },
            u16::MAX / 2 + 1
        ),
        cpal::SampleFormat::U32 => output!(
            u32,
            |sample: f32| {
                ((sample.clamp(-1.0, 1.0) as f64 * 0.5 + 0.5) * u32::MAX as f64).round() as u32
            },
            u32::MAX / 2 + 1
        ),
        cpal::SampleFormat::U64 => output!(
            u64,
            |sample: f32| {
                ((sample.clamp(-1.0, 1.0) as f64 * 0.5 + 0.5) * u64::MAX as f64).round() as u64
            },
            u64::MAX / 2 + 1
        ),
        cpal::SampleFormat::F32 => output!(f32, |sample: f32| sample, 0.0f32),
        cpal::SampleFormat::F64 => output!(f64, |sample: f32| sample as f64, 0.0f64),
        _ => return,
    }) else {
        return;
    };
    if stream.play().is_err() {
        return;
    }
    // Hold the stream until the buffer drains, then release the device.
    std::thread::sleep(DURATION + Duration::from_millis(150));
}
