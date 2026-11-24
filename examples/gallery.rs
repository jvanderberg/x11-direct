//! gallery: one panel per drawing feature, as a visual check of the whole
//! API: shapes, batches, polygons, images, canvases, masks, stencils,
//! stipples, tiled fills, fonts and Unicode text, with fence-paced frames.
//!
//! Run:  DISPLAY=HOST:0 cargo run --example gallery
//! Keys: q or Escape quits.

use x11_direct::{key, Event, Font, X11};

const CW: i32 = 200; // panel size
const CH: i32 = 150;

fn panel(x: &mut X11, col: i32, row: i32, title: &str) -> (i32, i32) {
    let (px, py) = (10 + col * (CW + 10), 10 + row * (CH + 10));
    x.color(x.rgb(36, 40, 52));
    x.fill_rect(px, py, CW, CH);
    x.color(x.rgb(160, 168, 186));
    x.text_font(Font::FIXED);
    x.text(px + 6, py + 14, title);
    (px, py + 20)
}

fn main() {
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let big = x.font("-*-helvetica-bold-r-normal--24-*-*-*-p-*-iso8859-1");
    let cell = x.font_cell("-misc-fixed-medium-r-normal--18-*-*-*-c-*-iso10646-1");
    x.background(x.rgb(20, 22, 30));
    x.window("gallery", 4 * (CW + 10) + 10, 2 * (CH + 10) + 10);

    // made once, used every frame: a sprite canvas with a round mask, its
    // stencil, a 4x4 dither, and a vertical gradient tile
    let sprite = x.canvas(48, 48);
    x.target(Some(sprite));
    for i in 0..48 {
        x.color(x.rgb(255, (i * 5) as u8, 40));
        x.fill_rect(0, i, 48, 1);
    }
    x.target(None);
    let mask = x.mask(48, 48);
    let rows: Vec<[i16; 4]> = (0..48)
        .map(|y| {
            let dy = y as f64 - 23.5;
            let half = (24.0f64 * 24.0 - dy * dy).max(0.0).sqrt() as i16;
            [24 - half, y as i16, 2 * half, 1]
        })
        .collect();
    x.mask_rects(mask, &rows);
    let stencil = x.stencil(mask);
    let dither = x.mask(4, 4);
    x.mask_rects(
        dither,
        &[[0, 0, 1, 1], [2, 2, 1, 1], [2, 0, 1, 1], [0, 2, 1, 1]],
    );
    let tile = x.canvas(1, CH);
    x.target(Some(tile));
    for i in 0..CH {
        x.color(x.rgb(30, (60 + i) as u8, (120 + i / 2) as u8));
        x.fill_rect(0, i, 1, 1);
    }
    x.target(None);
    let pixels: Vec<u32> = (0..120 * 100)
        .map(|i| x.rgb(((i % 120) * 2) as u8, ((i / 120) * 2) as u8, 160))
        .collect();

    let mut t = 0i32;
    loop {
        while let Some(e) = x.event() {
            match e {
                Event::Close => return,
                Event::KeyDown { key, .. } if key == 'q' as u32 || key == key::ESCAPE => return,
                _ => {}
            }
        }
        x.fence_wait(); // the last frame has been drawn: one frame in flight

        let (px, py) = panel(&mut x, 0, 0, "shapes");
        x.color(x.rgb(90, 200, 250));
        x.fill_rect(px + 10, py + 10, 60, 40);
        x.color(x.rgb(250, 180, 60));
        x.line_width(3);
        x.rect(px + 80, py + 10, 60, 40);
        x.ellipse(px + 10, py + 60, 80, 50);
        x.line_width(0);
        x.color(x.rgb(120, 220, 120));
        x.fill_ellipse(px + 110, py + 60, 70, 50);
        x.line(px + 150, py + 10, px + 190, py + 50);

        let (px, py) = panel(&mut x, 1, 0, "batches, polygons");
        let bars: Vec<[i16; 4]> = (0..12)
            .map(|i| {
                [
                    (px + 8 + i * 15) as i16,
                    (py + 60 - (i * 37 + t) % 50) as i16,
                    10,
                    ((i * 37 + t) % 50) as i16,
                ]
            })
            .collect();
        x.color(x.rgb(250, 120, 160));
        x.fill_rects(&bars);
        x.color(x.rgb(200, 200, 255));
        let segs: Vec<[i16; 4]> = (0..10)
            .map(|i| {
                [
                    (px + 8) as i16,
                    (py + 70 + i * 5) as i16,
                    (px + 90) as i16,
                    (py + 120 - i * 5) as i16,
                ]
            })
            .collect();
        x.segments(&segs);
        x.color(x.rgb(255, 220, 90));
        x.fill_poly(&[
            [(px + 140) as i16, (py + 70) as i16],
            [(px + 185) as i16, (py + 125) as i16],
            [(px + 100) as i16, (py + 125) as i16],
        ]);
        x.color(x.rgb(120, 255, 200));
        let zig: Vec<[i16; 2]> = (0..8)
            .map(|i| [(px + 100 + i * 12) as i16, (py + 62 + (i % 2) * 6) as i16])
            .collect();
        x.polyline(&zig);

        let (px, py) = panel(&mut x, 2, 0, "image (pixels)");
        x.image(px + 10, py + 6, 120, 100, &pixels, 120);

        let (px, py) = panel(&mut x, 3, 0, "fonts, unicode");
        x.text_font(big);
        x.color(x.rgb(255, 255, 255));
        let a = x.font_ascent(big);
        x.text(px + 8, py + 8 + a, "Helvetica");
        let w = x.text_width(big, "Helvetica");
        x.color(x.rgb(250, 180, 60));
        x.fill_rect(px + 8, py + 12 + a, w, 2);
        x.text_font(cell);
        x.color(x.rgb(140, 220, 255));
        let ca = x.font_ascent(cell);
        x.text_unicode(px + 8, py + 50 + ca, "λ → ∑ ░▒▓ ★");
        x.text_font(Font::FIXED);
        x.color(x.rgb(160, 168, 186));
        x.text(px + 8, py + 110, "café (Latin-1)");

        let (px, py) = panel(&mut x, 0, 1, "masked blit, stencil");
        x.blit(sprite, 0, 0, 48, 48, px + 10, py + 10); // square: no mask
        x.blit_masked(sprite, mask, 0, 0, 48, 48, px + 70, py + 10);
        for i in 0..4 {
            x.blit_stencil(
                stencil,
                sprite,
                0,
                0,
                48,
                48,
                px + 10 + i * 40 + (t % 20),
                py + 70,
            );
        }

        let (px, py) = panel(&mut x, 1, 1, "stipple tint");
        x.color(x.rgb(90, 200, 250));
        x.fill_rect(px + 20, py + 10, 160, 100);
        x.color(x.rgb(255, 40, 40));
        x.fill_stippled(dither, px + 60, py + 30, 100, 60);

        let (px, py) = panel(&mut x, 2, 1, "tiled polygon");
        let wave: Vec<[i16; 2]> = (0..=18)
            .map(|i| {
                [
                    (px + 10 + i * 10) as i16,
                    (py + 60 + (((i * 30 + t * 4) as f64).to_radians().sin() * 30.0) as i32) as i16,
                ]
            })
            .chain([
                [(px + 190) as i16, (py + 125) as i16],
                [(px + 10) as i16, (py + 125) as i16],
            ])
            .collect();
        x.fill_poly_tiled(tile, px, py - 20, &wave);

        let (px, py) = panel(&mut x, 3, 1, "frame copy");
        // copy the shapes panel out of the frame drawn so far, at half size
        let frame = x.frame();
        x.blit(frame, 10, 30, 180, 120, px + 10, py + 4);
        x.color(x.rgb(160, 168, 186));
        x.text(px + 10, py + 122, &format!("{} KB sent", x.sent() / 1024));

        x.present();
        x.fence(); // this frame is on its way
        t += 1;
        x.wait(40);
    }
}
