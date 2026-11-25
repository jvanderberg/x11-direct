---
name: x11-direct
description: Build lightweight X11 GUI apps in Rust with no dependencies (no Xlib, xcb or x11rb) using a single-file library that speaks the X11 protocol directly over a socket, with anti-aliased TrueType text, smooth shapes, translucency, clipboard and cursors through RENDER. Use when writing a small windowed tool, monitor, dashboard, game or demo for an X server (Linux, Xvfb, XQuartz, or a remote display over the network, e.g. from an embedded Linux board), when bandwidth or binary size matters, or when the user asks for an X11 app without a toolkit. Covers the library, the drawing patterns that keep traffic tiny, and a headless screenshot tool for checking what an app draws.
---

# x11-direct: small X11 apps in Rust, no dependencies

`src/lib.rs` in this skill's directory is the whole library: about 3,200
lines including its documentation, `std` only. It opens one window on an X server and gives an
immediate-mode drawing API (rectangles, lines, ellipses, polygons, text,
images, off-screen canvases, masks) with a back buffer, plus key, mouse,
resize, close and paste events. Where the server has the RENDER extension
it adds anti-aliased TrueType text (rasterized on the client, from a font
file), smooth shapes, translucency and full-colour cursors. It also has
the clipboard and Xauthority cookies (so `ssh -X`/`-Y` work). The program is an X client, so the window can be
on another machine: the drawing happens on the server, and the program
sends only small requests.

It started as `x11.h` in rp2350-native-linux, where it drives xpulse
(a system monitor), xmc (a two-panel file manager with terminal tabs),
xterm and Wolfenstein 3D from a microcontroller-class Linux board over
Wi-Fi. That's where the patterns in `reference/patterns.md` come from.

**Don't use it for:** HiDPI, non-Latin text in outline fonts, input
methods, accessibility, native widgets, or a Wayland session without
XWayland. Use egui, iced, Slint or GTK for those. If an app needs the full
protocol (other extensions, several windows), use the `x11rb` crate; the
patterns here still apply.

## Setting up a project

Pick one:

1. **Copy the file** (best for small tools and cross-compiling): copy
   `src/lib.rs` from this skill into the project as `src/x11.rs`, add
   `mod x11;` to `main.rs`, and `use crate::x11::{X11, Event, key};`.
   There's nothing to add to Cargo.toml.
2. **Path or git dependency**: `x11-direct = { path = "/path/to/this/skill" }`,
   then `use x11_direct::{X11, Event, key};`.

The examples in `examples/` (`hello`, `xeyes`, `graph`, `menu`, `gallery`,
`polish`) run with `cargo run --example NAME` from this skill's directory.
Start a new app from the closest one. For anything a person will look at
(a tool, a monitor, a form), start from `polish`.

## Making it look like a real app

The core protocol alone looks like 1995. These are cheap and make the
difference (`reference/patterns.md` section 13 has the details, and
`examples/polish.rs` has them all):

- **Text:** `font_ttf` with a TrueType file from disk (Helvetica Neue on
  macOS, DejaVu/Liberation/Noto on Linux), falling back to the core
  Helvetica. Row heights from font metrics, never constants.
- **Shapes:** rounded buttons, fields and panels with
  `aa_polygon(&rounded_points(..))`; graphs as `aa_triangles` batches
  with a translucent fill. Icons rendered on the client, anti-aliased
  against their background, and cached in canvases.
- **Popups:** a soft layered shadow, save-under that includes the shadow,
  hover highlight, a tick on the current choice, keyboard navigation. A
  dropdown instead of a row of buttons for one-of-many choices.
- **Destructive actions** ask first, over a window dimmed with
  `blend_rect`, with a red default button.
- **Cursors:** `cursor(Cursor::Hand)` over clickables and
  `cursor(Cursor::Text)` over fields, set on every `Motion`. The library
  draws macOS-style smooth versions with RENDER.
- **Clipboard:** Copy for anything worth pasting elsewhere, Ctrl+C/Ctrl+V.
- **Status feedback** after actions, errors in red, and dark mode from
  (light, dark) colour pairs.
