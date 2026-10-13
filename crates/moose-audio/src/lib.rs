//! Sound for the Moose engine (see the Sound Spec).
//!
//! [`Audio`] is the whole of it to the rest of the engine: sounds are played, moved and
//! stopped through it, and once a frame it is told where the listener is. What triggers a
//! sound (an event in the engine, a script) is the caller's business.
//!
//! Underneath are a software mixer ([`Mixer`]) running on the sound device's thread, and
//! the level's own geometry: a sound reaches the listener through the portals between its
//! sector and theirs, not through walls (see [`Routes`]).

mod device;
mod hear;
mod mixer;

use std::sync::mpsc::SyncSender;

use glam::Vec3;
use moose_assets::{Assets, SoundId};
use moose_scene::World;

pub use hear::{Listener, Route, Routes, heard};
pub use mixer::{Heard, MAX_VOICES, Mixer, OPEN};

use mixer::Command;

/// How close a sound is at its full loudness, and how far it carries, in meters, unless
/// it says (see [`Play::carrying`]).
pub const NEAR: f32 = 2.0;
pub const REACH: f32 = 50.0;
/// Quieter than this in both ears, a sound isn't mixed (it still plays on, unheard: see
/// [`Audio::update`]). About -60 dB.
const QUIET: f32 = 0.001;
/// How much louder a sound without a voice must be than one with, to take it: so two
/// about as loud as each other don't keep swapping.
const TAKEOVER: f32 = 1.25;

/// Where a sound is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// Nowhere in the world: heard the same wherever the listener is (the player's own
    /// sounds, the interface, music). A stereo sound keeps its two channels.
    Everywhere,
    /// At a point in a sector.
    At { sector: u32, position: Vec3 },
    /// Where an entity is (index in `World::entities`), following it.
    Entity(u32),
}

/// A sound to play, and how. Start from [`Play::new`] and change what differs:
/// `Play::new(sound).at(sector, position).volume(0.5)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Play {
    pub sound: SoundId,
    /// Its loudness: 1 as recorded.
    pub volume: f32,
    /// Its speed, which is its pitch: 1 as recorded, 2 an octave up in half the time.
    pub pitch: f32,
    /// It starts again when it ends, until stopped.
    pub looped: bool,
    pub place: Place,
    /// How close it is at its full loudness, and how far it carries, in meters, along the
    /// way it takes to the listener. Only for a sound with a place.
    pub near: f32,
    pub reach: f32,
}

impl Play {
    /// `sound` once through, as recorded, heard everywhere.
    pub fn new(sound: SoundId) -> Play {
        Play {
            sound,
            volume: 1.0,
            pitch: 1.0,
            looped: false,
            place: Place::Everywhere,
            near: NEAR,
            reach: REACH,
        }
    }

    pub fn at(self, sector: u32, position: Vec3) -> Play {
        Play { place: Place::At { sector, position }, ..self }
    }

    /// On entity `entity` (index in `World::entities`), following it.
    pub fn on(self, entity: u32) -> Play {
        Play { place: Place::Entity(entity), ..self }
    }

    pub fn volume(self, volume: f32) -> Play {
        Play { volume, ..self }
    }

    pub fn pitch(self, pitch: f32) -> Play {
        Play { pitch, ..self }
    }

    pub fn looped(self) -> Play {
        Play { looped: true, ..self }
    }

    pub fn carrying(self, near: f32, reach: f32) -> Play {
        Play { near, reach, ..self }
    }
}

/// A sound that was played: for asking after, moving and stopping it. It stays good (and
/// says the sound isn't playing) after the sound has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Playing(u32);

/// A sound playing.
struct Emitter {
    id: u32,
    play: Play,
    /// How far into the sound it is, in seconds of the sound.
    at: f32,
    /// It has been through an update: time passes for it from its first.
    begun: bool,
    /// How it is heard, as of the last update.
    heard: Heard,
    /// What the mixer was last told it is heard as, if it has a voice there.
    voice: Option<Heard>,
}

