# Sound Spec

## Overview

Sound is the `moose-audio` crate: our own software mixer, and the level's own geometry deciding what is heard. A sound reaches the listener through the portals between its sector and theirs, never through walls.

`cpal` opens the output device and nothing else; only `device.rs` names it. The rest of the engine talks to `Audio`.

**Who plays sounds is not the sound engine's business.** It has an API (below). Engine events (a footstep, taking damage) and scripts call it alike.

**Built so far (Oct 13):** WAV sounds as assets; the mixer; sounds heard everywhere, at a place, or on an entity; hearing through portals; the 32 loudest mixed; the player's footsteps and a test sound in the app; scripts playing, looping, stopping and awaiting sounds (see the Scripting Spec's Sound section). **Planned** sections at the end are the agenda, not gaps.

## Conventions

| Topic | Convention |
| --- | --- |
| Sound files | `assets/sounds/NAME.wav`: uncompressed PCM (8, 16, 24 or 32 bits, or 32-bit float), mono or stereo, any rate. Kept in memory as 16 bits |
| Handles | `SoundId`, from `Assets::load_sound`, like `MeshId` and `TextureId`. Hot reloaded when the file changes |
| Loudness | A multiplier: 1 is as recorded |
| Pitch | A speed: 1 is as recorded, 2 an octave up in half the time |
| Distances | Meters, along the way the sound takes (below), not the straight line |
| Ears | Stereo. The right ear points along the camera's right |

## The mixer

The mixer runs on the device's thread, which calls it for a few milliseconds of frames at a time. It never waits: it takes no locks and reserves no memory. The game tells it things through a queue (start a voice, how a voice is now heard, stop one), which it empties each time it is called.

| What | How |
| --- | --- |
| Voices | 32 at once (`MAX_VOICES`), plus 16 spare for ones fading out |
| Rate | Each voice is stretched to the device's rate (and its pitch) between its two nearest samples |
| Being heard anew | A voice glides to its new loudness and filter over 40 ms, so a sound moving at any frame rate doesn't click |
| Stopping | A 10 ms fade |
| Muffling | A one-pole low-pass filter per voice, set by a cutoff in Hz |
| Output | The device's own rate and channels: mono gets the ears' average, more than two get the two ears and silence. Too loud a mix is held at full, not wrapped |

## What the listener hears

Once a frame the game calls `Audio::update` with the listener (the camera). For each sound with a place it finds the **way** to the listener, and from that how loud it is in each ear and how muffled.

**The way.** Sectors are convex, so a way is straight inside each and bends only at portals. In the same sector it is the straight line. Otherwise it is the shortest way through the portals:

