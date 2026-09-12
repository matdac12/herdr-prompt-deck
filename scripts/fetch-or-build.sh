#!/bin/sh
# fetch-or-build.sh -- herdr [[build]] step for prompt-deck (macOS/Linux).
#
# Fast path: download the prebuilt binary matching THIS source's declared version and
# platform from the GitHub release, verify its SHA-256, install it at
# target/release/prompt-deck. Match is by VERSION, not commit.
# Fallback: on ANY miss (no asset, download error, checksum mismatch, unmapped
# platform, no curl/wget) print a clear notice and build from source with cargo.
#
# Overridable via env (PD_REPO_ROOT / PD_CARGO_TOML / PD_OUT / PD_BASE_URL) so the
# logic can be exercised against a mock.
set -u

repo="matdac12/herdr-prompt-deck"

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root="${PD_REPO_ROOT:-$script_dir/..}"
cargo_toml="${PD_CARGO_TOML:-$repo_root/Cargo.toml}"
out="${PD_OUT:-$repo_root/target/release/prompt-deck}"
base_url="${PD_BASE_URL:-https://github.com/$repo/releases/download}"

have() { command -v "$1" >/dev/null 2>&1; }

build_from_source() {
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  if ! have cargo; then
    echo "prompt-deck needs Rust 1.85+ to build, but cargo was not found. Install Rust from https://rustup.rs then re-run: herdr plugin install $repo" >&2
    exit 1
  fi
  exec cargo build --release
}

fallback() {
  echo "prompt-deck: $1 — building from source instead." >&2
  [ -n "${tmpdir:-}" ] && rm -rf "$tmpdir"
  build_from_source
}

download() {
  if have curl; then
    curl -fsSL -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    return 127
  fi
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 127
  fi
}

# --- resolve the target triple ---------------------------------------------------------------
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)
triple=""
case "$os" in
  Darwin)
    case "$arch" in
      arm64|aarch64) triple="aarch64-apple-darwin" ;;
      x86_64|amd64)  triple="x86_64-apple-darwin" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64) triple="x86_64-unknown-linux-musl" ;;
    esac
    ;;
esac
[ -n "$triple" ] || fallback "no prebuilt binary for $os/$arch"

# --- read the version this source declares ---------------------------------------------------
version=$(grep -E '^version *= *"' "$cargo_toml" 2>/dev/null | head -n 1 | sed -E 's/^version *= *"([^"]+)".*/\1/')
[ -n "$version" ] || fallback "could not read version from $cargo_toml"

asset="prompt-deck-$triple"

tmpdir=$(mktemp -d 2>/dev/null) || fallback "could not create a temp dir"
trap 'rm -rf "$tmpdir"' EXIT

# Transparency only: note when this checkout is ahead of the release commit.
ahead_note=""
if have git && git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  head_rev=$(git -C "$repo_root" rev-parse HEAD 2>/dev/null || echo nohead)
  if download "$base_url/v$version/COMMIT" "$tmpdir/COMMIT" 2>/dev/null; then
    release_commit=$(tr -d '[:space:]' < "$tmpdir/COMMIT" 2>/dev/null)
    if [ -n "$release_commit" ] && [ "$head_rev" != "$release_commit" ]; then
      ahead_note=" Note: this checkout ($head_rev) is ahead of the v$version release commit ($release_commit)."
    fi
  fi
fi

bin_url="$base_url/v$version/$asset"
sums_url="$base_url/v$version/SHA256SUMS"
tmpbin="$tmpdir/$asset"
tmpsums="$tmpdir/SHA256SUMS"

download "$bin_url" "$tmpbin"   || fallback "prebuilt binary not available for v$version ($asset)"
download "$sums_url" "$tmpsums" || fallback "checksums not available for v$version"

expected=$(grep -E "^[0-9a-f]{64} [ *]$asset\$" "$tmpsums" 2>/dev/null | awk '{print $1}' | head -n 1)
[ -n "$expected" ] || fallback "no checksum listed for $asset"

actual=$(sha256_of "$tmpbin") || fallback "no sha-256 tool (sha256sum/shasum) available"
if [ "$actual" != "$expected" ]; then
  fallback "checksum mismatch for $asset (expected $expected, got $actual)"
fi

chmod +x "$tmpbin"
mkdir -p "$(dirname "$out")"
mv -f "$tmpbin" "$out" || fallback "could not install the verified binary to $out"
echo "prompt-deck: installed prebuilt v$version ($triple), verified SHA-256.$ahead_note"
exit 0
