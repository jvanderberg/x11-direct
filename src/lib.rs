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
//! | outline text | `font_ttf(file_bytes, face, px)`: anti-aliased text from a TrueType file on the client's disk, rasterized here and sent once (RENDER). It then works with `text_font`, `text`, `text_width` and the metrics like any font. `Font::FIXED` without RENDER: fall back to `font()` |
//! | smooth shapes | `aa_polygon`, `aa_polyline`, `aa_triangles` (with `polygon_triangles`, `polyline_triangles` and `rounded_points` to build batches), `blend_rect` (see-through): in the current colour at an opacity, anti-aliased. Each returns false without RENDER (`has_render()`), so draw the plain version |
//! | clipboard | `set_clipboard(text)`; `request_paste()`, and the text arrives as `Event::Paste` |
//! | cursor | `cursor(Cursor::Hand)` and the rest: smooth macOS-style hand and I-beam with RENDER, else the cursor font. `define_cursor` sets your own ARGB image |
//! | input | `event()` returns key down and up (a real release, unlike a terminal; a held key repeats as up/down pairs), mouse buttons and motion, resize (draw again) and close. `key` is ASCII (Latin-1) for printable keys, else a [`key`] constant; `mods` holds [`MOD_SHIFT`], [`MOD_CTRL`], [`MOD_ALT`]. `pointer()` asks where the mouse is, even outside the window |
//! | window | `background(pixel)` before `window()`, for what the window shows where nothing is drawn yet; `min_size(w, h)` after it |
//! | timing | `sync()` waits until the server has drawn everything sent (call it after `present()` so a slow link can't queue up old frames); `fence()` after `present()` and `fence_wait()` before the next one instead, to draw the next frame while the last one travels; `wait(ms)`, `wait_with(fds, ms)` (also wakes for other files), `flush()`, `fd()`, `pending()` |
//!
//! Needs a TrueColor display (any current server, XQuartz and Xvfb
//! included) that lets this machine connect: on a remote machine with the
//! screen, allow network clients (XQuartz: Settings > Security) and run
//! `xhost +THIS-ADDRESS`. If `$XAUTHORITY` or `~/.Xauthority` has an
//! MIT-MAGIC-COOKIE-1 for the display, it is sent, so `ssh -X`/`-Y` and
//! servers started with `-auth` need no `xhost`. (An untrusted `ssh -X`
//! client is not shown RENDER: use `ssh -Y` for smooth text.)
//!
//! Drawing calls never fail: if the connection is lost, they do nothing and
//! the next [`X11::event`] returns [`Event::Close`].
#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The clipboard's text, after [`X11::request_paste`].
    Paste(String),
}

/// Mouse pointer shapes, from the server's cursor font.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cursor {
    /// whatever the window manager shows (the parent's cursor)
    Default,
    /// a pointing hand: something to click
    Hand,
    /// an I-beam: text to type in
    Text,
    Wait,
    ResizeH,
    ResizeV,
}

/// The atoms the clipboard needs, interned on first use.
#[derive(Clone, Copy, Default)]
struct ClipAtoms {
    clipboard: u32,
    utf8: u32,
    targets: u32,
    text: u32,
    xsel: u32,
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
    glyphset: u32,    // a RENDER glyph set (an outline font), or 0: a core font
}

