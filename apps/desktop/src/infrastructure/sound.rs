//! Task completion chime, played through the default output device.
//!
//! Playback is best effort: a missing device or a failing stream is silently
//! ignored, since a notification sound must never disturb the task flow.

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

    macro_rules! output {
        ($ty:ty, $convert:expr) => {{
            let mut position = 0usize;
            device.build_output_stream(
                &stream_config,
                move |data: &mut [$ty], _| {
                    for slot in data.iter_mut() {
                        *slot = match samples.get(position) {
                            Some(&sample) => {
                                position += 1;
                                $convert(sample)
                            }
                            None => <$ty>::default(), // silence once the chime ends
                        };
                    }
                },
                |_: cpal::StreamError| {},
                None,
            )
        }};
    }
    let Ok(stream) = (match config.sample_format() {
        cpal::SampleFormat::F32 => output!(f32, |sample: f32| sample),
        cpal::SampleFormat::I16 => output!(i16, |sample: f32| (sample * i16::MAX as f32) as i16),
        cpal::SampleFormat::U16 => {
            output!(u16, |sample: f32| ((sample * 0.5 + 0.5) * u16::MAX as f32)
                as u16)
        }
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
