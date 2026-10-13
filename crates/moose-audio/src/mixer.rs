//! The mixer: the part that runs on the sound device's thread, adding up the voices
//! playing into the frames the device asks for.
//!
//! It is told what to play through a queue and never waits for anything: it takes no
//! locks, and reserves no memory after it is made.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

/// How many sounds are mixed at once. The game keeps to this (see `Audio`): of the sounds
/// playing, the loudest get the voices.
pub const MAX_VOICES: usize = 32;
/// Voices kept beyond those, for the ones fading out as others start.
const SPARE_VOICES: usize = 16;
/// Commands that can wait for the mixer at once.
const QUEUE: usize = 1024;
/// A cutoff at or above this, in Hz, is no filter at all.
pub const OPEN: f32 = 20_000.0;
/// How long a voice takes to reach what it is newly heard as, in seconds: about the time
/// between two frames of a slow game, so a sound moving past glides and never clicks.
const HEAR_TIME: f32 = 0.04;
/// How long a stopped voice takes to fade out, in seconds.
const STOP_TIME: f32 = 0.01;
/// Frames mixed at a time (the device may ask for any number).
const BLOCK: usize = 512;
/// Added to each filter's memory every frame, so it never fades to numbers so small the
/// processor handles them slowly (denormals).
const TINY: f32 = 1e-18;

/// How a voice is heard: how loud in each ear, and how muffled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// What its samples are multiplied by for the left and right ear.
    pub left: f32,
    pub right: f32,
    /// The pitch above which it fades away, in Hz (a low-pass filter); [`OPEN`] or more
    /// for all of it.
    pub cutoff: f32,
}

impl Heard {
    pub const SILENT: Heard = Heard { left: 0.0, right: 0.0, cutoff: OPEN };

    /// The louder ear.
    pub fn loudness(&self) -> f32 {
        self.left.max(self.right)
    }
}

/// What the game tells the mixer.
pub(crate) enum Command {
    Start {
        id: u32,
        samples: Arc<[i16]>,
        channels: u8,
        /// The sound's own frames per second, times the pitch it plays at.
        rate: f32,
        /// The frame to start from.
        from: f64,
        looped: bool,
        /// It is heard from a place: played as mono, a stereo sound's channels averaged.
        placed: bool,
        heard: Heard,
    },
    Hear { id: u32, heard: Heard },
    Stop { id: u32 },
    StopAll,
    Volume(f32),
}

struct Voice {
    id: u32,
    samples: Arc<[i16]>,
    channels: usize,
    frames: u64,
    /// Where it is in its samples and how far it moves per frame mixed, in frames, as
    /// 32.32 fixed point.
    at: u64,
    step: u64,
    looped: bool,
    placed: bool,
    /// Left gain, right gain and the filter's coefficient (1 lets everything through):
    /// what they are, what they change by per frame, and what they're heading for.
    now: [f32; 3],
    delta: [f32; 3],
    target: [f32; 3],
    /// Frames until `now` reaches `target`.
    ramp: u32,
    /// The filter's memory, per channel.
    low: [f32; 2],
    /// It is fading out, and is done when it has.
    stopping: bool,
}

pub struct Mixer {
    rate: f32,
    commands: Receiver<Command>,
    voices: Vec<Voice>,
    /// Everything's volume: now, per-frame change, target and frames left, as a voice's.
    volume: (f32, f32, f32, u32),
    /// A block of stereo frames the voices are added up in.
    block: Vec<f32>,
}

impl Mixer {
    /// A mixer for a device playing `rate` frames a second, and the queue to tell it
    /// things through.
    pub(crate) fn new(rate: u32) -> (Mixer, SyncSender<Command>) {
        let (sender, commands) = sync_channel(QUEUE);
        let mixer = Mixer {
            rate: rate as f32,
            commands,
            voices: Vec::with_capacity(MAX_VOICES + SPARE_VOICES),
            volume: (1.0, 0.0, 1.0, 0),
            block: vec![0.0; BLOCK * 2],
        };
        (mixer, sender)
    }

