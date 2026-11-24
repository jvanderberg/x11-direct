// SPDX-License-Identifier: MIT
//! x11-direct: a tiny X11 client in one file, with no dependencies.
//!
//! It speaks the X11 protocol directly over a socket, so there is no Xlib,
//! xcb or C library to build or link: only `std`. It suits small machines
//! (an embedded Linux board with a network link, a minimal container) as
//! well as big ones. The program is the client; the screen belongs to an X
//! server, here or elsewhere, such as XQuartz on a Mac.
//!
//! Use it as a crate, or copy this file into a project as `src/x11.rs` and
//! write `mod x11;`.
//!
//! ```no_run
//! use x11_direct::{Event, X11, key};
//!
//! let mut x = X11::open(None).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
//! x.window("demo", 320, 200);
//! loop {
//!     while let Some(e) = x.event() {                 // never blocks
//!         match e {
//!             Event::Close => return,
//!             Event::KeyDown { key, .. } if key == 'q' as u32 || key == key::ESCAPE => return,
//!             _ => {}
//!         }
//!     }
//!     let (w, h) = (x.width(), x.height());
//!     x.color(x.rgb(0, 0, 80));
//!     x.fill_rect(0, 0, w, h);
//!     x.color(x.rgb(255, 255, 255));
//!     x.text(10, 20, "hello");
//!     x.present();                                    // show the frame
//!     x.wait(33);                                     // until input arrives, or 33 ms
//! }
//! ```
//!
//! Drawing goes to a back buffer the size of the window and appears only at
//! [`X11::present`], so frames never flicker; when the window is uncovered
//! the back buffer is copied again (in [`X11::event`], between frames)
//! without asking the program. When the window is resized it keeps its
//! picture, and new parts show the background ([`X11::background`]) until
//! the program draws again.
//!
//! | area | calls |
//! |---|---|
//! | colours | `rgb(r, g, b)` makes a pixel value; `color()` sets it for the drawing calls; `line_width()` |
//! | shapes | `fill_rect`, `rect`, `line`, `fill_ellipse`, `ellipse` (ellipses by bounding box) |
//! | text | `text(x, y, s)`: y is the baseline, in the server's "fixed" font (usually 6x13) unless `font()` found another by XLFD name and `text_font()` chose it. `text_width`, `font_ascent`, `font_descent` measure. `font_cell` opens a fixed-width font cheaply, even a big Unicode one, and `text_unicode` draws any `char` up to U+FFFF in it |
//! | batches | `fill_rects`, `segments`: many in one request; `fill_poly` (convex), `polyline` |
//! | images | `image(x, y, w, h, pixels, stride)`: pixels made by `rgb`. Costly over a slow link: 256 KB for a 320x200 frame, where shapes cost a few bytes each |
//! | canvases | `canvas(w, h)` makes an off-screen image; `target(Some(c))` sends the drawing calls there (`None`: back to the frame); `blit(c, ...)` copies part of it. Draw something once, then reuse it for 28 bytes a frame. `frame()` is the frame itself, to copy from: to save what a menu covers, or to scroll part of the frame |
//! | masks | `mask(w, h)` makes a 1-bit image, all clear; `mask_rects` sets rectangles of it. `blit_masked` copies only the pixels a mask has set. For a mask used again and again, make a `stencil` of it once and copy with `blit_stencil`. `fill_stippled` fills where a small repeated mask is set (a dither, for a see-through tint); `fill_poly_tiled` fills a polygon with a canvas repeated (a gradient under a graph). `target_window()` draws straight onto the window, over what `present` last showed |
//! | input | `event()` returns key down and up (a real release, unlike a terminal; a held key repeats as up/down pairs), mouse buttons and motion, resize (draw again) and close. `key` is ASCII (Latin-1) for printable keys, else a [`key`] constant; `mods` holds [`MOD_SHIFT`], [`MOD_CTRL`], [`MOD_ALT`]. `pointer()` asks where the mouse is, even outside the window |
//! | window | `background(pixel)` before `window()`, for what the window shows where nothing is drawn yet; `min_size(w, h)` after it |
//! | timing | `sync()` waits until the server has drawn everything sent (call it after `present()` so a slow link can't queue up old frames); `fence()` after `present()` and `fence_wait()` before the next one instead, to draw the next frame while the last one travels; `wait(ms)`, `wait_with(fds, ms)` (also wakes for other files), `flush()`, `fd()`, `pending()` |
//!
//! Needs a TrueColor display (any current server, XQuartz and Xvfb
//! included) that lets this machine connect: on a remote machine with the
//! screen, allow network clients (XQuartz: Settings > Security) and run
//! `xhost +THIS-ADDRESS`. Xauthority cookies are not supported, so a local
//! server must allow the connection too (`xhost +local:`).
//!
//! Drawing calls never fail: if the connection is lost, they do nothing and
//! the next [`X11::event`] returns [`Event::Close`].
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

/// X keysyms for keys that aren't printable characters.
pub mod key {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const INSERT: u32 = 0xff63;
    pub const KP_ENTER: u32 = 0xff8d;
    /// F1; F2 to F12 follow on (`F1 + n - 1`).
    pub const F1: u32 = 0xffbe;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const DELETE: u32 = 0xffff;
    /// Shift-Tab (ISO_Left_Tab).
    pub const BACKTAB: u32 = 0xfe20;
}

/// `mods` bits: the modifier keys held.
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CTRL: u32 = 4;
/// Mod1: Alt, or Option on a Mac.
pub const MOD_ALT: u32 = 8;

/// What [`X11::event`] returns. Coordinates are relative to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    KeyDown {
        key: u32,
        x: i32,
        y: i32,
        mods: u32,
    },
    KeyUp {
        key: u32,
        x: i32,
        y: i32,
        mods: u32,
    },
    /// Buttons: 1 left, 2 middle, 3 right, 4 and 5 the wheel (up, down).
    ButtonDown {
        button: u32,
        x: i32,
        y: i32,
        mods: u32,
    },
    ButtonUp {
        button: u32,
        x: i32,
        y: i32,
        mods: u32,
    },
    Motion {
        x: i32,
        y: i32,
        mods: u32,
    },
    /// The window has a new size: draw again.
    Resize {
        w: i32,
        h: i32,
    },
    /// The close button, or the connection was lost.
    Close,
}

/// An off-screen image on the server (a pixmap), or the frame itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Canvas(u32);

/// A 1-bit image: which pixels of a canvas to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mask(u32);

/// A mask set up once for repeated masked copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stencil(u32);

/// A font: `Font::FIXED` (the server's "fixed") or one from [`X11::font`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Font(usize);

impl Font {
    pub const FIXED: Font = Font(0);
}

const OUT_MAX: usize = 16384;
const MAX_FONTS: usize = 16;
const QUEUE_MAX: usize = 256;

