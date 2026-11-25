//! gallery: one panel per drawing feature, as a visual check of the whole
//! API: shapes, batches, polygons, images, canvases, masks, stencils,
//! see-through tints, tiled fills and fonts, with fence-paced frames.
//!
//! Everything is drawn smooth (anti-aliased text and shapes) where the
//! server has RENDER, and with the plain core calls where it doesn't, so
//! both paths are here side by side.
//!
//! Run:  DISPLAY=HOST:0 cargo run --example gallery
//! Keys: q or Escape quits.

use x11_direct::{key, polyline_triangles, rounded_points, Event, Font, X11};

const CW: i32 = 200; // panel size
const CH: i32 = 150;
const PANEL: (u8, u8, u8) = (36, 40, 52);

/// TrueType fonts to try: (file, face) for regular and bold.
const FONTS: [[(&str, &str); 2]; 2] = [
    [
        ("/System/Library/Fonts/HelveticaNeue.ttc", "Helvetica Neue"),
        (
            "/System/Library/Fonts/HelveticaNeue.ttc",
            "Helvetica Neue Bold",
        ),
    ],
    [
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "DejaVu Sans",
        ),
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "DejaVu Sans Bold",
        ),
    ],
];

struct Fonts {
    small: Font,
    body: Font,
    big: Font,
    /// a fixed-width Unicode font, for the core-font path
    cell: Font,
    /// the outline face's name, or None with the core fonts
    face: Option<&'static str>,
}

fn fonts(x: &mut X11) -> Fonts {
    for [(reg, reg_face), (bold, bold_face)] in FONTS {
        let (Ok(r), Ok(b)) = (std::fs::read(reg), std::fs::read(bold)) else {
            continue;
        };
        let small = x.font_ttf(&r, reg_face, 12.0);
        if small == Font::FIXED {
            break; // no RENDER: the core fonts, below
        }
        return Fonts {
            small,
            body: x.font_ttf(&r, reg_face, 15.0),
            big: x.font_ttf(&b, bold_face, 28.0),
            cell: Font::FIXED,
            face: Some(reg_face),
        };
    }
    Fonts {
        small: Font::FIXED,
        body: Font::FIXED,
        big: x.font("-*-helvetica-bold-r-normal--24-*-*-*-p-*-iso8859-1"),
        cell: x.font_cell("-misc-fixed-medium-r-normal--18-*-*-*-c-*-iso10646-1"),
        face: None,
    }
}

fn panel(x: &mut X11, f: &Fonts, col: i32, row: i32, title: &str) -> (i32, i32) {
    let (px, py) = (10 + col * (CW + 10), 10 + row * (CH + 10));
    x.color(x.rgb(PANEL.0, PANEL.1, PANEL.2));
    let outline = rounded_points(px as f32, py as f32, CW as f32, CH as f32, 8.0);
    if !x.aa_polygon(&outline, 255) {
        x.fill_rect(px, py, CW, CH);
    }
    x.color(x.rgb(160, 168, 186));
    x.text_font(f.small);
    let base = py + 6 + x.font_ascent(f.small);
    x.text(px + 8, base, title);
    (px, py + 20)
}

/// A sprite: a gradient ball, its edge blended onto the panel colour so it
/// looks smooth wherever it's copied onto that panel.
fn ball_pixels(x: &X11, size: i32) -> Vec<u32> {
    let r = size as f32 / 2.0;
    let mut px = Vec::with_capacity((size * size) as usize);
    for j in 0..size {
        for i in 0..size {
            let d = (i as f32 + 0.5 - r).hypot(j as f32 + 0.5 - r) - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            let c = [255.0, (j * 6) as f32, 40.0];
            let bg = [PANEL.0 as f32, PANEL.1 as f32, PANEL.2 as f32];
            let m = |k: usize| (c[k] * a + bg[k] * (1.0 - a)).round() as u8;
            px.push(x.rgb(m(0), m(1), m(2)));
        }
    }
    px
}