- Each portal is crossed at its point nearest the straight line from listener to sound. Where that line goes through an opening, the way is the line.
- The shortest way over those crossing points is found (a search outward from the listener, given up past the sound's reach).
- It is then pulled tighter: each crossing moves to its portal's point nearest the line between its two neighbours, three times over.

| From the way | Comes |
| --- | --- |
| Its length | Loudness. Full within `near`; then half as loud twice as far; and over that, faded smoothly to nothing at `reach` |
| Where it enters the listener's sector | The side it is heard on. A sound round a corner is heard from the doorway |
| How much it bends, all its turns added | Muffling: none straight, cut to about 3.7 kHz by a quarter turn and 700 Hz by half a turn or more. Also quieter: half at a quarter turn |
| Portals on it that aren't passable (a window, a shut door) | Each lets through 0.3 of the loudness and a tenth of the pitch range. A way through one counts as 8 m longer when ways are compared, so an open way round is preferred |

No way within `reach` means silence. A listener or sound outside every sector is heard straight, through everything.

Nothing else blocks sound: not props, terrain or view blockers.

The numbers above are constants at the top of `hear.rs` and haven't been tuned by ear.

**Sides.** A sound hard to one side still reaches the far ear a little (0.8 of the way to one ear only). Within 0.5 m it moves to the middle. There is no front or back.

## Voices

Every playing sound is tracked by the game whether it is heard or not: time passes for it, and it ends when its length has. This is so with no sound device at all, so anything waiting for a sound to end isn't kept waiting.

Only sounds loud enough to hear (about -60 dB) hold a voice in the mixer, and of those the 32 loudest. A sound that comes into hearing is mixed from wherever it has got to: walking up to a looping sound, or round a corner in the middle of a long one. One with a voice counts as a little louder than it is, so two about as loud don't keep swapping.

## In the app

- **Footsteps:** off unless switched on (Esc > Sound > Footsteps). On, a soft pat every 1.3 m walked (quieter crouched), and a thump on landing. One of each for every floor so far.
- **Esc > Sound:** Volume, Footsteps, and "Test sound here", which leaves a looping hum where the player stands, to walk away from and round corners. Each can be bound to a key like any setting.
- **F3** (the debug HUD) shows how many sounds are playing and how many of them are mixed.
- `--no-sound` opens no device; `--volume X` sets the loudness. Screenshots and tests never open one.
- The test sounds (`step.wav`, `land.wav`, `test_hum.wav`) are made by `tools/gen_test_sounds.py`.

## API

```rust
impl Assets {
    pub fn load_sound(&mut self, name: &str) -> Result<SoundId, LoadError>;   // assets/sounds/<name>
    pub fn reload_sound(&mut self, name: &str) -> Result<bool, LoadError>;
    pub fn add_sound(&mut self, sound: Sound) -> SoundId;                      // made in code
    pub fn sound(&self, id: SoundId) -> &Sound;                                // rate, channels, samples, seconds()
}

/// A sound to play, and how: Play::new(sound).at(sector, position).volume(0.5)
impl Play {
    pub fn new(sound: SoundId) -> Play;               // once, as recorded, heard everywhere
    pub fn at(self, sector: u32, position: Vec3) -> Play;
    pub fn on(self, entity: u32) -> Play;             // follows the entity; ends if it goes
    pub fn volume(self, volume: f32) -> Play;
    pub fn pitch(self, pitch: f32) -> Play;
    pub fn looped(self) -> Play;
    pub fn carrying(self, near: f32, reach: f32) -> Play;   // default 2 m and 50 m
}

impl Audio {
    pub fn open() -> Audio;       // the default device; silent, with problem(), if it can't
    pub fn silent() -> Audio;     // no device
    pub fn offline(rate: u32) -> (Audio, Mixer);   // the caller asks the mixer for frames: tests, files

    pub fn play(&mut self, play: Play) -> Playing;
    pub fn is_playing(&self, playing: Playing) -> bool;
    pub fn stop(&mut self, playing: Playing);
    pub fn stop_all(&mut self);
    pub fn set_place(&mut self, playing: Playing, place: Place);
    pub fn set_sound_volume(&mut self, playing: Playing, volume: f32);
    pub fn set_volume(&mut self, volume: f32);     // everything's

    /// Once a frame.
    pub fn update(&mut self, world: &World, assets: &Assets, listener: &Listener, dt: f32);
}
```

A sound heard everywhere keeps its two channels if it is stereo. One with a place is played as mono.

## Planned

- **Floors:** which footstep a surface makes, from its material (a `$` meta value). Physics has to say which surface the player stands on first.
- **Level format and editor:** sound entities placed in the level (a sound, its reach, looping), and a sector's ambience. Shown and moved in the editor like lights.
- **Music:** long files streamed from disk rather than held in memory, in a compressed format (OGG), with fades between tracks.
- **Rooms:** a reverb per sector, from its size or a meta value.
- **Doors:** a portal's opening as an amount, not just passable or not, so a door muffles less as it opens.
- **More to change while playing:** pitch (engines), pausing everything with the game.
- **The device going away:** following the system's default device when it changes (headphones unplugged).
- **Front and back:** a cue for sounds behind the listener, and Doppler for fast ones, if they are missed.
