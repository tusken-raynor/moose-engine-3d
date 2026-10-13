//! The editor's keys: what each key does in each part of the editor (its contexts: the
//! editor as a whole, and what is selected), kept apart from the game's hotkeys (which the
//! editor doesn't take). The gear menu shows them all and binds them; they're kept in
//! `~/.config/moose/editor_keys.txt`, with the editor's settings (the 2D views' turn).

use std::path::PathBuf;

use moose_present::Key;

/// Where an action's key works: everywhere in the editor, or with something selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Editor,
    Sector,
    Face,
    Vertex,
    Entity,
    Light,
    /// The mesh editor.
    Mesh,
}

impl Context {
    pub const ALL: [Context; 7] =
        [Context::Editor, Context::Sector, Context::Face, Context::Vertex, Context::Entity, Context::Light, Context::Mesh];

    pub fn name(self) -> &'static str {
        match self {
            Context::Editor => "Editor",
            Context::Sector => "Sector",
            Context::Face => "Face",
            Context::Vertex => "Vertex",
            Context::Entity => "Entity",
            Context::Light => "Light",
            Context::Mesh => "Mesh editor",
        }
    }
}

/// What a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    // Flying the 3D view (and panning a 2D one).
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    Ascend,
    Descend,
    // Modes: what clicks pick.
    SectorMode,
    FaceMode,
    VertexMode,
    // Views: the six along the axes, and 3D.
    ViewTop,
    ViewBottom,
    ViewFront,
    ViewBack,
    ViewLeft,
    ViewRight,
    View3d,
    /// Turns a 2D view (by the turn step).
    TurnLeft,
    TurnRight,
    TurnUp,
    TurnDown,
    Wire,
    Grid,
    /// Clicks in the 3D view pick openings too (they aren't drawn).
    Openings,
    /// Types a sector, face, vertex or light number (or an entity's name) to go to.
    Find,
    /// Reads the level's scripts again and starts them over.
    ReloadScripts,
    // With a sector selected.
    Cleave,
    MergeSectors,
    DeleteSector,
    // With a face selected.
    CutFace,
    Extrude,
    MergeFaces,
    OpenToMatch,
    // With an entity or light selected.
    MeshEditor,
    DeleteEntity,
    DeleteLight,
    // In the mesh editor.
    PickProxies,
    DeletePolygon,
    LeaveMesh,
}

impl Action {
    /// Every action, in the order the gear menu lists them.
    pub const ALL: [Action; 38] = {
        use Action::*;
        [
            Forward, Backward, StrafeLeft, StrafeRight, Ascend, Descend, SectorMode, FaceMode, VertexMode, ViewTop,
            ViewBottom, ViewFront, ViewBack, ViewLeft, ViewRight, View3d, TurnLeft, TurnRight, TurnUp, TurnDown, Wire,
            Grid, Openings, Find, ReloadScripts, Cleave, MergeSectors, DeleteSector, CutFace, Extrude, MergeFaces, OpenToMatch, MeshEditor,
            DeleteEntity, DeleteLight, PickProxies, DeletePolygon, LeaveMesh,
        ]
    };

    pub fn context(self) -> Context {
        use Action::*;
        match self {
            Cleave | MergeSectors | DeleteSector => Context::Sector,
            CutFace | Extrude | MergeFaces | OpenToMatch => Context::Face,
            MeshEditor | DeleteEntity => Context::Entity,
            DeleteLight => Context::Light,
            PickProxies | DeletePolygon | LeaveMesh => Context::Mesh,
            _ => Context::Editor,
        }
    }

