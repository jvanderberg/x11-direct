//! menu: a scrolling list with a right-click menu, drawn incrementally:
//! the shape of a file manager or any list-based tool.
//!
//! Run:   DISPLAY=HOST:0 cargo run --example menu   (lines typed on stdin
//!        are added to the list)
//! Mouse: click selects, the wheel scrolls, the right button opens a menu
//! Keys:  up/down, PgUp/PgDn, Home/End, Return, q or Escape quits
//!
//! The patterns it shows (see reference/patterns.md):
//!   - each visible row remembers what it last showed (a key string), and a
//!     row is sent again only when its key changes;
//!   - scrolling moves the rows that stay with one copy of the frame onto
//!     itself (blit from frame()), and draws only the rows that came in;
//!   - the popup menu saves what it covers in a canvas and puts it back
//!     with one copy when it closes, so nothing under it is drawn again;
//!   - an event-driven loop that sleeps on the X connection and another
//!     file (stdin) together, with wait_with().

use std::io::Read;
use x11_direct::{key, Canvas, Event, Font, X11};

const PAD: i32 = 8;
const MENU: [&str; 4] = ["Open", "Rename", "Duplicate", "Delete"];

struct Colors {
    bg: u32,
    row: u32,
    row2: u32,
    sel: u32,
    text: u32,
    sel_text: u32,
    dim: u32,
    menu: u32,
    border: u32,
}

struct Popup {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hl: Option<usize>,
    save: Canvas, // what the menu covers
}

struct App {
    c: Colors,
    font: Font,
    items: Vec<String>,
    top: i32,
    cur: i32,
    last_top: Option<i32>,
    row_h: i32,
    list_y: i32,
    rows: i32,
    status_y: i32,
    rowkey: Vec<String>, // what each visible row shows now
    status: String,
    status_shown: Option<String>,
    menu: Option<Popup>,
}

impl App {
    fn n(&self) -> i32 {
        self.items.len() as i32
    }

    fn layout(&mut self, x: &mut X11) {
        self.row_h = x.font_ascent(self.font) + x.font_descent(self.font) + 6;
        self.list_y = PAD;
        self.status_y = x.height() - self.row_h - PAD / 2;
        self.rows = ((self.status_y - self.list_y - PAD) / self.row_h).max(1);
        let (w, h) = (x.width(), x.height());
        x.color(self.c.bg);
        x.fill_rect(0, 0, w, h);
        self.rowkey = vec![String::new(); self.rows as usize]; // every row drawn again
        self.last_top = Some(self.top);
        self.status_shown = None;
    }

    fn keep_visible(&mut self) {
        self.cur = self.cur.clamp(0, (self.n() - 1).max(0));
        if self.cur < self.top {
            self.top = self.cur;
        }
        if self.cur >= self.top + self.rows {
            self.top = self.cur - self.rows + 1;
        }
        self.top = self.top.min(self.n() - self.rows).max(0);
    }

