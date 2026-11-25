//! polish: a small form that looks like a desktop app rather than a 1990s
//! X client. Start from this one for anything a person will look at.
//!
//! Run:   DISPLAY=HOST:0 cargo run --example polish
//! Mouse: type in the field, Copy, the Size dropdown, Clear... asks first
//! Keys:  Ctrl+C / Ctrl+V copy and paste the field, Ctrl+D light or dark,
//!        Escape closes or quits
//!
//! What it shows (see "Looking like a real app" in reference/patterns.md):
//!   - anti-aliased text from a TrueType font on disk (font_ttf), with the
//!     core Helvetica as the fallback, and every height from font metrics;
//!   - smooth rounded buttons and fields (aa_polygon + rounded_points);
//!   - a dropdown instead of a row of buttons: it floats with a soft
//!     shadow, saves what it covers, highlights on hover, ticks the current
//!     choice, and works from the keyboard;
//!   - a destructive action asks first, over a dimmed window (blend_rect);
//!   - the pointer is a hand over things to click and an I-beam over text;
//!   - the clipboard: Copy, Ctrl+C and Ctrl+V;
//!   - dark mode: every colour a (light, dark) pair, chosen by --dark or
//!     the macOS setting, and Ctrl+D switches;
//!   - a smooth graph line with a translucent fill, batched into one
//!     request (polyline_triangles / polygon_triangles);
//!   - each RENDER call returns false without RENDER (ssh -X), and the
//!     form falls back to square, solid shapes.

use x11_direct::{
    key, polygon_triangles, polyline_triangles, rounded_points, Canvas, Cursor, Event, Font,
    MOD_CTRL, X11,
};

/// TrueType fonts to try, by file and face (a .ttc holds several faces).
const FONTS: [(&str, &str); 3] = [
    ("/System/Library/Fonts/HelveticaNeue.ttc", "Helvetica Neue"),
    (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "DejaVu Sans",
    ),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "Liberation Sans",
    ),
];
const SIZES: [&str; 3] = ["Small", "Medium", "Large"];

