# API

Everything is a method on `X11`, except the event type, the handles and
the constants. Coordinates and sizes are `i32` in pixels, from the top left.
On the wire they are 16-bit, so keep them within ±32767. Drawing calls
return nothing and never fail: if the connection is lost they do nothing,
and the next `event()` returns `Event::Close`.

## Connection and window

| Call | Notes |
|---|---|
| `X11::open(display: Option<&str>) -> io::Result<X11>` | `None` uses `$DISPLAY`. Accepts `HOST:N` (TCP 6000+N), `:N` or `unix:N` (`/tmp/.X11-unix/XN`), or a socket path (XQuartz's `/private/tmp/com.apple.launchd.*/org.xquartz:0`). Sends the display's MIT-MAGIC-COOKIE-1 from `$XAUTHORITY` or `~/.Xauthority` if there is one, so `ssh -X`/`-Y` work. The error explains the fix (DISPLAY unset, `xhost`, xauth, not TrueColor). |
| `background(pixel)` | What the window shows where nothing has been drawn, including new areas after it grows. Call it before `window()`. |
| `window(title, w, h)` | Opens and maps the one window. Call it once. |
| `min_size(w, h)` | A hint to the window manager. Call it after `window()`. |
| `width()`, `height()` | The window's current size, updated as resize events arrive. |
| `sent() -> u64` | Bytes sent so far. Measure an update with it. |
| `fd() -> RawFd` | The socket, for your own `poll()`. Check `pending()` first. |
| `pending() -> bool` | Whether `event()` has something to return, or a repaint to do, without reading the socket. |
| dropping the `X11` | Flushes and closes the connection, and the server removes the window. |

## Colour and drawing state

| Call | Notes |
|---|---|
| `rgb(r: u8, g: u8, b: u8) -> u32` | A pixel value for the server's visual. Compute colours once and keep them. |
| `color(pixel)` | The colour for everything drawn next. Repeating the current colour sends nothing. |
| `line_width(w)` | For `line`, `rect`, `ellipse`, `segments` and `polyline`. 0 is the fast 1-pixel line. |
| `target(Some(canvas))` / `target(None)` | Where drawing goes: a canvas, or the frame (the back buffer). |
| `target_window()` | Draw straight onto the window, over what `present()` last showed, leaving the back buffer alone. Use it for transient overlays such as a cursor. `target(None)` goes back to the frame. |

## Shapes

| Call | Bytes | Notes |
|---|---|---|
| `fill_rect(x, y, w, h)`, `rect(...)` | 20 | Sizes of zero or less draw nothing. |
| `fill_ellipse(x, y, w, h)`, `ellipse(...)` | 24 | Within the bounding box. |
| `line(x1, y1, x2, y2)` | 20 | |
| `fill_rects(&[[x, y, w, h]])` | 12 + 8n | Batched: as many per request as fit in 16 KB. |
| `segments(&[[x1, y1, x2, y2]])` | 12 + 8n | Separate lines, batched. |
| `fill_poly(&[[x, y]])` | 16 + 4n | Convex polygon, 3 to 1000 points. |
| `polyline(&[[x, y]])` | 12 + 4n | Connected lines, 2 to 2000 points. |
| `image(x, y, w, h, &pixels, stride)` | 24 + 4wh | `pixels` from `rgb()`, `stride` in pixels. Split into several requests if large. This is the expensive call. |

## Text and fonts

| Call | Notes |
|---|---|
| `text(x, y, s)` | (x, y) is the left end of the **baseline**. Latin-1: other characters show as '?'. At most 254 characters per call. |
| `font(xlfd) -> Font` | The first font matching the pattern, or `Font::FIXED` if none does. Fetches per-glyph widths for `text_width`. Up to 16 fonts. |
| `font_cell(xlfd) -> Font` | For a fixed-width font, including big Unicode ones: it measures one glyph instead of fetching them all. |
| `text_font(f)` | The font for `text` and `text_unicode` from now on. |
| `text_unicode(x, y, s)` | Characters up to U+FFFF, for an `iso10646-1` font. It advances by the font's cell width. |
| `text_width(f, s) -> i32` | Computed locally, so it costs nothing to send. |
| `font_ascent(f)`, `font_descent(f)` | Above and below the baseline. A row's height is ascent + descent + padding. |
| `font_ttf(data, name, px) -> Font` | Anti-aliased text from a TrueType file's bytes (`.ttf`, or a `.ttc` collection: the face whose full name is `name`, such as `"Helvetica Neue Bold"`, else the first), `px` pixels to the em. Rasterized on the client and sent once as a RENDER glyph set (about 25 KB for a 13 px font), then text costs the same as core text. Latin-1 only; glyf outlines, not CFF (`.otf`). `Font::FIXED` without RENDER or if the file can't be read, so fall back to `font()`. Works with `text`, `text_width`, `font_ascent` and the rest. |
| `has_render() -> bool` | Whether the server has the RENDER extension: `font_ttf`, the `aa_` calls, `blend_rect` and the smooth cursors need it. Xorg, Xvfb and XQuartz have it; an untrusted `ssh -X` client doesn't (use `-Y`). |

## Canvases, masks and stencils

| Call | Notes |
|---|---|
| `canvas(w, h) -> Canvas` | An off-screen image on the server, with undefined contents until you draw. |
| `free_canvas(c)` | Ignored for `frame()`. |
| `frame() -> Canvas` | The back buffer, as a source for `blit`: for save-under and for scrolling. A new canvas after each resize. |
| `blit(c, sx, sy, w, h, dx, dy)` | 28 bytes for any size. Copies to the current target, and may copy a canvas onto itself (overlaps are handled). |
| `mask(w, h) -> Mask` | A 1-bit image, all clear. |
| `mask_rects(m, &[[x, y, w, h]])` | Set pixels of the mask. |
| `free_mask(m)` | |
| `blit_masked(c, m, sx, sy, w, h, dx, dy)` | Copies only the pixels set in `m`, which lines up with `c`. It sets the clip mask every time, so it's slow for big masks used often. |
| `stencil(m) -> Stencil`, `free_stencil(s)` | A clip prepared once from a mask. Free it before the mask. |
| `blit_stencil(s, c, sx, sy, w, h, dx, dy)` | A masked copy that only moves the clip origin, so it's cheap. |
| `fill_stippled(m, x, y, w, h)` | Fills where the small mask `m`, repeated, is set: a dither for a see-through tint. |
| `fill_poly_tiled(c, ox, oy, &[[x, y]])` | A polygon that doesn't cross itself, filled with canvas `c` repeated from (ox, oy). Up to 2000 points. Use it for gradients. |

## Smooth shapes and translucency (RENDER)

These draw in the current `color()` at an opacity (0-255), with
anti-aliased edges, onto the current target. Each returns `false` and
draws nothing without RENDER, so fall back to the plain calls:
`if !x.aa_polygon(&pts, 255) { x.fill_poly(&ipts) }`.

| Call | Notes |
|---|---|
| `aa_triangles(&[[x0, y0, x1, y1, x2, y2]], alpha) -> bool` | Fractional coordinates. One request per 500 triangles: collect a frame's shapes of one colour and send them together. |
| `aa_polygon(&[[x, y]], alpha) -> bool` | Convex. |
| `aa_polyline(&[[x, y]], width, alpha) -> bool` | Connected lines with square ends. |
| `aa_fill_ellipse(x, y, w, h, alpha) -> bool`, `aa_ellipse(x, y, w, h, width, alpha) -> bool` | Within the bounding box, like `fill_ellipse` and `ellipse`; the outline is `width` pixels wide, inside the box. |
| `blend_rect(x, y, w, h, alpha) -> bool` | A see-through rectangle: dims or tints what's under it (a dialog's backdrop, a selection). |
| `polygon_triangles(pts)`, `polyline_triangles(pts, width)`, `ring_triangles(outer, inner)` | Free functions: shapes as triangles for `aa_triangles`, to batch many shapes into one request. |
| `rounded_points(x, y, w, h, radius)`, `ellipse_points(x, y, w, h)` | Free functions: outlines for `aa_polygon` (buttons, panels, dots). |

