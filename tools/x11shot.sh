#!/usr/bin/env bash
# x11shot.sh: build a Rust X11 program with Cargo, run it on a headless X
# server (Xvfb, in Docker), optionally send it keys and clicks, and save the
# screen as a PNG: a way to see what a program draws without a display.
#
#   tools/x11shot.sh [options] [-- ARGS...]      (in the Cargo project)
#
#   -e NAME      run example NAME (cargo --example); -b NAME: binary NAME;
#                neither: the package's binary
#   -o OUT.png   where to save the screen (default: x11shot.png); with
#                shots taken mid-way (-i "...; shot; ..."), OUT-1.png,
#                OUT-2.png, ... with the last one after all the input
#   -s WxH       screen size (default 800x600)
#   -w SECONDS   how long to let it run before the input and the shot (default 1)
#   -i STEPS     input, after the wait: xdotool commands separated by ';',
#                such as "mousemove 200 100; click 3; key Down Return".
#                "sleep N" pauses; "shot" saves the screen there.
#   -l           show everything the program printed
#
# Run it in the Cargo project (or workspace) directory; it is mounted in the
# container, and so is this skill's directory, at the same path, so a path
# dependency on x11-direct works. The build is a Linux one, kept in a
# Docker volume (x11-direct-target), not in ./target. The window is at the
# top left (there is no window manager), and keys go to the window under
# the pointer, so move the pointer onto it first: "mousemove 50 50; key q".
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
SKILL=$(dirname "$HERE")
IMAGE=x11-direct-xvfb
out=x11shot.png size=800x600 wait=1 steps= sel=() showlog=0
while getopts "e:b:o:s:w:i:l" o; do
  case $o in
    e) sel=(--example "$OPTARG") ;; b) sel=(--bin "$OPTARG") ;;
    o) out=$OPTARG ;; s) size=$OPTARG ;; w) wait=$OPTARG ;;
    i) steps=$OPTARG ;; l) showlog=1 ;;
    *) sed -n '2,26p' "$0" >&2; exit 2 ;;
  esac
done
shift $((OPTIND - 1))
[[ ${1:-} == -- ]] && shift
[[ -f Cargo.toml ]] || { echo "x11shot: run it where Cargo.toml is" >&2; exit 2; }
command -v docker >/dev/null || { echo "x11shot: needs Docker" >&2; exit 1; }
docker image inspect $IMAGE >/dev/null 2>&1 ||
  docker build -q -t $IMAGE "$HERE" >/dev/null
case $out in
  /*) rel=${out#"$PWD"/}; [[ $rel == /* ]] && { echo "x11shot: -o must be under $PWD" >&2; exit 2; } ;;
  *) rel=$out ;;
esac
mkdir -p "$(dirname "$out")"
# one Linux target directory per project, in a volume
tdir=/target/$(printf %s "$PWD" | cksum | cut -d' ' -f1)

skill_mount=()
[[ $SKILL != "$PWD" && $PWD != "$SKILL"/* ]] && skill_mount=(-v "$SKILL:$SKILL:ro")
docker run --rm -i -v "$PWD:$PWD" "${skill_mount[@]}" -w "$PWD" \
  -v x11-direct-target:/target -v x11-direct-cargo:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR="$tdir" -e SIZE="$size" -e WAIT="$wait" -e STEPS="$steps" \
  -e OUT="$rel" -e SHOWLOG="$showlog" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  $IMAGE bash -s -- "${sel[@]}" -- "$@" <<'EOF'
set -u
sel=() args=()
while [[ $# -gt 0 && $1 != -- ]]; do sel+=("$1"); shift; done
[[ $# -gt 0 ]] && shift
args=("$@")
# build; the executable's path comes from cargo's JSON messages
exe=$(cargo build --release "${sel[@]}" --message-format=json-render-diagnostics 2>/tmp/build.log |
      sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' | tail -1)
if [[ -z $exe || ! -x $exe ]]; then
  echo "x11shot: build failed"; cat /tmp/build.log; exit 1
fi
grep -E '^(warning|error)' /tmp/build.log | head -20
export DISPLAY=:99
Xvfb :99 -screen 0 "${SIZE}x24" -nolisten tcp >/tmp/xvfb.log 2>&1 &
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
xdpyinfo >/dev/null 2>&1 || { echo "x11shot: Xvfb didn't start"; cat /tmp/xvfb.log; exit 1; }
xhost +local: >/dev/null 2>&1      # no Xauthority cookies in x11-direct
"$exe" "${args[@]}" </dev/null >/tmp/app.log 2>&1 &
pid=$!
sleep "$WAIT"
n=0 base=${OUT%.png}
shot() { n=$((n + 1)); import -window root "$base-$n.png"; }
IFS=';' read -ra st <<< "$STEPS"
for s in "${st[@]}"; do
  s=$(echo "$s" | sed 's/^ *//; s/ *$//')
  case $s in
    "") ;;
    shot) shot ;;
    sleep\ *) sleep "${s#sleep }" ;;
    *) xdotool $s; sleep 0.3 ;;
  esac
done
if [[ $n -gt 0 ]]; then shot; else import -window root "$OUT"; fi
if kill -0 $pid 2>/dev/null; then
  state="still running (stopped now)"; kill $pid 2>/dev/null; wait $pid 2>/dev/null
else
  wait $pid; state="exited with status $?"
fi
chown "$HOST_UID:$HOST_GID" "$base"*.png 2>/dev/null
if [[ $n -gt 0 ]]; then ls "$base"-*.png | sed 's/^/saved: /'; else echo "saved: $OUT"; fi
echo "program: $state"
if [[ $SHOWLOG == 1 ]]; then echo "--- output:"; cat /tmp/app.log
elif [[ -s /tmp/app.log ]]; then echo "--- output (last lines):"; tail -5 /tmp/app.log; fi
EOF
