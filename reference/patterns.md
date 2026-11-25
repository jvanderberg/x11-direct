# Patterns

These techniques make an X11 app cheap to run over a slow link and smooth on
a fast one. They come from xpulse, xmc, xmaze and Wolfenstein 3D, which run
from an RP2350 board to XQuartz over Wi-Fi at a few hundred KB/s. Code is
for x11-direct (`x` is the `X11`).

Use `x.sent()` to measure: read it before and after an update, and the
difference is what that update cost.

## 1. Draw once, keep it on the server

A canvas is a pixmap that lives on the server. Draw into it once and
`blit` it whenever it's needed, for 28 bytes per copy.

```rust
let bg = x.canvas(w, h);
x.target(Some(bg));          // drawing calls now go to the canvas
draw_static_background(&mut x);
x.target(None);              // back to the frame
// every frame:
x.blit(bg, 0, 0, w, h, 0, 0);
```

Good candidates: backgrounds, panels and their borders, sprites, icons,
textures, a starry sky (xmaze draws a panorama once and copies a window
of it each frame), and a minimap that only gets the tiles newly seen.

Canvases start with undefined contents, so fill them. Free them with
`free_canvas` when the layout changes. They use server memory (w·h·4 bytes
each), so keep a bounded cache. xwolf keeps a fixed number of scaled
textures and evicts the oldest.

## 2. Send only what changed

The back buffer keeps whatever was drawn last, so an update only needs to
draw the parts that differ. Keep a cheap description of what each region
shows now, and redraw a region only when that description changes:

```rust
// one key per visible row: the item, whether it's selected, its text
let key = format!("{i}|{sel}|{name}");
if key != self.rowkey[r] {
    draw_row(&mut x, r, i);
    self.rowkey[r] = key;
}
```

Do the same for a value label (compare the formatted string), a meter
(compare its pixel length, not its value), and a status line. xpulse
updates CPU, memory, network and disk panels and a process table every
second this way, in a few hundred bytes.

After a resize or a full relayout, clear all the keys so everything is
drawn once.

## 3. Move pixels instead of redrawing them

**Scrolling a list:** blit the frame onto itself, shift the row keys to
match, and draw only the rows that came into view (`examples/menu.rs`):

```rust
let d = self.top - last_top;            // rows scrolled (positive: down)
let keep = rows - d.abs();
let frame = x.frame();
if d > 0 {
    x.blit(frame, X, y0 + d * rh, w, keep * rh, X, y0);
    self.rowkey.rotate_left(d as usize);
    self.rowkey[keep as usize..].iter_mut().for_each(String::clear);
} else {
    x.blit(frame, X, y0, w, keep * rh, X, y0 - d * rh);
    self.rowkey.rotate_right(-d as usize);
    self.rowkey[..-d as usize].iter_mut().for_each(String::clear);
}
// then the normal changed-rows pass draws just the cleared rows
```

**A scrolling graph** lives in its own canvas. Each sample moves it one
step left and draws only the new strip (`examples/graph.rs`), so a graph
costs about 150 bytes per update whatever its size:

```rust
x.target(Some(plot));
x.blit(plot, STEP, 0, pw - STEP, ph, 0, 0);    // everything one step left
draw_strip_background(&mut x, pw - STEP, STEP);
draw_newest_segment(&mut x);
x.target(None);
x.blit(plot, 0, 0, pw, ph, px, py);
```

Redraw the whole graph only when its scale changes, and choose scales that
rarely change. xpulse rounds the scale up to 1-2-5 steps at once, and
lowers it only when the values fall below a quarter of it.

## 4. Popups save what they cover

```rust
// open
let save = x.canvas(w, h);
let frame = x.frame();
x.target(Some(save));
x.blit(frame, mx, my, w, h, 0, 0);
x.target(None);
draw_menu(&mut x);
// close
x.blit(save, 0, 0, w, h, mx, my);
x.free_canvas(save);
```

Nothing under the menu is drawn again. If the window is resized while the
menu is open, free the canvas without restoring it, because the relayout
draws everything anyway. Hover highlighting redraws only the menu's own
rows. Dialogs and drag feedback work the same way: xmc's drag-and-drop
draws a ghost over the frame and restores the area under it each time it
moves.

## 5. Resizing

The library recreates the back buffer on resize, keeping the old picture,
with the background colour in any new area. The window looks right during
a drag, so there's no need to redraw on every `Event::Resize`:

```rust
Event::Resize { .. } => resize_at = Some(Instant::now() + Duration::from_millis(80)),
// in the loop, once the size has stopped changing:
if resize_at.is_some_and(|t| Instant::now() >= t) {
    resize_at = None;
    if (x.width(), x.height()) != last_size {
        last_size = (x.width(), x.height());
        layout(&mut x);      // recompute positions, recreate size-dependent canvases,
        draw_all(&mut x);    // clear the change-tracking keys, draw everything
        x.present();
        x.sync();
    }
}
```