fn main() {
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    x.background(x.rgb(20, 22, 30));
    x.window("gallery", 4 * (CW + 10) + 10, 2 * (CH + 10) + 10);
    let f = fonts(&mut x);

    // made once, used every frame: a square sprite, a round one with a
    // mask and its stencil, a 4x4 dither, and a vertical gradient tile
    const S: i32 = 40;
    let square = x.canvas(S, S);
    x.target(Some(square));
    for i in 0..S {
        x.color(x.rgb(255, (i * 6) as u8, 40));
        x.fill_rect(0, i, S, 1);
    }
    let ball = x.canvas(S, S);
    let ball_px = ball_pixels(&x, S);
    x.target(Some(ball));
    x.image(0, 0, S, S, &ball_px, S as usize);
    x.target(None);
    // the mask covers every pixel the ball touches, edge included
    let mask = x.mask(S, S);
    let rows: Vec<[i16; 4]> = (0..S)
        .map(|y| {
            let (r, dy) = (S as f64 / 2.0 + 0.5, y as f64 + 0.5 - S as f64 / 2.0);
            let half = (r * r - dy * dy).max(0.0).sqrt();
            let x0 = (S as f64 / 2.0 - half).floor().max(0.0) as i16;
            [x0, y as i16, (S as i16 - 2 * x0).max(0), 1]
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
        x.color(x.rgb(20, 22, 30));
        x.fill_rect(0, 0, x.width(), x.height());

        let (px, py) = panel(&mut x, &f, 0, 0, "shapes");
        let (fx, fy) = (px as f32, py as f32);
        x.color(x.rgb(90, 200, 250));
        x.fill_rect(px + 10, py + 10, 60, 40);
        x.color(x.rgb(250, 180, 60));
        x.line_width(3);
        x.rect(px + 80, py + 10, 60, 40);
        if !x.aa_ellipse(fx + 10.0, fy + 60.0, 80.0, 50.0, 3.0, 255) {
            x.ellipse(px + 10, py + 60, 80, 50);
        }
        x.line_width(0);
        x.color(x.rgb(120, 220, 120));
        if !x.aa_fill_ellipse(fx + 110.0, fy + 60.0, 70.0, 50.0, 255) {
            x.fill_ellipse(px + 110, py + 60, 70, 50);
        }
        if !x.aa_polyline(
            &[[fx + 150.0, fy + 10.0], [fx + 190.0, fy + 50.0]],
            1.5,
            255,
        ) {
            x.line(px + 150, py + 10, px + 190, py + 50);
        }

        let (px, py) = panel(&mut x, &f, 1, 0, "batches, polygons");
        let (fx, fy) = (px as f32, py as f32);
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
        // many thin lines in one request: triangles, all at once
        x.color(x.rgb(200, 200, 255));
        let fan: Vec<[f32; 6]> = (0..10)
            .flat_map(|i| {
                let i = i as f32;
                let a = [fx + 8.0, fy + 70.0 + i * 5.0];
                let b = [fx + 90.0, fy + 120.0 - i * 5.0];
                polyline_triangles(&[a, b], 1.0)
            })
            .collect();
        if !x.aa_triangles(&fan, 255) {
            let segs: Vec<[i16; 4]> = (0..10)
                .map(|i| {
                    let (y0, y1) = (py + 70 + i * 5, py + 120 - i * 5);
                    [(px + 8) as i16, y0 as i16, (px + 90) as i16, y1 as i16]
                })
                .collect();
            x.segments(&segs);
        }
        x.color(x.rgb(255, 220, 90));
        let tri = [
            [fx + 140.0, fy + 70.0],
            [fx + 185.0, fy + 125.0],
            [fx + 100.0, fy + 125.0],
        ];
        if !x.aa_polygon(&tri, 255) {
            x.fill_poly(&tri.map(|p| [p[0] as i16, p[1] as i16]));
        }
        x.color(x.rgb(120, 255, 200));
        let zig: Vec<[f32; 2]> = (0..8)
            .map(|i| {
                [
                    fx + 100.0 + i as f32 * 12.0,
                    fy + 62.0 + (i % 2) as f32 * 6.0,
                ]
            })
            .collect();
        if !x.aa_polyline(&zig, 1.5, 255) {
            let zig: Vec<[i16; 2]> = zig.iter().map(|p| [p[0] as i16, p[1] as i16]).collect();
            x.polyline(&zig);
        }

        let (px, py) = panel(&mut x, &f, 2, 0, "image (pixels)");
        x.image(px + 10, py + 6, 120, 100, &pixels, 120);

        let (px, py) = panel(&mut x, &f, 3, 0, "fonts");
        x.text_font(f.big);
        x.color(x.rgb(255, 255, 255));
        let a = x.font_ascent(f.big);
        let name = f.face.unwrap_or("Helvetica");
        let shown = name.split(' ').next().unwrap_or(name); // the family
        x.text(px + 8, py + 6 + a, shown);
        let w = x.text_width(f.big, shown);
        x.color(x.rgb(250, 180, 60));
        x.fill_rect(px + 8, py + 12 + a, w, 2);
        x.color(x.rgb(140, 220, 255));
        if f.face.is_some() {
            x.text_font(f.body);
            let ba = x.font_ascent(f.body);
            x.text(px + 8, py + 54 + ba, "Anti-aliased, 15 px");
            x.text(px + 8, py + 76 + ba, "café, naïve, ½ ¾ © ±");
        } else {
            x.text_font(f.cell);
            let ca = x.font_ascent(f.cell);
            x.text_unicode(px + 8, py + 50 + ca, "λ → ∑ ░▒▓ ★");
        }
        x.text_font(f.small);
        x.color(x.rgb(160, 168, 186));
        x.text(px + 8, py + 120, name);

        let (px, py) = panel(&mut x, &f, 0, 1, "masked blit, stencil");
        x.blit(square, 0, 0, S, S, px + 10, py + 10); // square: no mask
        x.blit_masked(ball, mask, 0, 0, S, S, px + 70, py + 10);
        for i in 0..4 {
            let dx = px + 10 + i * 44 + t % 14;
            x.blit_stencil(stencil, ball, 0, 0, S, S, dx, py + 76);
        }

        let (px, py) = panel(&mut x, &f, 1, 1, "see-through tint");
        x.color(x.rgb(90, 200, 250));
        x.fill_rect(px + 20, py + 10, 160, 100);
        x.color(x.rgb(255, 40, 40));
        if !x.blend_rect(px + 60, py + 30, 100, 60, 150) {
            x.fill_stippled(dither, px + 60, py + 30, 100, 60);
        }

        let (px, py) = panel(&mut x, &f, 2, 1, "tiled polygon");
        let wave: Vec<[f32; 2]> = (0..=36)
            .map(|i| {
                let s = ((i * 15 + t * 4) as f32).to_radians().sin();
                [(px + 10 + i * 5) as f32, (py + 60) as f32 + s * 30.0]
            })
            .collect();
        let poly: Vec<[i16; 2]> = (wave.iter().map(|p| [p[0] as i16, p[1] as i16]))
            .chain([
                [(px + 190) as i16, (py + 120) as i16],
                [(px + 10) as i16, (py + 120) as i16],
            ])
            .collect();
        x.fill_poly_tiled(tile, px, py - 20, &poly);
        // a smooth line along the top hides the fill's stepped edge
        x.color(x.rgb(110, 220, 230));
        x.aa_polyline(&wave, 2.0, 255);

        let sent = format!("frame copy, {} KB sent", x.sent() / 1024);
        let (px, py) = panel(&mut x, &f, 3, 1, &sent);
        // copy the shapes panel out of the frame drawn so far
        let frame = x.frame();
        x.blit(frame, 10, 30, 180, 120, px + 10, py + 2);

        x.present();
        x.fence(); // this frame is on its way
        t += 1;
        x.wait(40);
    }
}
