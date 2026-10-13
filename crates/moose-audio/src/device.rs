//! The sound device, through `cpal`: the only file that names it. The device calls the
//! mixer for frames on a thread of its own.

use std::sync::mpsc::{Sender, channel};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::mixer::Mixer;

/// Frames the mixer is asked for at a time, when the device wants samples that aren't
/// floats (they are mixed as floats, then converted).
const CONVERT: usize = 2048;

/// An open device, playing. Dropping it stops the sound.
pub(crate) struct Output {
    /// What the device is called, and what it plays: frames per second and channels.
    pub name: String,
    pub rate: u32,
    pub channels: u16,
    /// The thread holding the stream ends when this is dropped. (The stream lives there
    /// because on some systems it can't be moved between threads, and this can.)
    _close: Sender<()>,
}

/// Opens the system's default output device as it is set up, and starts it playing what
/// the mixer `make` returns (given the device's frames per second) mixes.
pub(crate) fn open(make: impl FnOnce(u32) -> Mixer + Send + 'static) -> Result<Output, String> {
    let (ready, opened) = channel();
    let (close, closed) = channel::<()>();
    std::thread::Builder::new()
        .name("moose-audio".into())
        .spawn(move || match start(make) {
            Ok((stream, name, rate, channels)) => {
                let _ = ready.send(Ok((name, rate, channels)));
                // Until the `Output` is dropped.
                let _ = closed.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready.send(Err(e));
            }
        })
        .map_err(|e| format!("cannot start the sound thread: {e}"))?;
    let (name, rate, channels) = opened
        .recv()
        .map_err(|_| "the sound thread stopped before the device was open".to_string())??;
    Ok(Output { name, rate, channels, _close: close })
}

fn start(make: impl FnOnce(u32) -> Mixer) -> Result<(Stream, String, u32, u16), String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("there is no sound output device")?;
    let name = device.description().map_or_else(|_| "unnamed".to_string(), |d| d.name().to_string());
    let supported = device
        .default_output_config()
        .map_err(|e| format!("cannot ask the sound device {name} what it plays: {e}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let (rate, channels) = (config.sample_rate, config.channels);
    let mixer = make(rate);
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, config, mixer),
        SampleFormat::I16 => build::<i16>(&device, config, mixer),
        SampleFormat::U16 => build::<u16>(&device, config, mixer),
        SampleFormat::I32 => build::<i32>(&device, config, mixer),
        SampleFormat::U32 => build::<u32>(&device, config, mixer),
        SampleFormat::I8 => build::<i8>(&device, config, mixer),
        SampleFormat::U8 => build::<u8>(&device, config, mixer),
        SampleFormat::F64 => build::<f64>(&device, config, mixer),
        other => return Err(format!("the sound device {name} wants {other} samples, which aren't written")),
    }
    .and_then(|stream| stream.play().map(|()| stream))
    .map_err(|e| format!("cannot play on the sound device {name}: {e}"))?;
    Ok((stream, name, rate, channels))
}

fn build<T: SizedSample + FromSample<f32>>(device: &Device, config: StreamConfig, mut mixer: Mixer) -> Result<Stream, cpal::Error> {
    let channels = config.channels as usize;
    let mut mixed = vec![0.0f32; CONVERT * channels];
    device.build_output_stream(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            for out in out.chunks_mut(mixed.len()) {
                let mixed = &mut mixed[..out.len()];
                mixer.mix(mixed, channels);
                for (out, &x) in out.iter_mut().zip(mixed.iter()) {
                    *out = T::from_sample(x);
                }
            }
        },
        // (A late buffer is heard, and there is nothing to do about one here.)
        |e| {
            if e.kind() != ErrorKind::Xrun {
                eprintln!("sound: {e}");
            }
        },
        None,
    )
}
