//! What the listener hears of a sound somewhere in the level.
//!
//! Sound doesn't go through walls: it reaches the listener along the shortest way through
//! the level's portals. How long that way is sets how loud the sound is, where it comes
//! into the listener's sector sets the side it is heard on, and how much it bends on the
//! way sets how muffled it is.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::Vec3;
use moose_assets::Plane;
use moose_scene::{Camera, World};

use crate::mixer::{Heard, OPEN};

/// The pitch a sound is cut down to by going right round (bends adding up to half a turn
/// or more), in Hz. A quarter turn leaves it about 3.7 kHz.
const BENT_CUTOFF: f32 = 700.0;
/// How much quieter bends make a sound, beyond the longer way: by half at a quarter turn.
const BEND_LOSS: f32 = 2.0;
/// What a portal that isn't passable (a window, a shut door) lets through: this much of
/// the loudness, and this much of the pitch range.
const CLOSED_GAIN: f32 = 0.3;
const CLOSED_CUTOFF: f32 = 0.1;
/// How much longer, in meters, a way through such a portal counts as when ways are
/// compared: an open way round that is a little longer is the one heard.
const CLOSED_DETOUR: f32 = 8.0;
/// The lowest cutoff anything is muffled to, in Hz.
const LOWEST_CUTOFF: f32 = 200.0;
/// How far to the side a sound can be: at 1 the far ear would hear none of it.
const WIDTH: f32 = 0.8;
/// Nearer than this, in meters, a sound moves to the middle (it has no side at the
/// listener's own position).
const CLOSE: f32 = 0.5;
/// Times the taut way is pulled tighter after it is found (see [`Routes::route`]).
const TIGHTEN: usize = 3;
/// No portal: the way starts here.
const NONE: u32 = u32::MAX;

/// Who hears: usually the player's camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    /// The sector `position` is in (any that isn't one of the level's, as outside it:
    /// sounds are then heard straight through everything).
    pub sector: u32,
    pub position: Vec3,
    /// The way their right ear points (a unit vector).
    pub right: Vec3,
}

impl Listener {
    pub fn of(camera: &Camera) -> Listener {
        Listener {
            sector: camera.sector,
            position: camera.position,
            right: camera.right(),
        }
    }
}

/// The way a sound takes to the listener.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    /// How long it is, in meters.
    pub length: f32,
    /// Where it comes from as the listener hears it: the sound's place if the way is
    /// straight, or where it comes through the first portal on the way to it.
    pub toward: Vec3,
    /// How much it turns, in radians, all its bends added up.
    pub bend: f32,
    /// How many portals that aren't passable it goes through.
    pub closed: u32,
}

/// Finds routes. It keeps what it works with between calls, so finding one reserves no
/// memory.
#[derive(Default)]
pub struct Routes {
    /// Per portal: the shortest way found to it (its length, the portal before it and the
    /// point it is crossed at), if `round` is this search's.
    length: Vec<f32>,
    before: Vec<u32>,
    point: Vec<Vec3>,
    round: Vec<u32>,
    search: u32,
    /// Portals reached and not yet gone on from, nearest first (lengths as their bits,
    /// which order as they do, none being negative).
    reached: BinaryHeap<(Reverse<u32>, u32)>,
    /// The way found: its portals, and its points from the listener to the sound.
    portals: Vec<u32>,
    points: Vec<Vec3>,
    /// A portal's corners.
    corners: Vec<Vec3>,
}

