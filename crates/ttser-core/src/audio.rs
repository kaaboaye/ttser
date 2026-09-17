use anyhow::{Context, Result, bail, ensure};
use cpal::{
    FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rubato::{FftFixedInOut, Resampler};
use std::{
    borrow::Cow,
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug, serde::Serialize)]
pub struct AudioLevels {
    pub input_peak: f32,
    pub gain: f32,
    pub output_peak: f32,
}

pub(crate) fn normalize_volume(
    samples: &[f32],
    target_peak: f32,
    max_gain: f32,
) -> Result<(Cow<'_, [f32]>, AudioLevels)> {
    let mut input_peak = 0.0_f32;
    for &sample in samples {
        ensure!(sample.is_finite(), "Audio contains a non-finite sample");
        input_peak = input_peak.max(sample.abs());
    }
    // A finite cap avoids amplifying near-silence to full scale.
    let gain = if input_peak < 0.00001 {
        1.0
    } else {
        (target_peak / input_peak).min(max_gain)
    };
    let output = if gain == 1.0 {
        Cow::Borrowed(samples)
    } else {
        Cow::Owned(samples.iter().map(|sample| sample * gain).collect())
    };
    Ok((
        output,
        AudioLevels {
            input_peak,
            gain,
            output_peak: input_peak * gain,
        },
    ))
}

pub fn list_devices() -> Result<()> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    for device in host.input_devices()? {
        let name = device.name()?;
        println!(
            "{name}{}",
            if Some(&name) == default.as_ref() {
                " (default)"
            } else {
                ""
            }
        );
    }
    Ok(())
}

#[derive(Default)]
struct Capture {
    samples: Vec<f32>,
    error: Option<String>,
}

pub struct Recording {
    stream: Stream,
    data: Arc<Mutex<Capture>>,
    rate: u32,
}

impl Recording {
    pub fn start(device_name: Option<&str>, max_seconds: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            host.input_devices()?
                .find(|d| d.name().is_ok_and(|n| n == name))
                .with_context(|| format!("Input device not found: {name}"))?
        } else {
            host.default_input_device()
                .context("No default microphone")?
        };
        let supported = device
            .default_input_config()
            .context("Reading microphone format")?;
        let config: StreamConfig = supported.clone().into();
        let data = Arc::new(Mutex::new(Capture::default()));
        let limit = config.sample_rate.0 as usize * max_seconds as usize;
        let stream = match supported.sample_format() {
            SampleFormat::I8 => input::<i8>(&device, &config, &data, limit)?,
            SampleFormat::I16 => input::<i16>(&device, &config, &data, limit)?,
            SampleFormat::I32 => input::<i32>(&device, &config, &data, limit)?,
            SampleFormat::I64 => input::<i64>(&device, &config, &data, limit)?,
            SampleFormat::U8 => input::<u8>(&device, &config, &data, limit)?,
            SampleFormat::U16 => input::<u16>(&device, &config, &data, limit)?,
            SampleFormat::U32 => input::<u32>(&device, &config, &data, limit)?,
            SampleFormat::U64 => input::<u64>(&device, &config, &data, limit)?,
            SampleFormat::F32 => input::<f32>(&device, &config, &data, limit)?,
            SampleFormat::F64 => input::<f64>(&device, &config, &data, limit)?,
            format => bail!("Unsupported microphone format: {format}"),
        };
        stream.play().context("Starting microphone")?;
        Ok(Self {
            stream,
            data,
            rate: config.sample_rate.0,
        })
    }

    pub fn error(&self) -> Option<String> {
        self.data.lock().unwrap().error.clone()
    }

    pub fn finish(self) -> Result<Vec<f32>> {
        drop(self.stream);
        let mut data = self.data.lock().unwrap();
        if let Some(error) = &data.error {
            bail!("{error}");
        }
        let samples = std::mem::take(&mut data.samples);
        drop(data);
        resample(&samples, self.rate)
    }
}

fn input<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    data: &Arc<Mutex<Capture>>,
    limit: usize,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let capture = Arc::clone(data);
    let errors = Arc::clone(data);
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        config,
        move |frames: &[T], _| {
            let mut data = capture.lock().unwrap();
            if data.error.is_some() {
                return;
            }
            for frame in frames.chunks_exact(channels) {
                if data.samples.len() >= limit {
                    data.error = Some("Recording limit reached; audio discarded".into());
                    break;
                }
                data.samples.push(
                    frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / channels as f32,
                );
            }
        },
        move |error| {
            errors.lock().unwrap().error = Some(format!("Microphone error: {error}"));
        },
        None,
    )?)
}