/// The RENDER extension, once found: its opcode, the picture formats in
/// use, and the pictures made so far.
struct Render {
    major: u32,
    a8: u32,
    rgb: u32,
    argb: u32,                // 32-bit with alpha, for cursor images (0: none)
    solid: HashMap<u64, u32>, // colour (pixel, alpha) -> solid fill picture
    pict: HashMap<u32, u32>,  // drawable -> picture
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
    render: Option<Render>,
    render_tried: bool,
    clip_atoms: Option<ClipAtoms>,
    clip_text: Option<String>,
    paste_prop: Option<(u32, u32)>, // a pasted property to read: (property, type wanted)
    cursor_font: u32,
    cursors: HashMap<Cursor, u32>,
    cursor_now: Cursor,
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

/// A convex polygon as triangles (a fan), for `aa_triangles`: collect
/// many shapes of one colour and send them in one request.
pub fn polygon_triangles(pts: &[[f32; 2]]) -> Vec<[f32; 6]> {
    if pts.len() < 3 {
        return Vec::new();
    }
    let p0 = pts[0];
    pts[1..]
        .windows(2)
        .map(|w| [p0[0], p0[1], w[0][0], w[0][1], w[1][0], w[1][1]])
        .collect()
}

/// The outline of a rounded rectangle, for `aa_polygon`: a menu, a
/// dialog, a button or its shadow, with smooth corners.
pub fn rounded_points(x: f32, y: f32, w: f32, h: f32, rad: f32) -> Vec<[f32; 2]> {
    let rad = rad.min(w / 2.0).min(h / 2.0).max(0.0);
    let corners = [
        (x + w - rad, y + rad, -90.0f32),
        (x + w - rad, y + h - rad, 0.0),
        (x + rad, y + h - rad, 90.0),
        (x + rad, y + rad, 180.0),
    ];
    let mut v = Vec::with_capacity(28);
    for (cx, cy, a0) in corners {
        for i in 0..=6 {
            let a = (a0 + 15.0 * i as f32).to_radians();
            v.push([cx + rad * a.cos(), cy + rad * a.sin()]);
        }
    }
    v
}

/// Connected lines `width` pixels wide as triangles, for `aa_triangles`.
pub fn polyline_triangles(pts: &[[f32; 2]], width: f32) -> Vec<[f32; 6]> {
    let h = width / 2.0;
    let mut tris = Vec::with_capacity(pts.len() * 2);
    for w in pts.windows(2) {
        let ([x0, y0], [x1, y1]) = (w[0], w[1]);
        let len = (x1 - x0).hypot(y1 - y0);
        if len < 1e-3 {
            continue;
        }
        // half a width further at each end, so joins have no gaps
        let (dx, dy) = ((x1 - x0) / len * h, (y1 - y0) / len * h);
        let (ax, ay, bx, by) = (x0 - dx, y0 - dy, x1 + dx, y1 + dy);
        let (nx, ny) = (-dy, dx);
        tris.push([ax + nx, ay + ny, bx + nx, by + ny, bx - nx, by - ny]);
        tris.push([ax + nx, ay + ny, bx - nx, by - ny, ax - nx, ay - ny]);
    }
    tris
}

extern "C" {
    fn gethostname(name: *mut std::os::raw::c_char, len: usize) -> std::os::raw::c_int;
}

fn hostname() -> String {
    let mut b = [0u8; 256];
    // SAFETY: b is 256 bytes long; gethostname NUL-terminates within it.
    if unsafe { gethostname(b.as_mut_ptr().cast(), b.len()) } != 0 {
        return String::new();
    }
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The authorization for display d from the Xauthority file ($XAUTHORITY,
/// else ~/.Xauthority), as xlib finds it: an MIT-MAGIC-COOKIE-1 entry for
/// that display number, for this machine (a local or localhost display,
/// which is what `ssh -X` makes) or for the server's address. Returns the
/// protocol name and its data.
fn xauth_cookie(d: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let path = std::env::var_os("XAUTHORITY")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".Xauthority"))
        })?;
    let file = std::fs::read(path).ok()?;
    let colon = d.rfind(':')?;
    let host = if d.starts_with('/') { "" } else { &d[..colon] };
    let num = d[colon + 1..].split('.').next().unwrap_or("0");
    let local = matches!(host, "" | "unix" | "localhost" | "127.0.0.1" | "::1");
    let me = hostname();
    let addrs: Vec<std::net::IpAddr> = if local {
        Vec::new()
    } else {
        (host, 0)
            .to_socket_addrs()
            .map(|a| a.map(|s| s.ip()).collect())
            .unwrap_or_default()
    };
    // entries: family (2), then address, number, name, data: each a
    // 2-byte length and the bytes; all big-endian
    let mut p = 0;
    let field = |p: &mut usize| -> Option<Vec<u8>> {
        let n = u16::from_be_bytes([*file.get(*p)?, *file.get(*p + 1)?]) as usize;
        let v = file.get(*p + 2..*p + 2 + n)?.to_vec();
        *p += 2 + n;
        Some(v)
    };
    let mut wild = None;
    while p + 2 <= file.len() {
        let family = u16::from_be_bytes([file[p], file[p + 1]]);
        p += 2;
        let (Some(addr), Some(n), Some(name), Some(data)) =
            (field(&mut p), field(&mut p), field(&mut p), field(&mut p))
        else {
            break;
        };
        if name != b"MIT-MAGIC-COOKIE-1" || n != num.as_bytes() {
            continue;
        }
        let hit = match family {
            256 => (local && addr == me.as_bytes()) || addr == host.as_bytes(), // FamilyLocal
            0 if addr.len() == 4 => addrs.contains(&std::net::IpAddr::from([
                addr[0], addr[1], addr[2], addr[3],
            ])),
            6 if addr.len() == 16 => {
                let a: [u8; 16] = addr.as_slice().try_into().ok()?;
                addrs.contains(&std::net::IpAddr::from(a))
            }
            0xffff => {
                wild.get_or_insert((name.clone(), data.clone()));
                false
            }
            _ => false,
        };
        if hit {
            return Some((name, data));
        }
    }
    wild
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
        self.forget_picture(old);
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
            // SelectionClear: another program owns the clipboard now
            29 => {
                let sel = get32(b, 12);
                if self.clip_atoms.is_some_and(|a| a.clipboard == sel) {
                    self.clip_text = None;
                }
            }
            // SelectionRequest: someone is pasting what we copied
            30 => self.answer_selection(b),
            // SelectionNotify: the clipboard's owner answered our request
            31 => {
                let (target, prop) = (get32(b, 16), get32(b, 20));
                let a = self.clip_atoms.unwrap_or_default();
                if prop != 0 {
                    self.paste_prop = Some((prop, target));
                } else if target == a.utf8 {
                    self.convert_selection(31); // no UTF-8: try STRING (Latin-1)
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
            render: None,
            render_tried: false,
            clip_atoms: None,
            clip_text: None,
            paste_prop: None,
            cursor_font: 0,
            cursors: HashMap::new(),
            cursor_now: Cursor::Default,
        };
        // little-endian, protocol 11.0, and the display's cookie if the
        // Xauthority file has one (ssh -X, or a server started with -auth)
        let (auth_name, auth_data) = xauth_cookie(&d).unwrap_or_default();
        x.out.extend_from_slice(&[b'l', 0, 11, 0, 0, 0]);
        x.out
            .extend_from_slice(&(auth_name.len() as u16).to_le_bytes());
        x.out
            .extend_from_slice(&(auth_data.len() as u16).to_le_bytes());
        x.out.extend_from_slice(&[0, 0]);
        x.put_padded(&auth_name);
        x.put_padded(&auth_data);
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
                "x11: {d} refused the connection: {}\nx11: on that machine, try: xhost +ADDRESS-OF-THIS-ONE (or xhost +local: for a local server); with ssh -X, check that xauth list shows this display",
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
        if self.fonts[self.cur_font].glyphset != 0 {
            self.glyph_text(x, y, &bytes);
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
            if self.fonts[f].glyphset == 0 {
                self.change_gc(0x4000, id);
            }
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
        if self.fonts[self.cur_font].glyphset != 0 {
            return self.text(x, y, s); // outline fonts here hold Latin-1 only
        }
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

    // ---- outline fonts, through the RENDER extension ----
    // Core fonts are bitmaps, drawn without anti-aliasing. font_ttf() reads
    // a TrueType font, renders its Latin-1 characters here into 8-bit
    // coverage images and stores them on the server in a glyph set, once.
    // text() then draws with them (CompositeGlyphs8: about the same bytes
    // as core text), blended in the current colour.

    /// Find RENDER (0.10 or later, for solid fills) and the picture formats
    /// for glyphs (A8) and for this display's pixels.
    fn render_init(&mut self) -> Option<Render> {
        let name = b"RENDER";
        self.req(98, 0, 2 + (name.len() as u32).div_ceil(4)); // QueryExtension
        self.put16(name.len() as u32);
        self.put16(0);
        self.put_padded(name);
        let (b, _) = self.roundtrip()?;
        if b[8] == 0 {
            return None;
        }
        let major = b[9] as u32;
        self.req(major, 0, 3); // RenderQueryVersion
        self.put32(0);
        self.put32(11);
        let (b, _) = self.roundtrip()?;
        if get32(&b, 8) == 0 && get32(&b, 12) < 10 {
            return None;
        }
        self.req(major, 1, 1); // RenderQueryPictFormats
        let (b, x) = self.roundtrip()?;
        let n = get32(&b, 8) as usize;
        let (mut a8, mut rgb, mut argb) = (0, 0, 0);
        for i in 0..n {
            let f = 28 * i;
            if f + 28 > x.len() {
                break;
            }
            let (id, kind, depth) = (get32(&x, f), x[f + 4], x[f + 5]);
            let m = |k: usize| (get16(&x, f + 8 + 4 * k), get16(&x, f + 10 + 4 * k)); // shift, mask
            let (r, g, bl, a) = (m(0), m(1), m(2), m(3));
            if kind != 1 {
                continue; // not Direct
            }
            if depth == 8 && a.1 == 0xff && r.1 == 0 && g.1 == 0 && bl.1 == 0 {
                a8 = id;
            }
            if depth == 32
                && a == (24, 0xff)
                && r == (16, 0xff)
                && g == (8, 0xff)
                && bl == (0, 0xff)
            {
                argb = id;
            }
            let full = |(s, m): (u32, u32)| m << s;
            if depth == self.depth
                && full(r) == self.masks[0]
                && full(g) == self.masks[1]
                && full(bl) == self.masks[2]
            {
                rgb = id;
            }
        }
        (a8 != 0 && rgb != 0).then(|| Render {
            major,
            a8,
            rgb,
            argb,
            solid: HashMap::new(),
            pict: HashMap::new(),
        })
    }

    fn render_ready(&mut self) -> bool {
        if !self.render_tried {
            self.render_tried = true;
            self.render = self.render_init();
        }
        self.render.is_some()
    }

    /// Whether the server has RENDER, so `font_ttf` can work.
    pub fn has_render(&mut self) -> bool {
        self.render_ready()
    }

    /// Free the picture made for drawable d, if any (it's going away).
    fn forget_picture(&mut self, d: u32) {
        let Some(p) = self.render.as_mut().and_then(|r| r.pict.remove(&d)) else {
            return;
        };
        let major = self.render.as_ref().map_or(0, |r| r.major);
        self.req(major, 7, 2); // RenderFreePicture
        self.put32(p);
    }

    /// The picture for drawing on drawable d.
    fn picture(&mut self, d: u32) -> u32 {
        let Some(r) = &self.render else { return 0 };
        if let Some(&p) = r.pict.get(&d) {
            return p;
        }
        let (major, fmt) = (r.major, r.rgb);
        let p = self.new_id();
        self.req(major, 4, 5); // RenderCreatePicture
        self.put32(p);
        self.put32(d);
        self.put32(fmt);
        self.put32(0); // no attributes
        if let Some(r) = self.render.as_mut() {
            r.pict.insert(d, p);
        }
        p
    }

    /// A picture that is the colour `pixel` everywhere, opaque.
    fn solid(&mut self, pixel: u32) -> u32 {
        self.solid_alpha(pixel, 255)
    }

    /// The same at an opacity.
    fn solid_alpha(&mut self, pixel: u32, alpha: u8) -> u32 {
        let key = u64::from(pixel) | u64::from(alpha) << 32;
        let Some(r) = &self.render else { return 0 };
        if let Some(&p) = r.solid.get(&key) {
            return p;
        }
        let major = r.major;
        let c = self.premultiplied(pixel, alpha);
        let p = self.new_id();
        self.req(major, 33, 4); // RenderCreateSolidFill
        self.put32(p);
        for v in c {
            self.put16(v);
        }
        if let Some(r) = self.render.as_mut() {
            r.solid.insert(key, p);
        }
        p
    }

    /// An outline font from the bytes of a TrueType file (.ttf, or a .ttc
    /// collection: the face whose full name is `name`, such as "Helvetica
    /// Neue Bold", else the first), `px` pixels to the em. Text in it is
    /// anti-aliased. Font::FIXED if the server has no RENDER or the file
    /// can't be read.
    pub fn font_ttf(&mut self, data: &[u8], name: &str, px: f32) -> Font {
        if self.fonts.len() >= MAX_FONTS || !self.render_ready() {
            return Font::FIXED;
        }
        let Some(face) = ttf::Face::parse(data, name) else {
            return Font::FIXED;
        };
        let Some(r) = &self.render else {
            return Font::FIXED;
        };
        let (major, a8) = (r.major, r.a8);
        let scale = px / face.upem;
        let gs = self.new_id();
        self.req(major, 17, 3); // RenderCreateGlyphSet
        self.put32(gs);
        self.put32(a8);

        let qmark = face.glyph_index('?' as u32);
        let mut widths = vec![0i16; 256];
        // glyph id = the Latin-1 code; send them in batches of a few KB
        let mut batch: Vec<(u32, ttf::Glyph, i32)> = Vec::new();
        let mut size = 0;
        for c in 32u32..256 {
            if (127..160).contains(&c) {
                continue; // control characters
            }
            let mut g = face.glyph_index(c);
            if g == 0 {
                g = qmark;
            }
            let adv = (face.advance(g) * scale).round() as i32;
            widths[c as usize] = adv as i16;
            let glyph = ttf::render(&face, g, scale, 0.82);
            size += 12 + glyph.h * glyph.w.next_multiple_of(4) + 4;
            batch.push((c, glyph, adv));
            if size > 12_000 {
                self.add_glyphs(major, gs, &batch);
                batch.clear();
                size = 0;
            }
        }
        if !batch.is_empty() {
            self.add_glyphs(major, gs, &batch);
        }
        self.fonts.push(FontInfo {
            id: 0,
            ascent: (face.ascent * scale).ceil() as i32,
            descent: (-face.descent * scale).ceil() as i32,
            minc: 0,
            maxc: 255,
            defw: widths[b'?' as usize] as i32,
            widths,
            glyphset: gs,
        });
        Font(self.fonts.len() - 1)
    }

    fn add_glyphs(&mut self, major: u32, gs: u32, glyphs: &[(u32, ttf::Glyph, i32)]) {
        let n = glyphs.len() as u32;
        let data: usize = glyphs
            .iter()
            .map(|(_, g, _)| g.h * g.w.next_multiple_of(4))
            .sum();
        self.req(major, 20, 3 + n + 3 * n + (data / 4) as u32); // RenderAddGlyphs
        self.put32(gs);
        self.put32(n);
        for (c, _, _) in glyphs {
            self.put32(*c);
        }
        for (_, g, adv) in glyphs {
            self.put16(g.w as u32);
            self.put16(g.h as u32);
            self.put16((-g.left) as u32); // where the pen is, in the image
            self.put16((-g.top) as u32);
            self.put16(*adv as u32);
            self.put16(0);
        }
        for (_, g, _) in glyphs {
            let stride = g.w.next_multiple_of(4);
            for row in g.alpha.chunks(g.w.max(1)).take(g.h) {
                self.out.extend_from_slice(row);
                let pad = stride - row.len();
                self.out.extend_from_slice(&[0u8; 3][..pad]);
            }
        }
    }

    /// Text in an outline font: CompositeGlyphs8, one item per 252 glyphs.
    fn glyph_text(&mut self, x: i32, y: i32, bytes: &[u8]) {
        let Some(major) = self.render.as_ref().map(|r| r.major) else {
            return;
        };
        let gs = self.fonts[self.cur_font].glyphset;
        let dst = self.dst();
        let dpict = self.picture(dst);
        let src = self.solid(self.color);
        let items: usize = bytes
            .chunks(252)
            .map(|c| (8 + c.len()).next_multiple_of(4))
            .sum();
        self.req(major, 23, 7 + (items / 4) as u32); // RenderCompositeGlyphs8
        self.put8(3); // PictOpOver
        self.put8(0);
        self.put16(0);
        self.put32(src);
        self.put32(dpict);
        self.put32(0); // no mask format: each glyph blended on its own
        self.put32(gs);
        self.put16(0);
        self.put16(0);
        for (i, chunk) in bytes.chunks(252).enumerate() {
            let mut item = vec![chunk.len() as u8, 0, 0, 0];
            let (dx, dy) = if i == 0 { (x, y) } else { (0, 0) };
            item.extend_from_slice(&(dx as i16).to_le_bytes());
            item.extend_from_slice(&(dy as i16).to_le_bytes());
            item.extend_from_slice(chunk);
            self.put_padded(&item);
        }
    }

    // ---- the clipboard ----
    // Copying makes this window the owner of CLIPBOARD (and PRIMARY, for
    // middle-click paste); other programs then ask it for the text, and
    // handle() answers. Pasting asks the owner to put the text in a
    // property of this window, then reads it. XQuartz copies CLIPBOARD to
    // the Mac's pasteboard and back.

    fn clip_atoms(&mut self) -> ClipAtoms {
        if let Some(a) = self.clip_atoms {
            return a;
        }
        let a = ClipAtoms {
            clipboard: self.atom("CLIPBOARD"),
            utf8: self.atom("UTF8_STRING"),
            targets: self.atom("TARGETS"),
            text: self.atom("TEXT"),
            xsel: self.atom("X11_DIRECT_PASTE"),
        };
        self.clip_atoms = Some(a);
        a
    }

    /// Put text on the clipboard (and the primary selection).
    pub fn set_clipboard(&mut self, text: &str) {
        let a = self.clip_atoms();
        self.clip_text = Some(text.to_string());
        let win = self.win;
        for sel in [a.clipboard, 1] {
            self.req(22, 0, 4); // SetSelectionOwner (1 is PRIMARY)
            self.put32(win);
            self.put32(sel);
            self.put32(0); // CurrentTime
        }
        self.flush();
    }

    /// Ask for the clipboard's text; it arrives as Event::Paste.
    pub fn request_paste(&mut self) {
        let a = self.clip_atoms();
        self.convert_selection(a.utf8);
        self.flush();
    }

    fn convert_selection(&mut self, target: u32) {
        let a = self.clip_atoms.unwrap_or_default();
        let win = self.win;
        self.req(24, 0, 6); // ConvertSelection
        self.put32(win);
        self.put32(a.clipboard);
        self.put32(target);
        self.put32(a.xsel);
        self.put32(0);
    }

    fn read_paste(&mut self, prop: u32) -> Option<String> {
        let win = self.win;
        self.req(20, 1, 6); // GetProperty, deleting it
        self.put32(win);
        self.put32(prop);
        self.put32(0); // any type
        self.put32(0);
        self.put32(1 << 18); // up to 1 MB
        let (b, x) = self.roundtrip()?;
        let (format, n) = (b[1] as usize, get32(&b, 16) as usize);
        let bytes = x.get(..n * format / 8)?;
        if format != 8 {
            return None; // INCR (too big) or not text
        }
        Some(if get32(&b, 8) == 31 {
            bytes.iter().map(|&c| c as char).collect() // STRING is Latin-1
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        })
    }

    /// Another program wants what we copied: put it in the property it
    /// named, in the form it asked for, and tell it (SelectionNotify).
    fn answer_selection(&mut self, b: &[u8; 32]) {
        let (time, requestor, sel, target, prop) = (
            get32(b, 4),
            get32(b, 12),
            get32(b, 16),
            get32(b, 20),
            get32(b, 24),
        );
        let a = self.clip_atoms.unwrap_or_default();
        let prop = if prop == 0 { target } else { prop };
        let ok = match self.clip_text.clone() {
            Some(_) if target == a.targets => {
                let list = [a.targets, a.utf8, 31, a.text];
                self.change_property(requestor, prop, 4, 32, &list.map(u32::to_le_bytes).concat());
                true
            }
            Some(t) if target == a.utf8 || target == a.text => {
                self.change_property(requestor, prop, a.utf8, 8, t.as_bytes());
                true
            }
            Some(t) if target == 31 => {
                let latin1: Vec<u8> = t
                    .chars()
                    .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
                    .collect();
                self.change_property(requestor, prop, 31, 8, &latin1);
                true
            }
            _ => false,
        };
        self.req(25, 0, 11); // SendEvent: SelectionNotify to the requestor
        self.put32(requestor);
        self.put32(0);
        let mut ev = [0u8; 32];
        ev[0] = 31;
        ev[4..8].copy_from_slice(&time.to_le_bytes());
        ev[8..12].copy_from_slice(&requestor.to_le_bytes());
        ev[12..16].copy_from_slice(&sel.to_le_bytes());
        ev[16..20].copy_from_slice(&target.to_le_bytes());
        ev[20..24].copy_from_slice(&(if ok { prop } else { 0 }).to_le_bytes());
        self.out.extend_from_slice(&ev);
        self.flush();
    }

    fn change_property(&mut self, win: u32, prop: u32, kind: u32, format: u32, data: &[u8]) {
        let units = data.len() as u32 / (format / 8);
        self.req(18, 0, 6 + (data.len() as u32).div_ceil(4)); // ChangeProperty, Replace
        self.put32(win);
        self.put32(prop);
        self.put32(kind);
        self.put32(format);
        self.put32(units);
        self.put_padded(data);
    }

    // ---- the mouse pointer ----

    /// The pointer's shape over the window. Sends nothing if unchanged.
    pub fn cursor(&mut self, c: Cursor) {
        if c == self.cursor_now || self.win == 0 {
            return;
        }
        // the smooth hand and I-beam, made once, where the server has RENDER
        if !self.cursors.contains_key(&c) {
            let art = match c {
                Cursor::Hand => Some(cursor_art::Art::Hand),
                Cursor::Text => Some(cursor_art::Art::IBeam),
                _ => None,
            };
            if let Some(art) = art.filter(|_| self.render_ready()) {
                let (w, h, px, hot) = cursor_art::image(art);
                self.define_cursor(c, w, h, &px, hot);
            }
        }
        self.cursor_now = c;
        // glyphs of the standard cursor font (cursorfont.h)
        let glyph = match c {
            Cursor::Default => None,
            Cursor::Hand => Some(60),
            Cursor::Text => Some(152),
            Cursor::Wait => Some(150),
            Cursor::ResizeH => Some(108),
            Cursor::ResizeV => Some(116),
        };
        let id = match glyph {
            None => 0, // None: the parent's
            Some(g) => match self.cursors.get(&c) {
                Some(&id) => id,
                None => {
                    if self.cursor_font == 0 {
                        self.cursor_font = self.new_id();
                        let name = b"cursor";
                        let f = self.cursor_font;
                        self.req(45, 0, 3 + (name.len() as u32).div_ceil(4)); // OpenFont
                        self.put32(f);
                        self.put16(name.len() as u32);
                        self.put16(0);
                        self.put_padded(name);
                    }
                    let (id, f) = (self.new_id(), self.cursor_font);
                    self.req(94, 0, 8); // CreateGlyphCursor: the glyph and its mask
                    self.put32(id);
                    self.put32(f);
                    self.put32(f);
                    self.put16(g);
                    self.put16(g + 1);
                    for v in [0, 0, 0, 0xffff, 0xffff, 0xffff] {
                        self.put16(v); // black, outlined in white
                    }
                    self.cursors.insert(c, id);
                    id
                }
            },
        };
        let win = self.win;
        self.req(2, 0, 4); // ChangeWindowAttributes: cursor
        self.put32(win);
        self.put32(0x4000);
        self.put32(id);
        self.flush();
    }

    /// Use a full-colour image as cursor c from now on: w x h pixels of
    /// premultiplied ARGB (0xAARRGGBB), pointing at `hot`. False (and the
    /// cursor font's shape stays) if the server has no RENDER.
    pub fn define_cursor(
        &mut self,
        c: Cursor,
        w: i32,
        h: i32,
        argb: &[u32],
        hot: (i32, i32),
    ) -> bool {
        if !self.render_ready()
            || self.win == 0
            || argb.len() < (w * h) as usize
            || w <= 0
            || h <= 0
        {
            return false;
        }
        let Some((major, fmt)) = self.render.as_ref().map(|r| (r.major, r.argb)) else {
            return false;
        };
        if fmt == 0 {
            return false;
        }
        // the image in a 32-bit pixmap, as a picture, made into a cursor
        let (pix, gc, pict, cid, win) = (
            self.new_id(),
            self.new_id(),
            self.new_id(),
            self.new_id(),
            self.win,
        );
        self.req(53, 32, 4); // CreatePixmap, depth 32
        self.put32(pix);
        self.put32(win);
        self.put16(w as u32);
        self.put16(h as u32);
        self.req(55, 0, 4); // CreateGC for it
        self.put32(gc);
        self.put32(pix);
        self.put32(0);
        self.req(72, 2, 6 + (w * h) as u32); // PutImage, ZPixmap
        self.put32(pix);
        self.put32(gc);
        self.put16(w as u32);
        self.put16(h as u32);
        self.put16(0);
        self.put16(0);
        self.put8(0); // left pad
        self.put8(32); // depth
        self.put16(0);
        let msb = self.image_msb;
        for &p in &argb[..(w * h) as usize] {
            self.out.extend_from_slice(&if msb {
                p.to_be_bytes()
            } else {
                p.to_le_bytes()
            });
        }
        self.req(major, 4, 5); // RenderCreatePicture
        self.put32(pict);
        self.put32(pix);
        self.put32(fmt);
        self.put32(0);
        self.req(major, 27, 4); // RenderCreateCursor
        self.put32(cid);
        self.put32(pict);
        self.put16(hot.0 as u32);
        self.put16(hot.1 as u32);
        self.req(major, 7, 2); // RenderFreePicture
        self.put32(pict);
        self.req(60, 0, 2); // FreeGC
        self.put32(gc);
        self.req(54, 0, 2); // FreePixmap
        self.put32(pix);
        if let Some(old) = self.cursors.insert(c, cid) {
            self.req(95, 0, 2); // FreeCursor: the window keeps it while it shows it
            self.put32(old);
        }
        if self.cursor_now == c {
            self.cursor_now = Cursor::Default;
            self.cursor(c); // the new look, now
        }
        self.flush();
        true
    }

    // ---- anti-aliased shapes and translucency (RENDER) ----
    // These draw in the current colour at an opacity (0-255), with smooth
    // edges, onto the current target. Each returns false, drawing nothing,
    // when the server has no RENDER, so a caller can fall back to the
    // plain calls.

    /// Triangles, each [x0, y0, x1, y1, x2, y2], in pixels (fractions allowed).
    pub fn aa_triangles(&mut self, tris: &[[f32; 6]], alpha: u8) -> bool {
        if !self.render_ready() {
            return false;
        }
        if tris.is_empty() {
            return true;
        }
        let Some((major, a8)) = self.render.as_ref().map(|r| (r.major, r.a8)) else {
            return false;
        };
        let dst = self.dst();
        let dpict = self.picture(dst);
        let src = self.solid_alpha(self.color, alpha);
        for chunk in tris.chunks(500) {
            self.req(major, 11, 6 + 6 * chunk.len() as u32); // RenderTriangles
            self.put8(3); // Over
            self.put8(0);
            self.put16(0);
            self.put32(src);
            self.put32(dpict);
            self.put32(a8); // a mask: smooth edges, and overlaps don't add up
            self.put16(0);
            self.put16(0);
            for t in chunk {
                for v in t {
                    self.put32((v * 65536.0) as i32 as u32); // 16.16 fixed point
                }
            }
        }
        true
    }

    /// A convex polygon.
    pub fn aa_polygon(&mut self, pts: &[[f32; 2]], alpha: u8) -> bool {
        self.aa_triangles(&polygon_triangles(pts), alpha)
    }

    /// Connected lines `width` pixels wide.
    pub fn aa_polyline(&mut self, pts: &[[f32; 2]], width: f32, alpha: u8) -> bool {
        self.aa_triangles(&polyline_triangles(pts, width), alpha)
    }

    /// A rectangle that lets what's under it show through.
    pub fn blend_rect(&mut self, x: i32, y: i32, w: i32, h: i32, alpha: u8) -> bool {
        if !self.render_ready() {
            return false;
        }
        let Some(major) = self.render.as_ref().map(|r| r.major) else {
            return false;
        };
        let dst = self.dst();
        let dpict = self.picture(dst);
        let c = self.premultiplied(self.color, alpha);
        self.req(major, 26, 5 + 2); // RenderFillRectangles
        self.put8(3); // Over
        self.put8(0);
        self.put16(0);
        self.put32(dpict);
        for v in c {
            self.put16(v);
        }
        self.put16(x as u32);
        self.put16(y as u32);
        self.put16(w.max(0) as u32);
        self.put16(h.max(0) as u32);
        true
    }

    /// A pixel as a RENDER colour (16 bits each, premultiplied by alpha).
    fn premultiplied(&self, pixel: u32, alpha: u8) -> [u32; 4] {
        let a = u32::from(alpha) * 0x101;
        let mut out = [0, 0, 0, a];
        for (i, &m) in self.masks.iter().enumerate() {
            let shift = m.trailing_zeros().min(31);
            let bits = (m >> shift).trailing_ones();
            let v = (pixel & m) >> shift;
            let max = (1u32 << bits) - 1;
            let full = (v * 0xffff).checked_div(max).unwrap_or(0);
            out[i] = full * a / 0xffff;
        }
        out
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
        self.forget_picture(c.0);
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
        // a paste arrived: read it here, not in handle(), which may run
        // inside another round trip
        if let Some((prop, _)) = self.paste_prop.take() {
            if let Some(text) = self.read_paste(prop) {
                self.push(Event::Paste(text));
            }
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
        !self.queue.is_empty() || self.expose || self.paste_prop.is_some()
    }
}

impl Drop for X11 {
    /// Send what is left; closing the connection removes the window.
    fn drop(&mut self) {
        self.flush();
    }
}

// ---- TrueType fonts, for font_ttf ----

mod ttf {
    //! A small TrueType reader and anti-aliased rasterizer: enough to turn the
    //! Latin-1 characters of a .ttf or .ttc font (glyf outlines) into 8-bit
    //! coverage images for the RENDER extension. No hinting, no kerning, no
    //! CFF (.otf) outlines.

    fn u16_at(d: &[u8], o: usize) -> Option<u16> {
        Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
    }
    fn i16_at(d: &[u8], o: usize) -> Option<i16> {
        u16_at(d, o).map(|v| v as i16)
    }
    fn u32_at(d: &[u8], o: usize) -> Option<u32> {
        Some(u32::from_be_bytes([
            *d.get(o)?,
            *d.get(o + 1)?,
            *d.get(o + 2)?,
            *d.get(o + 3)?,
        ]))
    }

    pub(crate) struct Face<'a> {
        d: &'a [u8],
        glyf: usize,
        loca: usize,
        loca_long: bool,
        hmtx: usize,
        num_hmetrics: u16,
        num_glyphs: u16,
        cmap: (usize, u16), // the subtable and its format
        pub upem: f32,
        pub ascent: f32,
        pub descent: f32,
    }

    /// The table directory of the face at `off`: tag -> offset.
    fn tables(d: &[u8], off: usize) -> Option<Vec<([u8; 4], usize)>> {
        let n = u16_at(d, off + 4)? as usize;
        (0..n)
            .map(|i| {
                let r = off + 12 + 16 * i;
                let tag = [*d.get(r)?, *d.get(r + 1)?, *d.get(r + 2)?, *d.get(r + 3)?];
                Some((tag, u32_at(d, r + 8)? as usize))
            })
            .collect()
    }

    /// The face's full names (name ID 4), in any encoding we can read.
    fn full_names(d: &[u8], name: usize) -> Vec<String> {
        let mut out = Vec::new();
        let (Some(count), Some(so)) = (u16_at(d, name + 2), u16_at(d, name + 4)) else {
            return out;
        };
        for i in 0..count as usize {
            let r = name + 6 + 12 * i;
            let (Some(p), Some(id), Some(len), Some(off)) = (
                u16_at(d, r),
                u16_at(d, r + 6),
                u16_at(d, r + 8),
                u16_at(d, r + 10),
            ) else {
                continue;
            };
            let s = name + so as usize + off as usize;
            let Some(b) = d.get(s..s + len as usize) else {
                continue;
            };
            if id != 4 {
                continue;
            }
            let text = if p == 1 {
                b.iter().map(|&c| c as char).collect() // Mac Roman: ASCII names
            } else {
                let u: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&u)
            };
            out.push(text);
        }
        out
    }

    impl<'a> Face<'a> {
        /// The face named `name` (its full name, such as "Helvetica Neue
        /// Bold") in a .ttf or .ttc, or the first face if none is.
        pub(crate) fn parse(d: &'a [u8], name: &str) -> Option<Face<'a>> {
            let offsets: Vec<usize> = if d.get(0..4)? == b"ttcf" {
                let n = u32_at(d, 8)? as usize;
                (0..n.min(64))
                    .filter_map(|i| u32_at(d, 12 + 4 * i).map(|v| v as usize))
                    .collect()
            } else {
                vec![0]
            };
            let pick = offsets
                .iter()
                .copied()
                .find(|&off| {
                    tables(d, off)
                        .and_then(|t| t.iter().find(|(tag, _)| tag == b"name").map(|e| e.1))
                        .is_some_and(|n| {
                            full_names(d, n)
                                .iter()
                                .any(|f| f.eq_ignore_ascii_case(name))
                        })
                })
                .unwrap_or(offsets[0]);
            let t = tables(d, pick)?;
            let find = |tag: &[u8; 4]| t.iter().find(|(g, _)| g == tag).map(|e| e.1);
            let (head, hhea, maxp) = (find(b"head")?, find(b"hhea")?, find(b"maxp")?);
            let cmap = find(b"cmap")?;
            // the best Unicode subtable: format 12, else format 4
            let n = u16_at(d, cmap + 2)? as usize;
            let mut best: Option<(usize, u16)> = None;
            for i in 0..n {
                let r = cmap + 4 + 8 * i;
                let (p, e) = (u16_at(d, r)?, u16_at(d, r + 2)?);
                let sub = cmap + u32_at(d, r + 4)? as usize;
                let fmt = u16_at(d, sub)?;
                let unicode = p == 0 || (p == 3 && (e == 1 || e == 10));
                if unicode && (fmt == 12 || (fmt == 4 && best.is_none())) {
                    best = Some((sub, fmt));
                }
            }
            Some(Face {
                d,
                glyf: find(b"glyf")?,
                loca: find(b"loca")?,
                loca_long: i16_at(d, head + 50)? != 0,
                hmtx: find(b"hmtx")?,
                num_hmetrics: u16_at(d, hhea + 34)?,
                num_glyphs: u16_at(d, maxp + 4)?,
                cmap: best?,
                upem: f32::from(u16_at(d, head + 18)?.max(1)),
                ascent: f32::from(i16_at(d, hhea + 4)?),
                descent: f32::from(i16_at(d, hhea + 6)?),
            })
        }

        /// The glyph for character c (0: none).
        pub(crate) fn glyph_index(&self, c: u32) -> u16 {
            let (sub, fmt) = self.cmap;
            let d = self.d;
            let g = || -> Option<u16> {
                if fmt == 12 {
                    let n = u32_at(d, sub + 12)? as usize;
                    for i in 0..n {
                        let r = sub + 16 + 12 * i;
                        let (s, e) = (u32_at(d, r)?, u32_at(d, r + 4)?);
                        if (s..=e).contains(&c) {
                            return Some((u32_at(d, r + 8)? + c - s) as u16);
                        }
                    }
                    return Some(0);
                }
                if c > 0xffff {
                    return Some(0);
                }
                let seg2 = u16_at(d, sub + 6)? as usize;
                let ends = sub + 14;
                let starts = ends + seg2 + 2;
                let deltas = starts + seg2;
                let ranges = deltas + seg2;
                for i in (0..seg2).step_by(2) {
                    if c > u32::from(u16_at(d, ends + i)?) {
                        continue;
                    }
                    let start = u32::from(u16_at(d, starts + i)?);
                    if c < start {
                        return Some(0);
                    }
                    let delta = u16_at(d, deltas + i)?;
                    let ro = u16_at(d, ranges + i)? as usize;
                    if ro == 0 {
                        return Some((c as u16).wrapping_add(delta));
                    }
                    let gi = u16_at(d, ranges + i + ro + 2 * (c - start) as usize)?;
                    return Some(if gi == 0 { 0 } else { gi.wrapping_add(delta) });
                }
                Some(0)
            };
            g().unwrap_or(0)
        }

        /// How far the pen moves after glyph g, in font units.
        pub(crate) fn advance(&self, g: u16) -> f32 {
            let i = g.min(self.num_hmetrics.saturating_sub(1)) as usize;
            f32::from(u16_at(self.d, self.hmtx + 4 * i).unwrap_or(0))
        }

        fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
            if g >= self.num_glyphs {
                return None;
            }
            let i = g as usize;
            let (a, b) = if self.loca_long {
                (
                    u32_at(self.d, self.loca + 4 * i)? as usize,
                    u32_at(self.d, self.loca + 4 * i + 4)? as usize,
                )
            } else {
                (
                    u16_at(self.d, self.loca + 2 * i)? as usize * 2,
                    u16_at(self.d, self.loca + 2 * i + 2)? as usize * 2,
                )
            };
            (b > a).then_some((self.glyf + a, self.glyf + b))
        }

        /// Glyph g's contours: points in font units, with on-curve flags.
        pub(crate) fn outline(&self, g: u16, depth: u32) -> Vec<Vec<(f32, f32, bool)>> {
            self.outline_inner(g, depth).unwrap_or_default()
        }

        fn outline_inner(&self, g: u16, depth: u32) -> Option<Vec<Vec<(f32, f32, bool)>>> {
            let d = self.d;
            let (s, _) = self.glyph_range(g)?;
            let nc = i16_at(d, s)?;
            if nc < 0 {
                return self.composite(s + 10, depth);
            }
            let nc = nc as usize;
            let mut ends = Vec::with_capacity(nc);
            for i in 0..nc {
                ends.push(u16_at(d, s + 10 + 2 * i)? as usize);
            }
            let npts = ends.last().map_or(0, |e| e + 1);
            let ilen = u16_at(d, s + 10 + 2 * nc)? as usize;
            let mut p = s + 12 + 2 * nc + ilen;
            let mut flags = Vec::with_capacity(npts);
            while flags.len() < npts {
                let f = *d.get(p)?;
                p += 1;
                flags.push(f);
                if f & 8 != 0 {
                    let n = *d.get(p)?;
                    p += 1;
                    for _ in 0..n {
                        flags.push(f);
                    }
                }
            }
            flags.truncate(npts);
            let mut coords = |short: u8, same: u8| -> Option<Vec<f32>> {
                let mut v = 0i32;
                let mut out = Vec::with_capacity(npts);
                for &f in &flags {
                    if f & short != 0 {
                        let b = i32::from(*d.get(p)?);
                        p += 1;
                        v += if f & same != 0 { b } else { -b };
                    } else if f & same == 0 {
                        v += i32::from(i16_at(d, p)?);
                        p += 2;
                    }
                    out.push(v as f32);
                }
                Some(out)
            };
            let xs = coords(2, 16)?;
            let ys = coords(4, 32)?;
            let mut out = Vec::with_capacity(nc);
            let mut start = 0;
            for &e in &ends {
                out.push(
                    (start..=e.min(npts - 1))
                        .map(|i| (xs[i], ys[i], flags[i] & 1 != 0))
                        .collect(),
                );
                start = e + 1;
            }
            Some(out)
        }

        fn composite(&self, mut p: usize, depth: u32) -> Option<Vec<Vec<(f32, f32, bool)>>> {
            let d = self.d;
            let mut out = Vec::new();
            if depth > 6 {
                return Some(out);
            }
            loop {
                let flags = u16_at(d, p)?;
                let g = u16_at(d, p + 2)?;
                p += 4;
                let (dx, dy) = if flags & 1 != 0 {
                    let v = (i16_at(d, p)?, i16_at(d, p + 2)?);
                    p += 4;
                    (f32::from(v.0), f32::from(v.1))
                } else {
                    let v = (*d.get(p)? as i8, *d.get(p + 1)? as i8);
                    p += 2;
                    (f32::from(v.0), f32::from(v.1))
                };
                let f2 = |o: usize| i16_at(d, o).map(|v| f32::from(v) / 16384.0);
                let (mut a, mut b, mut c, mut e) = (1.0, 0.0, 0.0, 1.0);
                if flags & 8 != 0 {
                    a = f2(p)?;
                    e = a;
                    p += 2;
                } else if flags & 0x40 != 0 {
                    a = f2(p)?;
                    e = f2(p + 2)?;
                    p += 4;
                } else if flags & 0x80 != 0 {
                    a = f2(p)?;
                    b = f2(p + 2)?;
                    c = f2(p + 4)?;
                    e = f2(p + 6)?;
                    p += 8;
                }
                // only offsets (ARGS_ARE_XY_VALUES); point matching is rare
                let (dx, dy) = if flags & 2 != 0 { (dx, dy) } else { (0.0, 0.0) };
                for contour in self.outline(g, depth + 1) {
                    out.push(
                        contour
                            .into_iter()
                            .map(|(x, y, on)| (a * x + c * y + dx, b * x + e * y + dy, on))
                            .collect(),
                    );
                }
                if flags & 0x20 == 0 {
                    break;
                }
            }
            Some(out)
        }
    }

    /// A rendered glyph: coverage, 0-255, `w` x `h`; the image's top left is
    /// (`left`, `top`) from the pen, y down.
    pub(crate) struct Glyph {
        pub w: usize,
        pub h: usize,
        pub left: i32,
        pub top: i32,
        pub alpha: Vec<u8>,
    }

    /// Signed-area accumulation, as in font-rs: each edge adds the area it
    /// covers to the cells it crosses, and a running sum gives coverage.
    struct Raster {
        w: usize,
        h: usize,
        a: Vec<f32>,
    }

    impl Raster {
        fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
            if (p0.1 - p1.1).abs() <= f32::EPSILON {
                return;
            }
            let (dir, p0, p1) = if p0.1 < p1.1 {
                (1.0, p0, p1)
            } else {
                (-1.0, p1, p0)
            };
            let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
            let mut x = p0.0;
            let y0 = p0.1.max(0.0) as usize;
            if p0.1 < 0.0 {
                x -= p0.1 * dxdy;
            }
            let w = self.w as i32;
            let end = (p1.1.ceil().max(0.0) as usize).min(self.h);
            for y in y0..end {
                let row = y * self.w;
                let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
                let xnext = x + dxdy * dy;
                let d = dy * dir;
                let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
                let x0f = x0.floor();
                let x0i = (x0f as i32).clamp(0, w);
                let x1c = x1.ceil();
                let x1i = (x1c as i32).clamp(0, w);
                let at = |i: i32| row + i as usize;
                if x1i <= x0i + 1 {
                    let xm = 0.5 * (x + xnext) - x0f;
                    self.a[at(x0i)] += d - d * xm;
                    self.a[at(x0i + 1)] += d * xm;
                } else {
                    let s = (x1 - x0).recip();
                    let x0frac = x0 - x0f;
                    let a0 = 0.5 * s * (1.0 - x0frac) * (1.0 - x0frac);
                    let x1frac = x1 - x1c + 1.0;
                    let am = 0.5 * s * x1frac * x1frac;
                    self.a[at(x0i)] += d * a0;
                    if x1i == x0i + 2 {
                        self.a[at(x0i + 1)] += d * (1.0 - a0 - am);
                    } else {
                        let a1 = s * (1.5 - x0frac);
                        self.a[at(x0i + 1)] += d * (a1 - a0);
                        for xi in x0i + 2..x1i - 1 {
                            self.a[at(xi)] += d * s;
                        }
                        let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                        self.a[at(x1i - 1)] += d * (1.0 - a2 - am);
                    }
                    self.a[at(x1i)] += d * am;
                }
                x = xnext;
            }
        }

        fn quad(&mut self, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32)) {
            let dev = (p0.0 - 2.0 * p1.0 + p2.0).powi(2) + (p0.1 - 2.0 * p1.1 + p2.1).powi(2);
            if dev < 0.333 {
                self.line(p0, p2);
                return;
            }
            let n = 1 + (3.0 * dev).sqrt().sqrt().floor() as usize;
            let mut prev = p0;
            for i in 1..=n {
                let t = i as f32 / n as f32;
                let u = 1.0 - t;
                let p = (
                    u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
                    u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
                );
                self.line(prev, p);
                prev = p;
            }
        }
    }

    /// Render glyph g at `scale` pixels per font unit. `gamma` below 1 makes
    /// thin strokes a little heavier, as small text needs without hinting.
    pub(crate) fn render(face: &Face, g: u16, scale: f32, gamma: f32) -> Glyph {
        let empty = Glyph {
            w: 0,
            h: 0,
            left: 0,
            top: 0,
            alpha: Vec::new(),
        };
        let contours = face.outline(g, 0);
        // y up in the font, down in the image
        let pts: Vec<Vec<(f32, f32, bool)>> = contours
            .into_iter()
            .filter(|c| c.len() > 1)
            .map(|c| {
                c.into_iter()
                    .map(|(x, y, on)| (x * scale, -y * scale, on))
                    .collect()
            })
            .collect();
        let all = pts.iter().flatten();
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y, _) in all {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        if x0 > x1 || y0 > y1 {
            return empty;
        }
        let (left, top) = (x0.floor() as i32, y0.floor() as i32);
        let w = (x1.ceil() as i32 - left + 1).max(1) as usize;
        let h = (y1.ceil() as i32 - top + 1).max(1) as usize;
        if w > 512 || h > 512 {
            return empty;
        }
        let mut r = Raster {
            w,
            h,
            a: vec![0.0; w * h + w + 4],
        };
        let (ox, oy) = (left as f32, top as f32);
        for c in &pts {
            let p: Vec<(f32, f32, bool)> =
                c.iter().map(|&(x, y, on)| (x - ox, y - oy, on)).collect();
            let n = p.len();
            // start on an on-curve point (or between two off-curve ones)
            let first = p.iter().position(|q| q.2);
            let start = match first {
                Some(i) => (p[i].0, p[i].1),
                None => ((p[0].0 + p[1].0) / 2.0, (p[0].1 + p[1].1) / 2.0),
            };
            let base = first.unwrap_or(0);
            let mut cur = start;
            let mut ctrl: Option<(f32, f32)> = None;
            for k in 1..=n {
                let q = p[(base + k) % n];
                let qp = (q.0, q.1);
                if q.2 {
                    match ctrl.take() {
                        Some(cp) => r.quad(cur, cp, qp),
                        None => r.line(cur, qp),
                    }
                    cur = qp;
                } else if let Some(cp) = ctrl {
                    let mid = ((cp.0 + qp.0) / 2.0, (cp.1 + qp.1) / 2.0);
                    r.quad(cur, cp, mid);
                    cur = mid;
                    ctrl = Some(qp);
                } else {
                    ctrl = Some(qp);
                }
            }
            match ctrl {
                Some(cp) => r.quad(cur, cp, start),
                None => r.line(cur, start),
            }
        }
        let mut acc = 0.0f32;
        let alpha = r.a[..w * h]
            .iter()
            .map(|&v| {
                acc += v;
                let c = acc.abs().min(1.0).powf(gamma);
                (c * 255.0 + 0.5) as u8
            })
            .collect();
        Glyph {
            w,
            h,
            left,
            top,
            alpha,
        }
    }
}