    fn draw_list(&mut self, x: &mut X11) {
        let (w, rows, rh, ly) = (x.width() - 2 * PAD, self.rows, self.row_h, self.list_y);
        // Scrolled: move the rows still shown with one copy, and shift their
        // keys to match, so only the rows that came in are drawn below.
        match self.last_top {
            Some(last) if self.top != last && (self.top - last).abs() < rows => {
                let d = self.top - last;
                let keep = rows - d.abs();
                let frame = x.frame();
                if d > 0 {
                    x.blit(frame, PAD, ly + d * rh, w, keep * rh, PAD, ly);
                    self.rowkey.rotate_left(d as usize);
                    self.rowkey[keep as usize..]
                        .iter_mut()
                        .for_each(String::clear);
                } else {
                    x.blit(frame, PAD, ly, w, keep * rh, PAD, ly - d * rh);
                    self.rowkey.rotate_right(-d as usize);
                    self.rowkey[..-d as usize]
                        .iter_mut()
                        .for_each(String::clear);
                }
            }
            Some(last) if self.top != last => self.rowkey.iter_mut().for_each(String::clear),
            _ => {}
        }
        self.last_top = Some(self.top);

        x.text_font(self.font);
        let a = x.font_ascent(self.font);
        let num_w = x.text_width(self.font, "0000");
        for r in 0..rows {
            let (i, y) = (self.top + r, ly + r * rh);
            let item = self.items.get(i as usize);
            let key = format!("{i}|{}|{}", i == self.cur, item.map_or("", |s| s.as_str()));
            if key == self.rowkey[r as usize] {
                continue; // unchanged: send nothing
            }
            let sel = i == self.cur;
            x.color(if sel {
                self.c.sel
            } else if i % 2 == 1 {
                self.c.row2
            } else {
                self.c.row
            });
            x.fill_rect(PAD, y, w, rh);
            if let Some(item) = item {
                x.color(if sel { self.c.sel_text } else { self.c.dim });
                x.text(PAD + 6, y + 3 + a, &format!("{:4}", i + 1));
                x.color(if sel { self.c.sel_text } else { self.c.text });
                x.text(PAD + 12 + num_w, y + 3 + a, item);
            }
            self.rowkey[r as usize] = key;
        }
        if self.status_shown.as_ref() != Some(&self.status) {
            let (w, h) = (x.width(), x.height());
            x.color(self.c.bg);
            x.fill_rect(0, self.status_y, w, h - self.status_y);
            x.color(self.c.dim);
            x.text(PAD, self.status_y + 3 + a, &self.status);
            self.status_shown = Some(self.status.clone());
        }
    }

    fn menu_draw(&self, x: &mut X11) {
        let Some(m) = &self.menu else { return };
        x.color(self.c.border);
        x.fill_rect(m.x, m.y, m.w, m.h);
        let a = x.font_ascent(self.font);
        for (k, label) in MENU.iter().enumerate() {
            let y = m.y + 1 + k as i32 * self.row_h;
            let hl = m.hl == Some(k);
            x.color(if hl { self.c.sel } else { self.c.menu });
            x.fill_rect(m.x + 1, y, m.w - 2, self.row_h);
            x.color(if hl { self.c.sel_text } else { self.c.text });
            x.text(m.x + 10, y + 3 + a, label);
        }
    }

    fn menu_open(&mut self, x: &mut X11, px: i32, py: i32) {
        let w = MENU
            .iter()
            .map(|s| x.text_width(self.font, s))
            .max()
            .unwrap_or(0)
            + 20;
        let h = MENU.len() as i32 * self.row_h + 2;
        let (mx, my) = (px.min(x.width() - w), py.min(x.height() - h));
        // save what the menu will cover
        let save = x.canvas(w, h);
        let frame = x.frame();
        x.target(Some(save));
        x.blit(frame, mx, my, w, h, 0, 0);
        x.target(None);
        self.menu = Some(Popup {
            x: mx,
            y: my,
            w,
            h,
            hl: None,
            save,
        });
        self.menu_draw(x);
    }

    /// restore: put back what it covered (not after a resize: that is redrawn)
    fn menu_close(&mut self, x: &mut X11, restore: bool) {
        if let Some(m) = self.menu.take() {
            if restore {
                x.blit(m.save, 0, 0, m.w, m.h, m.x, m.y);
            }
            x.free_canvas(m.save);
        }
    }

    fn menu_hit(&self, px: i32, py: i32) -> Option<usize> {
        let m = self.menu.as_ref()?;
        if px < m.x || px >= m.x + m.w || py < m.y + 1 || py >= m.y + m.h - 1 {
            return None;
        }
        Some(((py - m.y - 1) / self.row_h) as usize)
    }

