#!/usr/bin/env bash
# Compile le relais pour Raspberry Pi (64 et 32 bits, binaires statiques) et
# prépare le dossier dist/ki-plotter-relay-pi/ à copier sur le Pi.
#
# Prérequis (une fois) :
#   rustup target add aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf
#   cargo install cargo-zigbuild
#   pip install ziglang        (ou zig dans le PATH)
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

if ! command -v zig >/dev/null 2>&1; then
  zigdir="$(python -c 'import ziglang, os; print(os.path.dirname(ziglang.__file__))' 2>/dev/null | tr -d '\r' || true)"
  # Git Bash on Windows: C:\Users\… → /c/Users/…
  if [ -n "$zigdir" ] && command -v cygpath >/dev/null 2>&1; then zigdir="$(cygpath -u "$zigdir")"; fi
  if [ -n "$zigdir" ]; then export PATH="$zigdir:$PATH"; fi
fi
command -v zig >/dev/null 2>&1 || { echo "zig introuvable : pip install ziglang" >&2; exit 1; }

out="$root/dist/ki-plotter-relay-pi"
rm -rf "$out"
mkdir -p "$out"
for pair in "aarch64-unknown-linux-musl:aarch64" "armv7-unknown-linux-musleabihf:armv7"; do
  target="${pair%%:*}"
  name="${pair##*:}"
  CARGO_TARGET_DIR=target/pi cargo zigbuild --release -p kp_relay --target "$target"
  cp "target/pi/$target/release/ki-plotter-relay" "$out/ki-plotter-relay-$name"
done
for f in install.sh ki-plotter-relay.service ki-plotter-relay.conf LISEZMOI.md; do
  # Unix line endings whatever the checkout did: a CRLF install.sh fails on the Pi.
  sed 's/\r$//' "deploy/raspberry-pi/$f" > "$out/$f"
done
chmod +x "$out/install.sh" "$out"/ki-plotter-relay-*
echo "Prêt : $out"