## Clipboard and cursor

| Call | Notes |
|---|---|
| `set_clipboard(text)` | Owns CLIPBOARD and PRIMARY and answers other programs' requests (UTF8_STRING, STRING, TEXT, TARGETS) from inside `event()`, so keep draining events. |
| `request_paste()` | Asks the CLIPBOARD owner for text. It arrives later as `Event::Paste(String)` (nothing arrives if the clipboard is empty or not text). |
| `cursor(Cursor)` | The pointer's shape over the window: `Default` (the window manager's arrow), `Hand`, `Text`, `Wait`, `ResizeH`, `ResizeV`. Sends nothing if unchanged, so call it on every `Motion`. With RENDER, `Hand` and `Text` are smooth full-colour cursors in the style of macOS's; otherwise they come from the core cursor font. |
| `define_cursor(c, w, h, &argb, hot) -> bool` | Use your own image for cursor `c`: premultiplied 0xAARRGGBB pixels, `hot` the pixel that points. Needs RENDER. |

## Showing frames and timing

| Call | Notes |
|---|---|
| `present()` | Copies the back buffer to the window, and flushes. |
| `flush()` | Sends what's buffered. Most calls that wait flush first. |
| `sync()` | A round trip: returns once the server has handled everything sent. Events read meanwhile are queued. |
| `fence()` / `fence_wait()` | Mark the frame just presented, and wait for the last mark: one frame in flight. |
| `event() -> Option<Event>` | The next event, never blocking. Also re-shows the back buffer after the window is uncovered. |
| `wait(ms)` | Flushes, then sleeps until an event arrives or `ms` pass (-1: no limit). Non-event messages don't end the sleep. |
| `wait_with(&[fd], ms) -> Vec<bool>` | The same, but also wakes when one of the fds can be read, and says which. |
| `pointer() -> Option<(i32, i32)>` | Where the mouse is, relative to the window, even outside it. A round trip. |