/// The sound engine.
pub struct Audio {
    /// The queue to the mixer, if there is one.
    commands: Option<SyncSender<Command>>,
    output: Option<device::Output>,
    /// Why there is no sound, if a device couldn't be opened.
    problem: Option<String>,
    emitters: Vec<Emitter>,
    /// Voices to stop that the mixer hasn't been told of (its queue was full).
    stops: Vec<u32>,
    last_id: u32,
    volume: f32,
    routes: Routes,
    /// The sounds loud enough to mix (indices in `emitters`), loudest first.
    loudest: Vec<u32>,
}

impl Audio {
    /// No device: nothing is heard, but sounds still play for as long as they last, so
    /// whatever waits for one to end isn't kept waiting.
    pub fn silent() -> Audio {
        Audio {
            commands: None,
            output: None,
            problem: None,
            emitters: Vec::new(),
            stops: Vec::new(),
            last_id: 0,
            volume: 1.0,
            routes: Routes::default(),
            loudest: Vec::new(),
        }
    }

    /// Opens the system's default sound device and starts mixing on it. If that can't be
    /// done this is as [`silent`](Self::silent), and [`problem`](Self::problem) says why.
    pub fn open() -> Audio {
        let (sent, queue) = std::sync::mpsc::channel();
        let output = device::open(move |rate| {
            let (mixer, commands) = Mixer::new(rate);
            let _ = sent.send(commands);
            mixer
        });
        match output {
            Ok(output) => Audio { commands: queue.recv().ok(), output: Some(output), ..Audio::silent() },
            Err(problem) => Audio { problem: Some(problem), ..Audio::silent() },
        }
    }

    /// No device, but a mixer the caller asks for frames (at `rate` a second) itself: for
    /// tests, and for writing what would be heard to a file.
    pub fn offline(rate: u32) -> (Audio, Mixer) {
        let (mixer, commands) = Mixer::new(rate);
        (Audio { commands: Some(commands), ..Audio::silent() }, mixer)
    }

    /// The open device: its name, frames per second and channels.
    pub fn device(&self) -> Option<(&str, u32, u16)> {
        self.output.as_ref().map(|o| (o.name.as_str(), o.rate, o.channels))
    }

    /// Why there is no sound, if [`open`](Self::open) couldn't open a device.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Starts a sound. It is first heard at the next [`update`](Self::update).
    pub fn play(&mut self, play: Play) -> Playing {
        self.last_id = self.last_id.wrapping_add(1);
        self.emitters.push(Emitter {
            id: self.last_id,
            play,
            at: 0.0,
            begun: false,
            heard: Heard::SILENT,
            voice: None,
        });
        Playing(self.last_id)
    }

    /// Whether the sound is still playing (heard or not).
    pub fn is_playing(&self, playing: Playing) -> bool {
        self.emitters.iter().any(|e| e.id == playing.0)
    }

    /// Stops a sound (with a fade too short to hear, so it doesn't click). Nothing
    /// happens if it has ended.
    pub fn stop(&mut self, playing: Playing) {
        if let Some(i) = self.emitters.iter().position(|e| e.id == playing.0)
            && self.emitters.swap_remove(i).voice.is_some()
        {
            self.stops.push(playing.0);
            self.send_stops();
        }
    }

    /// Stops every sound: as when the level is left.
    pub fn stop_all(&mut self) {
        self.emitters.clear();
        self.stops.clear();
        if let Some(commands) = &self.commands {
            let _ = commands.try_send(Command::StopAll);
        }
    }

    /// Moves a playing sound.
    pub fn set_place(&mut self, playing: Playing, place: Place) {
        if let Some(e) = self.emitters.iter_mut().find(|e| e.id == playing.0) {
            e.play.place = place;
        }
    }

    /// Changes a playing sound's loudness (see [`Play::volume`]).
    pub fn set_sound_volume(&mut self, playing: Playing, volume: f32) {
        if let Some(e) = self.emitters.iter_mut().find(|e| e.id == playing.0) {
            e.play.volume = volume;
        }
    }

