//! Presentation: a window that shows a finished framebuffer, paces frames, and reports
//! keyboard and mouse input. It only consumes frames; rendering happens elsewhere.
//!
//! Built on winit (the window and input) and softbuffer (putting pixels on screen). The
//! app polls: each [`Display::present`] shows a frame, waits out the frame rate cap, and
//! gathers the input that arrived meanwhile, which the `key_*` and `mouse_*` calls then
//! report until the next one.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
#[cfg(not(target_os = "macos"))]
use winit::window::Fullscreen;
use winit::window::{CursorGrabMode, Window, WindowId};

/// A key, by where it is on the keyboard (a US layout's names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Key0, Key1, Key2, Key3, Key4, Key5, Key6, Key7, Key8, Key9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Up, Down, Left, Right,
    Escape, Enter, Backspace, Tab, Space, Delete, Insert, Home, End, PageUp, PageDown,
    LeftShift, RightShift, LeftCtrl, RightCtrl, LeftAlt, RightAlt, LeftSuper, RightSuper,
    Minus, Equal, LeftBracket, RightBracket, Backslash, Semicolon, Apostrophe, Comma,
    Period, Slash, Backquote,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    fn index(self) -> usize {
        self as usize
    }
}

/// A window showing a 32-bit XRGB framebuffer of a fixed size, scaled to fill the window
/// (keeping its shape, with black bars) when the window is another size: a high-density
/// display, a resized window, or fullscreen.
pub struct Display {
    event_loop: EventLoop<()>,
    state: State,
}

/// The window and everything input has said since the last frame.
struct State {
    title: String,
    /// The framebuffer's size.
    width: u32,
    height: u32,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    open: bool,
    fullscreen: bool,
    cursor_locked: bool,
    keys_down: HashSet<Key>,
    /// Keys pressed, and pressed or repeated, since the last frame.
    pressed: HashSet<Key>,
    repeated: HashSet<Key>,
    buttons_down: [bool; 3],
    clicked: [bool; 3],
    /// Mouse movement since the last `mouse_delta`, in device units (raw, so it keeps
    /// coming while the cursor is locked).
    motion: (f64, f64),
    /// Where the cursor is in the window, in physical pixels.
    cursor: Option<PhysicalPosition<f64>>,
    /// Scrolling since the last frame, in lines (up is positive).
    scroll: f32,
    /// Text typed since the last frame.
    text: String,
    /// Frame pacing: the least time between frames, and when the last one was shown.
    interval: Option<Duration>,
    last_frame: Instant,
    /// For scaling: each window column's framebuffer column, for the window's size.
    columns: Vec<u32>,
    /// How long the last frame's showing took (see [`PresentTimes`]).
    times: PresentTimes,
    /// The system scales frames to fit the window (macOS): they go over at their own size.
    system_scales: bool,
}

/// Where the last [`Display::present`] spent its time, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PresentTimes {
    /// Copying (and scaling) the frame into the window's buffer.
    pub copy: f64,
    /// Handing the buffer to the system to show.
    pub show: f64,
    /// Waiting out the frame rate cap.
    pub wait: f64,
    /// Gathering input and window events.
    pub events: f64,
    /// The window's size in pixels.
    pub window: (u32, u32),
}

impl Display {
    /// Opens a window for a `width` x `height` framebuffer. `max_fps` caps the frame rate
    /// (0 for uncapped).
    pub fn open(title: &str, width: u32, height: u32, max_fps: u32) -> Result<Display, String> {
        let event_loop = EventLoop::new().map_err(|e| format!("cannot start the window system: {e}"))?;
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut display = Display {
            event_loop,
            state: State {
                title: title.to_string(),
                width,
                height,
                window: None,
                surface: None,
                open: true,
                fullscreen: false,
                cursor_locked: false,
                keys_down: HashSet::new(),
                pressed: HashSet::new(),
                repeated: HashSet::new(),
                buttons_down: [false; 3],
                clicked: [false; 3],
                motion: (0.0, 0.0),
                cursor: None,
                scroll: 0.0,
                text: String::new(),
                interval: None,
                last_frame: Instant::now(),
                columns: Vec::new(),
                times: PresentTimes::default(),
                system_scales: false,
            },
        };
        display.set_max_fps(max_fps);
        // The window is made once the event loop starts.
        let started = Instant::now();
        while display.state.surface.is_none() {
            display.pump(Some(Duration::from_millis(10)));
            if !display.state.open || started.elapsed() > Duration::from_secs(10) {
                return Err("cannot open window".into());
            }
        }
        Ok(display)
    }

