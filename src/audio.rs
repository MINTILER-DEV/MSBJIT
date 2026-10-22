//! Cooperative sound playback. Headless runs keep the same sound timeline.
use anyhow::{Context, Result, ensure};
use std::{collections::HashMap, io::Cursor, sync::Arc, time::Instant};
#[cfg(windows)]
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[derive(Clone, Copy)]
pub(crate) struct SoundState {
    pub volume: f64,
    pub pitch: f64,
    pub pan: f64,
}
impl Default for SoundState {
    fn default() -> Self {
        Self {
            volume: 100.0,
            pitch: 0.0,
            pan: 0.0,
        }
    }
}
impl SoundState {
    fn speed(self) -> f64 {
        2.0f64.powf(self.pitch / 120.0)
    }
    pub fn effect(&mut self, name: &str, n: f64, change: bool) {
        let (value, limit) = match name.to_ascii_lowercase().as_str() {
            "pitch" => (&mut self.pitch, 360.0),
            "pan" | "pan left/right" => (&mut self.pan, 100.0),
            _ => return,
        };
        *value = (if change { *value + n } else { n }).clamp(-limit, limit);
    }
}
struct Decoded {
    samples: Vec<f32>,
    #[cfg(windows)]
    channels: u16,
    #[cfg(windows)]
    rate: u32,
    duration: f64,
}
fn decode(data: &[u8]) -> Result<Decoded> {
    let mut wav = hound::WavReader::new(Cursor::new(data)).context("decode WAV sound")?;
    let spec = wav.spec();
    ensure!(
        (1..=2).contains(&spec.channels) && spec.sample_rate > 0,
        "WAV sound must be mono or stereo with a valid sample rate"
    );
    ensure!(
        wav.len() as usize <= 32 * 1024 * 1024,
        "decoded sound exceeds 128 MiB"
    );
    let duration = wav.duration() as f64 / spec.sample_rate as f64;
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => wav
            .samples::<f32>()
            .collect::<std::result::Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 2.0f64.powi(spec.bits_per_sample as i32 - 1) as f32;
            wav.samples::<i32>()
                .map(|s| s.map(|s| s as f32 / scale))
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
    };
    ensure!(
        samples.len().is_multiple_of(spec.channels as usize),
        "WAV sound has an incomplete sample frame"
    );
    Ok(Decoded {
        duration,
        samples,
        #[cfg(windows)]
        channels: spec.channels,
        #[cfg(windows)]
        rate: spec.sample_rate,
    })
}
struct Voice {
    id: u64,
    key: (usize, usize),
    owner: usize,
    remaining: f64,
    updated: Instant,
    speed: f64,
    #[cfg(windows)]
    player: Option<rodio::Player>,
    #[cfg(windows)]
    pan: Arc<AtomicU64>,
}
impl Voice {
    fn advance(&mut self) {
        let now = Instant::now();
        self.remaining -= (now - self.updated).as_secs_f64() * self.speed;
        self.updated = now;
    }
    fn finished(&self) -> bool {
        #[cfg(windows)]
        if let Some(player) = &self.player {
            return player.empty();
        }
        self.remaining <= 0.0
    }
    fn configure(&mut self, state: SoundState) {
        self.advance();
        self.speed = state.speed();
        #[cfg(windows)]
        {
            self.pan.store(state.pan.to_bits(), Ordering::Relaxed);
            if let Some(player) = &self.player {
                player.set_speed(self.speed as f32);
                player.set_volume((state.volume / 100.0) as f32);
            }
        }
    }
}
#[derive(Default)]
pub(crate) struct AudioPlayer {
    voices: Vec<Voice>,
    next: u64,
    cache: HashMap<(usize, usize), Arc<Decoded>>,
    cache_bytes: usize,
    #[cfg(windows)]
    device: Option<rodio::MixerDeviceSink>,
}
impl AudioPlayer {
    pub fn play(
        &mut self,
        owner: usize,
        key: (usize, usize),
        data: &[u8],
        output: bool,
        state: SoundState,
    ) -> Result<u64> {
        let sound = if let Some(sound) = self.cache.get(&key) {
            sound.clone()
        } else {
            let sound = Arc::new(decode(data)?);
            let bytes = sound.samples.len() * 4;
            if self.cache_bytes + bytes <= 64 * 1024 * 1024 {
                self.cache_bytes += bytes;
                self.cache.insert(key, sound.clone());
            }
            sound
        };
        #[cfg(windows)]
        let pan = Arc::new(AtomicU64::new(state.pan.to_bits()));
        #[cfg(windows)]
        let player = if output {
            if self.device.is_none() {
                let mut device =
                    rodio::DeviceSinkBuilder::open_default_sink().context("open audio output")?;
                device.log_on_drop(false);
                self.device = Some(device);
            }
            let player = rodio::Player::connect_new(self.device.as_ref().unwrap().mixer());
            player.set_speed(state.speed() as f32);
            player.set_volume((state.volume / 100.0) as f32);
            player.append(PannedSamples {
                sound: sound.clone(),
                pan: pan.clone(),
                frame: 0,
                right: None,
            });
            Some(player)
        } else {
            None
        };
        #[cfg(not(windows))]
        let _ = output;
        self.voices.retain(|v| v.key != key);
        self.next += 1;
        self.voices.push(Voice {
            id: self.next,
            key,
            owner,
            remaining: sound.duration,
            updated: Instant::now(),
            speed: state.speed(),
            #[cfg(windows)]
            player,
            #[cfg(windows)]
            pan,
        });
        Ok(self.next)
    }
    pub fn tick(&mut self) {
        for voice in &mut self.voices {
            voice.advance();
        }
        self.voices.retain(|v| !v.finished());
    }
    pub fn playing(&self, id: u64) -> bool {
        self.voices.iter().any(|v| v.id == id && !v.finished())
    }
    pub fn configure(&mut self, owner: usize, state: SoundState) {
        for voice in self.voices.iter_mut().filter(|v| v.owner == owner) {
            voice.configure(state);
        }
    }
    pub fn stop_target(&mut self, owner: usize) {
        self.voices.retain(|v| v.owner != owner);
    }
    pub fn stop(&mut self) {
        self.voices.clear();
    }
}

