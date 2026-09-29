//! Presentation: a window that shows a finished framebuffer, paces frames, and reports
//! keyboard and mouse input. It only consumes frames; rendering happens elsewhere.

pub use minifb::{Key, MouseButton};
use minifb::{KeyRepeat, MouseMode, Window, WindowOptions};

/// A window showing a 32-bit XRGB framebuffer of a fixed size.
pub struct Display {
    window: Window,
    width: usize,
    height: usize,
    last_mouse: Option<(f32, f32)>,
}

impl Display {
    /// Opens a window for a `width` x `height` framebuffer. `max_fps` caps the frame rate
    /// (0 for uncapped).
    pub fn open(title: &str, width: u32, height: u32, max_fps: u32) -> Result<Display, String> {
        let mut window = Window::new(
            title,
            width as usize,
            height as usize,
            WindowOptions::default(),
        )
        .map_err(|e| format!("cannot open window: {e}"))?;
        window.set_target_fps(max_fps as usize);
        Ok(Display {
            window,
            width: width as usize,
            height: height as usize,
            last_mouse: None,
        })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    /// Shows a finished frame and collects new input. Blocks as needed to hold the frame
    /// rate cap.
    pub fn present(&mut self, pixels: &[u32]) -> Result<(), String> {
        assert_eq!(
            pixels.len(),
            self.width * self.height,
            "framebuffer size does not match the window"
        );
        self.window
            .update_with_buffer(pixels, self.width, self.height)
            .map_err(|e| format!("cannot present frame: {e}"))
    }

    pub fn set_title(&mut self, title: &str) {
        self.window.set_title(title);
    }

    pub fn set_max_fps(&mut self, max_fps: u32) {
        self.window.set_target_fps(max_fps as usize);
    }

    /// True while `key` is held.
    pub fn key_down(&self, key: Key) -> bool {
        self.window.is_key_down(key)
    }

    /// True once for each press of `key` (no key repeat).
    pub fn key_pressed(&self, key: Key) -> bool {
        self.window.is_key_pressed(key, KeyRepeat::No)
    }

    /// True once for each press of `key`, and again as it repeats while held.
    pub fn key_repeated(&self, key: Key) -> bool {
        self.window.is_key_pressed(key, KeyRepeat::Yes)
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        self.window.get_mouse_down(button)
    }

    /// How far the mouse moved since the last call, in window pixels.
    pub fn mouse_delta(&mut self) -> (f32, f32) {
        let now = self.window.get_mouse_pos(MouseMode::Pass);
        let delta = match (self.last_mouse, now) {
            (Some((x0, y0)), Some((x1, y1))) => (x1 - x0, y1 - y0),
            _ => (0.0, 0.0),
        };
        self.last_mouse = now;
        delta
    }
}