pub fn resample(samples: &[f32], rate: u32) -> Result<Vec<f32>> {
    ensure!(rate > 0, "Invalid audio sample rate");
    if samples.is_empty() || rate == 16_000 {
        return Ok(samples.to_vec());
    }
    let mut resampler = FftFixedInOut::<f32>::new(rate as usize, 16_000, 1024, 1)?;
    let delay = resampler.output_delay();
    let length = (samples.len() as u64 * 16_000 / rate as u64) as usize;
    let mut result = Vec::with_capacity(length + delay + 2048);
    let mut offset = 0;
    while result.len() < length + delay {
        let size = resampler.input_frames_next();
        let mut block = vec![0.0; size];
        let count = size.min(samples.len().saturating_sub(offset));
        block[..count].copy_from_slice(&samples[offset..offset + count]);
        offset += count;
        result.extend_from_slice(&resampler.process(&[block], None)?[0]);
    }
    Ok(result[delay..delay + length].to_vec())
}

pub fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).context("Opening WAV")?;
    let spec = reader.spec();
    ensure!(spec.channels > 0, "WAV has no channels");
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 2f32.powi(spec.bits_per_sample as i32 - 1);
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = samples
        .chunks_exact(spec.channels as usize)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    resample(&mono, spec.sample_rate)
}

#[derive(Debug, Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
    Busy,
    Error,
}

pub struct Sounds {
    _stream: Stream,
    queue: Arc<Mutex<Feedback>>,
    rate: u32,
}

#[derive(Default)]
struct Feedback {
    samples: VecDeque<f32>,
    last_callback: Option<Instant>,
    error: Option<String>,
}

impl Feedback {
    fn render<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        self.last_callback = Some(Instant::now());
        for frame in data.chunks_exact_mut(channels) {
            frame.fill(T::from_sample(self.samples.pop_front().unwrap_or(0.0)));
        }
    }
}

impl Sounds {
    pub fn new() -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .context("No audio output for feedback")?;
        let supported = device.default_output_config()?;
        let config: StreamConfig = supported.clone().into();
        eprintln!(
            "Feedback output: device={:?}, rate={}, channels={}, format={:?}",
            device.name(),
            config.sample_rate.0,
            config.channels,
            supported.sample_format()
        );
        let queue = Arc::new(Mutex::new(Feedback::default()));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output::<f32>(&device, &config, &queue)?,
            SampleFormat::F64 => output::<f64>(&device, &config, &queue)?,
            SampleFormat::I16 => output::<i16>(&device, &config, &queue)?,
            SampleFormat::I32 => output::<i32>(&device, &config, &queue)?,
            SampleFormat::U16 => output::<u16>(&device, &config, &queue)?,
            format => bail!("Unsupported output format: {format}"),
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            queue,
            rate: config.sample_rate.0,
        })
    }

    pub fn play(&self, cue: Cue) {
        let hz = match cue {
            Cue::Start => 880.,
            Cue::Stop => 440.,
            Cue::Busy => 220.,
            Cue::Error => 140.,
        };
        let count = (self.rate as f32 * 0.09) as usize;
        let mut queue = self.queue.lock().unwrap();
        let pending = queue.samples.len();
        let callback_age = queue.last_callback.map(|time| time.elapsed().as_millis());
        let output_error = queue.error.clone();
        // Key repeat must not accumulate seconds of feedback sounds.
        queue.samples.clear();
        for i in 0..count {
            let envelope = ((i.min(count - 1 - i) as f32) / (self.rate as f32 * 0.008)).min(1.0);
            queue.samples.push_back(
                0.15 * envelope * (std::f32::consts::TAU * hz * i as f32 / self.rate as f32).sin(),
            );
        }
        drop(queue);
        eprintln!(
            "Feedback queued: cue={cue:?}, samples={count}, replaced_samples={pending}, callback_age_ms={callback_age:?}, output_error={output_error:?}"
        );
    }
}

