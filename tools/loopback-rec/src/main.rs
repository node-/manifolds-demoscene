//! Streams the default output device's mix (WASAPI loopback) to stdout as raw
//! interleaved f32le PCM for `seconds`, then exits.
//!
//!   loopback-rec <seconds> [rate] [channels]  |  ffmpeg -f f32le -ar R -ac C -i - ...
//!
//! WASAPI loopback delivers no packets while nothing is playing. A writer
//! thread therefore paces output against the wall clock and pads silence, so
//! the stream stays in sync with a video captured alongside it.

#[cfg(not(windows))]
fn main() {
    eprintln!("loopback-rec is Windows-only");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    use cpal::{
        traits::{DeviceTrait, HostTrait, StreamTrait},
        FromSample, Sample, SampleFormat, SizedSample,
    };
    use std::{
        collections::VecDeque,
        io::Write,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    let args: Vec<String> = std::env::args().collect();
    let seconds: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        eprintln!("usage: loopback-rec <seconds> [rate] [channels]");
        std::process::exit(2)
    });

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .expect("no default output device");
    let config = device.default_output_config().expect("no output config");
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    eprintln!(
        "loopback-rec: {} | {} Hz, {} ch, {:?}",
        device.name().unwrap_or_default(),
        rate,
        channels,
        config.sample_format()
    );
    if let (Some(r), Some(c)) = (
        args.get(2).and_then(|s| s.parse::<u32>().ok()),
        args.get(3).and_then(|s| s.parse::<usize>().ok()),
    ) {
        if r != rate || c != channels {
            eprintln!("error: device is {rate} Hz / {channels} ch, expected {r} / {c}");
            std::process::exit(3);
        }
    }

    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

    fn build<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        queue: Arc<Mutex<VecDeque<f32>>>,
    ) -> cpal::Stream
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    let mut q = queue.lock().unwrap();
                    q.extend(data.iter().map(|s| f32::from_sample(*s)));
                },
                |err| eprintln!("loopback stream error: {err}"),
                None,
            )
            .expect("build loopback stream")
    }

    let stream_config: cpal::StreamConfig = config.clone().into();
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, queue.clone()),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, queue.clone()),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, queue.clone()),
        SampleFormat::U16 => build::<u16>(&device, &stream_config, queue.clone()),
        other => panic!("unsupported sample format {other:?}"),
    };
    stream.play().expect("start loopback");

    let start = Instant::now();
    let mut written_frames: u64 = 0;
    let mut out = std::io::BufWriter::with_capacity(1 << 16, std::io::stdout().lock());
    let mut buf: Vec<u8> = Vec::new();

    while start.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_millis(4));
        let expected = (start.elapsed().as_secs_f64() * rate as f64) as u64;
        let mut need = expected.saturating_sub(written_frames) as usize;
        if need == 0 {
            continue;
        }
        buf.clear();
        {
            let mut q = queue.lock().unwrap();
            let avail = q.len() / channels;
            // Real audio first; never let the queue grow without bound.
            let take = if avail > need {
                avail.min(need + rate as usize / 20)
            } else {
                avail
            };
            for _ in 0..take * channels {
                buf.extend_from_slice(&q.pop_front().unwrap().to_le_bytes());
            }
            let wrote = take;
            written_frames += wrote as u64;
            need = need.saturating_sub(wrote);
        }
        // Starved for more than 40 ms: loopback is idle (silence). Pad it.
        if need > (rate as usize * 40) / 1000 {
            buf.resize(buf.len() + need * channels * 4, 0);
            written_frames += need as u64;
        }
        if !buf.is_empty() && out.write_all(&buf).is_err() {
            break; // downstream closed
        }
    }
    let _ = out.flush();
    drop(stream);
    eprintln!(
        "loopback-rec: wrote {:.2}s",
        written_frames as f64 / rate as f64
    );
}