## Events

```rust
pub enum Event {
    KeyDown { key: u32, x: i32, y: i32, mods: u32 },
    KeyUp { key: u32, x: i32, y: i32, mods: u32 },
    ButtonDown { button: u32, x: i32, y: i32, mods: u32 },   // 1 left, 2 middle, 3 right, 4/5 wheel
    ButtonUp { button: u32, x: i32, y: i32, mods: u32 },
    Motion { x: i32, y: i32, mods: u32 },
    Resize { w: i32, h: i32 },
    Close,                                                   // close button, or lost connection
    Paste(String),                                           // after request_paste()
}
```

- `key`: a printable key's Latin-1 code (Shift and Caps Lock applied), or a
  `key::` constant: `BACKSPACE TAB RETURN ESCAPE HOME LEFT UP RIGHT DOWN
  PAGE_UP PAGE_DOWN END INSERT DELETE KP_ENTER F1 (F2.. = F1+1..) SHIFT_L
  SHIFT_R CONTROL_L CONTROL_R ALT_L ALT_R BACKTAB`. Anything else is the
  raw X keysym (see `keysymdef.h`), or 0 if the key has none.
- `mods`: test bits with `MOD_SHIFT`, `MOD_CTRL` or `MOD_ALT` (Mod1: Alt,
  or Option with XQuartz). Other bits are other X state, such as Caps Lock
  (2) and buttons held (256 for button 1).
- A held key repeats as `KeyUp` then `KeyDown`.
- Up to 256 events are queued, and more are dropped. Drain `event()`
  every iteration.

## Limits

- One window per connection, and one thread. Use a single `X11` per
  process.
- The display must be TrueColor at 24 or 32 bits per pixel. Every current
  server is.
- Authentication is MIT-MAGIC-COOKIE-1 from the Xauthority file, or
  none (the server allows the client with `xhost`).
- One extension, RENDER, for anti-aliased text and shapes, translucency
  and cursors. No MIT-SHM, XInput2 or input methods. Clipboard text only,
  without INCR (large transfers). Outline fonts cover Latin-1. See
  `protocol.md` for adding requests.