Shorten the `wait` timeout while a resize is pending, so the relayout
isn't late. Call `x.background(colour)` before `window()` so new areas
match the app. Set `x.min_size(w, h)` so the layout never has to deal with
a tiny window. `frame()` is a new canvas after each resize, so don't keep
it around.

## 6. Pacing

`present()` only queues a copy. If the program makes frames faster than
the link carries them, the queue grows and input lags behind. There are
two remedies:

- `x.present(); x.sync();`: wait until the server has drawn the frame.
  This is simple, but the program is idle for the whole round trip.
- A **fence** keeps exactly one frame in flight. The program draws frame
  N+1 while frame N is in transit:

  ```rust
  loop {
      handle_events(&mut x);
      x.fence_wait();          // frame N-1 has been drawn
      draw_frame(&mut x);
      x.present();
      x.fence();               // mark frame N
      x.wait(ms_until_next_frame);
  }
  ```

`wait(ms)` sleeps until an *event* or the timeout. Replies and other
messages that aren't events (such as a fence's reply) are handled without
ending the sleep, so the loop keeps its pace.

## 7. Many shapes: batches, runs and polygons

- `fill_rects(&[[x, y, w, h], ...])` and `segments(&[[x1, y1, x2, y2], ...])`
  send many shapes in one request (8 bytes each). Use them for bar charts,
  grids, tick marks, cell backgrounds and particles.
- **A changed bitmap as runs.** To send a frame that's only available as
  pixels (an 8-bit game screen, say), compare it with the last frame
  sent. Send each horizontal run of one colour that changed as a
  rectangle, and merge runs that line up with the same run on the next
  row. Find unchanged rows first with a single memcmp each. xwolf sends
  its menus and status bar this way.
- **3D as polygons.** xmaze casts one ray per two columns, groups
  neighbouring columns that hit the same wall face, and sends each group
  as one `fill_poly` in that face's fogged colour, with the mortar as
  `segments`. The floor is one quad per tile the rays crossed. A frame is
  a few KB, and the picture stays sharp at any window size.
- **Textured walls by copying.** xwolf uploads each 64x64 texture once,
  scaled to the window, and draws a column of wall as a `blit` from it.
  Neighbouring columns that continue the same texture merge into one copy.

## 8. Sprites: masks and stencils

For a non-rectangular shape, draw it into a canvas and build a 1-bit mask
of the pixels that belong to it:

```rust
let sprite = x.canvas(48, 48);      // draw the sprite into it
let mask = x.mask(48, 48);          // all clear
x.mask_rects(mask, &rows);          // set the sprite's pixels, as runs [x, y, w, 1]
x.blit_masked(sprite, mask, 0, 0, 48, 48, dx, dy);
```

`blit_masked` makes the server turn the bitmap into a clip region on every
call, which is slow for a big mask. For a mask used repeatedly, make a
stencil once:

```rust
let st = x.stencil(mask);
x.blit_stencil(st, sprite, 0, 0, 48, 48, dx, dy);   // only moves the clip origin
```

Free the stencil (`free_stencil`) before the mask (`free_mask`). xwolf
keeps the weapon, and any sprite seen at the same size two frames
running, as a canvas with a stencil. Other sprites are drawn as a
rectangle per run of one colour in each column.

## 9. Tints, gradients and see-through effects

- **A tint** (a flash of red for damage, a dimmed area behind a dialog):
  call `fill_stippled(mask, ...)` with a 4x4 mask holding an ordered
  dither. Only the set pixels are filled, so the picture shows through.
  It's one request, where a palette or alpha change would mean sending the
  whole frame.
- **See-through, properly:** with RENDER, `blend_rect(x, y, w, h, alpha)`
  in the current colour is a real blend, one request. Use it for a
  dialog's dimmed backdrop and keep the stipple as the fallback.
- **A gradient:** draw a 1-pixel-wide canvas once, then fill any polygon
  with it repeated, using `fill_poly_tiled(tile, ox, oy, &pts)`. xpulse
  shades the area under each graph this way in one request.

## 10. Fonts and text

**Prefer an outline font.** With RENDER (every current server, but not an
untrusted `ssh -X` client), `font_ttf` reads a TrueType file on the
*client*, rasterizes it with anti-aliasing and sends the glyphs once
(about 25 KB a font). The server needs no fonts at all, and text then
costs what core text costs. Try a list and fall back to the core fonts:

