//! Audio-reactivity source.
//!
//! Windows uses WASAPI loopback via `cpal` and analyzes six configurable FFT
//! bands from the OS output mix. Other platforms keep a synthetic source so the
//! render/control loop remains testable everywhere.

use crate::control::AudioControl;
use dscore::scene::NBANDS;
use std::time::Instant;

pub struct AudioSource {
    imp: imp::AudioInput,
    smoothed: [f32; NBANDS],
    last: Instant,
}

impl AudioSource {
    pub fn new() -> Self {
        Self {
            imp: imp::AudioInput::new(),
            smoothed: [0.0; NBANDS],
            last: Instant::now(),
        }
    }

    pub fn bands(&mut self, control: &AudioControl, t: f32) -> [f32; NBANDS] {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().max(1.0 / 240.0);
        self.last = now;
        let targets = self.imp.bands(control, t);
        for (i, smoothed) in self.smoothed.iter_mut().enumerate() {
            let smooth_s = (control.bands[i].smooth_ms * 0.001).max(0.0);
            let alpha = if smooth_s <= f32::EPSILON {
                1.0
            } else {
                1.0 - (-dt / smooth_s).exp()
            };
            *smoothed += (targets[i] - *smoothed) * alpha;
            *smoothed = (*smoothed).clamp(0.0, 1.0);
        }
        self.smoothed
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use cpal::{
        traits::{DeviceTrait, HostTrait, StreamTrait},
        FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    };
    use rustfft::{num_complex::Complex32, Fft, FftPlanner};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    const FFT_SIZE: usize = 4096;
    const MAX_RING_SECONDS: usize = 2;

    struct Ring {
        samples: VecDeque<f32>,
        sample_rate: f32,
        capacity: usize,
    }

    pub struct AudioInput {
        ring: Option<Arc<Mutex<Ring>>>,
        _stream: Option<Stream>,
        fft: std::sync::Arc<dyn Fft<f32>>,
        bins: Vec<Complex32>,
        /// Per-band running peak (dB) for auto-gain; decays linearly in dB.
        peak_db: [f32; NBANDS],
        last: std::time::Instant,
    }

    impl AudioInput {
        pub fn new() -> Self {
            let mut planner = FftPlanner::new();
            let fft = planner.plan_fft_forward(FFT_SIZE);
            let bins = vec![Complex32::ZERO; FFT_SIZE];

            match open_audio_stream() {
                Ok((ring, stream, source)) => {
                    log::info!("native audio active: {source}");
                    Self {
                        ring: Some(ring),
                        _stream: Some(stream),
                        fft,
                        bins,
                        peak_db: [-120.0; NBANDS],
                        last: std::time::Instant::now(),
                    }
                }
                Err(err) => {
                    log::warn!("native audio unavailable, using synthetic bands: {err}");
                    Self {
                        ring: None,
                        _stream: None,
                        fft,
                        bins,
                        peak_db: [-120.0; NBANDS],
                        last: std::time::Instant::now(),
                    }
                }
            }
        }

        pub fn bands(&mut self, control: &AudioControl, t: f32) -> [f32; NBANDS] {
            let Some(ring) = &self.ring else {
                return synthetic_bands(control, t);
            };

            let (samples, sample_rate) = {
                let ring = ring.lock().unwrap();
                if ring.samples.len() < FFT_SIZE / 4 {
                    return synthetic_bands(control, t);
                }
                let mut samples = ring
                    .samples
                    .iter()
                    .rev()
                    .take(FFT_SIZE)
                    .copied()
                    .collect::<Vec<_>>();
                samples.reverse();
                (samples, ring.sample_rate)
            };

            self.bins.fill(Complex32::ZERO);
            let offset = FFT_SIZE.saturating_sub(samples.len());
            for (i, sample) in samples.iter().enumerate() {
                let n = i + offset;
                let phase = n as f32 / FFT_SIZE as f32;
                let hann = 0.5 - 0.5 * (std::f32::consts::TAU * phase).cos();
                self.bins[n] = Complex32::new(sample * hann, 0.0);
            }

            self.fft.process(&mut self.bins);
            let now = std::time::Instant::now();
            let dt = (now - self.last).as_secs_f32().min(0.25);
            self.last = now;
            analyze_bands(&self.bins, sample_rate, control, &mut self.peak_db, dt)
        }
    }

    fn open_audio_stream() -> Result<(Arc<Mutex<Ring>>, Stream, String), String> {
        let host = cpal::default_host();
        let mut errors = Vec::new();

        if let Some(device) = host.default_output_device() {
            let name = device
                .name()
                .unwrap_or_else(|_| "default output".to_string());
            match device.default_output_config() {
                Ok(config) => match open_stream(&device, config, "WASAPI loopback") {
                    Ok((ring, stream)) => {
                        return Ok((ring, stream, format!("WASAPI loopback: {name}")))
                    }
                    Err(err) => errors.push(format!("output loopback {name}: {err}")),
                },
                Err(err) => errors.push(format!("default output config: {err}")),
            }
        } else {
            errors.push("no default output device".to_string());
        }

        if let Some(device) = host.default_input_device() {
            let name = device
                .name()
                .unwrap_or_else(|_| "default input".to_string());
            match device.default_input_config() {
                Ok(config) => match open_stream(&device, config, "input") {
                    Ok((ring, stream)) => {
                        return Ok((ring, stream, format!("input fallback: {name}")))
                    }
                    Err(err) => errors.push(format!("input fallback {name}: {err}")),
                },
                Err(err) => errors.push(format!("default input config: {err}")),
            }
        } else {
            errors.push("no default input device".to_string());
        }

        Err(errors.join("; "))
    }

    fn open_stream(
        device: &cpal::Device,
        config: SupportedStreamConfig,
        source_kind: &'static str,
    ) -> Result<(Arc<Mutex<Ring>>, Stream), String> {
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels().max(1) as usize;
        let capacity = sample_rate as usize * MAX_RING_SECONDS;
        let ring = Arc::new(Mutex::new(Ring {
            samples: VecDeque::with_capacity(capacity),
            sample_rate,
            capacity,
        }));
        let err_fn = |err| log::warn!("native audio stream error: {err}");
        let stream_config = config.clone().into();
        let sample_format = config.sample_format();
        let stream = match sample_format {
            SampleFormat::F32 => {
                build_stream::<f32>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::F64 => {
                build_stream::<f64>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::I8 => {
                build_stream::<i8>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::I16 => {
                build_stream::<i16>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::I32 => {
                build_stream::<i32>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::I64 => {
                build_stream::<i64>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::U8 => {
                build_stream::<u8>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::U16 => {
                build_stream::<u16>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::U32 => {
                build_stream::<u32>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            SampleFormat::U64 => {
                build_stream::<u64>(device, &stream_config, ring.clone(), channels, err_fn)
            }
            other => return Err(format!("unsupported {source_kind} sample format {other:?}")),
        }
        .map_err(|err| format!("build input stream: {err}"))?;
        stream
            .play()
            .map_err(|err| format!("start input stream: {err}"))?;
        log::info!(
            "opened {source_kind} audio stream: {} Hz, {} channels, {sample_format}",
            sample_rate as u32,
            channels
        );
        Ok((ring, stream))
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        ring: Arc<Mutex<Ring>>,
        channels: usize,
        err_fn: impl Fn(cpal::StreamError) + Send + 'static,
    ) -> Result<Stream, cpal::BuildStreamError>
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        device.build_input_stream(
            config,
            move |data: &[T], _| push_frames(&ring, data, channels),
            err_fn,
            None,
        )
    }

    fn push_frames<T>(ring: &Arc<Mutex<Ring>>, data: &[T], channels: usize)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        push_samples(
            ring,
            data.chunks(channels).map(|frame| {
                frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / frame.len().max(1) as f32
            }),
        );
    }

    fn push_samples<I>(ring: &Arc<Mutex<Ring>>, samples: I)
    where
        I: IntoIterator<Item = f32>,
    {
        let mut ring = ring.lock().unwrap();
        for sample in samples {
            ring.samples.push_back(sample.clamp(-1.0, 1.0));
            while ring.samples.len() > ring.capacity {
                ring.samples.pop_front();
            }
        }
    }

    fn analyze_bands(
        bins: &[Complex32],
        sample_rate: f32,
        control: &AudioControl,
        peak_db: &mut [f32; NBANDS],
        dt: f32,
    ) -> [f32; NBANDS] {
        let mut out = [0.0; NBANDS];
        let bin_hz = sample_rate / FFT_SIZE as f32;
        let nyquist_bin = FFT_SIZE / 2;
        let floor = control.floor_db.min(control.ceiling_db - 1.0);
        let ceiling = control.ceiling_db.max(floor + 1.0);
        for (i, band) in control.bands.iter().enumerate() {
            let lo = (band.low_hz / bin_hz).floor().max(1.0) as usize;
            let hi = (band.high_hz / bin_hz).ceil().max(lo as f32 + 1.0) as usize;
            let lo = lo.min(nyquist_bin - 1);
            let hi = hi.min(nyquist_bin);
            let mut sum = 0.0;
            let mut count = 0usize;
            for bin in &bins[lo..hi] {
                sum += bin.norm_sqr();
                count += 1;
            }
            let rms = if count == 0 {
                0.0
            } else {
                (sum / count as f32).sqrt() / FFT_SIZE as f32
            };
            let mut db = 20.0 * rms.max(1.0e-8).log10();
            // Spectral tilt: program material falls ~3-6 dB/oct, so lift each
            // band by its distance (in octaves) above/below 1 kHz.
            let centre_hz = (band.low_hz * band.high_hz).sqrt().max(1.0);
            db += control.tilt_db_per_oct * (centre_hz / 1000.0).log2();

            let manual = ((db - floor) / (ceiling - floor)).clamp(0.0, 1.0);

            // Per-band auto-gain: normalize against this band's own recent
            // peak, so a quiet band still spans its full 0..1 range. Attack is
            // instant; release is linear in dB over `agc_release_s`.
            let range = control.agc_range_db.max(6.0);
            let release_db = range / control.agc_release_s.max(0.1) * dt;
            // Never sink the reference below `floor + 12 dB`, so silence stays dark.
            peak_db[i] = db.max(peak_db[i] - release_db).max(floor + 12.0);
            let agc = ((db - (peak_db[i] - range)) / range).clamp(0.0, 1.0);

            let mut v = manual + (agc - manual) * control.agc_amount.clamp(0.0, 1.0);
            if v < control.noise_gate {
                v = 0.0;
            }
            out[i] = (v * band.gain * control.master_gain).clamp(0.0, 1.0);
        }
        out
    }

    fn synthetic_bands(control: &AudioControl, t: f32) -> [f32; NBANDS] {
        super::synthetic_bands(control, t)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub struct AudioInput;

    impl AudioInput {
        pub fn new() -> Self {
            log::warn!("native audio input is Windows-only in this build; using synthetic bands");
            Self
        }

        pub fn bands(&mut self, control: &AudioControl, t: f32) -> [f32; NBANDS] {
            super::synthetic_bands(control, t)
        }
    }
}

fn synthetic_bands(control: &AudioControl, t: f32) -> [f32; NBANDS] {
    let phases = [1.7, 2.3, 3.1, 4.7, 5.9, 7.3];
    let db_span = (control.ceiling_db - control.floor_db).abs().max(1.0);
    let range_scale = (60.0 / db_span).clamp(0.5, 2.0);
    let mut out = [0.0; NBANDS];
    for (i, out) in out.iter_mut().enumerate() {
        let phase = (t * phases[i]).fract();
        let pulse = (1.0 - phase).powf(3.0);
        let shimmer = ((t * (0.17 + i as f32 * 0.09)).sin() * 0.5 + 0.5) * 0.25;
        let mut v = ((pulse + shimmer) * range_scale).clamp(0.0, 1.0);
        if v < control.noise_gate {
            v = 0.0;
        }
        *out = (v * control.bands[i].gain * control.master_gain).clamp(0.0, 1.0);
    }
    out
}