    /// Returns true to quit.
    fn on_event(&mut self, x: &mut X11, e: Event) -> bool {
        match e {
            Event::Close => return true,
            Event::Resize { .. } => {
                self.menu_close(x, false);
                self.layout(x);
                self.keep_visible();
            }
            Event::Motion { x: px, y: py, .. } if self.menu.is_some() => {
                let hit = self.menu_hit(px, py);
                if let Some(m) = self.menu.as_mut().filter(|m| m.hl != hit) {
                    m.hl = hit;
                    self.menu_draw(x);
                }
            }
            Event::ButtonDown { x: px, y: py, .. } if self.menu.is_some() => {
                let hit = self.menu_hit(px, py);
                self.menu_close(x, true);
                if let (Some(k), Some(item)) = (hit, self.items.get(self.cur as usize)) {
                    self.status = format!("{}: {item}", MENU[k]);
                }
            }
            Event::ButtonDown {
                button: b @ (4 | 5),
                ..
            } => {
                self.top = (self.top + if b == 4 { -3 } else { 3 })
                    .min(self.n() - self.rows)
                    .max(0);
            }
            Event::ButtonDown {
                button,
                x: px,
                y: py,
                ..
            } => {
                let r = (py - self.list_y) / self.row_h;
                if py >= self.list_y && r < self.rows && self.top + r < self.n() {
                    self.cur = self.top + r;
                    if button == 3 {
                        self.draw_list(x); // show the selection under the menu
                        self.menu_open(x, px, py);
                    }
                }
            }
            Event::KeyDown { key: k, .. } if self.menu.is_some() => {
                if k == key::ESCAPE {
                    self.menu_close(x, true);
                }
            }
            Event::KeyDown { key: k, .. } => {
                let page = self.rows - 1;
                match k {
                    k if k == 'q' as u32 || k == key::ESCAPE => return true,
                    key::UP => self.cur -= 1,
                    key::DOWN => self.cur += 1,
                    key::PAGE_UP => self.cur -= page,
                    key::PAGE_DOWN => self.cur += page,
                    key::HOME => self.cur = 0,
                    key::END => self.cur = self.n() - 1,
                    key::RETURN => {
                        if let Some(item) = self.items.get(self.cur as usize) {
                            self.status = format!("Open: {item}");
                        }
                    }
                    _ => {}
                }
                self.keep_visible();
            }
            _ => {}
        }
        false
    }
}

fn main() {
    const WORDS: [&str; 12] = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima",
    ];
    let items = (0..120)
        .map(|i| format!("{}-{}.txt", WORDS[i % 12], WORDS[(i * 7 + 3) % 12]))
        .collect();
    let mut x = X11::open(None).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let c = Colors {
        bg: x.rgb(245, 246, 248),
        row: x.rgb(255, 255, 255),
        row2: x.rgb(247, 248, 251),
        sel: x.rgb(52, 120, 246),
        text: x.rgb(30, 32, 38),
        sel_text: x.rgb(255, 255, 255),
        dim: x.rgb(120, 126, 138),
        menu: x.rgb(252, 252, 253),
        border: x.rgb(170, 176, 188),
    };
    let font = x.font("-*-helvetica-medium-r-normal--14-*-*-*-p-*-iso8859-1");
    x.background(c.bg);
    x.window("menu", 360, 420);
    x.min_size(200, 120);
    let mut app = App {
        c,
        font,
        items,
        top: 0,
        cur: 0,
        last_top: None,
        row_h: 0,
        list_y: 0,
        rows: 1,
        status_y: 0,
        rowkey: Vec::new(),
        status: "right-click an item".into(),
        status_shown: None,
        menu: None,
    };
    app.layout(&mut x);

    let (mut stdin_open, mut partial) = (true, Vec::new());
    loop {
        while let Some(e) = x.event() {
            if app.on_event(&mut x, e) {
                return;
            }
        }
        app.draw_list(&mut x);
        app.menu_draw(&mut x);
        x.present();
        // sleep until the X server or stdin has something
        let fds: &[i32] = if stdin_open { &[0] } else { &[] };
        if x.wait_with(fds, -1).first() == Some(&true) {
            // as big as Stdin's own buffer, so read() bypasses it: data left
            // in that buffer would wait unseen by wait_with()
            let mut buf = [0u8; 8192];
            match std::io::stdin().read(&mut buf) {
                Ok(0) | Err(_) => stdin_open = false, // end of input: stop watching it
                Ok(n) => {
                    partial.extend_from_slice(&buf[..n]);
                    while let Some(nl) = partial.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = partial.drain(..=nl).collect();
                        let s = String::from_utf8_lossy(&line[..nl]).trim().to_string();
                        if !s.is_empty() {
                            app.items.push(s);
                        }
                    }
                }
            }
        }
    }
}