fn output<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    queue: &Arc<Mutex<Feedback>>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let errors = Arc::clone(queue);
    let queue = Arc::clone(queue);
    let channels = config.channels as usize;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            queue.lock().unwrap().render(data, channels);
        },
        move |error| {
            errors.lock().unwrap().error = Some(error.to_string());
            eprintln!("Audio output error: {error}");
        },
        None,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_tracks_output_callbacks_and_drains_one_sample_per_frame() {
        for channels in [1, 2, 6] {
            let mut feedback = Feedback {
                samples: [0.25, -0.5].into(),
                ..Feedback::default()
            };
            assert!(feedback.last_callback.is_none());
            let mut first = vec![0.0_f32; channels];
            feedback.render(&mut first, channels);
            assert_eq!(first, vec![0.25; channels]);
            assert_eq!(feedback.samples.len(), 1);
            assert!(feedback.last_callback.is_some());
            let mut rest = vec![1.0_f32; channels * 2];
            feedback.render(&mut rest, channels);
            assert_eq!(&rest[..channels], vec![-0.5; channels]);
            assert_eq!(&rest[channels..], vec![0.0; channels]);
            assert!(feedback.samples.is_empty());
        }
    }

    #[test]
    fn volume_normalization_preserves_shape_and_original_samples() {
        let original = [-0.1, 0.0, 0.025, 0.05];
        let (output, levels) = normalize_volume(&original, 0.5, 10.0).unwrap();
        assert_eq!(output.as_ref(), [-0.5, 0.0, 0.125, 0.25]);
        assert_eq!(original, [-0.1, 0.0, 0.025, 0.05]);
        assert_eq!(levels.gain, 5.0);
        assert_eq!(levels.input_peak, 0.1);
        assert_eq!(levels.output_peak, 0.5);
    }

    #[test]
    fn volume_normalization_limits_gain_and_handles_loud_audio() {
        let (quiet, levels) = normalize_volume(&[0.001, -0.002], 0.5, 10.0).unwrap();
        assert_eq!(levels.gain, 10.0);
        assert!((quiet[1] + 0.02).abs() < 1e-7);
        let (loud, levels) = normalize_volume(&[-1.0, 0.5], 0.5, 10.0).unwrap();
        assert_eq!(loud.as_ref(), [-0.5, 0.25]);
        assert_eq!(levels.gain, 0.5);
    }

    #[test]
    fn volume_normalization_preserves_silence_and_rejects_invalid_audio() {
        for samples in [&[][..], &[0.0, 0.0], &[0.000001, -0.000001]] {
            let (output, levels) = normalize_volume(samples, 0.5, 10.0).unwrap();
            assert_eq!(output.as_ref(), samples);
            assert_eq!(levels.gain, 1.0);
        }
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(normalize_volume(&[sample], 0.5, 10.0).is_err());
        }
    }

    #[test]
    fn resampling_preserves_duration_and_speech_band() {
        for rate in [44_100, 48_000] {
            let input: Vec<f32> = (0..rate)
                .map(|i| (std::f32::consts::TAU * 1000. * i as f32 / rate as f32).sin())
                .collect();
            let output = resample(&input, rate).unwrap();
            assert_eq!(output.len(), 16_000);
            // Non-integral ratios can leave fractional-sample phase delay.
            // Check frequency and energy independently of that phase.
            let steady = &output[100..15900];
            let power = steady.iter().map(|x| x * x).sum::<f32>() / steady.len() as f32;
            assert!((power - 0.5).abs() < 0.005, "rate={rate}, power={power}");
            let crossings = steady
                .windows(2)
                .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
                .count();
            assert!((crossings as i32 - 987).abs() <= 2);
        }
    }

    #[test]
    fn resampling_handles_empty_short_and_native_audio() {
        assert!(resample(&[], 48_000).unwrap().is_empty());
        assert_eq!(resample(&[0.1, 0.2], 16_000).unwrap(), [0.1, 0.2]);
        assert_eq!(resample(&[0.1; 30], 48_000).unwrap().len(), 10);
        assert!(resample(&[0.1], 0).is_err());
    }

    #[test]
    fn resampling_filters_frequencies_above_output_nyquist() {
        let input: Vec<f32> = (0..48_000)
            .map(|i| (std::f32::consts::TAU * 10_000. * i as f32 / 48_000.).sin())
            .collect();
        let output = resample(&input, 48_000).unwrap();
        let power = output[100..15900].iter().map(|x| x * x).sum::<f32>() / 15800.;
        assert!(power < 0.001, "Aliased out-of-band signal: {power}");
    }
}
