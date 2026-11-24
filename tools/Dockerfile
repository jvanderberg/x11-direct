# A headless X server for trying x11-direct programs: Xvfb with the classic
# bitmap fonts (fixed, helvetica, lucidatypewriter), Rust, xdotool to send
# keys and clicks, and ImageMagick to save the screen as a PNG.
# Built and used by x11shot.sh.
FROM rust:1-slim-bookworm
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        xvfb x11-utils x11-xserver-utils xdotool imagemagick \
        xfonts-base xfonts-75dpi xfonts-100dpi \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /work