enum Conn {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Conn {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read(b),
            Conn::Unix(s) => s.read(b),
        }
    }
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(b),
            Conn::Unix(s) => s.write(b),
        }
    }
    fn fd(&self) -> RawFd {
        match self {
            Conn::Tcp(s) => s.as_raw_fd(),
            Conn::Unix(s) => s.as_raw_fd(),
        }
    }
}

// poll(2), from the C library std already links: std has no way to wait on
// a socket with a timeout without reading from it.
#[repr(C)]
struct PollFd {
    fd: std::os::raw::c_int,
    events: std::os::raw::c_short,
    revents: std::os::raw::c_short,
}
#[cfg(target_os = "linux")]
type NfdsT = std::os::raw::c_ulong;
#[cfg(not(target_os = "linux"))]
type NfdsT = std::os::raw::c_uint;
extern "C" {
    fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: std::os::raw::c_int) -> std::os::raw::c_int;
}
const POLLIN: std::os::raw::c_short = 1;

/// Wait until one of fds can be read, or ms pass (-1: no limit); which can.
fn poll_fds(fds: &[RawFd], ms: i32) -> Vec<bool> {
    let mut p: Vec<PollFd> = fds
        .iter()
        .map(|&fd| PollFd {
            fd,
            events: POLLIN,
            revents: 0,
        })
        .collect();
    loop {
        // SAFETY: p is a valid array of p.len() pollfd structures.
        let r = unsafe { poll(p.as_mut_ptr(), p.len() as NfdsT, ms) };
        if r >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
    p.iter().map(|q| q.revents != 0).collect()
}

fn get16(b: &[u8], i: usize) -> u32 {
    b[i] as u32 | (b[i + 1] as u32) << 8
}
fn get32(b: &[u8], i: usize) -> u32 {
    get16(b, i) | get16(b, i + 2) << 16
}

#[derive(Default)]
struct FontInfo {
    id: u32,
    ascent: i32,
    descent: i32,
    minc: i32,
    maxc: i32,
    defw: i32,
    widths: Vec<i16>, // of minc..=maxc, or empty: every glyph is defw wide
}

/// The connection to the X server, and its one window.
pub struct X11 {
    conn: Conn,
    dead: bool,
    out: Vec<u8>,
    seq: u32,
    id_base: u32,
    id_next: u32,
    root: u32,
    depth: u8,
    bpp: u8,
    max_req: u32,
    masks: [u32; 3],
    image_msb: bool,
    win: u32,
    back: u32,
    gc: u32,
    fixed: u32,
    color: u32,
    target: u32,
    wm_protocols: u32,
    wm_delete: u32,
    min_kc: i32,
    kc_count: i32,
    syms_per: i32,
    keysyms: Vec<u32>,
    queue: VecDeque<Event>,
    width: i32,
    height: i32,
    sent: u64,
    bg: u32,
    expose: bool,
    fence_seq: u32,
    fence_on: bool,
    fonts: Vec<FontInfo>,
    cur_font: usize,
    gc1: u32,
    gc1_fg: u32,
}

fn other(msg: String) -> io::Error {
    io::Error::other(msg)
}

/// DISPLAY is HOST:N[.S] (TCP port 6000+N), :N or unix:N (the local socket),
/// or a socket path such as XQuartz's /private/tmp/.../org.xquartz:0.
fn connect(d: &str) -> io::Result<Conn> {
    if d.starts_with('/') {
        return Ok(Conn::Unix(UnixStream::connect(d)?));
    }
    let colon = d
        .rfind(':')
        .ok_or_else(|| other(format!("x11: bad DISPLAY {d}")))?;
    let host = &d[..colon];
    let num: u16 = d[colon + 1..]
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    if host.is_empty() || host == "unix" {
        return Ok(Conn::Unix(UnixStream::connect(format!(
            "/tmp/.X11-unix/X{num}"
        ))?));
    }
    let mut last = other(format!("x11: no address for {host}"));
    for a in (host, 6000 + num).to_socket_addrs()? {
        match TcpStream::connect(a) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(Conn::Tcp(s));
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl X11 {
    // ---- the request buffer ----

    fn lost(&mut self) {
        if !self.dead {
            self.dead = true;
            self.push(Event::Close);
        }
    }

    fn push(&mut self, e: Event) {
        if self.queue.len() < QUEUE_MAX {
            self.queue.push_back(e);
        }
    }

    fn write_raw(&mut self, mut b: &[u8]) {
        while !b.is_empty() && !self.dead {
            match self.conn.write(b) {
                Ok(0) => self.lost(),
                Ok(n) => {
                    b = &b[n..];
                    self.sent += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => self.lost(),
            }
        }
    }

    /// Send what is buffered.
    pub fn flush(&mut self) {
        if !self.out.is_empty() {
            let out = std::mem::take(&mut self.out);
            self.write_raw(&out);
            self.out = out;
            self.out.clear();
        }
    }

    fn put8(&mut self, v: u32) {
        self.out.push(v as u8);
    }
    fn put16(&mut self, v: u32) {
        self.out.extend_from_slice(&(v as u16).to_le_bytes());
    }
    fn put32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    /// Bytes, then zeros to a multiple of 4.
    fn put_padded(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
        for _ in 0..(4 - b.len() % 4) % 4 {
            self.out.push(0);
        }
    }

    /// Start a request len 4-byte words long.
    fn req(&mut self, opcode: u32, data: u32, len: u32) {
        if self.out.len() + len as usize * 4 > OUT_MAX {
            self.flush();
        }
        self.put8(opcode);
        self.put8(data);
        self.put16(len);
        self.seq = self.seq.wrapping_add(1);
    }

    fn new_id(&mut self) -> u32 {
        self.id_next += 1;
        self.id_base | (self.id_next - 1)
    }

    fn read_exact(&mut self, b: &mut [u8]) -> bool {
        let mut off = 0;
        while off < b.len() && !self.dead {
            match self.conn.read(&mut b[off..]) {
                Ok(0) => self.lost(),
                Ok(n) => off += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => self.lost(),
            }
        }
        !self.dead
    }

    /// Read one message: its 32 bytes, and a reply's extra data.
    fn message(&mut self, b: &mut [u8; 32]) -> Option<Vec<u8>> {
        if !self.read_exact(b) {
            return None;
        }
        let code = b[0] & 0x7f;
        let n = if code == 1 || code == 35 {
            get32(b, 4) as usize * 4
        } else {
            0
        };
        let mut extra = vec![0u8; n];
        if n > 0 && !self.read_exact(&mut extra) {
            return None;
        }
        Some(extra)
    }

    /// Send what is buffered and wait for the reply to the last request;
    /// events that arrive first are queued. None on an error or lost link.
    fn roundtrip(&mut self) -> Option<([u8; 32], Vec<u8>)> {
        // Remember which reply: handling an event below can send requests
        // (repainting after Expose, a new back buffer after a resize).
        let want = self.seq & 0xffff;
        self.flush();
        loop {
            let mut b = [0u8; 32];
            let extra = self.message(&mut b)?;
            let t = b[0] & 0x7f;
            if (t == 0 || t == 1) && get16(&b, 2) == want {
                if t == 0 {
                    self.handle(&b);
                    return None;
                }
                return Some((b, extra));
            }
            self.handle(&b);
        }
    }

    fn keysym(&self, keycode: i32, state: u32) -> u32 {
        if self.keysyms.is_empty()
            || keycode < self.min_kc
            || keycode >= self.min_kc + self.kc_count
        {
            return 0;
        }
        let i = ((keycode - self.min_kc) * self.syms_per) as usize;
        let lo = self.keysyms[i];
        let mut hi = if self.syms_per > 1 && self.keysyms[i + 1] != 0 {
            self.keysyms[i + 1]
        } else {
            lo
        };
        let letter = (b'a' as u32..=b'z' as u32).contains(&lo);
        if letter && hi == lo {
            hi = lo - 32;
        }
        let shift = state & 1 != 0;
        if letter && state & 2 != 0 {
            return if shift { lo } else { hi }; // Caps Lock
        }
        if shift {
            hi
        } else {
            lo
        }
    }

    fn copy_back(&mut self) {
        let (back, win, gc, w, h) = (self.back, self.win, self.gc, self.width, self.height);
        self.req(62, 0, 7); // CopyArea
        self.put32(back);
        self.put32(win);
        self.put32(gc);
        self.put32(0);
        self.put32(0);
        self.put16(w as u32);
        self.put16(h as u32);
    }

    /// A back buffer for the window's size now. The old picture is copied
    /// into it, on the background for the new part, so that until the
    /// program draws again the window shows what it did.
    fn make_back(&mut self, old_w: i32, old_h: i32) {
        let old = self.back;
        self.back = self.new_id();
        let (back, win, depth, w, h, gc, bg, color) = (
            self.back,
            self.win,
            self.depth,
            self.width,
            self.height,
            self.gc,
            self.bg,
            self.color,
        );
        self.req(53, depth as u32, 4); // CreatePixmap
        self.put32(back);
        self.put32(win);
        self.put16(w as u32);
        self.put16(h as u32);
        if old == 0 {
            return;
        }
        self.change_gc(0x4, bg);
        self.req(70, 0, 3 + 2); // PolyFillRectangle
        self.put32(back);
        self.put32(gc);
        self.put32(0);
        self.put16(w as u32);
        self.put16(h as u32);
        self.req(62, 0, 7); // CopyArea: the old picture
        self.put32(old);
        self.put32(back);
        self.put32(gc);
        self.put32(0);
        self.put32(0);
        self.put16(old_w.min(w) as u32);
        self.put16(old_h.min(h) as u32);
        self.change_gc(0x4, color);
        self.req(54, 0, 2); // FreePixmap
        self.put32(old);
    }

    fn handle(&mut self, b: &[u8; 32]) {
        let code = b[0] & 0x7f;
        if self.fence_on && code <= 1 && get16(b, 2) == self.fence_seq {
            self.fence_on = false; // the fence's reply (or error)
            return;
        }
        let x = get16(b, 24) as i16 as i32;
        let y = get16(b, 26) as i16 as i32;
        let mods = get16(b, 28);
        match code {
            0 => eprintln!("x11: error {} from request {}", b[1], b[10]),
            2 => {
                let key = self.keysym(b[1] as i32, mods);
                self.push(Event::KeyDown { key, x, y, mods })
            }
            3 => {
                let key = self.keysym(b[1] as i32, mods);
                self.push(Event::KeyUp { key, x, y, mods })
            }
            4 => self.push(Event::ButtonDown {
                button: b[1] as u32,
                x,
                y,
                mods,
            }),
            5 => self.push(Event::ButtonUp {
                button: b[1] as u32,
                x,
                y,
                mods,
            }),
            6 => self.push(Event::Motion { x, y, mods }),
            // Expose: show the back buffer again, in event(), between
            // frames, not now, perhaps halfway through one
            12 => self.expose = true,
            22 => {
                // ConfigureNotify
                let (w, h) = (get16(b, 20) as i32, get16(b, 22) as i32);
                if w != self.width || h != self.height {
                    let (ow, oh) = (self.width, self.height);
                    self.width = w;
                    self.height = h;
                    self.make_back(ow, oh);
                    self.push(Event::Resize { w, h });
                }
            }
            // ClientMessage: the close button
            33 if b[1] == 32
                && get32(b, 8) == self.wm_protocols
                && get32(b, 12) == self.wm_delete =>
            {
                self.push(Event::Close)
            }
            _ => {}
        }
    }

    fn atom(&mut self, name: &str) -> u32 {
        let n = name.len() as u32;
        self.req(16, 0, 2 + n.div_ceil(4)); // InternAtom
        self.put16(n);
        self.put16(0);
        self.put_padded(name.as_bytes());
        self.roundtrip().map_or(0, |(b, _)| get32(&b, 8))
    }

    /// Pick what we need out of the set-up reply.
    fn parse_setup(&mut self, b: &[u8]) -> bool {
        if b.len() < 32 {
            return false;
        }
        let nformats = b[21] as usize;
        self.id_base = get32(b, 4);
        self.max_req = get16(b, 18);
        self.image_msb = b[22] != 0;
        self.min_kc = b[26] as i32;
        self.kc_count = b[27] as i32 - b[26] as i32 + 1;
        let formats = 32 + ((get16(b, 16) as usize + 3) & !3); // after the vendor string
        let s = formats + 8 * nformats; // the first screen
        if s + 40 > b.len() {
            return false;
        }
        self.root = get32(b, s);
        let visual = get32(b, s + 32);
        self.depth = b[s + 38];
        for i in 0..nformats {
            if b[formats + i * 8] == self.depth {
                self.bpp = b[formats + i * 8 + 1];
            }
        }
        // find the root visual among the screen's depths
        let mut p = s + 40;
        for _ in 0..b[s + 39] {
            if p + 8 > b.len() {
                break;
            }
            let nv = get16(b, p + 2) as usize;
            p += 8;
            for _ in 0..nv {
                if p + 24 > b.len() {
                    break;
                }
                if get32(b, p) == visual {
                    if b[p + 4] != 4 {
                        return false; // not TrueColor
                    }
                    self.masks = [get32(b, p + 8), get32(b, p + 12), get32(b, p + 16)];
                }
                p += 24;
            }
        }
        self.masks[0] != 0
    }

    // ---- the API ----

    /// Connect to display (None: $DISPLAY). The error says why, with a hint.
    pub fn open(display: Option<&str>) -> io::Result<X11> {
        let d = match display {
            Some(d) => d.to_string(),
            None => std::env::var("DISPLAY").unwrap_or_default(),
        };
        if d.is_empty() {
            return Err(other(
                "x11: set DISPLAY to the X server, e.g. DISPLAY=192.168.1.2:0".into(),
            ));
        }
        let conn = connect(&d)
            .map_err(|e| other(format!("x11: can't connect to the X server at {d}: {e}")))?;
        let mut x = X11 {
            conn,
            dead: false,
            out: Vec::with_capacity(OUT_MAX),
            seq: 0,
            id_base: 0,
            id_next: 0,
            root: 0,
            depth: 0,
            bpp: 0,
            max_req: 0,
            masks: [0; 3],
            image_msb: false,
            win: 0,
            back: 0,
            gc: 0,
            fixed: 0,
            color: 0xffff_ffff,
            target: 0,
            wm_protocols: 0,
            wm_delete: 0,
            min_kc: 0,
            kc_count: 0,
            syms_per: 0,
            keysyms: Vec::new(),
            queue: VecDeque::new(),
            width: 0,
            height: 0,
            sent: 0,
            bg: 0,
            expose: false,
            fence_seq: 0,
            fence_on: false,
            fonts: vec![FontInfo::default()],
            cur_font: 0,
            gc1: 0,
            gc1_fg: 2,
        };
        // little-endian, protocol 11.0, no authorization
        x.out
            .extend_from_slice(&[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        x.flush();
        let mut head = [0u8; 8];
        if !x.read_exact(&mut head) {
            return Err(other(format!("x11: the X server at {d} hung up")));
        }
        let mut setup = vec![0u8; get16(&head, 6) as usize * 4];
        if !x.read_exact(&mut setup) {
            return Err(other(format!("x11: no set-up reply from {d}")));
        }
        if head[0] != 1 {
            let n = if head[0] == 0 {
                (head[1] as usize).min(setup.len())
            } else {
                setup.len()
            };
            return Err(other(format!(
                "x11: {d} refused the connection: {}\nx11: on that machine, try: xhost +ADDRESS-OF-THIS-ONE (or xhost +local: for a local server)",
                String::from_utf8_lossy(&setup[..n]).trim_end_matches('\0')
            )));
        }
        if !x.parse_setup(&setup) || x.bpp != 32 {
            return Err(other(format!(
                "x11: {d} is not a 24/32-bit TrueColor display"
            )));
        }

        let (min_kc, count) = (x.min_kc as u32, x.kc_count as u32);
        x.req(101, 0, 2); // GetKeyboardMapping
        x.put8(min_kc);
        x.put8(count);
        x.put16(0);
        if let Some((b, syms)) = x.roundtrip() {
            x.syms_per = b[1] as i32;
            x.keysyms = (0..get32(&b, 4) as usize)
                .map(|i| get32(&syms, i * 4))
                .collect();
        }

        x.fixed = x.new_id();
        let fixed = x.fixed;
        x.req(45, 0, 3 + 2); // OpenFont "fixed"
        x.put32(fixed);
        x.put16(5);
        x.put16(0);
        x.put_padded(b"fixed");
        x.wm_protocols = x.atom("WM_PROTOCOLS");
        x.wm_delete = x.atom("WM_DELETE_WINDOW");
        if x.dead {
            return Err(other(format!("x11: lost the connection to {d}")));
        }
        Ok(x)
    }

    /// The window's size now.
    pub fn width(&self) -> i32 {
        self.width
    }
    pub fn height(&self) -> i32 {
        self.height
    }

    /// Bytes sent to the X server so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// A pixel value for a colour, each component 0-255.
    pub fn rgb(&self, r: u8, g: u8, b: u8) -> u32 {
        let mut v = 0;
        for (c, &m) in [r, g, b].iter().zip(self.masks.iter()) {
            let (shift, bits) = (
                m.trailing_zeros(),
                (m >> m.trailing_zeros().min(31)).trailing_ones(),
            );
            let c = *c as u32;
            v |= (if bits >= 8 {
                c << (bits - 8)
            } else {
                c >> (8 - bits)
            }) << shift;
        }
        v
    }

    /// The window's background: what the server fills new parts of the
    /// window with when it grows, until the program draws them. Call
    /// before [`X11::window`] (it can't change after).
    pub fn background(&mut self, pixel: u32) {
        self.bg = pixel;
    }

    /// Open the program's one window and show it; it starts out in the
    /// background colour (black unless [`X11::background`] said otherwise).
    pub fn window(&mut self, title: &str, w: i32, h: i32) {
        self.width = w;
        self.height = h;
        self.win = self.new_id();
        let (win, root, bg) = (self.win, self.root, self.bg);
        self.req(1, 0, 8 + 3); // CreateWindow, depth from the parent
        self.put32(win);
        self.put32(root);
        self.put16(0);
        self.put16(0);
        self.put16(w as u32);
        self.put16(h as u32);
        self.put16(0); // border
        self.put16(1); // InputOutput
        self.put32(0); // visual from the parent
        self.put32(0x2 | 0x10 | 0x800); // background-pixel, bit-gravity, event-mask
        self.put32(bg);
        self.put32(1); // NorthWest: a resize keeps the picture
                       // keys, buttons, motion, exposure, structure
        self.put32(0x1 | 0x2 | 0x4 | 0x8 | 0x40 | 0x8000 | 0x20000);

        let t = title.as_bytes();
        self.req(18, 0, 6 + (t.len() as u32).div_ceil(4)); // ChangeProperty WM_NAME
        self.put32(win);
        self.put32(39); // WM_NAME
        self.put32(31); // STRING
        self.put32(8); // format 8
        self.put32(t.len() as u32);
        self.put_padded(t);

        if self.wm_protocols != 0 && self.wm_delete != 0 {
            let (p, d) = (self.wm_protocols, self.wm_delete);
            self.req(18, 0, 6 + 1); // WM_PROTOCOLS = WM_DELETE_WINDOW
            self.put32(win);
            self.put32(p);
            self.put32(4); // ATOM
            self.put32(32);
            self.put32(1);
            self.put32(d);
        }

        self.gc = self.new_id();
        let (gc, white, fixed) = (self.gc, self.rgb(255, 255, 255), self.fixed);
        self.req(55, 0, 4 + 3); // CreateGC
        self.put32(gc);
        self.put32(win);
        self.put32(0x4 | 0x4000 | 0x10000); // foreground, font, graphics-exposures
        self.put32(white);
        self.put32(fixed);
        self.put32(0);
        self.color = white;

        self.make_back(0, 0);
        self.req(8, 0, 2); // MapWindow
        self.put32(win);
        self.flush();
    }

    /// The smallest size the window manager should let the window be made.
    pub fn min_size(&mut self, w: i32, h: i32) {
        let win = self.win;
        self.req(18, 0, 6 + 18); // ChangeProperty WM_NORMAL_HINTS
        self.put32(win);
        self.put32(40); // WM_NORMAL_HINTS
        self.put32(41); // WM_SIZE_HINTS
        self.put32(32);
        self.put32(18);
        self.put32(16); // flags: PMinSize
        for i in 1..18 {
            self.put32(match i {
                5 => w as u32,
                6 => h as u32,
                _ => 0,
            });
        }
    }

    fn change_gc(&mut self, mask: u32, value: u32) {
        let gc = self.gc;
        self.req(56, 0, 3 + 1); // ChangeGC
        self.put32(gc);
        self.put32(mask);
        self.put32(value);
    }

    /// The colour for the drawing calls that follow.
    pub fn color(&mut self, pixel: u32) {
        if pixel != self.color {
            self.change_gc(0x4, pixel);
            self.color = pixel;
        }
    }

    /// Line width for lines, rectangles and ellipses (0: thin and fast).
    pub fn line_width(&mut self, w: i32) {
        self.change_gc(0x10, w.max(0) as u32);
    }

    fn dst(&self) -> u32 {
        if self.target != 0 {
            self.target
        } else {
            self.back
        }
    }

    fn box_(&mut self, opcode: u32, x: i32, y: i32, w: i32, h: i32, arc: bool) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (dst, gc) = (self.dst(), self.gc);
        self.req(opcode, 0, if arc { 3 + 3 } else { 3 + 2 });
        self.put32(dst);
        self.put32(gc);
        self.put16(x as u32);
        self.put16(y as u32);
        self.put16(w as u32);
        self.put16(h as u32);
        if arc {
            self.put16(0); // from 0 degrees
            self.put16(360 * 64); // all the way round, in 1/64 degree
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.box_(70, x, y, w, h, false);
    }
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.box_(67, x, y, w, h, false);
    }
    /// An ellipse filling the box at (x, y), w by h.
    pub fn fill_ellipse(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.box_(71, x, y, w, h, true);
    }
    pub fn ellipse(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.box_(68, x, y, w, h, true);
    }

    pub fn line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) {
        let (dst, gc) = (self.dst(), self.gc);
        self.req(66, 0, 3 + 2); // PolySegment
        self.put32(dst);
        self.put32(gc);
        self.put16(x1 as u32);
        self.put16(y1 as u32);
        self.put16(x2 as u32);
        self.put16(y2 as u32);
    }

    /// Text in the current colour and font; (x, y) is the left end of the
    /// baseline. Characters past U+00FF show as '?' (see `text_unicode`).
    pub fn text(&mut self, x: i32, y: i32, s: &str) {
        let bytes: Vec<u8> = s
            .chars()
            .take(254)
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        if bytes.is_empty() {
            return;
        }
        let n = bytes.len() as u32;
        let (dst, gc) = (self.dst(), self.gc);
        self.req(74, 0, 4 + (n + 2).div_ceil(4)); // PolyText8
        self.put32(dst);
        self.put32(gc);
        self.put16(x as u32);
        self.put16(y as u32);
        let mut item = vec![n as u8, 0]; // one text item: length, delta, chars
        item.extend_from_slice(&bytes);
        self.put_padded(&item);
    }

    // ---- fonts ----
    // Text starts out in Font::FIXED, the server's "fixed". font() finds
    // another by XLFD pattern, such as
    //     "-*-helvetica-bold-r-normal--24-*-*-*-p-*-iso8859-1",
    // and gives Font::FIXED back if the server has none that match, so the
    // program still runs, in "fixed".

    /// Read a font's metrics (QueryFont) into slot f.
    fn font_metrics(&mut self, f: usize) {
        let id = self.fonts[f].id;
        self.req(47, 0, 2); // QueryFont
        self.put32(id);
        let Some((b, x)) = self.roundtrip().filter(|(_, x)| x.len() >= 28) else {
            let fi = &mut self.fonts[f];
            (fi.ascent, fi.descent, fi.defw) = (11, 2, 6);
            return;
        };
        // x is the reply from byte 32 on: max-bounds' width is at byte 28,
        // so in b; the rest of the fixed part follows in x.
        let fi = &mut self.fonts[f];
        fi.defw = get16(&b, 28) as i16 as i32;
        fi.minc = get16(&x, 8) as i32;
        fi.maxc = get16(&x, 10) as i32;
        fi.ascent = get16(&x, 20) as i16 as i32;
        fi.descent = get16(&x, 22) as i16 as i32;
        let nprops = get16(&x, 14) as usize;
        let m = get32(&x, 24) as usize;
        let ci = 28 + 8 * nprops;
        if m > 0 && m as i32 == fi.maxc - fi.minc + 1 && ci + 12 * m <= x.len() {
            fi.widths = (0..m).map(|i| get16(&x, ci + 12 * i + 4) as i16).collect();
        }
    }

    /// ListFonts: the first name that matches, if any.
    fn list_font(&mut self, pattern: &str) -> Option<Vec<u8>> {
        let p = pattern.as_bytes();
        self.req(49, 0, 2 + (p.len() as u32).div_ceil(4));
        self.put16(1);
        self.put16(p.len() as u32);
        self.put_padded(p);
        let (b, x) = self.roundtrip()?;
        if get16(&b, 8) == 0 || x.is_empty() {
            return None;
        }
        let n = (x[0] as usize).min(x.len() - 1);
        Some(x[1..1 + n].to_vec())
    }

    fn open_font(&mut self, name: &[u8]) -> usize {
        let id = self.new_id();
        self.req(45, 0, 3 + (name.len() as u32).div_ceil(4)); // OpenFont
        self.put32(id);
        self.put16(name.len() as u32);
        self.put16(0);
        self.put_padded(name);
        self.fonts.push(FontInfo {
            id,
            ..Default::default()
        });
        self.fonts.len() - 1
    }

    /// A font by XLFD pattern, or Font::FIXED if the server has none.
    pub fn font(&mut self, pattern: &str) -> Font {
        if self.fonts.len() >= MAX_FONTS || pattern.len() > 255 {
            return Font::FIXED;
        }
        if self.list_font(pattern).is_none() {
            return Font::FIXED;
        }
        let f = self.open_font(pattern.as_bytes());
        self.font_metrics(f);
        Font(f)
    }

    /// A fixed-width font, for a grid of characters (a terminal), which may
    /// be a large Unicode one: QueryFont would send the metrics of every
    /// glyph, which can be hundreds of KB, so this measures one "M" instead
    /// (QueryTextExtents). All its characters are then that wide.
    pub fn font_cell(&mut self, pattern: &str) -> Font {
        if self.fonts.len() >= MAX_FONTS || pattern.len() > 255 {
            return Font::FIXED;
        }
        // Open it by the name the server gave: a scalable font won't open
        // from a pattern with wildcards.
        let Some(name) = self.list_font(pattern) else {
            return Font::FIXED;
        };
        let f = self.open_font(&name);
        let id = self.fonts[f].id;
        self.req(48, 1, 2 + 1); // QueryTextExtents of "M" (odd length: one CHAR2B)
        self.put32(id);
        self.put8(0); // CHAR2B: high byte first
        self.put8(b'M' as u32);
        self.put16(0);
        match self.roundtrip() {
            Some((b, _)) if get32(&b, 16) as i32 > 0 => {
                let fi = &mut self.fonts[f];
                fi.ascent = get16(&b, 8) as i16 as i32;
                fi.descent = get16(&b, 10) as i16 as i32;
                fi.defw = get32(&b, 16) as i32;
                fi.minc = 1; // no table: every glyph is defw wide
                fi.maxc = 0;
                Font(f)
            }
            _ => {
                self.fonts.pop(); // listed, but it won't open
                Font::FIXED
            }
        }
    }

    /// Slot 0 is "fixed", opened by open(); its metrics are read when needed.
    fn font_ready(&mut self, f: Font) -> usize {
        let f = if f.0 < self.fonts.len() { f.0 } else { 0 };
        if f == 0 && self.fonts[0].id == 0 {
            self.fonts[0].id = self.fixed;
            self.font_metrics(0);
        }
        f
    }

    /// The font for the text calls that follow.
    pub fn text_font(&mut self, f: Font) {
        let f = self.font_ready(f);
        if f != self.cur_font {
            let id = self.fonts[f].id;
            self.change_gc(0x4000, id);
            self.cur_font = f;
        }
    }

    /// How wide s is in font f, in pixels.
    pub fn text_width(&mut self, f: Font, s: &str) -> i32 {
        let f = self.font_ready(f);
        let fi = &self.fonts[f];
        s.chars()
            .map(|c| {
                let c = c as i32;
                if !fi.widths.is_empty() && c >= fi.minc && c <= fi.maxc {
                    fi.widths[(c - fi.minc) as usize] as i32
                } else {
                    fi.defw
                }
            })
            .sum()
    }

    /// Height above the baseline.
    pub fn font_ascent(&mut self, f: Font) -> i32 {
        let f = self.font_ready(f);
        self.fonts[f].ascent
    }

    /// Height below the baseline.
    pub fn font_descent(&mut self, f: Font) -> i32 {
        let f = self.font_ready(f);
        self.fonts[f].descent
    }

    /// Text as Unicode (up to U+FFFF; others show as '?'), for a font that
    /// covers it, such as an iso10646-1 one from `font_cell`. (x, y) is the
    /// left end of the baseline.
    pub fn text_unicode(&mut self, mut x: i32, y: i32, s: &str) {
        let codes: Vec<u32> = s
            .chars()
            .map(|c| {
                if (c as u32) > 0xffff {
                    '?' as u32
                } else {
                    c as u32
                }
            })
            .collect();
        let defw = self.fonts[self.cur_font].defw;
        for chunk in codes.chunks(254) {
            let k = chunk.len() as u32;
            let (dst, gc) = (self.dst(), self.gc);
            self.req(75, 0, 4 + (2 + 2 * k).div_ceil(4)); // PolyText16
            self.put32(dst);
            self.put32(gc);
            self.put16(x as u32);
            self.put16(y as u32);
            let mut item = vec![k as u8, 0]; // one item: length, delta, chars
            for &c in chunk {
                item.push((c >> 8) as u8); // CHAR2B: high byte first
                item.push(c as u8);
            }
            self.put_padded(&item);
            x += k as i32 * defw;
        }
    }

    /// The back buffer as a canvas to copy from: blit(frame(), ...) into a
    /// canvas saves part of the frame (to put back after a menu, say), and
    /// into the frame itself moves part of it (scrolling a list). It
    /// changes when the window is resized.
    pub fn frame(&self) -> Canvas {
        Canvas(self.back)
    }

    /// Copy w x h pixels (from `rgb`) to (x, y); stride is the row length
    /// in pixels. Large images go in several requests, rows at a time.
    pub fn image(&mut self, x: i32, y: i32, w: i32, h: i32, pixels: &[u32], stride: usize) {
        if w <= 0 || h <= 0 {
            return;
        }
        let rows_max = (self.max_req as i64 * 4 - 24) / (w as i64 * 4);
        if rows_max < 1 {
            return; // wider than one request can hold
        }
        let (mut y, mut h, mut off) = (y, h, 0usize);
        let mut row_bytes = Vec::with_capacity(w as usize * 4);
        while h > 0 && !self.dead {
            let rows = (h as i64).min(rows_max) as i32;
            let (dst, gc, depth) = (self.dst(), self.gc, self.depth);
            self.req(72, 2, 6 + (w * rows) as u32); // PutImage, ZPixmap
            self.put32(dst);
            self.put32(gc);
            self.put16(w as u32);
            self.put16(rows as u32);
            self.put16(x as u32);
            self.put16(y as u32);
            self.put8(0); // left pad
            self.put8(depth as u32);
            self.put16(0);
            self.flush();
            for r in 0..rows as usize {
                let row = &pixels[off + r * stride..off + r * stride + w as usize];
                row_bytes.clear();
                for &v in row {
                    row_bytes.extend_from_slice(&if self.image_msb {
                        v.to_be_bytes()
                    } else {
                        v.to_le_bytes()
                    });
                }
                let rb = std::mem::take(&mut row_bytes);
                self.write_raw(&rb);
                row_bytes = rb;
            }
            off += rows as usize * stride;
            y += rows;
            h -= rows;
        }
    }

    /// Many shapes of one kind in as few requests as fit the buffer.
    fn batch(&mut self, opcode: u32, v: &[i16], per: usize) {
        let most = (OUT_MAX - 12) / (per * 2);
        for chunk in v.chunks(most * per) {
            let (dst, gc) = (self.dst(), self.gc);
            self.req(opcode, 0, 3 + (chunk.len() / 2) as u32);
            self.put32(dst);
            self.put32(gc);
            for &s in chunk {
                self.put16(s as u16 as u32);
            }
        }
    }

    /// Rectangles: [x, y, w, h] each.
    pub fn fill_rects(&mut self, r: &[[i16; 4]]) {
        self.batch(70, r.as_flattened(), 4);
    }

    /// Line segments: [x1, y1, x2, y2] each.
    pub fn segments(&mut self, s: &[[i16; 4]]) {
        self.batch(66, s.as_flattened(), 4);
    }

    fn poly(&mut self, opcode: u32, shape: Option<u32>, pts: &[[i16; 2]]) {
        let (dst, gc) = (self.dst(), self.gc);
        let n = pts.len() as u32;
        self.req(opcode, 0, if shape.is_some() { 4 + n } else { 3 + n });
        self.put32(dst);
        self.put32(gc);
        if let Some(shape) = shape {
            self.put8(shape);
            self.put8(0); // coordinates relative to the origin
            self.put16(0);
        }
        for p in pts {
            self.put16(p[0] as u16 as u32);
            self.put16(p[1] as u16 as u32);
        }
    }

    /// A filled convex polygon (up to 1000 points).
    pub fn fill_poly(&mut self, pts: &[[i16; 2]]) {
        if pts.len() >= 3 && pts.len() <= 1000 {
            self.poly(69, Some(2), pts); // FillPoly, Convex
        }
    }

    /// Connected lines through the points (up to 2000), in the current
    /// colour and width.
    pub fn polyline(&mut self, pts: &[[i16; 2]]) {
        if pts.len() >= 2 && pts.len() <= 2000 {
            self.poly(65, None, pts); // PolyLine, coordinates from the origin
        }
    }

    /// An off-screen image w x h; its contents start undefined, so fill it.
    pub fn canvas(&mut self, w: i32, h: i32) -> Canvas {
        let c = self.new_id();
        let (win, depth) = (self.win, self.depth);
        self.req(53, depth as u32, 4); // CreatePixmap
        self.put32(c);
        self.put32(win);
        self.put16(w.max(1) as u32);
        self.put16(h.max(1) as u32);
        Canvas(c)
    }

    pub fn free_canvas(&mut self, c: Canvas) {
        if c.0 == 0 || c.0 == self.back {
            return;
        }
        if self.target == c.0 {
            self.target = 0;
        }
        self.req(54, 0, 2); // FreePixmap
        self.put32(c.0);
    }

    /// Where the drawing calls go: a canvas, or None for the frame.
    pub fn target(&mut self, c: Option<Canvas>) {
        self.target = c.map_or(0, |c| c.0);
    }

    /// Draw straight onto the window, over the last frame `present` showed;
    /// the back buffer is left as it was. `target(None)` goes back to it.
    pub fn target_window(&mut self) {
        self.target = self.win;
    }

    /// Copy w x h from (sx, sy) of canvas c to (dx, dy) of the current target.
    #[allow(clippy::too_many_arguments)]
    pub fn blit(&mut self, c: Canvas, sx: i32, sy: i32, w: i32, h: i32, dx: i32, dy: i32) {
        let gc = self.gc;
        self.copy_area(gc, c.0, sx, sy, w, h, dx, dy);
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_area(&mut self, gc: u32, src: u32, sx: i32, sy: i32, w: i32, h: i32, dx: i32, dy: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        let dst = self.dst();
        self.req(62, 0, 7); // CopyArea
        self.put32(src);
        self.put32(dst);
        self.put32(gc);
        self.put16(sx as u32);
        self.put16(sy as u32);
        self.put16(dx as u32);
        self.put16(dy as u32);
        self.put16(w as u32);
        self.put16(h as u32);
    }

    // ---- masks: 1-bit images that say which pixels of a canvas to copy ----

    fn mask_fg(&mut self, m: u32, fg: u32) {
        if self.gc1 == 0 {
            self.gc1 = self.new_id();
            let gc1 = self.gc1;
            self.req(55, 0, 4 + 1); // CreateGC, for depth-1 drawables
            self.put32(gc1);
            self.put32(m);
            self.put32(0x10000); // graphics-exposures
            self.put32(0);
        }
        if fg != self.gc1_fg {
            let gc1 = self.gc1;
            self.req(56, 0, 3 + 1); // ChangeGC: foreground
            self.put32(gc1);
            self.put32(0x4);
            self.put32(fg);
            self.gc1_fg = fg;
        }
    }

    /// A w x h mask, all clear (nothing copied). Free it with `free_mask`.
    pub fn mask(&mut self, w: i32, h: i32) -> Mask {
        let m = self.new_id();
        let (win, w, h) = (self.win, w.max(1) as u32, h.max(1) as u32);
        self.req(53, 1, 4); // CreatePixmap, depth 1
        self.put32(m);
        self.put32(win);
        self.put16(w);
        self.put16(h);
        self.mask_fg(m, 0);
        let gc1 = self.gc1;
        self.req(70, 0, 3 + 2); // PolyFillRectangle: clear it
        self.put32(m);
        self.put32(gc1);
        self.put32(0);
        self.put16(w);
        self.put16(h);
        Mask(m)
    }

    pub fn free_mask(&mut self, m: Mask) {
        if m.0 != 0 {
            self.req(54, 0, 2); // FreePixmap
            self.put32(m.0);
        }
    }

    /// Set rectangles ([x, y, w, h] each) of mask m: those pixels get copied.
    pub fn mask_rects(&mut self, m: Mask, r: &[[i16; 4]]) {
        self.mask_fg(m.0, 1);
        let most = (OUT_MAX - 12) / 8;
        for chunk in r.chunks(most) {
            let gc1 = self.gc1;
            self.req(70, 0, 3 + chunk.len() as u32 * 2);
            self.put32(m.0);
            self.put32(gc1);
            for v in chunk.as_flattened() {
                self.put16(*v as u16 as u32);
            }
        }
    }

    /// `blit`, copying only the pixels set in mask m (which lines up with c).
    #[allow(clippy::too_many_arguments)]
    pub fn blit_masked(
        &mut self,
        c: Canvas,
        m: Mask,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
    ) {
        if w <= 0 || h <= 0 {
            return;
        }
        let gc = self.gc;
        self.req(56, 0, 3 + 3); // ChangeGC: clip origin and mask
        self.put32(gc);
        self.put32(0x20000 | 0x40000 | 0x80000);
        self.put32((dx - sx) as u32);
        self.put32((dy - sy) as u32);
        self.put32(m.0);
        self.blit(c, sx, sy, w, h, dx, dy);
        self.change_gc(0x80000, 0); // clip mask: None
    }

    /// A stencil: a mask set up once for repeated masked copies. Setting a
    /// mask makes the server turn the bitmap into a clip region, which is
    /// slow for big masks; a stencil does that once, and each
    /// `blit_stencil` only moves it. Free it with `free_stencil` before
    /// freeing the mask.
    pub fn stencil(&mut self, m: Mask) -> Stencil {
        let g = self.new_id();
        let win = self.win;
        self.req(55, 0, 4 + 2); // CreateGC
        self.put32(g);
        self.put32(win);
        self.put32(0x10000 | 0x80000); // graphics-exposures, clip-mask
        self.put32(0);
        self.put32(m.0);
        Stencil(g)
    }

    pub fn free_stencil(&mut self, s: Stencil) {
        if s.0 != 0 {
            self.req(60, 0, 2); // FreeGC
            self.put32(s.0);
        }
    }

    /// `blit` through stencil s, made from the mask that lines up with c.
    #[allow(clippy::too_many_arguments)]
    pub fn blit_stencil(
        &mut self,
        s: Stencil,
        c: Canvas,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        dx: i32,
        dy: i32,
    ) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.req(56, 0, 3 + 2); // ChangeGC: clip origin
        self.put32(s.0);
        self.put32(0x20000 | 0x40000);
        self.put32((dx - sx) as u32);
        self.put32((dy - sy) as u32);
        self.copy_area(s.0, c.0, sx, sy, w, h, dx, dy);
    }

    /// Fill a rectangle in the current colour, but only where mask m,
    /// repeated across it like wallpaper, is set: a dither, for tints.
    pub fn fill_stippled(&mut self, m: Mask, x: i32, y: i32, w: i32, h: i32) {
        let gc = self.gc;
        self.req(56, 0, 3 + 4); // ChangeGC: fill-style, stipple, ts origin
        self.put32(gc);
        self.put32(0x100 | 0x800 | 0x1000 | 0x2000);
        self.put32(2); // FillStippled
        self.put32(m.0);
        self.put32(0);
        self.put32(0);
        self.box_(70, x, y, w, h, false);
        self.change_gc(0x100, 0); // FillSolid
    }

    /// A polygon (not necessarily convex, but not crossing itself; up to
    /// 2000 points) filled with canvas c repeated, its top left at (ox, oy):
    /// a gradient, say.
    pub fn fill_poly_tiled(&mut self, c: Canvas, ox: i32, oy: i32, pts: &[[i16; 2]]) {
        if pts.len() < 3 || pts.len() > 2000 {
            return;
        }
        let gc = self.gc;
        self.req(56, 0, 3 + 4); // ChangeGC: fill-style, tile, ts origin
        self.put32(gc);
        self.put32(0x100 | 0x400 | 0x1000 | 0x2000);
        self.put32(1); // FillTiled
        self.put32(c.0);
        self.put32(ox as u32);
        self.put32(oy as u32);
        self.poly(69, Some(1), pts); // FillPoly, Nonconvex
        self.change_gc(0x100, 0); // FillSolid
    }

    // ---- frames and timing ----

    /// Show what has been drawn since the last `present`.
    pub fn present(&mut self) {
        self.copy_back();
        self.flush();
    }

    /// Wait until the server has dealt with everything sent so far.
    pub fn sync(&mut self) {
        self.req(43, 0, 1); // GetInputFocus: any request with a reply
        let _ = self.roundtrip();
    }

    /// Mark this point (after `present`); `fence_wait` waits for it. Waiting
    /// for the previous frame's fence instead of `sync` keeps one frame in
    /// flight, so the program draws the next frame while the last one
    /// travels.
    pub fn fence(&mut self) {
        self.fence_wait(); // only one at a time
        self.req(43, 0, 1);
        self.fence_seq = self.seq & 0xffff;
        self.fence_on = true;
        self.flush();
    }

    /// Wait until the server has dealt with everything up to the last fence.
    pub fn fence_wait(&mut self) {
        self.flush();
        while self.fence_on && !self.dead {
            let mut b = [0u8; 32];
            if self.message(&mut b).is_none() {
                break;
            }
            self.handle(&b);
        }
        self.fence_on = false;
    }

    /// The next event, if there is one; never blocks.
    pub fn event(&mut self) -> Option<Event> {
        while self.queue.is_empty() && !self.dead {
            if !poll_fds(&[self.conn.fd()], 0)[0] {
                break;
            }
            let mut b = [0u8; 32];
            if self.message(&mut b).is_some() {
                self.handle(&b);
            }
        }
        if self.expose && self.back != 0 {
            self.expose = false;
            self.copy_back();
            self.flush();
        }
        self.queue.pop_front()
    }

    /// Send what is buffered, then sleep until an event arrives or ms pass
    /// (-1: until an event).
    pub fn wait(&mut self, ms: i32) {
        self.wait_with(&[], ms);
    }

    /// Like `wait`, but also wake when one of fds (stdin, a pipe, a socket)
    /// can be read; says which of them can. Returns at once, all false, if
    /// events are already waiting. Messages from the server that aren't
    /// events for the program (a fence's reply, say) are dealt with and the
    /// sleep goes on, so a loop paced by `wait` keeps its pace.
    pub fn wait_with(&mut self, fds: &[RawFd], ms: i32) -> Vec<bool> {
        self.flush();
        let deadline = (ms >= 0).then(|| Instant::now() + Duration::from_millis(ms as u64));
        loop {
            if self.pending() || self.dead {
                return vec![false; fds.len()];
            }
            let left = deadline.map_or(-1, |d| {
                d.saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(i32::MAX as u128) as i32
            });
            let mut all = vec![self.conn.fd()];
            all.extend_from_slice(fds);
            let ready = poll_fds(&all, left);
            if ready[1..].iter().any(|&r| r) {
                return ready[1..].to_vec();
            }
            if !ready[0] || deadline.is_some_and(|d| Instant::now() >= d) {
                self.drain();
                return vec![false; fds.len()];
            }
            self.drain();
        }
    }

    /// Read and handle everything the server has sent, without blocking.
    fn drain(&mut self) {
        while !self.dead && poll_fds(&[self.conn.fd()], 0)[0] {
            let mut b = [0u8; 32];
            if self.message(&mut b).is_some() {
                self.handle(&b);
            }
        }
    }

    /// Where the mouse is, relative to the window, even outside it. None if
    /// it is on another screen or the server didn't answer.
    pub fn pointer(&mut self) -> Option<(i32, i32)> {
        let win = self.win;
        self.req(38, 0, 2); // QueryPointer
        self.put32(win);
        let (b, _) = self.roundtrip()?;
        if b[1] == 0 {
            return None;
        }
        Some((get16(&b, 20) as i16 as i32, get16(&b, 22) as i16 as i32))
    }

    /// The connection's file descriptor, for your own poll(); check
    /// `pending` first.
    pub fn fd(&self) -> RawFd {
        self.conn.fd()
    }

    /// Is anything waiting that `event` would return (or a repaint it would
    /// do)? `sync` and other round trips read events off the socket into a
    /// queue, where poll() can't see them.
    pub fn pending(&self) -> bool {
        !self.queue.is_empty() || self.expose
    }
}

impl Drop for X11 {
    /// Send what is left; closing the connection removes the window.
    fn drop(&mut self) {
        self.flush();
    }
}