impl Routes {
    /// The shortest way from the listener (`ear`: its sector and position) to a sound
    /// (`source`, likewise) no longer than `reach`, if there is one. `positions` is the
    /// level geometry's.
    ///
    /// Sectors are convex, so a way is straight inside each and bends only at portals. Each
    /// portal is crossed at its point nearest the straight line from listener to sound;
    /// the shortest way over those points is found, and then pulled tighter, each crossing
    /// moved to its portal's point nearest the line between the two either side of it.
    pub fn route(&mut self, world: &World, positions: &[Vec3], ear: (u32, Vec3), source: (u32, Vec3), reach: f32) -> Option<Route> {
        let ((from, p), (to, q)) = (ear, source);
        if p.distance(q) >= reach {
            return None;
        }
        let sectors = world.sectors.len() as u32;
        if from == to || from >= sectors || to >= sectors {
            return Some(Route { length: p.distance(q), toward: q, bend: 0.0, closed: 0 });
        }
        self.search = self.search.wrapping_add(1);
        if self.search == 0 || self.round.len() != world.portals.len() {
            self.search = 1;
            self.round.clear();
            self.round.resize(world.portals.len(), 0);
            self.length.resize(world.portals.len(), 0.0);
            self.before.resize(world.portals.len(), NONE);
            self.point.resize(world.portals.len(), Vec3::ZERO);
        }
        self.reached.clear();
        for k in world.sectors[from as usize].portals.clone() {
            let c = self.crossing(world, positions, k, p, q);
            self.note(world, k, NONE, c, p.distance(c), reach);
        }
        // The best whole way so far: what it counts as, and its last portal.
        let mut best: Option<(f32, u32)> = None;
        while let Some((Reverse(bits), k)) = self.reached.pop() {
            let so_far = f32::from_bits(bits);
            if so_far > self.length[k as usize] {
                continue;
            }
            if best.is_some_and(|(whole, _)| so_far >= whole) {
                break;
            }
            let (portal, c) = (&world.portals[k as usize], self.point[k as usize]);
            if portal.target == to {
                let whole = so_far + c.distance(q);
                if whole < reach && best.is_none_or(|(least, _)| whole < least) {
                    best = Some((whole, k));
                }
                continue;
            }
            for j in world.sectors[portal.target as usize].portals.clone() {
                if j != portal.mirror {
                    let next = self.crossing(world, positions, j, p, q);
                    self.note(world, j, k, next, so_far + c.distance(next), reach);
                }
            }
        }
        let (_, last) = best?;
        // The way back from the sound, then turned round.
        self.portals.clear();
        let mut k = last;
        while k != NONE {
            self.portals.push(k);
            k = self.before[k as usize];
        }
        self.portals.reverse();
        self.points.clear();
        self.points.push(p);
        self.points.extend(self.portals.iter().map(|&k| self.point[k as usize]));
        self.points.push(q);
        for _ in 0..TIGHTEN {
            for i in 0..self.portals.len() {
                let (k, a, b) = (self.portals[i], self.points[i], self.points[i + 2]);
                let c = self.crossing(world, positions, k, a, b);
                self.points[i + 1] = c;
            }
        }
        let mut route = Route {
            length: 0.0,
            toward: q,
            bend: 0.0,
            closed: self.portals.iter().filter(|&&k| !world.portals[k as usize].flags.passable()).count() as u32,
        };
        // Its legs, leaving out any too short to have a direction.
        let mut heading: Option<Vec3> = None;
        for pair in self.points.windows(2) {
            let leg = pair[1] - pair[0];
            let length = leg.length();
            route.length += length;
            if length > 1e-3 {
                let along = leg / length;
                match heading {
                    None => route.toward = pair[1],
                    Some(before) => route.bend += before.dot(along).clamp(-1.0, 1.0).acos(),
                }
                heading = Some(along);
            }
        }
        // (Float error in a straight way's legs isn't a bend.)
        if route.bend < 1e-3 {
            route.bend = 0.0;
        }
        Some(route)
    }

    /// Notes a way of `length` to portal `k` (crossed at `c`, from portal `before`), if it
    /// is within `reach` and shorter than any found to it.
    fn note(&mut self, world: &World, k: u32, before: u32, c: Vec3, length: f32, reach: f32) {
        let closed = !world.portals[k as usize].flags.passable();
        let length = length + if closed { CLOSED_DETOUR } else { 0.0 };
        let i = k as usize;
        if length < reach && (self.round[i] != self.search || length < self.length[i]) {
            (self.round[i], self.length[i], self.before[i], self.point[i]) = (self.search, length, before, c);
            self.reached.push((Reverse(length.to_bits()), k));
        }
    }

