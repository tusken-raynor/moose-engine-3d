//! A level's day and night (its sun's `cycle=`) and its sky's clouds (`clouds=`): from the
//! time and the sun's options, where the sun is and how it lights the level, the ambient
//! light, and what the sky box draws (day fading to night, the stars' turn, the clouds).
//!
//! The sun goes round on an arc through its authored direction, which is where it is at
//! noon: up from the east (a quarter of the way through the day), down in the west (three
//! quarters), below the horizon at night, when it lights nothing and casts no shadows. The
//! stars turn the same way about the same pole (the arc's axis). Low, the sun is warmer and
//! dimmer; behind a cloud, dimmer still; on a cloudy day its shadows are softer and the
//! ambient light (the sky's) brighter.
//!
//! The clouds' cover eases toward the weather (the level's, or the player's setting) over
//! about a minute, wandering a little about it on its own, so the sky clears up or clouds
//! over when the weather changes.

use glam::{Vec2, Vec3};
use moose_assets::{Clouds, Sky, cloud_cut};

/// How high the cloud layer is, and how far across a field of it, in meters (so a field is
/// `ALTITUDE / FIELD` of the view's slope across: see `sky_box`'s `[20]`).
const ALTITUDE: f32 = 1500.0;
const FIELD: f32 = 8000.0;
/// Seconds the cover takes to come most of the way to the weather.
const EASE: f32 = 40.0;
/// How far the cover wanders about the weather on its own, and how slowly (seconds).
const WANDER: f32 = 0.12;
const WANDER_TIME: f32 = 90.0;
/// How much softer a fully cloudy sun's shadows are: degrees added to its size.
const OVERCAST_SPREAD: f32 = 6.0;

/// What the cycle and the clouds come to at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    /// The way toward the sun (unit), and its light: its color (black below the horizon)
    /// and its size, in degrees.
    pub toward: Vec3,
    pub sun: Vec3,
    pub angle: f32,
    /// How much of the sun's disk shows in the sky (0 below the horizon).
    pub disk: f32,
    pub ambient: Vec3,
    /// How far to night the sky is, 0 to 1, and the day's sky's tint.
    pub night: f32,
    pub day_tint: Vec3,
    /// The stars' pole (unit) and turn (radians).
    pub pole: Vec3,
    pub turn: f32,
    /// The clouds (if there are any): the field's level they start at, where the layer is
    /// read from (the wind's slide, in fields), how far between generations, and their
    /// sunlit and shaded colors.
    pub clouds: Option<CloudsNow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudsNow {
    pub cut: f32,
    pub slide: Vec2,
    pub blend: f32,
    pub sunlit: Vec3,
    pub shaded: Vec3,
}

/// A level's day and night and its clouds, as they go.
pub struct DayNight {
    pub sky: Sky,
    /// The sun as authored: toward it at noon (unit), its color and size.
    noon: Vec3,
    color: Vec3,
    angle: f32,
    /// Across the arc (toward sunrise is `-east`) and the pole.
    east: Vec3,
    pole: Vec3,
    /// The ambient light as authored (the day's).
    ambient: Vec3,
    /// The cover now (eased toward the weather), and the clouds' field.
    cover: f32,
    pub clouds: Option<Clouds>,
    /// Seconds the cycle has been held (paused), which don't count toward the time of day.
    held: f32,
    last: Option<f32>,
}

impl DayNight {
    /// The cycle of a level's sun (traveling along `direction`, `color`, `angle` degrees
    /// across) with its `sky` options, and its authored `ambient` light. `time` is the
    /// app's, in seconds.
    pub fn new(direction: Vec3, color: Vec3, angle: f32, sky: Sky, ambient: Vec3, time: f32) -> DayNight {
        let noon = (-direction).normalize();
        // Across the arc: level and square to where the sun is at noon (straight up: -Z's
        // square).
        let flat = Vec3::new(noon.x, 0.0, noon.z);
        let north = if flat.length() > 1e-4 { flat.normalize() } else { Vec3::NEG_Z };
        let east = Vec3::Y.cross(north).normalize();
        let pole = noon.cross(east).normalize();
        let clouds = sky.clouds.map(|seed| Clouds::new(seed, time as f64, noon));
        DayNight { sky, noon, color, angle, east, pole, ambient, cover: sky.weather, clouds, held: 0.0, last: None }
    }

    /// Whether the sun moves (the level has a cycle).
    pub fn cycles(&self) -> bool {
        self.sky.cycle > 0.0
    }