    pub fn label(self) -> &'static str {
        use Action::*;
        match self {
            Forward => "Fly forward (2D: pan up)",
            Backward => "Fly back (2D: pan down)",
            StrafeLeft => "Fly left (2D: pan left)",
            StrafeRight => "Fly right (2D: pan right)",
            Ascend => "Fly up",
            Descend => "Fly down",
            SectorMode => "Sector mode",
            FaceMode => "Face mode",
            VertexMode => "Vertex mode",
            ViewTop => "Top view",
            ViewBottom => "Bottom view",
            ViewFront => "Front view",
            ViewBack => "Back view",
            ViewLeft => "Left view",
            ViewRight => "Right view",
            View3d => "3D view",
            TurnLeft => "Turn a 2D view left",
            TurnRight => "Turn a 2D view right",
            TurnUp => "Turn a 2D view up",
            TurnDown => "Turn a 2D view down",
            Wire => "Wireframe over 3D",
            Grid => "Grid step",
            Openings => "Pick openings in 3D",
            Find => "Find by number",
            ReloadScripts => "Reload the scripts",
            Cleave => "Cleave (then the cut)",
            MergeSectors => "Merge sectors",
            DeleteSector => "Delete sector",
            CutFace => "Cut (then the cut)",
            Extrude => "Extrude",
            MergeFaces => "Merge faces",
            OpenToMatch => "Open to a match",
            MeshEditor => "Mesh editor",
            DeleteEntity => "Delete",
            DeleteLight => "Delete",
            PickProxies => "Pick proxies",
            DeletePolygon => "Delete polygon",
            LeaveMesh => "Leave",
        }
    }

    fn default_key(self) -> Key {
        use Action::*;
        match self {
            Forward => Key::W,
            Backward => Key::S,
            StrafeLeft => Key::A,
            StrafeRight => Key::D,
            Ascend => Key::Space,
            Descend => Key::Z,
            SectorMode => Key::R,
            FaceMode => Key::F,
            VertexMode => Key::V,
            ViewTop => Key::Key1,
            ViewBottom => Key::Key2,
            ViewFront => Key::Key3,
            ViewBack => Key::Key4,
            ViewLeft => Key::Key5,
            ViewRight => Key::Key6,
            View3d => Key::Key7,
            TurnLeft => Key::Left,
            TurnRight => Key::Right,
            TurnUp => Key::Up,
            TurnDown => Key::Down,
            Wire => Key::F5,
            Grid => Key::G,
            Openings => Key::O,
            Find => Key::Slash,
            ReloadScripts => Key::F9,
            Cleave => Key::C,
            MergeSectors => Key::M,
            DeleteSector => Key::X,
            CutFace => Key::C,
            Extrude => Key::E,
            MergeFaces => Key::M,
            OpenToMatch => Key::J,
            MeshEditor => Key::M,
            DeleteEntity => Key::X,
            DeleteLight => Key::X,
            PickProxies => Key::P,
            DeletePolygon => Key::X,
            LeaveMesh => Key::M,
        }
    }
}

/// Keys that aren't bound here: what the editor always does with them, for the gear menu.
pub const FIXED: [(&str, &str); 14] = [
    ("Click", "select (Shift: add or drop)"),
    ("Drag", "select what's in the box (Shift: add)"),
    ("Right button held", "look (3D) or pan (2D)"),
    ("Wheel", "change the row under the pointer; zoom 2D"),
    ("Shift+arrows, PgUp/PgDn", "move the selection"),
    ("[ ]", "turn an entity or a sector, aim a spot"),
    ("Del", "delete"),
    ("Esc", "stop, back, drop the selection, menu"),
    ("Ctrl+Z, Ctrl+Y", "undo, redo"),
    ("Ctrl+S", "save"),
    ("Ctrl+C, Ctrl+V, Ctrl+D", "copy, paste, duplicate"),
    ("Ctrl+Shift+1-4", "keep a camera place"),
    ("Ctrl+1-4", "go back to a camera place"),
    ("Tab, F6", "play; cycle 3D and the 2D views"),
];

/// The 2D views' turn steps the gear menu offers, in degrees.
pub const TURN_STEPS: [f32; 6] = [1.0, 3.0, 5.0, 6.0, 10.0, 15.0];

/// The editor's keys and settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Keys {
    bound: Vec<(Action, Key)>,
    /// How far a 2D view turns a key press, in degrees.
    pub turn_step: f32,
}

