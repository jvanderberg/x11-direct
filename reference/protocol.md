# The protocol inside, and adding requests

x11-direct covers what small apps need. When an app needs something more,
add a method to `src/lib.rs` (or to the project's copy, `src/x11.rs`). The
encoding is simple. The full specification is "X Window System Protocol"
(x.org/releases/X11R7.7/doc/xproto/x11protocol.html), and its
"Encoding" appendix gives every request's byte layout.

## How requests are encoded

- The client opens with byte order `'l'` (little-endian), so every
  multi-byte value is little-endian.
- A request is a 4-byte header (opcode: u8, a "data" byte that some
  requests use, length: u16 in **4-byte units including the header**)
  followed by its fields. It's padded with zeros to a multiple of 4.
- `self.req(opcode, data, len_words)` starts a request (flushing first if
  the 16 KB buffer would overflow) and counts the sequence number.
  `put8/put16/put32` append fields, and `put_padded(bytes)` appends a
  string and pads it.
- Resource IDs (windows, pixmaps, GCs, fonts, cursors) are allocated by
  the client with `self.new_id()`.
- A request with a reply: send it, then call `self.roundtrip()`, which
  flushes, reads until that reply, queues any events that arrive first,
  and returns `Some((first32bytes, extra))`, or `None` on an X error.
  Reply fields at byte offset 32 and beyond are in `extra`, starting at
  `extra[0]`.
- Events and errors are 32 bytes. `handle()` turns events into `Event`s,
  by code: 2/3 keys, 4/5 buttons, 6 motion, 12 expose, 22 configure,
  33 client message. It prints errors as `x11: error E from request R`,
  where R is the opcode. When a request misbehaves, look for that line on
  stderr.

The library's state: `self.win` (the window), `self.back` (the back
buffer), `self.gc` (the GC for drawing, whose foreground is `self.color`),
`self.root`, and `self.dst()` (the current drawing target).

## Examples

### A UTF-8 window title

`window()` sets `WM_NAME` as Latin-1. Modern window managers prefer
`_NET_WM_NAME` in UTF-8:

```rust
pub fn title_utf8(&mut self, title: &str) {
    let (prop, utf8) = (self.atom("_NET_WM_NAME"), self.atom("UTF8_STRING"));
    let t = title.as_bytes();
    let win = self.win;
    self.req(18, 0, 6 + (t.len() as u32).div_ceil(4)); // ChangeProperty, mode Replace
    self.put32(win);
    self.put32(prop);
    self.put32(utf8);
    self.put32(8);                                       // format 8, then 3 unused bytes
    self.put32(t.len() as u32);                          // length in format units
    self.put_padded(t);
}
```

`atom()` is a round trip, so look atoms up once and keep them.

### The bell

```rust
pub fn bell(&mut self) {
    self.req(104, 0, 1); // Bell, percent 0 (the data byte)
}
```

### Cursors

`cursor()` is built in now (the cursor font, or a RENDER cursor from
`define_cursor`). Create each cursor once, keep its ID, and switch with
ChangeWindowAttributes (2, value mask 0x4000), which is 16 bytes.

### A new event: focus in and out

1. Add to the window's event mask in `window()`: FocusChange is
   `0x200000`. Masks: KeyPress 0x1, KeyRelease 0x2, ButtonPress 0x4,
   ButtonRelease 0x8, EnterWindow 0x10, LeaveWindow 0x20,
   PointerMotion 0x40, Exposure 0x8000, StructureNotify 0x20000,
   FocusChange 0x200000, PropertyChange 0x400000.
2. Add `Event::Focus(bool)` to the enum.
3. In `handle()`: `9 => self.push(Event::Focus(true)), 10 => self.push(Event::Focus(false)),`
   (FocusIn and FocusOut). EnterNotify (7) and LeaveNotify (8) work the
   same way, for hover that ends when the pointer leaves.

### GC values

`change_gc(mask, value)` sets one GC value. The masks and their order
(values go in mask-bit order when several are set at once): function 0x1,
plane-mask 0x2, foreground 0x4, background 0x8, line-width 0x10,
line-style 0x20, cap-style 0x40, join-style 0x80, fill-style 0x100,
fill-rule 0x200, tile 0x400, stipple 0x800, tile/stipple x 0x1000, y 0x2000,
font 0x4000, subwindow-mode 0x8000, graphics-exposures 0x10000,
clip x 0x20000, clip y 0x40000, clip-mask 0x80000, dash-offset 0x100000,
dashes 0x200000, arc-mode 0x400000.

For example, dashed lines are `change_gc(0x20, 1)` (LineOnOffDash), and
XOR drawing (a rubber-band rectangle you can erase by drawing it again)
is `change_gc(0x1, 6)` (GXxor, with foreground = the XOR mask). Set it
back afterwards (function 3 is GXcopy), because every later call uses
the same GC.

## Bigger additions: what they involve

- **Clipboard:** built in (`set_clipboard`, `request_paste`). The parts
  that are easy to get wrong: answer SelectionRequest (30) from the event
  loop, never with a round trip of your own while handling another
  reply; send a SelectionNotify with property None for targets you
  don't support; and read the pasted property with GetProperty only
  after SelectionNotify (31) arrives, outside the event handler.
- **More than one window:** keep a list of window IDs, give each its own
  back buffer, and route events by the window field: offset 12 in key,
  button and motion events, 4 in Expose, 8 in ConfigureNotify.
- **MIT-SHM** (shared-memory images, local server only): call
  QueryExtension (98) with the name to get the extension's major opcode,
  then send its requests with that opcode and the minor opcode in the
  data byte. RENDER in `lib.rs` is a worked example: QueryVersion, then
  QueryPictFormats to find the A8, RGB24 and ARGB32 formats, then
  CreatePicture on the drawables it draws to.

## RENDER, as used here

- **Text:** a glyph set in A8 format (CreateGlyphSet), glyphs added in
  batches under 16 KB (AddGlyphs, each glyph's rows padded to 4 bytes),
  then CompositeGlyphs8 with a solid-fill picture (CreateSolidFill) as the
  source. The client rasterizes the outlines, so the server needs no
  fonts at all.
- **Shapes:** Triangles (minor 11) with op Over and an A8 mask format,
  which is what makes the edges smooth. Coordinates are 16.16 fixed point.
- **Translucency:** FillRectangles (minor 26) with op Over. Colours are
  16 bits per channel and premultiplied by alpha.
- **Cursors:** a depth-32 pixmap, PutImage of premultiplied ARGB, a
  picture in the ARGB32 format, CreateCursor (minor 27), then free the
  picture and pixmap: the cursor keeps its own copy.
- A drawable needs a picture before RENDER can draw on it. Make it once
  per canvas and free it with the canvas, or the server leaks it.
- An untrusted client (`ssh -X`, the SECURITY extension) is told RENDER
  doesn't exist. `ssh -Y` is trusted.

## Debugging

- `x11: error E from request R` on stderr: E 2 BadValue, 3 BadWindow,
  4 BadPixmap, 7 BadFont, 8 BadMatch (often a depth mismatch: copies
  between canvases and masks need the same depth), 9 BadDrawable,
  13 BadGC, 14 BadIDChoice, 16 BadLength (the length field doesn't match
  what was sent).
- Count the bytes in a new request against the spec. Most bugs are a
  wrong length word or missing padding, and they corrupt everything sent
  after them.
- To find which call caused an error, call `self.sync()` after it while
  debugging. The error then arrives while that call's caller is on the
  stack.
