//! graph: a live scrolling graph that costs a few hundred bytes per
//! update, whatever the window's size.
//!
//! Run:  DISPLAY=HOST:0 cargo run --example graph
//! Keys: space pauses, + and - change the speed, q or Escape quits.
//!
//! The patterns it shows (see reference/patterns.md):
//!   - the plot lives on the X server in a canvas. Each sample moves it one
//!     step left with a single copy and draws only the new strip at the
//!     right, instead of drawing the whole graph again;
//!   - the title and the panel are drawn once, and again only when the
//!     window's size has settled after a resize (not on every resize event);
//!   - the label is sent only when its text changes;
//!   - fonts by XLFD pattern, falling back to "fixed" when the server
//!     doesn't have them;
//!   - sync() after each frame, so a slow link can't queue up old frames;
//!   - a timer loop: wait() sleeps until input or the next sample is due.

use std::time::{Duration, Instant};
use x11_direct::{key, Canvas, Event, Font, X11};

const STEP: i32 = 3; // pixels per sample
const PAD: i32 = 12;

struct Colors {
    bg: u32,
    panel: u32,
    grid: u32,
    fill: u32,
    line: u32,
    text: u32,
    dim: u32,
}

struct Graph {
    c: Colors,
    title: Font,
    label: Font,
    plot: Option<Canvas>,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    hist: Vec<i32>, // 0-100, newest last
    shown: String,  // the label as last drawn
}

impl Graph {
    fn y(&self, v: i32) -> i32 {
        self.ph - 1 - v * (self.ph - 1) / 100
    }

    /// age 0 is the newest sample, at the right edge
    fn x(&self, age: i32) -> i32 {
        self.pw - 1 - age * STEP
    }

    /// The background and grid of plot columns x..x+w-1.
    fn strip(&self, x: &mut X11, sx: i32, w: i32) {
        x.color(self.c.panel);
        x.fill_rect(sx, 0, w, self.ph);
        x.color(self.c.grid);
        for i in 1..4 {
            x.line(sx, self.ph * i / 4, sx + w - 1, self.ph * i / 4);
        }
    }

    /// The segment from the sample at age+1 to the one at age, with the
    /// area under it filled.
    fn segment(&self, x: &mut X11, age: usize) {
        let n = self.hist.len();
        let (x0, x1) = (self.x(age as i32 + 1), self.x(age as i32));
        let (y0, y1) = (
            self.y(self.hist[n - 2 - age]),
            self.y(self.hist[n - 1 - age]),
        );
        let ph = self.ph as i16;
        let (x0s, x1s) = (x0 as i16, x1 as i16);
        x.color(self.c.fill);
        x.fill_poly(&[[x0s, y0 as i16], [x1s, y1 as i16], [x1s, ph], [x0s, ph]]);
        x.color(self.c.line);
        x.line(x0, y0, x1, y1);
    }

    /// Lay out for the window's size now: everything drawn whole, once.
    fn layout(&mut self, x: &mut X11) {
        let th = x.font_ascent(self.title) + x.font_descent(self.title);
        let lh = x.font_ascent(self.label) + x.font_descent(self.label);
        self.px = PAD;
        self.py = PAD + th + 8;
        self.pw = x.width() - 2 * PAD;
        self.ph = x.height() - self.py - PAD - lh - 6;
        if let Some(c) = self.plot.take() {
            x.free_canvas(c);
        }
        if self.pw < 2 * STEP || self.ph < 10 {
            return;
        }
        let plot = x.canvas(self.pw, self.ph);
        self.plot = Some(plot);

        let (w, h) = (x.width(), x.height());
        x.color(self.c.bg);
        x.fill_rect(0, 0, w, h);
        x.text_font(self.title);
        x.color(self.c.text);
        let a = x.font_ascent(self.title);
        x.text(PAD, PAD + a, "x11-direct graph");

        x.target(Some(plot));
        self.strip(x, 0, self.pw);
        x.line_width(2);
        let mut age = 0;
        while age + 1 < self.hist.len() && self.x(age as i32 + 1) >= -STEP {
            self.segment(x, age);
            age += 1;
        }
        x.line_width(0);
        x.target(None);
        x.blit(plot, 0, 0, self.pw, self.ph, self.px, self.py);
        self.shown.clear(); // the label is drawn again
    }