- **Fallbacks:** every RENDER call returns false without RENDER
  (`has_render()`); draw the plain version and keep it tidy.

## How to think about it: the cost model

Every call becomes bytes on a socket, and the X server does the drawing.
Design around what each thing costs to send:

| Operation | Bytes sent |
|---|---|
| a rectangle, line or ellipse | 20-28 |
| `color()` (only when it changes) | 16 |
| text, n characters | 16 + n |
| `blit` (any size, from a canvas the server already has) | 28 |
| `present()` (copy the back buffer to the window) | 28 |
| `image()` of w x h pixels | 4·w·h (**320x200 is 256 KB**) |

So:

1. **Draw shapes, not pixels.** A raycaster frame can be a few hundred
   polygons (a few KB) instead of a pixel buffer (hundreds of KB). Use
   `image()` only for small or one-off pictures, and put them in a canvas
   so they're sent once.
2. **Draw once, keep it on the server.** Anything that repeats (a
   background, a sprite, a glyph strip, a graph) goes in a `canvas()` once
   and is copied with `blit()` each time it's needed.
3. **Send only what changed.** Remember what each region shows (a key
   string per list row, the last value text) and skip unchanged ones. The
   back buffer keeps everything else.
4. **Move pixels instead of redrawing them.** To scroll a list, blit
   `frame()` onto itself and draw only the new rows. For a scrolling
   graph, blit the graph's canvas one step left and draw the new column.
5. **Popups save what they cover.** Copy the area from `frame()` into a
   canvas before drawing the menu, and blit it back when it closes.
6. **Don't run ahead of the server.** Call `sync()` after `present()`, or
   pipeline with `fence()`/`fence_wait()`, so a slow link can't build up a
   queue of stale frames.

`reference/patterns.md` has each of these with working code, plus
sprites with masks and stencils, tints, fonts, resizing, input handling,
and how xmaze and Wolfenstein 3D fit in a few KB per frame.

## Program shapes

Every program: `X11::open(None)` (uses `$DISPLAY`; print the error, which
explains the fix), optionally `background()`, then `window()`, then a
loop that drains `event()`, draws, calls `present()`, and sleeps.

```rust
use x11_direct::{key, Event, X11};

let mut x = X11::open(None).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
x.background(x.rgb(20, 22, 30));
x.window("tool", 640, 400);
let mut dirty = true;
loop {
    while let Some(e) = x.event() {
        match e {
            Event::Close => return,
            Event::KeyDown { key, .. } if key == 'q' as u32 || key == key::ESCAPE => return,
            Event::Resize { .. } => dirty = true,
            _ => {}
        }
    }
    if dirty {
        draw(&mut x);          // to the back buffer
        x.present();           // show it
        x.sync();              // let the server catch up
        dirty = false;
    }
    x.wait(-1);                // sleep until an event; wait(ms) for a timer
}
```

- **Animation or timer:** `x.wait(ms_until_next_tick)`. It sleeps until an
  event or the timeout, and isn't woken early by replies the library
  itself is waiting for.
- **Event-driven tool:** `x.wait(-1)`, and draw only when something
  changed. It uses no CPU while idle.
- **Other input too (stdin, a pipe, a socket, a pty):**
  `let ready = x.wait_with(&[fd], ms);` returns which of those fds can be
  read. Use it instead of a second thread. Read stdin with a buffer of at
  least 8 KiB, so Rust's `Stdin` buffer can't hide data from the poll.
- **Pipelined frames (games over a link):** `x.fence_wait()` before
  drawing and `x.present(); x.fence();` after, so the next frame is drawn
  while the last one is still in transit.

## Things that go wrong

- **Nothing appears:** draw calls go to the back buffer, and only
  `present()` shows them. After a resize, `frame()` is a new canvas and
  the window shows the old picture until you draw again.
- **Resizing is slow or flickers:** on `Event::Resize`, don't lay out
  straight away. Wait until the size has been stable for about 80 ms (see
  `examples/graph.rs`), then lay out and draw everything once.