#[derive(Clone, Copy, PartialEq)]
struct R {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl R {
    fn has(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

struct Colors {
    dark: bool,
    bg: u32,
    face: u32,
    edge: u32,
    text: u32,
    dim: u32,
    accent: u32,
    on_accent: u32,
    danger: u32,
    black: u32,
}

/// A popup and what it covers (save-under): put back with one copy.
struct Saved {
    canvas: Canvas,
    at: R,
}

struct App {
    c: Colors,
    font: Font,
    field: String,
    size: usize,
    status: String,
    menu: Option<(Saved, Option<usize>)>,
    dialog: Option<Saved>,
    data: Vec<f32>,
}

// fixed layout, for brevity: a real app lays out from the window size
const FIELD: R = R {
    x: 20,
    y: 20,
    w: 280,
    h: 28,
};
const COPY: R = R {
    x: 310,
    y: 20,
    w: 80,
    h: 28,
};
const SIZE: R = R {
    x: 400,
    y: 20,
    w: 120,
    h: 28,
};
const CLEAR: R = R {
    x: 20,
    y: 60,
    w: 100,
    h: 28,
};
const GRAPH: R = R {
    x: 20,
    y: 104,
    w: 500,
    h: 150,
};
const MENU_ROW: i32 = 26;
const DIALOG: R = R {
    x: 120,
    y: 90,
    w: 300,
    h: 120,
};
const OK: R = R {
    x: 300,
    y: 166,
    w: 100,
    h: 28,
};
const CANCEL: R = R {
    x: 190,
    y: 166,
    w: 100,
    h: 28,
};

fn menu_rect() -> R {
    R {
        x: SIZE.x,
        y: SIZE.y + SIZE.h + 4,
        w: SIZE.w,
        h: 8 + MENU_ROW * SIZES.len() as i32,
    }
}

impl App {
    /// A rounded rectangle: smooth with RENDER, square without.
    fn rrect(&self, x: &mut X11, r: R, rad: f32, edge: u32, face: u32) {
        let (fx, fy, fw, fh) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        x.color(edge);
        if !x.aa_polygon(&rounded_points(fx, fy, fw, fh, rad), 255) {
            x.fill_rect(r.x, r.y, r.w, r.h);
            x.color(face);
            x.fill_rect(r.x + 1, r.y + 1, r.w - 2, r.h - 2);
            return;
        }
        x.color(face);
        x.aa_polygon(
            &rounded_points(fx + 1.0, fy + 1.0, fw - 2.0, fh - 2.0, rad - 1.0),
            255,
        );
    }

    /// A soft shadow under something floating: layers of faint black,
    /// each a little bigger, a few pixels down. Nothing without RENDER.
    fn shadow(&self, x: &mut X11, r: R, rad: f32) {
        x.color(self.c.black);
        for i in 0..6 {
            let g = 1.0 + i as f32 * 1.6;
            let (fx, fy) = (r.x as f32 - g, r.y as f32 - g + 3.0);
            let (fw, fh) = (r.w as f32 + 2.0 * g, r.h as f32 + 2.0 * g);
            // shadows need more weight to show on a dark ground
            let alpha = if self.c.dark { 16 } else { 8 };
            if !x.aa_polygon(&rounded_points(fx, fy, fw, fh, rad + g), alpha) {
                return;
            }
        }
    }

    /// Text centred in r, vertically by the font's own metrics.
    fn label(&self, x: &mut X11, r: R, s: &str, col: u32) {
        let f = self.font;
        let base = r.y + (r.h + x.font_ascent(f) - x.font_descent(f)) / 2;
        x.color(col);
        let left = r.x + (r.w - x.text_width(f, s)) / 2;
        x.text(left, base, s);
    }

    fn button(&self, x: &mut X11, r: R, s: &str) {
        self.rrect(x, r, 6.0, self.c.edge, self.c.face);
        self.label(x, r, s, self.c.text);
    }

    fn draw(&self, x: &mut X11) {
        let (w, h) = (x.width(), x.height());
        x.color(self.c.bg);
        x.fill_rect(0, 0, w, h);
        x.text_font(self.font);

        // the field: text, then a caret
        self.rrect(x, FIELD, 6.0, self.c.edge, self.c.face);
        let base = FIELD.y + (FIELD.h + x.font_ascent(self.font) - x.font_descent(self.font)) / 2;
        let (s, col) = match self.field.is_empty() {
            true => ("Type here", self.c.dim),
            false => (self.field.as_str(), self.c.text),
        };
        x.color(col);
        x.text(FIELD.x + 8, base, s);
        let cx = FIELD.x + 8 + x.text_width(self.font, &self.field);
        x.color(self.c.accent);
        x.fill_rect(cx, FIELD.y + 6, 1, FIELD.h - 12);

        self.button(x, COPY, "Copy");
        // a dropdown shows its current choice and a chevron
        self.rrect(x, SIZE, 6.0, self.c.edge, self.c.face);
        let lbl = R {
            w: SIZE.w - 18,
            ..SIZE
        };
        self.label(x, lbl, SIZES[self.size], self.c.text);
        let (vx, vy) = ((SIZE.x + SIZE.w - 18) as f32, (SIZE.y + SIZE.h / 2) as f32);
        x.color(self.c.dim);
        if !x.aa_polyline(
            &[[vx, vy - 2.0], [vx + 4.0, vy + 2.0], [vx + 8.0, vy - 2.0]],
            1.5,
            255,
        ) {
            x.polyline(&[
                [vx as i16, vy as i16 - 2],
                [vx as i16 + 4, vy as i16 + 2],
                [vx as i16 + 8, vy as i16 - 2],
            ]);
        }
        self.button(x, CLEAR, "Clear...");

        // the graph: one request for the fill, one for the line
        self.rrect(x, GRAPH, 8.0, self.c.edge, self.c.face);
        let n = self.data.len();
        let step = (GRAPH.w - 20) as f32 / (n - 1) as f32;
        let bottom = (GRAPH.y + GRAPH.h - 10) as f32;
        let pts: Vec<[f32; 2]> = (self.data.iter().enumerate())
            .map(|(i, v)| {
                [
                    GRAPH.x as f32 + 10.0 + i as f32 * step,
                    bottom - v * (GRAPH.h - 20) as f32,
                ]
            })
            .collect();
        let fill: Vec<[f32; 6]> = (pts.windows(2))
            .flat_map(|p| polygon_triangles(&[p[0], p[1], [p[1][0], bottom], [p[0][0], bottom]]))
            .collect();
        x.color(self.c.accent);
        if x.aa_triangles(&fill, 50) {
            x.aa_triangles(&polyline_triangles(&pts, 1.5), 255);
        } else {
            let ipts: Vec<[i16; 2]> = pts.iter().map(|p| [p[0] as i16, p[1] as i16]).collect();
            x.polyline(&ipts);
        }

        x.color(self.c.dim);
        let sb = h - 12 - x.font_descent(self.font);
        x.text(20, sb, &self.status);
    }

    /// Copy part of the frame into a canvas before covering it.
    fn save(x: &mut X11, at: R) -> Saved {
        let canvas = x.canvas(at.w, at.h);
        let frame = x.frame();
        x.target(Some(canvas));
        x.blit(frame, at.x, at.y, at.w, at.h, 0, 0);
        x.target(None);
        Saved { canvas, at }
    }

    fn restore(x: &mut X11, s: Saved) {
        x.blit(s.canvas, 0, 0, s.at.w, s.at.h, s.at.x, s.at.y);
        x.free_canvas(s.canvas);
    }

    fn open_menu(&mut self, x: &mut X11) {
        let r = menu_rect();
        // save the shadow's area too, so closing leaves nothing behind
        let saved = Self::save(
            x,
            R {
                x: r.x - 10,
                y: r.y - 8,
                w: r.w + 20,
                h: r.h + 22,
            },
        );
        self.shadow(x, r, 8.0);
        self.menu = Some((saved, Some(self.size)));
        self.draw_menu(x);
    }

    fn draw_menu(&self, x: &mut X11) {
        let Some((_, hl)) = &self.menu else { return };
        let r = menu_rect();
        self.rrect(x, r, 8.0, self.c.edge, self.c.face);
        for (i, s) in SIZES.iter().enumerate() {
            let row = R {
                x: r.x + 4,
                y: r.y + 4 + i as i32 * MENU_ROW,
                w: r.w - 8,
                h: MENU_ROW,
            };
            let on = *hl == Some(i);
            let (face, col) = if on {
                (self.c.accent, self.c.on_accent)
            } else {
                (self.c.face, self.c.text)
            };
            self.rrect(x, row, 5.0, face, face);
            // the current choice gets a tick
            if i == self.size {
                let (tx, ty) = ((row.x + 10) as f32, (row.y + row.h / 2) as f32);
                x.color(col);
                x.aa_polyline(
                    &[[tx - 4.0, ty], [tx - 1.0, ty + 3.0], [tx + 4.0, ty - 3.0]],
                    1.6,
                    255,
                );
            }
            let f = self.font;
            x.color(col);
            let base = row.y + (row.h + x.font_ascent(f) - x.font_descent(f)) / 2;
            x.text(row.x + 22, base, s);
        }
    }

    fn close_menu(&mut self, x: &mut X11) {
        if let Some((saved, _)) = self.menu.take() {
            Self::restore(x, saved);
        }
    }

    fn menu_row(&self, px: i32, py: i32) -> Option<usize> {
        let r = menu_rect();
        let i = (py - r.y - 4).div_euclid(MENU_ROW);
        (r.has(px, py) && (0..SIZES.len() as i32).contains(&i)).then_some(i as usize)
    }

    /// Ask before doing something that can't be undone: keep the whole
    /// window, dim it, and float the question over it.
    fn open_dialog(&mut self, x: &mut X11) {
        let (w, h) = (x.width(), x.height());
        self.dialog = Some(Self::save(x, R { x: 0, y: 0, w, h }));
        // without RENDER it stays undimmed (or use a stipple: patterns.md 9)
        x.color(self.c.black);
        x.blend_rect(0, 0, w, h, if self.c.dark { 110 } else { 70 });
        self.shadow(x, DIALOG, 10.0);
        self.rrect(x, DIALOG, 10.0, self.c.edge, self.c.face);
        x.color(self.c.text);
        x.text(DIALOG.x + 20, DIALOG.y + 34, "Clear the field?");
        x.color(self.c.dim);
        x.text(DIALOG.x + 20, DIALOG.y + 56, "This can't be undone.");
        self.button(x, CANCEL, "Cancel");
        // the default, destructive button is coloured to say so
        self.rrect(x, OK, 6.0, self.c.danger, self.c.danger);
        self.label(x, OK, "Clear", self.c.on_accent);
    }

    fn close_dialog(&mut self, x: &mut X11, clear: bool) {
        if let Some(saved) = self.dialog.take() {
            Self::restore(x, saved);
        }
        if clear {
            self.field.clear();
            self.status = "Cleared".into();
            self.draw(x);
        }
    }

    /// The pointer's shape says what a click will do.
    fn cursor_at(&self, px: i32, py: i32) -> Cursor {
        let hand = match (&self.dialog, &self.menu) {
            (Some(_), _) => OK.has(px, py) || CANCEL.has(px, py),
            (_, Some(_)) => self.menu_row(px, py).is_some(),
            _ if FIELD.has(px, py) => return Cursor::Text,
            _ => [COPY, SIZE, CLEAR].iter().any(|r| r.has(px, py)),
        };
        if hand {
            Cursor::Hand
        } else {
            Cursor::Default
        }
    }
}

/// The palette as (light, dark) pairs.
fn colors(x: &mut X11, dark: bool) -> Colors {
    let pick = |l: (u8, u8, u8), d: (u8, u8, u8)| {
        let (r, g, b) = if dark { d } else { l };
        x.rgb(r, g, b)
    };
    Colors {
        dark,
        bg: pick((236, 236, 236), (30, 30, 32)),
        face: pick((255, 255, 255), (52, 52, 56)),
        edge: pick((200, 200, 200), (78, 78, 84)),
        text: pick((30, 30, 30), (235, 235, 235)),
        dim: pick((130, 130, 130), (150, 150, 156)),
        accent: pick((10, 110, 230), (10, 132, 255)),
        on_accent: pick((255, 255, 255), (255, 255, 255)),
        danger: pick((215, 50, 45), (230, 70, 60)),
        black: pick((0, 0, 0), (0, 0, 0)),
    }
}

/// macOS says whether it's in dark mode; elsewhere, assume light.
fn system_dark() -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .is_ok_and(|o| o.stdout.starts_with(b"Dark"))
}

fn ui_font(x: &mut X11) -> Font {
    for (path, face) in FONTS {
        if let Ok(data) = std::fs::read(path) {
            let f = x.font_ttf(&data, face, 14.0);
            if f != Font::FIXED {
                return f;
            }
        }
    }
    x.font("-*-helvetica-medium-r-normal--14-*-*-*-p-*-iso8859-1")
}

fn main() {
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let dark = match std::env::args().nth(1).as_deref() {
        Some("--dark") => true,
        Some("--light") => false,
        _ => system_dark(),
    };
    let c = colors(&mut x, dark);
    x.background(c.bg);
    x.window("polish", 540, 300);
    x.min_size(540, 300);
    let font = ui_font(&mut x);
    let data = (0..40)
        .map(|i| 0.5 + 0.3 * (i as f32 * 0.35).sin() + 0.1 * (i as f32 * 1.3).cos())
        .collect();
    let status = if x.has_render() {
        "Smooth: RENDER is here"
    } else {
        "Plain: no RENDER (try ssh -Y)"
    };
    let mut app = App {
        c,
        font,
        field: String::new(),
        size: 1,
        status: status.into(),
        menu: None,
        dialog: None,
        data,
    };
    app.draw(&mut x);
    x.present();

    loop {
        let mut changed = false;
        while let Some(e) = x.event() {
            changed = true;
            match e {
                Event::Close => return,
                Event::Resize { .. } => {
                    // the relayout redraws everything: drop popups unrestored
                    if let Some((s, _)) = app.menu.take() {
                        x.free_canvas(s.canvas);
                    }
                    if let Some(s) = app.dialog.take() {
                        x.free_canvas(s.canvas);
                    }
                    app.draw(&mut x);
                }
                Event::Motion { x: px, y: py, .. } => {
                    let c = app.cursor_at(px, py);
                    x.cursor(c);
                    let now = app.menu_row(px, py);
                    if let Some((_, hl)) = &mut app.menu {
                        if *hl != now {
                            *hl = now;
                            app.draw_menu(&mut x);
                        }
                    }
                }
                Event::ButtonDown {
                    button: 1,
                    x: px,
                    y: py,
                    ..
                } => {
                    if app.dialog.is_some() {
                        if OK.has(px, py) || CANCEL.has(px, py) {
                            app.close_dialog(&mut x, OK.has(px, py));
                        }
                    } else if app.menu.is_some() {
                        let row = app.menu_row(px, py);
                        app.close_menu(&mut x);
                        if let Some(i) = row {
                            app.size = i;
                            app.status = format!("Size: {}", SIZES[i]);
                            app.draw(&mut x);
                        }
                    } else if COPY.has(px, py) {
                        x.set_clipboard(&app.field);
                        app.status = "Copied".into();
                        app.draw(&mut x);
                    } else if SIZE.has(px, py) {
                        app.open_menu(&mut x);
                    } else if CLEAR.has(px, py) {
                        app.open_dialog(&mut x);
                    }
                    let c = app.cursor_at(px, py);
                    x.cursor(c);
                }
                Event::Paste(s) => {
                    app.field.push_str(s.lines().next().unwrap_or(""));
                    app.status = "Pasted".into();
                    app.draw(&mut x);
                }
                Event::KeyDown { key: k, mods, .. } => {
                    let ctrl = mods & MOD_CTRL != 0;
                    if let Some((_, hl)) = &mut app.menu {
                        // menus work from the keyboard too
                        let n = SIZES.len();
                        match k {
                            key::UP => *hl = Some(hl.map_or(n - 1, |i| (i + n - 1) % n)),
                            key::DOWN => *hl = Some(hl.map_or(0, |i| (i + 1) % n)),
                            key::RETURN => {
                                let pick = *hl;
                                app.close_menu(&mut x);
                                if let Some(i) = pick {
                                    app.size = i;
                                    app.status = format!("Size: {}", SIZES[i]);
                                    app.draw(&mut x);
                                }
                                continue;
                            }
                            key::ESCAPE => {
                                app.close_menu(&mut x);
                                continue;
                            }
                            _ => {}
                        }
                        app.draw_menu(&mut x);
                    } else if app.dialog.is_some() {
                        match k {
                            key::RETURN => app.close_dialog(&mut x, true),
                            key::ESCAPE => app.close_dialog(&mut x, false),
                            _ => {}
                        }
                    } else if k == key::ESCAPE || (ctrl && k == 'q' as u32) {
                        return;
                    } else if ctrl && k == 'd' as u32 {
                        app.c = colors(&mut x, !app.c.dark);
                        app.draw(&mut x);
                    } else if ctrl && k == 'c' as u32 {
                        x.set_clipboard(&app.field);
                        app.status = "Copied".into();
                        app.draw(&mut x);
                    } else if ctrl && k == 'v' as u32 {
                        x.request_paste(); // the text comes back as Event::Paste
                    } else if k == key::BACKSPACE {
                        app.field.pop();
                        app.draw(&mut x);
                    } else if (32..256).contains(&k) && k != 127 && !ctrl {
                        app.field.push(char::from_u32(k).unwrap_or('?'));
                        app.draw(&mut x);
                    }
                }
                _ => changed = false,
            }
        }
        if changed {
            x.present();
            x.sync();
        }
        x.wait(-1);
    }
}