// ---- the smooth cursors ----

mod cursor_art {
    //! The hand and I-beam that `X11::cursor` shows where the server has
    //! RENDER: each a union of simple shapes, measured by signed distance,
    //! painted white with a dark edge (the I-beam the other way round) and a
    //! light shadow, like the macOS cursors, then sent once as a
    //! full-colour cursor.

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Art {
        Hand,
        IBeam,
    }

    /// Signed distance from (px, py) to a capsule: the segment widened by r.
    fn capsule(px: f32, py: f32, a: (f32, f32), b: (f32, f32), r: f32) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
        (px - (a.0 + t * dx)).hypot(py - (a.1 + t * dy)) - r
    }

    /// Signed distance to a rounded box from (x0, y0) to (x1, y1).
    fn rbox(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
        let (qx, qy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
    }

    /// How wide a cursor's edge is, outside its outline.
    const EDGE: f32 = 1.0;

    /// The cursor's outline (negative inside) and its creases.
    fn cursor_shape(art: Art, px: f32, py: f32) -> (f32, f32) {
        match art {
            Art::Hand => {
                // drawn in a 32-unit frame, like the system's pointing hand;
                // cursor_image crops it
                let d = [
                    capsule(px, py, (12.1, 9.8), (12.75, 16.0), 0.8), // index, pointing
                    capsule(px, py, (15.9, 14.25), (15.9, 17.0), 1.05), // knuckles
                    capsule(px, py, (18.35, 14.05), (18.35, 17.0), 1.1),
                    capsule(px, py, (20.85, 14.75), (20.85, 17.0), 1.05),
                    rbox(px, py, 12.0, 15.6, 21.9, 20.2, 0.9), // palm
                    rbox(px, py, 13.3, 18.0, 20.9, 23.3, 1.2), // wrist
                    capsule(px, py, (9.4, 15.4), (13.6, 21.2), 1.1), // thumb
                ]
                .into_iter()
                .fold(f32::MAX, f32::min);
                // three lines on the back of the hand, and the gaps between knuckles
                let crease = [
                    capsule(px, py, (15.5, 17.2), (15.5, 21.0), 0.0),
                    capsule(px, py, (17.5, 17.2), (17.5, 21.0), 0.0),
                    capsule(px, py, (19.5, 17.2), (19.5, 21.0), 0.0),
                    capsule(px, py, (17.2, 14.2), (17.2, 15.6), 0.0),
                    capsule(px, py, (19.7, 14.6), (19.7, 15.6), 0.0),
                ]
                .into_iter()
                .fold(f32::MAX, f32::min);
                (d, crease)
            }
            Art::IBeam => {
                // a one-pixel stem on pixel centres, and serifs that curve in
                let d = [
                    capsule(px, py, (4.5, 2.5), (4.5, 16.5), 0.5),
                    capsule(px, py, (1.5, 1.5), (3.5, 1.5), 0.5),
                    capsule(px, py, (3.5, 1.5), (4.5, 2.5), 0.5),
                    capsule(px, py, (7.5, 1.5), (5.5, 1.5), 0.5),
                    capsule(px, py, (5.5, 1.5), (4.5, 2.5), 0.5),
                    capsule(px, py, (1.5, 17.5), (3.5, 17.5), 0.5),
                    capsule(px, py, (3.5, 17.5), (4.5, 16.5), 0.5),
                    capsule(px, py, (7.5, 17.5), (5.5, 17.5), 0.5),
                    capsule(px, py, (5.5, 17.5), (4.5, 16.5), 0.5),
                ]
                .into_iter()
                .fold(f32::MAX, f32::min);
                (d, f32::MAX)
            }
        }
    }

    /// A cursor's image: width, height, premultiplied ARGB pixels, and the
    /// hot spot (the pixel that points).
    pub(super) fn image(art: Art) -> (i32, i32, Vec<u32>, (i32, i32)) {
        // (size, hot spot, where the image starts in the shape's frame, fill, edge)
        type Spec = (i32, i32, (i32, i32), (f32, f32), f32, f32);
        let (w, h, hot, (ox, oy), fill, edge): Spec = match art {
            Art::Hand => (17, 20, (5, 1), (7.0, 7.0), 1.0, 0.0),
            // the I-beam is black, edged in white, so it shows on any text
            Art::IBeam => (9, 21, (4, 9), (0.0, 0.0), 0.0, 1.0),
        };
        let mut px = Vec::with_capacity((w * h) as usize);
        for j in 0..h {
            for i in 0..w {
                let (x, y) = (i as f32 + 0.5 + ox, j as f32 + 0.5 + oy);
                let (d, crease) = cursor_shape(art, x, y);
                // premultiplied colour and alpha, painted back to front
                let (mut c, mut a) = (0.0f32, 0.0f32);
                let mut over = |col: f32, cov: f32| {
                    c = col * cov + c * (1.0 - cov);
                    a = cov + a * (1.0 - cov);
                };
                // a light shadow, one pixel down
                let (ds, _) = cursor_shape(art, x, y - 1.0);
                over(0.0, 0.28 * (1.0 - (ds - EDGE) / 1.3).clamp(0.0, 1.0));
                over(edge, (0.5 - (d - EDGE)).clamp(0.0, 1.0));
                let inside = (0.5 - d).clamp(0.0, 1.0);
                over(fill, inside);
                if crease < f32::MAX {
                    over(edge, (0.5 - (crease - 0.38)).clamp(0.0, 1.0) * inside);
                }
                let v = (c * 255.0).round() as u32;
                let al = (a * 255.0).round() as u32;
                px.push(al << 24 | v << 16 | v << 8 | v);
            }
        }
        (w, h, px, hot)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hot_spots_are_on_the_shape() {
            for art in [Art::Hand, Art::IBeam] {
                let (w, h, px, (hx, hy)) = image(art);
                assert_eq!(px.len(), (w * h) as usize);
                assert!(px[(hy * w + hx) as usize] >> 24 > 128, "{art:?}");
            }
        }
    }
}