    /// One more sample: the plot moves left a step and gets a new strip.
    fn push(&mut self, x: &mut X11, v: i32) {
        self.hist.push(v);
        if self.hist.len() > 4096 {
            self.hist.drain(..2048);
        }
        let Some(plot) = self.plot else { return };
        if self.hist.len() < 2 {
            return;
        }
        x.target(Some(plot));
        x.blit(plot, STEP, 0, self.pw - STEP, self.ph, 0, 0);
        self.strip(x, self.pw - STEP, STEP);
        x.line_width(2);
        self.segment(x, 0);
        x.line_width(0);
        x.target(None);
        x.blit(plot, 0, 0, self.pw, self.ph, self.px, self.py);
    }

    /// The label under the plot, sent only when its text changes.
    fn label(&mut self, x: &mut X11, s: String) {
        if s == self.shown || self.plot.is_none() {
            return;
        }
        let y = self.py + self.ph + 6;
        let (a, d) = (x.font_ascent(self.label), x.font_descent(self.label));
        x.color(self.c.bg);
        x.fill_rect(self.px, y, self.pw, a + d);
        x.text_font(self.label);
        x.color(self.c.dim);
        x.text(self.px, y + a, &s);
        self.shown = s;
    }
}

/// A small random-number generator, so the example needs no crates.
fn rand(state: &mut u64) -> i32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 33) as i32
}

fn main() {
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let c = Colors {
        bg: x.rgb(24, 26, 33),
        panel: x.rgb(32, 36, 46),
        grid: x.rgb(52, 58, 72),
        fill: x.rgb(40, 84, 110),
        line: x.rgb(90, 200, 250),
        text: x.rgb(235, 238, 245),
        dim: x.rgb(150, 158, 175),
    };
    // Font::FIXED (the server's "fixed") if the server has no Helvetica
    let title = x.font("-*-helvetica-bold-r-normal--17-*-*-*-p-*-iso8859-1");
    let label = x.font("-*-helvetica-medium-r-normal--12-*-*-*-p-*-iso8859-1");
    x.background(c.bg);
    x.window("graph", 520, 260);
    x.min_size(200, 120);
    let mut g = Graph {
        c,
        title,
        label,
        plot: None,
        px: 0,
        py: 0,
        pw: 0,
        ph: 0,
        hist: Vec::new(),
        shown: String::new(),
    };
    g.layout(&mut x);

    let (mut v, mut paused, mut period, mut bytes) = (50, false, 100u64, 0u64);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64 ^ std::process::id() as u64;
    let mut size = (x.width(), x.height());
    let mut resize_at: Option<Instant> = None;
    let mut next = Instant::now();
    loop {
        while let Some(e) = x.event() {
            match e {
                Event::Close => return,
                // lay out once the size settles, not on every step of a drag
                Event::Resize { .. } => {
                    resize_at = Some(Instant::now() + Duration::from_millis(80))
                }
                Event::KeyDown { key, .. } => match key {
                    k if k == 'q' as u32 || k == key::ESCAPE => return,
                    k if k == ' ' as u32 => paused = !paused,
                    k if k == '+' as u32 || k == '=' as u32 => {
                        period = period.saturating_sub(20).max(20)
                    }
                    k if k == '-' as u32 => period = (period + 20).min(1000),
                    _ => {}
                },
                _ => {}
            }
        }
        let now = Instant::now();
        if resize_at.is_some_and(|t| now >= t) {
            resize_at = None;
            if size != (x.width(), x.height()) {
                size = (x.width(), x.height());
                g.layout(&mut x);
                g.label(
                    &mut x,
                    format!("value {v:3}   {bytes} bytes/update   every {period} ms"),
                );
                x.present();
                x.sync();
            }
        }
        if !paused && now >= next {
            let before = x.sent();
            next = now + Duration::from_millis(period);
            v += rand(&mut seed) % 13 - 6; // a random walk, with spikes
            if rand(&mut seed) % 40 == 0 {
                v += rand(&mut seed) % 50 - 25;
            }
            v = v.clamp(0, 100);
            g.push(&mut x, v);
            g.label(
                &mut x,
                format!("value {v:3}   {bytes} bytes/update   every {period} ms"),
            );
            x.present();
            x.sync(); // don't run ahead of the server
            bytes = x.sent() - before;
        }
        let until = if paused {
            Duration::from_secs(1)
        } else {
            next.saturating_duration_since(Instant::now())
        };
        let until = resize_at.map_or(until, |t| {
            until.min(t.saturating_duration_since(Instant::now()))
        });
        x.wait(until.as_millis() as i32);
    }
}
