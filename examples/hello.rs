//! hello: the smallest x11-direct program: a window with text and shapes
//! that redraws when resized and quits on q, Escape or the close button.
//!
//! Run:  DISPLAY=HOST:0 cargo run --example hello

use x11_direct::{key, Event, X11};

fn draw(x: &mut X11) {
    let (w, h) = (x.width(), x.height());
    x.color(x.rgb(20, 24, 40));
    x.fill_rect(0, 0, w, h);
    x.color(x.rgb(70, 130, 220));
    x.fill_ellipse(w / 2 - 60, h / 2 - 60, 120, 120);
    x.color(x.rgb(240, 180, 60));
    x.line_width(3);
    x.rect(10, 10, w - 20, h - 20);
    x.color(x.rgb(255, 255, 255));
    x.text(20, 34, "hello from x11-direct - q quits");
    x.present();
}

fn main() {
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    x.window("hello", 360, 220);
    draw(&mut x);
    loop {
        while let Some(e) = x.event() {
            match e {
                Event::Close => return,
                Event::KeyDown { key, .. } if key == 'q' as u32 || key == key::ESCAPE => return,
                Event::Resize { .. } => draw(&mut x),
                _ => {}
            }
        }
        x.wait(-1); // nothing animates: sleep until input
    }
}