#[cfg(windows)]
struct PannedSamples {
    sound: Arc<Decoded>,
    pan: Arc<AtomicU64>,
    frame: usize,
    right: Option<f32>,
}
#[cfg(windows)]
impl Iterator for PannedSamples {
    type Item = f32;
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = (self.sound.samples.len() / self.sound.channels as usize - self.frame) * 2
            + usize::from(self.right.is_some());
        (n, Some(n))
    }
    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.right.take() {
            return Some(right);
        }
        let channels = self.sound.channels as usize;
        let index = self.frame * channels;
        let left = *self.sound.samples.get(index)?;
        let pan = f64::from_bits(self.pan.load(Ordering::Relaxed)) / 100.0;
        let (left, right) = if channels == 1 {
            let angle = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
            (left * angle.cos() as f32, left * angle.sin() as f32)
        } else {
            let right = self.sound.samples[index + 1];
            if pan <= 0.0 {
                let angle = (pan + 1.0) * std::f64::consts::FRAC_PI_2;
                (
                    left + right * angle.cos() as f32,
                    right * angle.sin() as f32,
                )
            } else {
                let angle = pan * std::f64::consts::FRAC_PI_2;
                (left * angle.cos() as f32, right + left * angle.sin() as f32)
            }
        };
        self.frame += 1;
        self.right = Some(right);
        Some(left)
    }
}
#[cfg(windows)]
impl rodio::Source for PannedSamples {
    fn current_span_len(&self) -> Option<usize> {
        // One fixed-format span. Remaining sample counts may be odd between
        // channel reads; they belong in size_hint, not in a span boundary.
        if self.size_hint().0 == 0 {
            Some(0)
        } else {
            None
        }
    }
    fn channels(&self) -> rodio::ChannelCount {
        2.try_into().unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.sound.rate.try_into().unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(self.sound.duration))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn equal_power_pan_and_stereo_fold_follow_channel_boundaries() {
        use rodio::Source;
        for (channels, samples, pan, expected) in [
            (1, vec![1.0], -100.0f64, [1.0, 0.0]),
            (1, vec![1.0], 0.0, [std::f32::consts::FRAC_1_SQRT_2; 2]),
            (1, vec![1.0], 100.0, [0.0, 1.0]),
            (2, vec![1.0, -0.25], 0.0, [1.0, -0.25]),
            (2, vec![1.0, -0.25], -100.0, [0.75, 0.0]),
            (2, vec![1.0, -0.25], 100.0, [0.0, 0.75]),
        ] {
            let mut source = PannedSamples {
                sound: Arc::new(Decoded {
                    channels,
                    samples,
                    rate: 8000,
                    duration: 1.0 / 8000.0,
                }),
                pan: Arc::new(AtomicU64::new(pan.to_bits())),
                frame: 0,
                right: None,
            };
            assert_eq!(source.size_hint(), (2, Some(2)));
            assert_eq!(source.current_span_len(), None);
            assert!((source.next().unwrap() - expected[0]).abs() < 1e-6);
            assert_eq!(source.size_hint(), (1, Some(1)));
            assert_eq!(source.current_span_len(), None);
            assert!((source.next().unwrap() - expected[1]).abs() < 1e-6);
            assert_eq!(source.next(), None);
            assert_eq!(source.current_span_len(), Some(0));
        }
    }
}