    /// The filter coefficient for a cutoff in Hz: how far the filter's memory moves toward
    /// each new sample.
    fn pass(&self, cutoff: f32) -> f32 {
        if cutoff >= OPEN {
            1.0
        } else {
            1.0 - (-std::f32::consts::TAU * cutoff.max(10.0) / self.rate).exp()
        }
    }

    fn targets(&self, heard: Heard) -> [f32; 3] {
        [heard.left, heard.right, self.pass(heard.cutoff)]
    }

    fn obey(&mut self, command: Command) {
        match command {
            Command::Start { id, samples, channels, rate, from, looped, placed, heard } => {
                let channels = channels.max(1) as usize;
                let frames = (samples.len() / channels) as u64;
                // (Dropping what can't be played frees nothing: the game still has it.)
                if frames == 0 || self.voices.len() >= MAX_VOICES + SPARE_VOICES {
                    return;
                }
                let mut at = (from.max(0.0) * 4294967296.0) as u64;
                if at >= frames << 32 {
                    if !looped {
                        return;
                    }
                    at %= frames << 32;
                }
                let now = self.targets(heard);
                self.voices.push(Voice {
                    id,
                    samples,
                    channels,
                    frames,
                    at,
                    step: ((rate / self.rate) as f64 * 4294967296.0) as u64,
                    looped,
                    placed,
                    now,
                    delta: [0.0; 3],
                    target: now,
                    ramp: 0,
                    low: [0.0; 2],
                    stopping: false,
                });
            }
            Command::Hear { id, heard } => {
                let (target, frames) = (self.targets(heard), (HEAR_TIME * self.rate) as u32);
                if let Some(voice) = self.voices.iter_mut().find(|v| v.id == id && !v.stopping) {
                    voice.head_for(target, frames);
                }
            }
            Command::Stop { id } => {
                let frames = (STOP_TIME * self.rate) as u32;
                if let Some(voice) = self.voices.iter_mut().find(|v| v.id == id && !v.stopping) {
                    voice.stop(frames);
                }
            }
            Command::StopAll => {
                let frames = (STOP_TIME * self.rate) as u32;
                for voice in self.voices.iter_mut().filter(|v| !v.stopping) {
                    voice.stop(frames);
                }
            }
            Command::Volume(volume) => {
                let frames = ((HEAR_TIME * self.rate) as u32).max(1);
                let (now, ..) = self.volume;
                self.volume = (now, (volume - now) / frames as f32, volume, frames);
            }
        }
    }

    /// Fills `out` with the next frames: `channels` samples each, in -1..1. Mono gets the
    /// two ears' average; of more than two channels, the first two get the ears and the
    /// rest silence.
    pub fn mix(&mut self, out: &mut [f32], channels: usize) {
        while let Ok(command) = self.commands.try_recv() {
            self.obey(command);
        }
        let channels = channels.max(1);
        for out in out.chunks_mut(BLOCK * channels) {
            let frames = out.len() / channels;
            let block = &mut self.block[..frames * 2];
            block.fill(0.0);
            for voice in &mut self.voices {
                voice.add_to(block);
            }
            self.voices.retain(|v| !v.done());
            let (mut volume, delta, target, mut ramp) = self.volume;
            for (frame, ears) in out.chunks_mut(channels).zip(block.chunks_exact(2)) {
                if ramp > 0 {
                    ramp -= 1;
                    volume = if ramp == 0 { target } else { volume + delta };
                }
                let (left, right) = ((ears[0] * volume).clamp(-1.0, 1.0), (ears[1] * volume).clamp(-1.0, 1.0));
                match frame {
                    [mono] => *mono = (left + right) * 0.5,
                    [l, r, rest @ ..] => {
                        (*l, *r) = (left, right);
                        rest.fill(0.0);
                    }
                    [] => {}
                }
            }
            self.volume = (volume, delta, target, ramp);
        }
    }