    pub fn is_open(&self) -> bool {
        self.state.open
    }

    /// Shows a finished frame, waits out the frame rate cap, and gathers new input.
    pub fn present(&mut self, pixels: &[u32]) -> Result<(), String> {
        assert_eq!(
            pixels.len(),
            (self.state.width * self.state.height) as usize,
            "framebuffer size does not match the window"
        );
        self.state.draw(pixels)?;
        // Frames keep to a schedule, one interval apart: a sleep that overshoots makes the
        // next wait shorter, rather than every frame late. A frame that's late anyway
        // starts the schedule again.
        let waiting = Instant::now();
        self.state.last_frame = match self.state.interval {
            Some(interval) if self.state.last_frame + interval > waiting => {
                let next = self.state.last_frame + interval;
                std::thread::sleep(next - waiting);
                next
            }
            _ => waiting,
        };
        self.state.times.wait = ms(waiting.elapsed());
        let s = &mut self.state;
        s.pressed.clear();
        s.repeated.clear();
        s.clicked = [false; 3];
        s.scroll = 0.0;
        s.text.clear();
        let pumping = Instant::now();
        self.pump(Some(Duration::ZERO));
        self.state.times.events = ms(pumping.elapsed());
        Ok(())
    }

    /// Where the last `present` spent its time.
    pub fn present_times(&self) -> PresentTimes {
        self.state.times
    }

    fn pump(&mut self, timeout: Option<Duration>) {
        if let PumpStatus::Exit(_) = self.event_loop.pump_app_events(timeout, &mut self.state) {
            self.state.open = false;
        }
    }

    pub fn set_title(&mut self, title: &str) {
        self.state.title = title.to_string();
        if let Some(window) = &self.state.window {
            window.set_title(title);
        }
    }

    pub fn set_max_fps(&mut self, max_fps: u32) {
        self.state.interval =
            (max_fps > 0).then(|| Duration::from_secs_f64(1.0 / max_fps as f64));
    }

    /// Fills the screen with the window (borderless), or puts it back.
    pub fn set_fullscreen(&mut self, on: bool) {
        self.state.fullscreen = on;
        if let Some(window) = &self.state.window {
            apply_fullscreen(window, on);
        }
    }

    pub fn is_fullscreen(&self) -> bool {
        self.state.fullscreen
    }

    /// Locks the cursor in place and hides it (for mouse look: `mouse_delta` keeps
    /// reporting movement), or frees and shows it (for pointing and clicking).
    pub fn set_cursor_locked(&mut self, locked: bool) {
        if self.state.cursor_locked == locked {
            return;
        }
        self.state.cursor_locked = locked;
        if let Some(window) = &self.state.window {
            apply_cursor_lock(window, locked);
        }
    }

    /// True while `key` is held.
    pub fn key_down(&self, key: Key) -> bool {
        self.state.keys_down.contains(&key)
    }

    /// True once for each press of `key` (no key repeat).
    pub fn key_pressed(&self, key: Key) -> bool {
        self.state.pressed.contains(&key)
    }

    /// True once for each press of `key`, and again as it repeats while held.
    pub fn key_repeated(&self, key: Key) -> bool {
        self.state.repeated.contains(&key)
    }

    pub fn mouse_down(&self, button: MouseButton) -> bool {
        self.state.buttons_down[button.index()]
    }

    /// True once for each press of `button`.
    pub fn mouse_clicked(&self, button: MouseButton) -> bool {
        self.state.clicked[button.index()]
    }