    /// Where a way from `a` to `b` crosses portal `k`: the portal's point nearest the
    /// straight line between them.
    fn crossing(&mut self, world: &World, positions: &[Vec3], k: u32, a: Vec3, b: Vec3) -> Vec3 {
        let portal = &world.portals[k as usize];
        self.corners.clear();
        self.corners.extend(portal.positions.iter().map(|&i| positions[i as usize]));
        nearest_on_polygon(&self.corners, portal.plane, a, b)
    }
}

/// The point of a convex polygon nearest the segment from `a` to `b`: where the segment
/// goes through it, or else its point nearest where the segment meets its plane (or comes
/// nearest to meeting it).
fn nearest_on_polygon(corners: &[Vec3], plane: Plane, a: Vec3, b: Vec3) -> Vec3 {
    let (da, db) = (plane.distance(a), plane.distance(b));
    let on_plane = if da * db < 0.0 {
        a + (b - a) * (da / (da - db))
    } else if da.abs() <= db.abs() {
        a - plane.normal * da
    } else {
        b - plane.normal * db
    };
    let n = corners.len();
    let edges = (0..n).map(|i| (corners[i], corners[(i + 1) % n]));
    if edges.clone().all(|(c, d)| (d - c).cross(on_plane - c).dot(plane.normal) >= 0.0) {
        return on_plane;
    }
    edges
        .map(|(c, d)| {
            let edge = d - c;
            let t = ((on_plane - c).dot(edge) / edge.length_squared().max(1e-12)).clamp(0.0, 1.0);
            c + edge * t
        })
        .min_by(|x, y| x.distance_squared(on_plane).total_cmp(&y.distance_squared(on_plane)))
        .unwrap_or(on_plane)
}