```rust
const FONTS: [(&str, &str); 3] = [
    ("/System/Library/Fonts/HelveticaNeue.ttc", "Helvetica Neue"),   // a .ttc: pick the face by full name
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "DejaVu Sans"),
    ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "Liberation Sans"),
];
let mut f = Font::FIXED;
for (path, face) in FONTS {
    if let Ok(data) = std::fs::read(path) {
        f = x.font_ttf(&data, face, 13.0);                 // px to the em
        if f != Font::FIXED { break; }
    }
}
if f == Font::FIXED { f = x.font("-*-helvetica-medium-r-normal--14-*-*-*-p-*-iso8859-1"); }
```

Bold is another face (`"Helvetica Neue Bold"` from the same .ttc,
`DejaVuSans-Bold.ttf`). Outline fonts are taller for their size than the
bitmap ones, so take every row and line height from `font_ascent` and
`font_descent`, never from a constant: a heading laid out for 6x13 gets
clipped in DejaVu. If the font list comes up empty because RENDER is
missing, print one line saying so (with the `ssh -Y` hint) rather than
silently looking worse.

The server's core fonts are bitmaps named by XLFD patterns. A server with
the standard font packages (Xorg, Xvfb, XQuartz) has:

| Font | Pattern |
|---|---|
| the default, about 6x13 | `Font::FIXED` |
| Helvetica, sizes 8-34 | `-*-helvetica-medium-r-normal--14-*-*-*-p-*-iso8859-1` (`bold` for bold) |
| Times | `-*-times-medium-r-normal--14-*-*-*-p-*-iso8859-1` |
| Courier, fixed width | `-*-courier-medium-r-normal--14-*-*-*-m-*-iso8859-1` |
| Lucida Typewriter | `-b&h-lucidatypewriter-medium-r-normal-sans-12-*-*-*-m-*-iso8859-1` |
| A terminal font with Unicode and line drawing | `-misc-fixed-medium-r-normal--13-*-*-*-c-*-iso10646-1` (13, 14, 15, 18, 20) |

- Ask for fonts at start-up, try fallbacks, and lay out with the measured
  metrics:

  ```rust
  let mut f = x.font("-adobe-helvetica-bold-r-normal--24-*-*-*-p-*-iso8859-1");
  if f == Font::FIXED { f = x.font("-*-helvetica-bold-r-normal--24-*-*-*-p-*-iso8859-1"); }
  let row_h = x.font_ascent(f) + x.font_descent(f) + 6;
  x.text_font(f);
  x.text(left, top + x.font_ascent(f), "Title");   // y is the baseline
  ```

- `text_width(f, s)` is computed on the client from metrics fetched once,
  so it costs nothing to send. Use it freely for alignment and truncation
  with "...".
- `font()` fetches the width of every glyph, which is large for a big
  Unicode font. For a terminal grid, use `font_cell(pattern)`, which
  measures one "M" and treats every glyph as that wide, together with
  `text_unicode()` for characters past U+00FF. xterm and xmc's terminal
  tabs work this way.
- `text()` sends Latin-1. Characters it can't encode show as '?'.

## 11. Input

- **Keys.** `key` is a keysym: printable keys arrive as their Latin-1 code,
  already shifted (`'A' as u32` with Shift). Everything else is a `key::`
  constant. `mods & MOD_CTRL` gives you Ctrl-letter: the key is still the
  letter.
- **Held keys (games).** X sends a KeyUp and KeyDown pair on every
  auto-repeat. Keep a set of the keys that are down, add on `KeyDown`,
  remove on `KeyUp`, and move by what's in the set each tick. xmaze steers
  this way.
- **Mouse.** Buttons 1-3 are left, middle and right; 4 and 5 are the wheel.
  `Motion` arrives only while the pointer is over the window. For hover,
  compare against the last hit and redraw only when it changes. For a
  double click, compare the time and position of the last `ButtonDown`
  (xmc uses 400 ms and a few pixels). For a drag, record the press, follow
  `Motion` with a save-under ghost (pattern 4), and act on `ButtonUp`.
- **Pointer outside the window.** `pointer()` is a round trip, so poll it
  only on a timer (xeyes polls every 30 ms).
- **Focus and the close button.** `Event::Close` is the window manager's
  close button, or a lost connection. Always handle it.

## 12. Other input alongside X

The library never blocks unless you ask it to, so a single thread works:

```rust
let ready = x.wait_with(&[pty_fd, socket_fd], -1);
if ready[0] { read_pty(); }
if ready[1] { read_socket(); }
// then: drain x.event(), redraw what changed, present
```

If you poll yourself, check `x.pending()` first. Round trips such as
`sync()` read events into the queue, where your poll can't see them.
xterm waits on the X connection and its pty this way, and xmc waits on X
and every terminal tab's pty.

