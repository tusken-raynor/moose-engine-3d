//! Times showing frames: opens a window, presents frames as fast as it can for a few
//! seconds, and prints where the time went (see `PresentTimes`).
//!
//! Usage: cargo run --release -p moose-present --example present_bench [WIDTHxHEIGHT] [CAP]
//! [full] [toggle] [pattern]. `full` runs it fullscreen; `toggle` goes fullscreen after a
//! second and back after three, with the cursor locked as in play, printing the window's
//! size as it goes; `pattern` shows a still test pattern
//! (a checkerboard of single pixels at the left, a red border, a green diagonal) for
//! checking how the frame is scaled.

use std::time::{Duration, Instant};

use moose_present::Display;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let (width, height) = args
        .get(1)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let cap: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let full = args.iter().any(|a| a == "full");
    let pattern = args.iter().any(|a| a == "pattern");
    let toggle = args.iter().any(|a| a == "toggle");
    let mut display = Display::open("present bench", width, height, cap)?;
    display.set_fullscreen(full);
    let mut pixels = vec![0u32; (width * height) as usize];
    let (mut frames, mut sum) = (0u32, [0.0f64; 4]);
    let started = Instant::now();
    let mut window = (0, 0);
    while display.is_open() && started.elapsed() < Duration::from_secs(4) {
        if pattern {
            for y in 0..height {
                for x in 0..width {
                    let border = x < 4 || y < 4 || x >= width - 4 || y >= height - 4;
                    let checker = x < width / 4 && (x + y) % 2 == 0;
                    let diagonal = x * height / width == y;
                    pixels[(y * width + x) as usize] = if border {
                        0xFF_00_00
                    } else if diagonal {
                        0x00_FF_00
                    } else if checker {
                        0xFF_FF_FF
                    } else {
                        0x30_30_60
                    };
                }
            }
        } else {
            let shade = frames % 256;
            pixels.fill(shade << 16 | shade << 8 | shade);
        }
        display.present(&pixels)?;
        let t = display.present_times();
        if toggle {
            let at = started.elapsed().as_secs_f64();
            display.set_cursor_locked(true);
            let want = (1.0..3.0).contains(&at);
            if want != display.is_fullscreen() {
                display.set_fullscreen(want);
                println!("{at:.2} s: fullscreen {want}");
            }
            if frames % 30 == 0 {
                println!("{at:.2} s: window {}x{}", t.window.0, t.window.1);
            }
        }
        // The first second settles (the window appearing); count the rest.
        if started.elapsed() > Duration::from_secs(1) {
            frames += 1;
            for (s, v) in sum.iter_mut().zip([t.copy, t.show, t.wait, t.events]) {
                *s += v;
            }
            window = t.window;
        }
    }
    let seconds = started.elapsed().as_secs_f64() - 1.0;
    let n = frames.max(1) as f64;
    println!(
        "{width}x{height} in a {}x{} window, cap {cap}: {:.0} fps; per frame copy {:.2} ms, show {:.2} ms, wait {:.2} ms, events {:.2} ms",
        window.0, window.1, frames as f64 / seconds, sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n
    );
    Ok(())
}