    /// How many voices are sounding (fading ones too).
    pub fn voices(&self) -> usize {
        self.voices.len()
    }
}

impl Voice {
    fn head_for(&mut self, target: [f32; 3], frames: u32) {
        let frames = frames.max(1);
        self.target = target;
        self.delta = std::array::from_fn(|k| (target[k] - self.now[k]) / frames as f32);
        self.ramp = frames;
    }

    fn stop(&mut self, frames: u32) {
        // (The filter stays as it is: only the loudness fades.)
        self.head_for([0.0, 0.0, self.now[2]], frames);
        self.stopping = true;
    }

    fn done(&self) -> bool {
        (self.stopping && self.ramp == 0) || (!self.looped && self.at >= self.frames << 32)
    }

    /// The sample of `channel` at frame `i`; past the end, the start again if it loops,
    /// or silence.
    fn sample(&self, i: u64, channel: usize) -> f32 {
        let i = if i < self.frames {
            i
        } else if self.looped {
            i % self.frames
        } else {
            return 0.0;
        };
        self.samples[i as usize * self.channels + channel] as f32 * (1.0 / 32768.0)
    }

    /// Adds its next frames to `block` (stereo frames), moving on through its samples.
    fn add_to(&mut self, block: &mut [f32]) {
        let end = self.frames << 32;
        // Its own two channels go to the two ears only if it has two and isn't placed.
        let stereo = self.channels == 2 && !self.placed;
        let both = self.channels == 2 && self.placed;
        for ears in block.chunks_exact_mut(2) {
            if self.at >= end {
                if !self.looped {
                    break;
                }
                self.at -= end;
            }
            if self.stopping && self.ramp == 0 {
                break;
            }
            // Between its two nearest frames, by how far it is from the first to the second.
            let (i, t) = (self.at >> 32, (self.at & 0xffff_ffff) as f32 * (1.0 / 4294967296.0));
            let at = |channel| {
                let (a, b) = (self.sample(i, channel), self.sample(i + 1, channel));
                a + (b - a) * t
            };
            let [left, right, pass] = self.now;
            if stereo {
                let (l, r) = (at(0), at(1));
                self.low = [filter(self.low[0], l, pass), filter(self.low[1], r, pass)];
                ears[0] += self.low[0] * left;
                ears[1] += self.low[1] * right;
            } else {
                let x = if both { (at(0) + at(1)) * 0.5 } else { at(0) };
                self.low[0] = filter(self.low[0], x, pass);
                ears[0] += self.low[0] * left;
                ears[1] += self.low[0] * right;
            }
            self.at += self.step;
            if self.ramp > 0 {
                self.ramp -= 1;
                for k in 0..3 {
                    self.now[k] = if self.ramp == 0 { self.target[k] } else { self.now[k] + self.delta[k] };
                }
            }
        }
    }
}