impl Default for Keys {
    fn default() -> Keys {
        Keys { bound: Action::ALL.iter().map(|&a| (a, a.default_key())).collect(), turn_step: 6.0 }
    }
}

impl Keys {
    /// The key bound to `action`.
    pub fn key(&self, action: Action) -> Key {
        self.bound.iter().find(|(a, _)| *a == action).map_or(action.default_key(), |&(_, k)| k)
    }

    /// Binds `key` to `action`. Another action of the same context (or the editor's, which
    /// work everywhere) with that key gets `action`'s old key instead, so no key does two
    /// things at once. Returns what changed.
    pub fn bind(&mut self, action: Action, key: Key) -> String {
        let old = self.key(action);
        let clashes = |other: Action| {
            other != action
                && (other.context() == action.context()
                    || other.context() == Context::Editor
                    || action.context() == Context::Editor)
        };
        let mut said = format!("{}: {}", action.label(), key.name());
        for (other, k) in &mut self.bound {
            if *k == key && clashes(*other) {
                *k = old;
                said = format!("{said} ({} now {})", other.label(), old.name());
            }
        }
        match self.bound.iter_mut().find(|(a, _)| *a == action) {
            Some((_, k)) => *k = key,
            None => self.bound.push((action, key)),
        }
        said
    }

    /// Saved as lines of `ACTION KEY` and `turn_step DEGREES`.
    pub fn to_text(&self) -> String {
        let mut text = String::from("# Moose editor keys: an action, then its key; and the editor's settings.\n");
        for &(action, key) in &self.bound {
            text.push_str(&format!("{action:?} {}\n", key.name()));
        }
        text.push_str(&format!("turn_step {}\n", self.turn_step));
        text
    }

    /// Read back from [`Keys::to_text`]'s text; lines it doesn't know are skipped (left from
    /// another version), and actions it doesn't name keep their default keys.
    pub fn parse(text: &str) -> Keys {
        let mut keys = Keys::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((name, value)) = line.split_once(' ') else {
                continue;
            };
            if name == "turn_step" {
                if let Ok(step) = value.trim().parse::<f32>()
                    && step > 0.0
                    && step <= 90.0
                {
                    keys.turn_step = step;
                }
                continue;
            }
            if let (Some(action), Some(key)) =
                (Action::ALL.iter().find(|a| format!("{a:?}") == name), Key::named(value.trim()))
            {
                keys.bound.retain(|(a, _)| a != action);
                keys.bound.push((*action, key));
            }
        }
        keys
    }

    /// The saved keys, or the defaults.
    pub fn load() -> Keys {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map_or_else(Keys::default, |t| Keys::parse(&t))
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = path() else {
            return if cfg!(test) { Ok(()) } else { Err("no home folder".into()) };
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, self.to_text()).map_err(|e| e.to_string())
    }
}

/// `~/.config/moose/editor_keys.txt` (none in tests, which keep off the real file).
fn path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/moose/editor_keys.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_bind_without_doing_two_things_and_read_back() {
        let mut keys = Keys::default();
        assert_eq!(keys.key(Action::Cleave), Key::C);
        // C in a face's context and a sector's is fine: one is selected at a time.
        assert_eq!(keys.key(Action::CutFace), Key::C);
        // Binding Extrude to J takes Open to a match's key: it gets E.
        assert_eq!((keys.key(Action::MergeFaces), keys.key(Action::OpenToMatch)), (Key::M, Key::J));
        keys.bind(Action::Extrude, Key::J);
        assert_eq!((keys.key(Action::Extrude), keys.key(Action::OpenToMatch)), (Key::J, Key::E));
        // An editor-wide key clashes with every context's.
        keys.bind(Action::Grid, Key::X);
        assert_eq!(keys.key(Action::DeleteSector), Key::G);
        keys.turn_step = 10.0;
        assert_eq!(Keys::parse(&keys.to_text()), keys);
        // Every action has a label and a context the gear menu lists.
        for a in Action::ALL {
            assert!(!a.label().is_empty() && Context::ALL.contains(&a.context()));
        }
    }
}