    /// The time of day at app time `time`, 0 to 1 (0 midnight, 0.5 noon).
    pub fn time_of_day(&self, time: f32) -> f32 {
        if !self.cycles() {
            return self.sky.time;
        }
        (self.sky.time + (time - self.held) / self.sky.cycle).rem_euclid(1.0)
    }

    /// Moves to app time `time`: `running` (the cycle isn't held), the cover easing toward
    /// `weather` (none: the level's), and the clouds' field (true if its texture changed).
    pub fn advance(&mut self, time: f32, running: bool, weather: Option<f32>) -> bool {
        let dt = self.last.map_or(0.0, |last| (time - last).max(0.0));
        self.last = Some(time);
        if !running {
            self.held += dt;
        }
        let target = weather.unwrap_or(self.sky.weather);
        self.cover += (target - self.cover) * (1.0 - (-dt / EASE).exp());
        let toward = self.toward(time);
        match &mut self.clouds {
            Some(clouds) => clouds.advance_to(time as f64, toward),
            None => false,
        }
    }

    /// The cover now, wandering a little about where it eases to.
    pub fn cover(&self, time: f32) -> f32 {
        let seed = self.sky.clouds.unwrap_or(0);
        (self.cover + WANDER * (wander(seed, time / WANDER_TIME) * 2.0 - 1.0)).clamp(0.0, 1.0)
    }

    /// Toward the sun at app time `time` (unit).
    fn toward(&self, time: f32) -> Vec3 {
        let hour = (self.time_of_day(time) - 0.5) * std::f32::consts::TAU;
        (self.noon * hour.cos() + self.east * hour.sin()).normalize()
    }

    /// Where the view toward `d` meets the cloud layer, in fields (before the wind).
    fn on_layer(d: Vec3) -> Vec2 {
        Vec2::new(d.x, d.z) / d.y.max(1e-4) * (ALTITUDE / FIELD)
    }

    /// The wind's slide of the layer at app time `time`, in fields.
    fn slide(&self, time: f32) -> Vec2 {
        let from = self.sky.wind_from.to_radians();
        // It comes from `wind_from` and goes the other way; the layer is read back along it.
        let goes = Vec2::new(from.sin(), from.cos());
        -goes * (self.sky.wind * time / FIELD)
    }

