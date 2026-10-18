//! Captures what the PC is playing (WASAPI loopback) and turns it into 16 frequency bands.
//! Only runs while an audio effect is selected.

use crate::effects::AudioFrame;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const N: usize = 2048;
const HOP: usize = 512;
const BANDS: usize = 16;

struct State {
    ring: Vec<f32>,
    pos: usize,
    since: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    spectrum: Vec<realfft::num_complex::Complex<f32>>,
    edges: [usize; BANDS + 1],
    peak: f32,
    frame: AudioFrame,
    updated: Instant,
}

impl State {
    fn new(rate: f32) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
        let spectrum = fft.make_output_vec();
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos())
            .collect();
        // log-spaced bands from 40 Hz to 16 kHz
        let mut edges = [0usize; BANDS + 1];
        for (i, e) in edges.iter_mut().enumerate() {
            let hz = 40.0 * (16000.0f32 / 40.0).powf(i as f32 / BANDS as f32);
            *e = ((hz / rate * N as f32) as usize).clamp(1, N / 2);
        }
        for i in 1..=BANDS {
            if edges[i] <= edges[i - 1] {
                edges[i] = edges[i - 1] + 1;
            }
        }
        State {
            ring: vec![0.0; N],
            pos: 0,
            since: 0,
            fft,
            window,
            input: vec![0.0; N],
            spectrum,
            edges,
            peak: -60.0,
            frame: AudioFrame::default(),
            updated: Instant::now() - Duration::from_secs(1),
        }
    }

    fn push(&mut self, sample: f32) {
        self.ring[self.pos] = sample;
        self.pos = (self.pos + 1) % N;
        self.since += 1;
        if self.since >= HOP {
            self.since = 0;
            self.analyze();
        }
    }

    fn analyze(&mut self) {
        for i in 0..N {
            self.input[i] = self.ring[(self.pos + i) % N] * self.window[i];
        }
        if self.fft.process(&mut self.input, &mut self.spectrum).is_err() {
            return;
        }
        let mut db = [0f32; BANDS];
        for b in 0..BANDS {
            let (lo, hi) = (self.edges[b], self.edges[b + 1].min(self.spectrum.len()));
            let e: f32 = self.spectrum[lo..hi].iter().map(|c| c.norm_sqr()).sum::<f32>() / (hi - lo).max(1) as f32;
            db[b] = 10.0 * (e + 1e-12).log10();
        }
        // automatic gain: follow the loudest band, slowly forget
        let max = db.iter().cloned().fold(f32::MIN, f32::max);
        self.peak = (self.peak - 0.03).max(max).max(-40.0);
        let range = 45.0;
        let f = &mut self.frame;
        for b in 0..BANDS {
            let v = ((db[b] - (self.peak - range)) / range).clamp(0.0, 1.0).powf(1.6);
            // fast attack, slower release
            f.bands[b] = if v > f.bands[b] { f.bands[b] * 0.3 + v * 0.7 } else { f.bands[b] * 0.82 + v * 0.18 };
        }
        f.bass = (f.bands[0] + f.bands[1] + f.bands[2]) / 3.0;
        f.level = f.bands.iter().sum::<f32>() / BANDS as f32;
        self.updated = Instant::now();
    }
}

pub struct AudioCapture {
    _stream: cpal::Stream,
    state: Arc<Mutex<State>>,
}

impl AudioCapture {
    pub fn start() -> Option<Self> {
        let host = cpal::default_host();
        let dev = host.default_output_device()?;
        let cfg = dev.default_output_config().ok()?;
        if cfg.sample_format() != cpal::SampleFormat::F32 {
            return None;
        }
        let channels = cfg.channels().max(1) as usize;
        let stream_cfg: cpal::StreamConfig = cfg.into();
        let state = Arc::new(Mutex::new(State::new(stream_cfg.sample_rate as f32)));
        let st = state.clone();
        // building an input stream on an output device = loopback capture (WASAPI)
        let stream = dev
            .build_input_stream(
                stream_cfg,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut s) = st.lock() {
                        for frame in data.chunks(channels) {
                            let m = frame.iter().sum::<f32>() / channels as f32;
                            s.push(m);
                        }
                    }
                },
                |_| {},
                None,
            )
            .ok()?;
        stream.play().ok()?;
        Some(AudioCapture { _stream: stream, state })
    }

    pub fn frame(&self) -> AudioFrame {
        let s = match self.state.lock() {
            Ok(s) => s,
            Err(_) => return AudioFrame::default(),
        };
        // Windows stops delivering loopback data during silence.
        let age = s.updated.elapsed().as_secs_f32();
        if age > 0.15 {
            let k = (1.0 - (age - 0.15) * 3.0).max(0.0);
            let mut f = s.frame;
            f.bands.iter_mut().for_each(|b| *b *= k);
            f.bass *= k;
            f.level *= k;
            return f;
        }
        s.frame
    }
}