## 13. Looking like a real app

The core protocol draws jagged circles, square corners and bitmap text,
which reads as a 1990s X client. A handful of touches make the same
program look current. `examples/polish.rs` has all of them in one small
form. Each RENDER call returns false without RENDER, so draw the plain
version then.

- **Anti-aliased text.** Use `font_ttf` (pattern 10). It does more for the
  look than anything else here.
- **Smooth shapes.** Rounded rectangles for buttons, fields, menus and
  dialogs: `x.aa_polygon(&rounded_points(x, y, w, h, radius), 255)` in the
  edge colour, then again one pixel in for the face. Collect many shapes
  of one colour with `polygon_triangles`/`polyline_triangles` and send them
  in one `aa_triangles`. A graph with a 1.5 px line and a fill under it
  at alpha 40-60 is two requests per colour.
- **Smooth icons without RENDER.** Render an icon on the client (signed
  distance per pixel, coverage = `clamp(0.5 - d, 0, 1)`), blended onto the
  colour it will sit on, and send it once into a canvas keyed by
  (icon, colours). After that it's a 28-byte `blit`. The same trick makes
  rounded corners: four small corner images plus `fill_rects` for the
  middle. A theme change means new colours, so clear that cache.
- **Floating things float.** A menu or dialog gets a soft shadow: six
  `aa_polygon`s of black, each 1.6 px bigger and 3 px down, at alpha 8 on
  a light theme and 16 on a dark one. Save the area under the *shadow*
  too (pattern 4), or closing leaves a grey ring behind.
- **Dropdowns, not rows of buttons.** A choice among several (signals,
  sizes, modes) is a button showing the current value and a chevron,
  opening a menu: hover highlight (redraw only the menu), a tick on the
  current choice, short hints aligned right (`SIGKILL`, `Ctrl+C`), thin
  rules between groups, Up/Down/Return/Escape, and a click outside closes
  it. A context menu (right click) works the same way and is the natural
  home for "Copy ...".
- **Ask before destroying.** A Force Quit, a delete or a clear opens a
  dialog over the whole window dimmed with `blend_rect` (black, alpha
  about 64 light, 110 dark). The default button is the action, coloured
  red, and Return/Escape answer it. Save the whole frame first, so
  closing is one copy back.
- **The pointer says what a click does.** `x.cursor(Cursor::Hand)` over
  anything clickable, `Cursor::Text` over a text field, `Cursor::Default`
  elsewhere. Work it out from the same hit test as clicks, in a
  `cursor_at(x, y)`, and call `cursor()` on every `Motion` and after every
  click (the thing under the pointer may have changed): it sends nothing
  when the shape hasn't changed. With RENDER the hand and I-beam are smooth
  macOS-style images; `define_cursor` takes your own.
- **Copy and paste.** Offer Copy on whatever a user may want elsewhere
  (an ID, a path, a command line, shell-quoted), and bind Ctrl+C/Ctrl+V.
  `set_clipboard` is immediate; `request_paste` answers later with
  `Event::Paste`, which goes into the focused field (first line only for a
  one-line field).
- **Feedback.** After an action, a short status line: "Sent SIGTERM to
  1234" in the normal colour, the OS's error ("Operation not permitted")
  in red.
- **Dark mode.** Keep the palette as (light, dark) pairs, pick at start
  (on macOS, `defaults read -g AppleInterfaceStyle` prints `Dark`), and
  offer a toggle that redraws everything. Shadows and dimming need more
  alpha on dark.
- **Lay out from the font.** Toolbar, header and row heights from font
  metrics; columns that drop out in a set order as the window narrows;
  text that doesn't fit truncated with "..." by `text_width`.

### Checking it

- `tools/x11shot.sh` (Docker, Xvfb with DejaVu fonts) shows the Linux
  look, and drives hover, menus and dialogs with xdotool.
- On a Mac, XQuartz ships `Xvfb` and `xwd` in `/opt/X11/bin`: run
  `/opt/X11/bin/Xvfb :77 -screen 0 1280x800x24 &`, start the app with
  `DISPLAY=:77`, and capture with `DISPLAY=:77 xwd -root -silent`. The
  output is XWD, which needs a converter (a few lines of Python with PIL).
- Also try it without RENDER (`ssh -X` into the machine running it, or a
  server started with `-extension RENDER`) and make sure every fallback
  still looks deliberate.
- XQuartz has no Retina mode: it scales X pixels up 2x, so everything
  looks slightly soft on a Mac however it's drawn. `DISPLAY=:0` reaches
  the XQuartz started by `startx` when launchd's DISPLAY socket is stale.