    /// What it all comes to at app time `time`; `fixed`, with the sun where the level has it
    /// (the cycle off: noon), the clouds going on.
    pub fn now(&self, time: f32, fixed: bool) -> Now {
        let toward = if fixed { self.noon } else { self.toward(time) };
        let up = toward.y;
        let smooth = |a: f32, b: f32, x: f32| {
            let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let cover = self.cover(time);
        let night = 1.0 - smooth(-0.12, 0.05, up);
        let day = 1.0 - night;
        // Low, the sun is warmer and dimmer.
        let low = 1.0 - smooth(0.0, 0.35, up);
        let warm = Vec3::ONE.lerp(Vec3::new(1.0, 0.62, 0.38), low);
        // Behind a cloud, dimmer still.
        let behind = match &self.clouds {
            Some(c) if up > 0.0 => c.density_at(Self::on_layer(toward) + self.slide(time), time as f64, cover),
            _ => 0.0,
        };
        let lit = smooth(-0.02, 0.12, up) * (1.0 - 0.85 * behind) * (1.0 - 0.25 * cover);
        let sun = self.color * warm * lit;
        let angle = (self.angle + OVERCAST_SPREAD * cover).min(45.0);
        let night_ambient = self.ambient * 0.25 * Vec3::new(0.55, 0.65, 1.0);
        let ambient = night_ambient.lerp(self.ambient * (1.0 + 0.6 * cover), day);
        let brightness = 0.35 + 0.65 * smooth(-0.05, 0.4, up);
        let day_tint = Vec3::ONE.lerp(Vec3::new(1.0, 0.8, 0.65), low * 0.6) * brightness;
        let clouds = self.clouds.as_ref().map(|c| {
            let tint = warm / warm.max_element();
            let sunlit = Vec3::new(0.06, 0.07, 0.10).lerp(tint * 1.1 * brightness, day);
            let shaded = Vec3::new(0.03, 0.035, 0.05).lerp(Vec3::new(0.55, 0.58, 0.63) * brightness, day);
            CloudsNow { cut: cloud_cut(cover), slide: self.slide(time), blend: c.blend(time as f64), sunlit, shaded }
        });
        let hour = if fixed { 0.0 } else { (self.time_of_day(time) - 0.5) * std::f32::consts::TAU };
        Now {
            toward,
            sun,
            angle,
            disk: smooth(-0.03, 0.03, up) * (1.0 - 0.9 * behind),
            ambient,
            night,
            day_tint,
            pole: self.pole,
            turn: hour,
            clouds,
        }
    }

    /// Goes on from `old` (the level rebuilt): its held time, cover and clock.
    pub fn carry_on(&mut self, old: &DayNight) {
        (self.held, self.cover, self.last) = (old.held, old.cover, old.last);
    }

    /// How many fields of the layer a unit of the view's slope spans (`sky_box`'s `[20]`).
    pub fn layer_scale() -> f32 {
        ALTITUDE / FIELD
    }
}

/// A time of day (a fraction of it, 0 midnight) on a clock: `08:24`.
pub fn clock(time_of_day: f32) -> String {
    let minutes = (time_of_day.rem_euclid(1.0) * 24.0 * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Smooth noise in one dimension, 0 to 1, for the cover's wander.
fn wander(seed: u32, t: f32) -> f32 {
    let hash = |k: i64| {
        let mut h = seed ^ (k as u32).wrapping_mul(0x9E37_79B9);
        h = (h ^ (h >> 15)).wrapping_mul(0x85EB_CA6B);
        h ^= h >> 13;
        (h & 0xFFFF) as f32 / 0xFFFF as f32
    };
    let k = t.floor();
    let f = t - k;
    let s = f * f * (3.0 - 2.0 * f);
    hash(k as i64) + (hash(k as i64 + 1) - hash(k as i64)) * s
}

#[cfg(test)]
mod tests {
    use super::*;

    impl DayNight {
        fn now_t(&self, time: f32) -> Now {
            self.now(time, false)
        }
    }

    fn cycle(time: f32) -> DayNight {
        let sky = Sky { cycle: 600.0, time, clouds: Some(5), ..Sky::default() };
        DayNight::new(Vec3::new(0.3, -0.85, 0.42), Vec3::new(1.8, 1.7, 1.5), 0.53, sky, Vec3::splat(0.2), 0.0)
    }

    #[test]
    fn the_sun_rises_in_the_east_peaks_at_noon_and_sets_and_the_night_is_dark() {
        let d = cycle(0.5);
        let noon = d.now_t(0.0);
        // At noon, where it was authored.
        assert!((noon.toward - Vec3::new(-0.3, 0.85, -0.42).normalize()).length() < 1e-4);
        assert_eq!(noon.night, 0.0);
        // A quarter of a day later (150 s), it sets: level, warm and dim.
        let set = d.now_t(150.0);
        assert!(set.toward.y.abs() < 1e-3, "{}", set.toward.y);
        assert!(set.sun.x > set.sun.z && set.sun.length() < noon.sun.length());
        // Midnight: below, no sunlight, the sky at night, a dimmer ambient light.
        let midnight = d.now_t(300.0);
        assert!(midnight.toward.y < -0.5);
        assert_eq!(midnight.sun, Vec3::ZERO);
        assert_eq!(midnight.night, 1.0);
        assert!(midnight.ambient.length() < noon.ambient.length());
        // The stars turn as the sun does (a quarter turn by sunset), about the arc's pole.
        assert!(noon.turn.abs() < 1e-5);
        assert!((set.turn - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!(noon.pole.dot(noon.toward).abs() < 1e-4);
    }

    #[test]
    fn held_the_time_of_day_stands_still_and_the_cover_eases_to_the_weather() {
        let mut d = cycle(0.3);
        d.advance(0.0, true, None);
        d.advance(10.0, false, None);
        // Held for 10 s: still 0.3 of the day.
        assert!((d.time_of_day(10.0) - 0.3).abs() < 1e-5);
        d.advance(20.0, true, None);
        assert!((d.time_of_day(20.0) - (0.3 + 10.0 / 600.0)).abs() < 1e-5);
        // Overcast asked for: a minute on, most of the way there.
        for k in 0..60 {
            d.advance(21.0 + k as f32, true, Some(1.0));
        }
        assert!(d.cover > 0.7, "{}", d.cover);
        let now = d.now_t(80.0);
        assert!(now.clouds.unwrap().cut < 0.0);
        // Cloudy days' shadows are softer.
        assert!(now.angle > 0.53 + 3.0);
    }
}