    /// How far the mouse moved since the last call, in device units (about pixels),
    /// whether or not the cursor is locked.
    pub fn mouse_delta(&mut self) -> (f32, f32) {
        let (x, y) = std::mem::take(&mut self.state.motion);
        (x as f32, y as f32)
    }

    /// Where the cursor is, in framebuffer pixels (through the scaling), if it is over
    /// the framebuffer and not locked.
    pub fn cursor_position(&self) -> Option<(f32, f32)> {
        let s = &self.state;
        if s.cursor_locked {
            return None;
        }
        let (position, window) = (s.cursor?, s.window.as_ref()?);
        let size = window.inner_size();
        let (scale, left, top) = fit(s.width, s.height, size.width, size.height);
        let x = (position.x as f32 - left as f32) / scale;
        let y = (position.y as f32 - top as f32) / scale;
        (x >= 0.0 && y >= 0.0 && x < s.width as f32 && y < s.height as f32).then_some((x, y))
    }

    /// How far the mouse wheel turned since the last frame, in lines (up is positive).
    pub fn scroll(&self) -> f32 {
        self.state.scroll
    }

    /// The text typed since the last frame.
    pub fn text(&self) -> &str {
        &self.state.text
    }
}

/// How a `width` x `height` framebuffer fills a window: its scale, and the left and top
/// margins (black bars) that center it.
fn fit(width: u32, height: u32, window_width: u32, window_height: u32) -> (f32, u32, u32) {
    let scale = (window_width as f32 / width as f32).min(window_height as f32 / height as f32);
    let (w, h) = ((width as f32 * scale) as u32, (height as f32 * scale) as u32);
    (scale, (window_width - w.min(window_width)) / 2, (window_height - h.min(window_height)) / 2)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Sets up how macOS shows our frames; returns whether it scales them itself.
///
/// - The window gets the color space softbuffer tags frames with (device RGB), so they're
///   shown as they are. Otherwise macOS converts every pixel of every frame to the display's
///   color profile on the CPU as the frame is shown: about 11 ms a frame in a 2560 x 1440
///   window, which held the frame rate near 80.
/// - softbuffer's layer scales its contents to fit, keeping their shape, nearest pixel (on
///   the GPU, by the compositor), with the window black around them. So frames go over at
///   the framebuffer's size: no scaling on the CPU, and softbuffer (which allocates a fresh
///   buffer each frame) clears a quarter of the memory on a Retina display.
#[cfg(target_os = "macos")]
fn setup_macos(window: &Window) -> bool {
    use objc2_app_kit::{NSColor, NSColorSpace, NSView};
    use objc2_quartz_core::{kCAFilterNearest, kCAGravityResizeAspect};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return false;
    };
    // SAFETY: winit's handle is to the window's live NSView, used on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let Some(ns_window) = view.window() else {
        return false;
    };
    unsafe {
        ns_window.setColorSpace(Some(&NSColorSpace::deviceRGBColorSpace()));
        ns_window.setBackgroundColor(Some(&NSColor::blackColor()));
    }
    // softbuffer's layer: the one it adds to the view's.
    let layer = unsafe { view.layer().and_then(|root| root.sublayers()?.lastObject()) };
    let Some(layer) = layer else {
        return false;
    };
    unsafe {
        layer.setContentsGravity(kCAGravityResizeAspect);
        layer.setMagnificationFilter(kCAFilterNearest);
        layer.setMinificationFilter(kCAFilterNearest);
    }
    true
}

/// Fills the screen with the window, or puts it back. On macOS, the "simple" way: the
/// window covers the screen and the menu bar and Dock hide. (winit's borderless fullscreen
/// there is the system's own, which animates the window into a Space of its own; that got
/// stuck partway, the window on a grey backdrop and keys lost.)
fn apply_fullscreen(window: &Window, on: bool) {
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowExtMacOS;
        window.set_simple_fullscreen(on);
    }
    #[cfg(not(target_os = "macos"))]
    window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
}

fn apply_cursor_lock(window: &Window, locked: bool) {
    if locked {
        // Locked where supported (macOS, Wayland); otherwise kept inside the window.
        if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
    }
    window.set_cursor_visible(!locked);
}

