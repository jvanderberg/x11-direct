//! xeyes: a pair of eyes that follow the mouse, even outside the window.
//!
//! Run:  DISPLAY=HOST:0 cargo run --example xeyes
//! Keys: q or Escape quits, as does closing the window.
//!
//! Shows: asking where the pointer is (`pointer`), drawing only when
//! something changed, and a 30 ms polling loop.

use x11_direct::{key, Event, X11};

fn oval(x: &mut X11, cx: i32, cy: i32, rx: i32, ry: i32) {
    x.fill_ellipse(cx - rx, cy - ry, 2 * rx, 2 * ry);
}

/// One eye centred on (cx, cy), looking at the mouse at (mx, my).
fn eye(x: &mut X11, (cx, cy): (i32, i32), (rx, ry): (i32, i32), (mx, my): (i32, i32)) {
    let m = rx.min(ry);
    let t = m / 10 + 1; // outline
    let pr = m / 3; // pupil
    let (ax, ay) = (
        (rx - t - pr - 1).max(0) as f64,
        (ry - t - pr - 1).max(0) as f64,
    );
    let (mut dx, mut dy) = ((mx - cx) as f64, (my - cy) as f64);

    x.color(x.rgb(0, 0, 0));
    oval(x, cx, cy, rx, ry);
    x.color(x.rgb(255, 255, 255));
    oval(x, cx, cy, rx - t, ry - t);
    // keep the pupil inside: (dx/ax)^2 + (dy/ay)^2 <= 1
    let r = if ax > 0.0 && ay > 0.0 {
        (dx / ax).powi(2) + (dy / ay).powi(2)
    } else {
        0.0
    };
    if r > 1.0 {
        dx /= r.sqrt();
        dy /= r.sqrt();
    }
    x.color(x.rgb(0, 0, 0));
    oval(x, cx + dx as i32, cy + dy as i32, pr, pr);
}

fn main() {
    let mut x = X11::open(std::env::args().nth(1).as_deref()).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    x.window("xeyes", 320, 200);
    let (mut last, mut dirty) = (None, true);
    loop {
        while let Some(e) = x.event() {
            match e {
                Event::Close => return,
                Event::KeyDown { key, .. } if key == 'q' as u32 || key == key::ESCAPE => return,
                Event::Resize { .. } => dirty = true,
                _ => {}
            }
        }
        let p = x.pointer();
        if p.is_some() && p != last {
            last = p;
            dirty = true;
        }
        if dirty {
            let (w, h) = (x.width(), x.height());
            let (rx, ry) = (w / 4 - 4, h / 2 - 4);
            let m = last.unwrap_or((0, 0));
            x.color(x.rgb(255, 255, 255));
            x.fill_rect(0, 0, w, h);
            if rx > 4 && ry > 4 {
                eye(&mut x, (w / 4, h / 2), (rx, ry), m);
                eye(&mut x, (3 * w / 4, h / 2), (rx, ry), m);
            }
            x.present();
            dirty = false;
        }
        x.wait(30);
    }
}
