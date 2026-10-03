//! cpal output fed from a shared ring buffer of stereo f32 frames.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Interleaved stereo samples shared between the emulation thread and the audio callback.
pub type Ring = Arc<Mutex<VecDeque<f32>>>;

pub struct Audio {
    _stream: Stream,
    pub ring: Ring,
    pub sample_rate: u32,
    /// Fill level (in stereo frames) the pacer tries to maintain.
    pub target_frames: usize,
    max_frames: usize,
}

impl Audio {
    pub fn new() -> Result<Audio, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let sample_rate = config.sample_rate;
        let channels = config.channels as usize;
        let ring: Ring = Arc::new(Mutex::new(VecDeque::new()));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, channels, ring.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, channels, ring.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, channels, ring.clone()),
            other => return Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Audio {
            _stream: stream,
            ring,
            sample_rate,
            // ~3 video frames of latency (~50 ms).
            target_frames: sample_rate as usize / 20,
            max_frames: sample_rate as usize / 4,
        })
    }

    /// Queue interleaved stereo samples, dropping the oldest if the consumer has stalled.
    pub fn push(&self, samples: &[f32]) {
        let mut ring = self.ring.lock().unwrap();
        ring.extend(samples.iter().copied());
        let max = self.max_frames * 2;
        if ring.len() > max {
            let excess = ring.len() - max;
            ring.drain(..excess);
        }
    }

    /// Buffered stereo frames not yet played.
    pub fn fill_frames(&self) -> usize {
        self.ring.lock().unwrap().len() / 2
    }
}

fn build<T>(device: &cpal::Device, config: StreamConfig, channels: usize, ring: Ring) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let err_fn = |err: cpal::Error| eprintln!("audio stream error: {err}");
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let mut ring = ring.lock().unwrap();
                for frame in data.chunks_mut(channels) {
                    let l = ring.pop_front();
                    let r = ring.pop_front();
                    let (l, r) = (l.unwrap_or(0.0), r.unwrap_or(0.0));
                    for (i, out) in frame.iter_mut().enumerate() {
                        let v = match (channels, i) {
                            (1, _) => (l + r) * 0.5,
                            (_, 0) => l,
                            (_, 1) => r,
                            _ => 0.0,
                        };
                        *out = T::from_sample(v);
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| e.to_string())
}