/// How a sound of `volume` is heard over `route`: `near` is how close it is at its full
/// loudness, and `reach` how far it carries.
///
/// From `near` out it fades as one over the distance (half as loud twice as far), and
/// over that, smoothly to nothing at `reach`. Bends and closed portals muffle it.
pub fn heard(listener: &Listener, route: &Route, volume: f32, near: f32, reach: f32) -> Heard {
    let d = route.length;
    if d >= reach {
        return Heard::SILENT;
    }
    let near = near.max(1e-3);
    let fade = 1.0 - (d / reach) * (d / reach);
    let turns = (route.bend / std::f32::consts::PI).min(1.0);
    let closed = route.closed as i32;
    let gain = volume * (near / d.max(near)) * fade * fade / (1.0 + BEND_LOSS * turns) * CLOSED_GAIN.powi(closed);
    let cutoff = if turns == 0.0 && closed == 0 {
        OPEN
    } else {
        (OPEN * (BENT_CUTOFF / OPEN).powf(turns) * CLOSED_CUTOFF.powi(closed)).max(LOWEST_CUTOFF)
    };
    // Side: -1 hard left to 1 hard right, as an angle from the left ear (0) to the right
    // (a quarter turn), at equal power: straight ahead is as loud in both ears as a sound
    // with no place would be.
    let toward = route.toward - listener.position;
    let distance = toward.length();
    let side = if distance > 1e-4 {
        (toward / distance).dot(listener.right) * (distance / CLOSE).min(1.0) * WIDTH
    } else {
        0.0
    };
    let angle = (side + 1.0) * std::f32::consts::FRAC_PI_4;
    Heard {
        left: gain * angle.cos() * std::f32::consts::SQRT_2,
        right: gain * angle.sin() * std::f32::consts::SQRT_2,
        cutoff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moose_assets::Assets;

    /// `two_rooms.mmp`: room_a (sector 0; x -4..4, z 0..8) and room_b (sector 2; z
    /// -20..-12) joined by a hallway (sector 1; x -1..1, z -12..0) with a doorway 3 m high
    /// at each end. `edit` changes its text first.
    fn two_rooms(edit: impl Fn(String) -> String) -> (World, Assets) {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let mut assets = Assets::new(root);
        let src = std::fs::read_to_string(format!("{root}/levels/two_rooms.mmp")).unwrap();
        let level = assets.parse_level("two_rooms.mmp", &edit(src)).unwrap();
        (World::new(level, &assets), assets)
    }

    fn route(world: &World, assets: &Assets, ear: (u32, Vec3), source: (u32, Vec3), reach: f32) -> Option<Route> {
        Routes::default().route(world, &assets.mesh(world.geometry).positions, ear, source, reach)
    }

    fn facing_north(sector: u32, position: Vec3) -> Listener {
        // Yaw 0 faces -Z, so the right ear points along +X.
        Listener { sector, position, right: Vec3::X }
    }

    #[test]
    fn each_sectors_portals_are_its_own() {
        let (world, _) = two_rooms(|s| s);
        for (s, sector) in world.sectors.iter().enumerate() {
            assert!(sector.portals.clone().all(|k| world.portals[k as usize].sector == s as u32));
        }
    }

    #[test]
    fn in_the_same_sector_the_way_is_straight() {
        let (world, assets) = two_rooms(|s| s);
        let (p, q) = (Vec3::new(0.0, 1.7, 6.0), Vec3::new(3.0, 1.0, 2.0));
        let route = route(&world, &assets, (0, p), (0, q), 50.0).unwrap();
        assert_eq!(route, Route { length: p.distance(q), toward: q, bend: 0.0, closed: 0 });
    }

    #[test]
    fn seen_through_both_doorways_the_way_is_still_straight() {
        let (world, assets) = two_rooms(|s| s);
        let (p, q) = (Vec3::new(0.5, 1.5, 6.0), Vec3::new(-0.5, 1.0, -16.0));
        let route = route(&world, &assets, (0, p), (2, q), 50.0).unwrap();
        assert!((route.length - p.distance(q)).abs() < 1e-3, "{route:?}");
        assert_eq!((route.bend, route.closed), (0.0, 0));
        // It is heard from where it comes through the first doorway, which is on the line.
        assert!((route.toward - p).normalize().distance((q - p).normalize()) < 1e-3, "{route:?}");
        assert!(route.toward.z.abs() < 1e-3, "{route:?}");
    }

    #[test]
    fn round_the_corners_it_goes_by_the_doorways_edges() {
        let (world, assets) = two_rooms(|s| s);
        // Beside the doorway in each room, on opposite sides: the straight line is all wall.
        let (p, q) = (Vec3::new(-3.0, 1.5, 1.0), Vec3::new(3.0, 1.5, -13.0));
        let route = route(&world, &assets, (0, p), (2, q), 50.0).unwrap();
        let (a, b) = (Vec3::new(-1.0, 1.5, 0.0), Vec3::new(1.0, 1.5, -12.0));
        let taut = p.distance(a) + a.distance(b) + b.distance(q);
        assert!((route.length - taut).abs() < 0.02, "{route:?} for {taut}");
        assert!(route.toward.distance(a) < 0.05, "{route:?}");
        // It turns at each doorway: about 54 degrees each.
        assert!((route.bend.to_degrees() - 108.0).abs() < 3.0, "{}", route.bend.to_degrees());
        // The same way back.
        let back = self::route(&world, &assets, (2, q), (0, p), 50.0).unwrap();
        assert!((back.length - route.length).abs() < 0.02 && back.toward.distance(b) < 0.05, "{back:?}");
    }

    #[test]
    fn a_way_longer_than_the_sound_carries_is_none() {
        let (world, assets) = two_rooms(|s| s);
        let (p, q) = (Vec3::new(-3.0, 1.5, 1.0), Vec3::new(3.0, 1.5, -13.0));
        // 16.6 m round, 15.2 m straight.
        assert!(route(&world, &assets, (0, p), (2, q), 16.0).is_none());
        assert!(route(&world, &assets, (0, p), (2, q), 17.0).is_some());
    }

    #[test]
    fn portals_that_are_not_passable_are_counted() {
        let (world, assets) = two_rooms(|s| s.replace("0x3", "0x1"));
        let (p, q) = (Vec3::new(0.0, 1.5, 6.0), Vec3::new(0.0, 1.5, -16.0));
        let route = route(&world, &assets, (0, p), (2, q), 100.0).unwrap();
        assert_eq!(route.closed, 2);
        assert!((route.length - 22.0).abs() < 1e-3, "the detour they count as isn't heard as distance");
    }

    #[test]
    fn outside_the_level_everything_is_heard_straight() {
        let (world, assets) = two_rooms(|s| s);
        let (p, q) = (Vec3::new(30.0, 1.5, 1.0), Vec3::new(3.0, 1.5, -13.0));
        let route = route(&world, &assets, (u32::MAX, p), (2, q), 100.0).unwrap();
        assert_eq!((route.length, route.toward, route.bend), (p.distance(q), q, 0.0));
    }

    #[test]
    fn a_sound_is_louder_nearer_and_in_the_ear_on_its_side() {
        let ear = facing_north(0, Vec3::ZERO);
        let at = |q: Vec3| heard(&ear, &Route { length: q.length(), toward: q, bend: 0.0, closed: 0 }, 1.0, 2.0, 50.0);
        let ahead = at(Vec3::new(0.0, 0.0, -1.0));
        // (All but full: the fade toward its reach has just begun.)
        assert!(ahead.left == ahead.right && (ahead.left - 0.9992).abs() < 1e-4, "{ahead:?}");
        assert_eq!(ahead.cutoff, OPEN);
        let right = at(Vec3::new(2.0, 0.0, 0.0));
        assert!(right.right > 1.3 && right.left < 0.3 && right.left > 0.0, "{right:?}");
        let left = at(Vec3::new(-2.0, 0.0, 0.0));
        assert!((left.left - right.right).abs() < 1e-5 && (left.right - right.left).abs() < 1e-5);
        let far = at(Vec3::new(0.0, 0.0, -8.0));
        assert!((far.left - 0.25 * (1.0 - 0.0256f32).powi(2)).abs() < 1e-4, "{far:?}");
        assert_eq!(at(Vec3::new(0.0, 0.0, -50.0)), Heard::SILENT);
        // On top of the listener it has no side.
        let here = at(Vec3::new(1e-6, 0.0, 0.0));
        assert!((here.left - here.right).abs() < 1e-3, "{here:?}");
    }

    #[test]
    fn bends_and_closed_portals_muffle() {
        let ear = facing_north(0, Vec3::ZERO);
        let with = |bend: f32, closed| {
            heard(&ear, &Route { length: 4.0, toward: Vec3::NEG_Z, bend, closed }, 1.0, 2.0, 50.0)
        };
        let (clear, bent, shut) = (with(0.0, 0), with(std::f32::consts::FRAC_PI_2, 0), with(0.0, 1));
        assert!((bent.left / clear.left - 0.5).abs() < 1e-3);
        assert!((bent.cutoff - (OPEN * BENT_CUTOFF).sqrt()).abs() < 1.0, "{bent:?}");
        assert!((shut.left / clear.left - CLOSED_GAIN).abs() < 1e-3);
        assert!((shut.cutoff - OPEN * CLOSED_CUTOFF).abs() < 1.0, "{shut:?}");
        assert_eq!(with(9.0, 3).cutoff, LOWEST_CUTOFF);
    }
}