    /// The loudness of everything: 1 as mixed.
    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        if let Some(commands) = &self.commands {
            let _ = commands.try_send(Command::Volume(volume));
        }
    }

    /// How many sounds are playing, and how many of those are being mixed (the rest are
    /// out of hearing, or quieter than [`MAX_VOICES`] others).
    pub fn counts(&self) -> (usize, usize) {
        (self.emitters.len(), self.emitters.iter().filter(|e| e.voice.is_some()).count())
    }

    /// How a playing sound was heard at the last update.
    pub fn heard(&self, playing: Playing) -> Option<Heard> {
        self.emitters.iter().find(|e| e.id == playing.0).map(|e| e.heard)
    }

    /// Once a frame, `dt` seconds after the last: moves every sound on by that, drops the
    /// ones that have ended, works out how the listener hears each of the rest, and tells
    /// the mixer.
    ///
    /// Only the [`MAX_VOICES`] loudest are mixed. The others play on unheard, and are
    /// mixed from wherever they have got to when they can be heard: walking up to a
    /// looping sound, or round a corner in the middle of a long one.
    pub fn update(&mut self, world: &World, assets: &Assets, listener: &Listener, dt: f32) {
        let Audio { commands, emitters, stops, routes, loudest, .. } = self;
        let positions = &assets.mesh(world.geometry).positions;
        emitters.retain_mut(|e| {
            let play = e.play;
            let place = match play.place {
                Place::Everywhere => Some(None),
                Place::At { sector, position } => Some(Some((sector, position))),
                Place::Entity(i) => world.entities.get(i as usize).map(|at| Some((at.sector, at.position))),
            };
            let seconds = assets.sounds().get(play.sound.0 as usize).map_or(0.0, |s| s.seconds());
            if e.begun {
                e.at += dt * play.pitch.max(0.0);
            }
            e.begun = true;
            // What can't go on: no such entity or sound (or an empty one) any more.
            let (Some(place), true) = (place, seconds > 0.0) else {
                if e.voice.is_some() {
                    stops.push(e.id);
                }
                return false;
            };
            if play.looped {
                e.at %= seconds;
            } else if e.at >= seconds {
                // Its voice has ended too, or is about to.
                return false;
            }
            e.heard = match place {
                _ if commands.is_none() => Heard::SILENT,
                None => Heard { left: play.volume, right: play.volume, cutoff: OPEN },
                Some(source) => routes
                    .route(world, positions, (listener.sector, listener.position), source, play.reach)
                    .map_or(Heard::SILENT, |route| heard(listener, &route, play.volume, play.near, play.reach)),
            };
            true
        });
        let Some(commands) = commands else {
            return;
        };
        // The loudest get the voices, those that have one counting as a little louder.
        let loudness = |e: &Emitter| e.heard.loudness() * if e.voice.is_some() { TAKEOVER } else { 1.0 };
        loudest.clear();
        loudest.extend((0..emitters.len() as u32).filter(|&i| emitters[i as usize].heard.loudness() >= QUIET));
        loudest.sort_unstable_by(|&a, &b| loudness(&emitters[b as usize]).total_cmp(&loudness(&emitters[a as usize])));
        loudest.truncate(MAX_VOICES);
        // First the voices given up, then the ones taken.
        for (i, e) in emitters.iter_mut().enumerate() {
            if e.voice.is_some() && !loudest.contains(&(i as u32)) {
                e.voice = None;
                stops.push(e.id);
            }
        }
        stops.retain(|&id| commands.try_send(Command::Stop { id }).is_err());
        for &i in loudest.iter() {
            let e = &mut emitters[i as usize];
            let command = match e.voice {
                Some(told) if told == e.heard => continue,
                Some(_) => Command::Hear { id: e.id, heard: e.heard },
                None => {
                    let sound = assets.sound(e.play.sound);
                    Command::Start {
                        id: e.id,
                        samples: sound.samples.clone(),
                        channels: sound.channels,
                        rate: sound.rate as f32 * e.play.pitch.max(0.0),
                        from: e.at as f64 * sound.rate as f64,
                        looped: e.play.looped,
                        placed: e.play.place != Place::Everywhere,
                        heard: e.heard,
                    }
                }
            };
            // (If the queue is full, it is told at the next update.)
            if commands.try_send(command).is_ok() {
                e.voice = Some(e.heard);
            }
        }
    }

    fn send_stops(&mut self) {
        if let Some(commands) = &self.commands {
            self.stops.retain(|&id| commands.try_send(Command::Stop { id }).is_err());
        } else {
            self.stops.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moose_assets::Sound;

    const RATE: u32 = 48_000;

    /// `two_rooms.mmp` (see `hear`'s tests for its plan), with a one-second tone loaded.
    fn two_rooms() -> (World, Assets, SoundId) {
        let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let level = assets.load_level("two_rooms.mmp").unwrap();
        let world = World::new(level, &assets);
        let wave = (0..RATE).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5);
        let tone = assets.add_sound(Sound::mono("tone", RATE, wave));
        (world, assets, tone)
    }

    /// In room_a, facing the doorway (-Z): the right ear points along +X.
    fn listener(x: f32, z: f32) -> Listener {
        Listener { sector: 0, position: Vec3::new(x, 1.7, z), right: Vec3::X }
    }

    /// The loudest sample in each ear over the next `frames`.
    fn peaks(mixer: &mut Mixer, frames: usize) -> (f32, f32) {
        let mut out = vec![0.0; frames * 2];
        mixer.mix(&mut out, 2);
        out.chunks(2).fold((0.0f32, 0.0f32), |(l, r), f| (l.max(f[0].abs()), r.max(f[1].abs())))
    }

    #[test]
    #[ignore = "needs a sound device (it plays nothing)"]
    fn the_default_device_opens() {
        let audio = Audio::open();
        let (name, rate, channels) = audio.device().unwrap_or_else(|| panic!("{:?}", audio.problem()));
        println!("{name}: {rate} Hz, {channels} channels");
        assert!(rate >= 8000 && channels >= 1);
        // Long enough for the device to ask for frames.
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    #[test]
    fn a_sound_plays_for_as_long_as_it_is_even_with_no_device() {
        let (world, assets, tone) = two_rooms();
        let mut audio = Audio::silent();
        let once = audio.play(Play::new(tone));
        let fast = audio.play(Play::new(tone).pitch(2.0));
        let looping = audio.play(Play::new(tone).looped());
        // (Time starts for a sound at its first update.)
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.3);
        for _ in 0..2 {
            audio.update(&world, &assets, &listener(0.0, 6.0), 0.3);
        }
        assert!(audio.is_playing(once) && !audio.is_playing(fast) && audio.is_playing(looping));
        for _ in 0..2 {
            audio.update(&world, &assets, &listener(0.0, 6.0), 0.3);
        }
        assert!(!audio.is_playing(once) && audio.is_playing(looping));
        audio.stop(looping);
        assert!(!audio.is_playing(looping));
        assert_eq!(audio.counts(), (0, 0));
    }

    #[test]
    fn a_sound_to_one_side_is_louder_in_that_ear_and_a_farther_one_is_quieter() {
        let (world, assets, tone) = two_rooms();
        let (mut audio, mut mixer) = Audio::offline(RATE);
        let playing = audio.play(Play::new(tone).at(0, Vec3::new(3.0, 1.7, 6.0)).looped());
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        let (left, right) = peaks(&mut mixer, 4800);
        assert!(right > 0.4 && left < 0.1 && left > 0.01, "{left} {right}");
        // Round to the listener's left, and further off.
        audio.set_place(playing, Place::At { sector: 0, position: Vec3::new(-3.5, 1.7, 1.0) });
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.1);
        peaks(&mut mixer, 4800);
        let (left_now, right_now) = peaks(&mut mixer, 4800);
        assert!(left_now > right_now && left_now < right, "{left_now} {right_now}");
    }

    #[test]
    fn a_sound_with_no_place_is_as_recorded_in_both_ears() {
        let (world, assets, tone) = two_rooms();
        let (mut audio, mut mixer) = Audio::offline(RATE);
        audio.play(Play::new(tone).volume(0.5));
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        let (left, right) = peaks(&mut mixer, 4800);
        assert!((left - 0.25).abs() < 1e-3 && left == right, "{left} {right}");
    }

    #[test]
    fn round_a_corner_a_sound_is_heard_from_the_doorway_and_muffled() {
        let (world, assets, tone) = two_rooms();
        let (mut audio, _mixer) = Audio::offline(RATE);
        // In room_b, off to the listener's right through the wall; the doorway out of
        // room_a is to their left.
        let source = Vec3::new(3.0, 1.5, -13.0);
        let playing = audio.play(Play::new(tone).at(2, source).looped());
        let ear = Listener { sector: 0, position: Vec3::new(3.0, 1.5, 1.0), right: Vec3::X };
        audio.update(&world, &assets, &ear, 0.0);
        let heard = audio.heard(playing).unwrap();
        assert!(heard.left > heard.right * 2.0 && heard.cutoff < 10_000.0, "{heard:?}");
        // In line with both doorways it is clear, and in the middle.
        let ear = Listener { sector: 0, position: Vec3::new(0.0, 1.5, 6.0), right: Vec3::X };
        audio.set_place(playing, Place::At { sector: 2, position: Vec3::new(0.0, 1.5, -16.0) });
        audio.update(&world, &assets, &ear, 0.0);
        let clear = audio.heard(playing).unwrap();
        assert!((clear.left - clear.right).abs() < 1e-4 && clear.cutoff == OPEN, "{clear:?}");
    }

    #[test]
    fn a_sound_out_of_hearing_plays_on_and_is_mixed_from_where_it_has_got_to() {
        let (world, assets, tone) = two_rooms();
        let (mut audio, mut mixer) = Audio::offline(RATE);
        // In room_b, carrying 10 m: not as far as room_a.
        let far = Play::new(tone).at(2, Vec3::new(0.0, 1.5, -16.0)).carrying(2.0, 10.0);
        let playing = audio.play(far);
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        assert_eq!((audio.counts(), peaks(&mut mixer, 480)), ((1, 0), (0.0, 0.0)));
        // Half a second on, the listener is in the hallway by it: half of it is left.
        let near = Listener { sector: 1, position: Vec3::new(0.0, 1.5, -11.0), right: Vec3::X };
        audio.update(&world, &assets, &near, 0.5);
        assert_eq!(audio.counts(), (1, 1));
        assert!(peaks(&mut mixer, RATE as usize / 4).0 > 0.1);
        peaks(&mut mixer, RATE as usize / 4);
        assert_eq!(mixer.voices(), 0, "it ended at its own end");
        audio.update(&world, &assets, &near, 0.6);
        assert!(!audio.is_playing(playing));
    }

    #[test]
    fn with_more_sounds_than_voices_the_loudest_are_mixed() {
        let (world, assets, tone) = two_rooms();
        let (mut audio, mut mixer) = Audio::offline(RATE);
        // A row of them, nearer and nearer the listener.
        let row: Vec<Playing> = (0..MAX_VOICES + 8)
            .map(|i| audio.play(Play::new(tone).at(0, Vec3::new(0.0, 1.7, 0.5 + i as f32 * 0.1)).looped()))
            .collect();
        audio.update(&world, &assets, &listener(0.0, 7.0), 0.0);
        peaks(&mut mixer, 480);
        assert_eq!((audio.counts(), mixer.voices()), ((MAX_VOICES + 8, MAX_VOICES), MAX_VOICES));
        let mixed = |audio: &Audio, i: usize| audio.emitters.iter().find(|e| e.id == row[i].0).unwrap().voice.is_some();
        assert!((0..8).all(|i| !mixed(&audio, i)) && (8..row.len()).all(|i| mixed(&audio, i)));
        // From the other end of the row, it is the other end's that are.
        audio.update(&world, &assets, &listener(0.0, 0.2), 0.0);
        peaks(&mut mixer, 4800);
        assert!((0..MAX_VOICES).all(|i| mixed(&audio, i)) && (MAX_VOICES..row.len()).all(|i| !mixed(&audio, i)));
        assert_eq!(mixer.voices(), MAX_VOICES);
        audio.stop_all();
        peaks(&mut mixer, 4800);
        assert_eq!((audio.counts(), mixer.voices()), ((0, 0), 0));
    }

    #[test]
    fn a_sound_on_an_entity_follows_it_and_ends_with_it() {
        let (mut world, assets, tone) = two_rooms();
        let (mut audio, _mixer) = Audio::offline(RATE);
        let crate_a1 = world.entities.iter().position(|e| e.name == "crate_a1").unwrap();
        let playing = audio.play(Play::new(tone).on(crate_a1 as u32).looped());
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        let before = audio.heard(playing).unwrap();
        assert!(before.left > before.right, "the crate is to the left: {before:?}");
        world.entities[crate_a1].position.x = 2.5;
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        let after = audio.heard(playing).unwrap();
        assert!(after.right > after.left, "{after:?}");
        world.entities.clear();
        audio.update(&world, &assets, &listener(0.0, 6.0), 0.0);
        assert!(!audio.is_playing(playing));
    }
}