- **Text is in the wrong place:** `text(x, y, s)` puts the **baseline** at
  y. Use `y = top + font_ascent(f)`, and give rows a height of
  `font_ascent + font_descent + padding`.
- **A font is missing:** `font(pattern)` returns `Font::FIXED` when the
  server has no match, so the program still works. Lay out with the
  measured `text_width`/`font_ascent`, never with assumed sizes. Try a
  couple of XLFD patterns and keep the first that isn't `Font::FIXED`.
- **Non-Latin-1 text shows '?':** use `font_cell(...iso10646-1)` and
  `text_unicode()`.
- **Keys:** `key` is the keysym: a Latin-1 code (`'a' as u32`, already
  shifted) or a `key::` constant. A held key repeats as KeyUp/KeyDown pairs,
  so a game should keep a set of held keys and update it on both events.
- **Wheel:** comes as `ButtonDown` with button 4 (up) or 5 (down).
- **Coordinates are i16 on the wire:** keep shapes within ±32767.
  Batch arrays (`fill_rects`, `segments`, `fill_poly`) take `[i16; N]`.
- **Connection refused:** the server must allow the client. For a remote
  server, run `xhost +CLIENT-ADDRESS` on it (with XQuartz, first enable
  Settings > Security > "Allow connections from network clients"). For a
  local server, run `xhost +local:`. The library sends the display's
  MIT-MAGIC-COOKIE-1 from `~/.Xauthority`, so `ssh -X`/`-Y` and servers
  started with `-auth` work without `xhost`.
- **Smooth text and shapes over `ssh -X` come out plain:** an untrusted
  client doesn't get RENDER. Use `ssh -Y`. With XQuartz, run it as
  `DISPLAY=:0 ssh -Y host app` if launchd's DISPLAY is stale.
- **Use one `X11` per process and one window.** The library isn't
  thread-safe, so keep it on one thread.

## Checking what an app draws (no display needed)

`tools/x11shot.sh` (in this skill) builds the Cargo project inside Docker,
runs it on Xvfb with the classic X fonts, optionally drives it with
xdotool, and saves PNG screenshots that you can then look at:

```sh
cd my-app
/path/to/skill/tools/x11shot.sh -o shots/main.png                 # the package's binary
/path/to/skill/tools/x11shot.sh -e graph -w 3 -o shots/g.png      # an example, after 3 s
/path/to/skill/tools/x11shot.sh -o shots/m.png -s 640x480 \
    -i "mousemove 100 80; click 3; shot; key Down Return"        # input, a shot mid-way
```

It prints whether the program was still running or exited, and the last
lines it printed (`-l` for all). Use it after each meaningful change,
and read the PNGs to check layout, fonts, resizing (`-i "search --name
TITLE windowsize %@ 900 500; sleep 1"`) and interactions. The first run
builds the Docker image, which takes a minute or two.

To see it live: point `DISPLAY` at a real server, such as
`DISPLAY=:0 cargo run` on a Linux desktop, or `DISPLAY=mac.local:0` with
XQuartz after the `xhost` step above.

## Before calling an app done

- It uses an outline font where RENDER exists, has hover cursors, and
  looks deliberate both with RENDER and without it (see "Making it look
  like a real app").
- It handles `Event::Close`, q/Escape where that fits, and resizing (lays
  out again for any size, with `min_size()` set).
- An idle app sleeps in `wait(-1)` or a long timeout and sends nothing.
- Repeated drawing comes from canvases. Unchanged regions aren't sent.
  `x.sent()` before and after an update shows its cost; aim for hundreds
  of bytes for a typical update, not kilobytes.
- `sync()` or a fence after each frame.
- There are screenshots from `x11shot.sh` at the default size and after a
  resize, and you have looked at them.

## Reference

- `reference/api.md`: every call, with its arguments and notes.
- `reference/patterns.md`: the techniques above in detail, with code.
- `reference/protocol.md`: how the library encodes requests, how it uses
  RENDER, and how to add one it doesn't have (window title in UTF-8, the
  bell, focus events, an extension).