impl State {
    /// Scales the framebuffer into the window's buffer (nearest pixel), and shows it.
    fn draw(&mut self, pixels: &[u32]) -> Result<(), String> {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let window_size = window.inner_size();
        if window_size.width == 0 || window_size.height == 0 {
            return Ok(()); // minimized
        }
        let size = if self.system_scales {
            winit::dpi::PhysicalSize::new(self.width, self.height)
        } else {
            window_size
        };
        let (Some(ww), Some(wh)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        surface
            .resize(ww, wh)
            .map_err(|e| format!("cannot size the window's buffer: {e}"))?;
        let copying = Instant::now();
        let mut buffer = surface
            .buffer_mut()
            .map_err(|e| format!("cannot draw to the window: {e}"))?;
        let (width, height) = (self.width, self.height);
        let (window_width, window_height) = (size.width, size.height);
        if (window_width, window_height) == (width, height) {
            buffer.copy_from_slice(pixels);
        } else {
            let (scale, left, top) = fit(width, height, window_width, window_height);
            let (shown_w, shown_h) = (window_width - 2 * left, window_height - 2 * top);
            self.columns.clear();
            self.columns.extend(
                (0..shown_w).map(|x| (((x as f32 + 0.5) / scale) as u32).min(width - 1)),
            );
            let columns = &self.columns;
            buffer
                .par_chunks_mut(window_width as usize)
                .enumerate()
                .for_each(|(y, row)| {
                    let y = y as u32;
                    if y < top || y >= top + shown_h {
                        row.fill(0);
                        return;
                    }
                    let source_y = (((y - top) as f32 + 0.5) / scale) as u32;
                    let source = &pixels[(source_y.min(height - 1) * width) as usize..][..width as usize];
                    row[..left as usize].fill(0);
                    for (out, &x) in row[left as usize..(left + shown_w) as usize].iter_mut().zip(columns) {
                        *out = source[x as usize];
                    }
                    row[(left + shown_w) as usize..].fill(0);
                });
        }
        let showing = Instant::now();
        self.times.copy = ms(showing - copying);
        self.times.window = (window_size.width, window_size.height);
        buffer.present().map_err(|e| format!("cannot present frame: {e}"))?;
        self.times.show = ms(showing.elapsed());
        Ok(())
    }
}

impl ApplicationHandler for State {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(LogicalSize::new(self.width, self.height));
        let Ok(window) = event_loop.create_window(attributes) else {
            self.open = false;
            return;
        };
        let window = Rc::new(window);
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        let Ok(surface) = surface else {
            self.open = false;
            return;
        };
        #[cfg(target_os = "macos")]
        {
            self.system_scales = setup_macos(&window);
        }
        if self.fullscreen {
            apply_fullscreen(&window, true);
        }
        apply_cursor_lock(&window, self.cursor_locked);
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.open = false,
            WindowEvent::Focused(false) => {
                self.keys_down.clear();
                self.buttons_down = [false; 3];
            }
            WindowEvent::Focused(true) => {
                // Grabs don't survive losing focus everywhere: take it back.
                if let Some(window) = &self.window {
                    apply_cursor_lock(window, self.cursor_locked);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = match event.physical_key {
                    PhysicalKey::Code(code) => key_of(code),
                    PhysicalKey::Unidentified(_) => None,
                };
                if event.state == ElementState::Pressed {
                    if let Some(key) = key {
                        if !event.repeat {
                            self.keys_down.insert(key);
                            self.pressed.insert(key);
                        }
                        self.repeated.insert(key);
                    }
                    if let Some(text) = &event.text {
                        self.text.extend(text.chars().filter(|c| !c.is_control()));
                    }
                } else if let Some(key) = key {
                    self.keys_down.remove(&key);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                let pressed = state == ElementState::Pressed;
                self.buttons_down[button.index()] = pressed;
                if pressed {
                    self.clicked[button.index()] = true;
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor = Some(position),
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::MouseWheel { delta, .. } => {
                self.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0,
                };
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.motion.0 += delta.0;
            self.motion.1 += delta.1;
        }
    }
}

/// Our key for a winit key code, if we name it.
fn key_of(code: KeyCode) -> Option<Key> {
    use Key::*;
    Some(match code {
        KeyCode::KeyA => A, KeyCode::KeyB => B, KeyCode::KeyC => C, KeyCode::KeyD => D,
        KeyCode::KeyE => E, KeyCode::KeyF => F, KeyCode::KeyG => G, KeyCode::KeyH => H,
        KeyCode::KeyI => I, KeyCode::KeyJ => J, KeyCode::KeyK => K, KeyCode::KeyL => L,
        KeyCode::KeyM => M, KeyCode::KeyN => N, KeyCode::KeyO => O, KeyCode::KeyP => P,
        KeyCode::KeyQ => Q, KeyCode::KeyR => R, KeyCode::KeyS => S, KeyCode::KeyT => T,
        KeyCode::KeyU => U, KeyCode::KeyV => V, KeyCode::KeyW => W, KeyCode::KeyX => X,
        KeyCode::KeyY => Y, KeyCode::KeyZ => Z,
        KeyCode::Digit0 => Key0, KeyCode::Digit1 => Key1, KeyCode::Digit2 => Key2,
        KeyCode::Digit3 => Key3, KeyCode::Digit4 => Key4, KeyCode::Digit5 => Key5,
        KeyCode::Digit6 => Key6, KeyCode::Digit7 => Key7, KeyCode::Digit8 => Key8,
        KeyCode::Digit9 => Key9,
        KeyCode::F1 => F1, KeyCode::F2 => F2, KeyCode::F3 => F3, KeyCode::F4 => F4,
        KeyCode::F5 => F5, KeyCode::F6 => F6, KeyCode::F7 => F7, KeyCode::F8 => F8,
        KeyCode::F9 => F9, KeyCode::F10 => F10, KeyCode::F11 => F11, KeyCode::F12 => F12,
        KeyCode::ArrowUp => Up, KeyCode::ArrowDown => Down, KeyCode::ArrowLeft => Left,
        KeyCode::ArrowRight => Right,
        KeyCode::Escape => Escape, KeyCode::Enter | KeyCode::NumpadEnter => Enter,
        KeyCode::Backspace => Backspace, KeyCode::Tab => Tab, KeyCode::Space => Space,
        KeyCode::Delete => Delete, KeyCode::Insert => Insert, KeyCode::Home => Home,
        KeyCode::End => End, KeyCode::PageUp => PageUp, KeyCode::PageDown => PageDown,
        KeyCode::ShiftLeft => LeftShift, KeyCode::ShiftRight => RightShift,
        KeyCode::ControlLeft => LeftCtrl, KeyCode::ControlRight => RightCtrl,
        KeyCode::AltLeft => LeftAlt, KeyCode::AltRight => RightAlt,
        KeyCode::SuperLeft => LeftSuper, KeyCode::SuperRight => RightSuper,
        KeyCode::Minus => Minus, KeyCode::Equal => Equal,
        KeyCode::BracketLeft => LeftBracket, KeyCode::BracketRight => RightBracket,
        KeyCode::Backslash => Backslash, KeyCode::Semicolon => Semicolon,
        KeyCode::Quote => Apostrophe, KeyCode::Comma => Comma, KeyCode::Period => Period,
        KeyCode::Slash => Slash, KeyCode::Backquote => Backquote,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn a_framebuffer_fills_the_window_keeping_its_shape() {
        // Twice the size (a high-density display): scaled 2x, no bars.
        assert_eq!(fit(1280, 720, 2560, 1440), (2.0, 0, 0));
        // A 16:10 screen: 1.5x, bars above and below.
        assert_eq!(fit(1280, 720, 1920, 1200), (1.5, 0, 60));
        // A 4:3 screen from 320x180: 3.2x, bars above and below.
        let (scale, left, top) = fit(320, 180, 1024, 768);
        assert_eq!((scale, left, top), (3.2, 0, 96));
        // Narrower than the framebuffer's shape: bars at the sides.
        assert_eq!(fit(1280, 720, 1000, 720), (1000.0 / 1280.0, 0, (720 - 562) / 2));
    }
}