/// One step of the low-pass filter: its memory `low` moves toward the sample `x` by `pass`
/// of the way (all the way at 1: no filter).
fn filter(low: f32, x: f32, pass: f32) -> f32 {
    if pass < 1.0 { low + pass * (x - low) + TINY } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn heard(left: f32, right: f32) -> Heard {
        Heard { left, right, cutoff: OPEN }
    }

    fn start(id: u32, samples: &[i16], channels: u8, rate: f32, looped: bool, heard: Heard) -> Command {
        Command::Start { id, samples: samples.into(), channels, rate, from: 0.0, looped, placed: channels == 1, heard }
    }

    /// `frames` stereo frames from the mixer.
    fn mix(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0; frames * 2];
        mixer.mix(&mut out, 2);
        out
    }

    /// A sine wave of `hz`, a second long at `RATE`, at half of full loudness.
    fn sine(hz: f32) -> Vec<i16> {
        (0..RATE)
            .map(|i| ((i as f32 * hz * std::f32::consts::TAU / RATE as f32).sin() * 16384.0) as i16)
            .collect()
    }

    /// The loudest sample, either ear.
    fn peak(frames: &[f32]) -> f32 {
        frames.iter().fold(0.0, |m, x| m.max(x.abs()))
    }

    #[test]
    fn with_nothing_playing_it_is_silent() {
        let (mut mixer, _sender) = Mixer::new(RATE);
        assert!(mix(&mut mixer, 1000).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn a_voice_plays_its_samples_at_each_ears_gain_and_then_ends() {
        let (mut mixer, sender) = Mixer::new(RATE);
        sender.try_send(start(1, &[16384, -16384, 8192], 1, RATE as f32, false, heard(1.0, 0.5))).ok().unwrap();
        let out = mix(&mut mixer, 5);
        let expected = [0.5, 0.25, -0.5, -0.25, 0.25, 0.125, 0.0, 0.0, 0.0, 0.0];
        for (got, want) in out.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{out:?}");
        }
        assert_eq!(mixer.voices(), 0);
    }

    #[test]
    fn a_sound_at_half_the_devices_rate_is_stretched_between_its_samples() {
        let (mut mixer, sender) = Mixer::new(RATE);
        sender.try_send(start(1, &[0, 16384, 0], 1, RATE as f32 / 2.0, false, heard(1.0, 1.0))).ok().unwrap();
        let left: Vec<f32> = mix(&mut mixer, 7).chunks(2).map(|f| f[0]).collect();
        for (got, want) in left.iter().zip([0.0, 0.25, 0.5, 0.25, 0.0, 0.0, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
    }

    #[test]
    fn a_looping_voice_goes_round_until_stopped_and_then_fades_without_a_jump() {
        let (mut mixer, sender) = Mixer::new(RATE);
        sender.try_send(start(7, &[16384; 100], 1, RATE as f32, true, heard(1.0, 1.0))).ok().unwrap();
        let out = mix(&mut mixer, 1000);
        assert!(out.iter().all(|&x| (x - 0.5).abs() < 1e-6));
        sender.try_send(Command::Stop { id: 7 }).ok().unwrap();
        let out: Vec<f32> = mix(&mut mixer, 1000).chunks(2).map(|f| f[0]).collect();
        let fade = (STOP_TIME * RATE as f32) as usize;
        assert!(out.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] < 0.5 / fade as f32 + 1e-5), "it steps down evenly");
        assert!(out[fade / 2] > 0.2 && out[fade / 2] < 0.3);
        assert!(out[fade..].iter().all(|&x| x == 0.0));
        assert_eq!(mixer.voices(), 0);
    }

    #[test]
    fn being_heard_anew_is_reached_gradually() {
        let (mut mixer, sender) = Mixer::new(RATE);
        sender.try_send(start(1, &[16384; 100], 1, RATE as f32, true, heard(1.0, 1.0))).ok().unwrap();
        mix(&mut mixer, 10);
        sender.try_send(Command::Hear { id: 1, heard: heard(0.0, 1.0) }).ok().unwrap();
        let out = mix(&mut mixer, 4000);
        let ramp = (HEAR_TIME * RATE as f32) as usize;
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        // (The last step lands on the target exactly, taking up the steps' rounding.)
        let steepest = left.windows(2).map(|w| w[0] - w[1]).fold(0.0, f32::max);
        assert!(steepest < 0.5 / ramp as f32 + 1e-5, "{steepest}");
        assert!((left[ramp / 2] - 0.25).abs() < 0.01);
        assert!(left[ramp..].iter().all(|&x| x == 0.0));
        assert!(out.chunks(2).all(|f| (f[1] - 0.5).abs() < 1e-6), "the other ear stays as it was");
    }

    #[test]
    fn a_low_cutoff_takes_away_high_pitches_and_leaves_low_ones() {
        let loudness = |hz: f32, cutoff: f32| {
            let (mut mixer, sender) = Mixer::new(RATE);
            let heard = Heard { left: 1.0, right: 1.0, cutoff };
            sender.try_send(start(1, &sine(hz), 1, RATE as f32, false, heard)).ok().unwrap();
            peak(&mix(&mut mixer, RATE as usize / 2)[RATE as usize / 2..])
        };
        // (At a quarter of the device's rate, every other sample is a peak.)
        assert!((loudness(12000.0, OPEN) - 0.5).abs() < 0.01, "{}", loudness(12000.0, OPEN));
        assert!(loudness(12000.0, 500.0) < 0.05, "{}", loudness(12000.0, 500.0));
        assert!(loudness(100.0, 500.0) > 0.45, "{}", loudness(100.0, 500.0));
    }

    #[test]
    fn a_stereo_sound_keeps_its_channels_unless_it_is_placed() {
        let (mut mixer, sender) = Mixer::new(RATE);
        let samples: Arc<[i16]> = [16384i16, 0, 16384, 0].into();
        let command = |id, placed| Command::Start {
            id,
            samples: samples.clone(),
            channels: 2,
            rate: RATE as f32,
            from: 0.0,
            looped: false,
            placed,
            heard: heard(1.0, 1.0),
        };
        sender.try_send(command(1, false)).ok().unwrap();
        assert_eq!(mix(&mut mixer, 2), [0.5, 0.0, 0.5, 0.0]);
        sender.try_send(command(2, true)).ok().unwrap();
        assert_eq!(mix(&mut mixer, 2), [0.25, 0.25, 0.25, 0.25]);
    }

    #[test]
    fn a_voice_can_start_part_way_through() {
        let (mut mixer, sender) = Mixer::new(RATE);
        let command = Command::Start {
            id: 1,
            samples: [0i16, 0, 16384, 8192].into(),
            channels: 1,
            rate: RATE as f32,
            from: 2.0,
            looped: false,
            placed: true,
            heard: heard(1.0, 1.0),
        };
        sender.try_send(command).ok().unwrap();
        assert_eq!(mix(&mut mixer, 3), [0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn too_loud_a_mix_is_held_at_full_and_the_volume_scales_it() {
        let (mut mixer, sender) = Mixer::new(RATE);
        for id in 0..4 {
            sender.try_send(start(id, &[i16::MAX; 20000], 1, RATE as f32, false, heard(1.0, 1.0))).ok().unwrap();
        }
        assert!(mix(&mut mixer, 100).iter().all(|&x| x == 1.0));
        sender.try_send(Command::Volume(0.125)).ok().unwrap();
        let out = mix(&mut mixer, 4000);
        assert!((out.last().unwrap() - 0.5).abs() < 1e-3, "{:?}", out.last());
    }

    #[test]
    fn mono_and_surround_devices_get_the_two_ears() {
        let (mut mixer, sender) = Mixer::new(RATE);
        sender.try_send(start(1, &[16384; 4], 1, RATE as f32, false, heard(1.0, 0.0))).ok().unwrap();
        let mut mono = [9.0; 2];
        mixer.mix(&mut mono, 1);
        assert_eq!(mono, [0.25, 0.25]);
        let mut six = [9.0; 12];
        mixer.mix(&mut six, 6);
        assert_eq!(six, [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn more_voices_than_it_holds_are_left_out_not_crashed_on() {
        let (mut mixer, sender) = Mixer::new(RATE);
        for id in 0..(MAX_VOICES + SPARE_VOICES + 5) as u32 {
            sender.try_send(start(id, &[1; 20000], 1, RATE as f32, false, heard(1.0, 1.0))).ok().unwrap();
        }
        mix(&mut mixer, 10);
        assert_eq!(mixer.voices(), MAX_VOICES + SPARE_VOICES);
        sender.try_send(Command::StopAll).ok().unwrap();
        mix(&mut mixer, 1000);
        assert_eq!(mixer.voices(), 0);
    }
}
