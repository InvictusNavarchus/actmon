#!/usr/bin/env bash
# Build from this checkout and replace only the installed executable.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: bash scripts/install.sh [--bin-dir PATH]

Build and install/update Actmon from this checkout (Linux, Rust, C compiler).
  --bin-dir PATH  Existing installation directory (default: /usr/local/bin).
  -h, --help      Show this help.

Run as your normal user; sudo is requested only if the destination needs it.
Services, device permissions, configuration, and recorded data are unchanged.
EOF
}

die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

bin_dir=/usr/local/bin
while (($#)); do
    case "$1" in
        --bin-dir)
            (($# >= 2)) && [[ -n "$2" ]] || die '--bin-dir requires a path.'
            bin_dir=$2
            shift 2
            ;;
        -h | --help) usage; exit 0 ;;
        *) die "Unknown option '$1'. Use --help for usage." ;;
    esac
done

[[ $(uname -s) == Linux ]] || die 'Actmon only runs on Linux.'
[[ -d "$bin_dir" ]] || die "Installation directory does not exist: $bin_dir"
bin_dir=$(cd -- "$bin_dir" && pwd -P)
destination=$bin_dir/actmon
[[ ! -L "$destination" ]] || die "Refusing to replace a symlink: $destination"
[[ ! -e "$destination" || -f "$destination" ]] || die "Not a regular file: $destination"

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$repo"
command -v cargo >/dev/null || die 'cargo is required; install Rust first.'
command -v rustc >/dev/null || die 'rustc is required; install Rust first.'
# Select the host explicitly so cross-compilation settings cannot install a
# foreign executable or cause us to pick up a stale target/release binary.
host=$(rustc -vV | sed -n 's/^host: //p')
[[ -n "$host" ]] || die 'Could not determine the Rust host target.'
printf 'Building Actmon for %s...\n' "$host"
cargo build --locked --release --bin actmon --target "$host" --target-dir "$repo/target"
binary=$repo/target/$host/release/actmon
[[ -f "$binary" && -x "$binary" ]] || die "Build produced no executable at $binary"
version=$("$binary" --version)
[[ "$version" == 'actmon '* ]] || die "Unexpected executable version: $version"

privileged=()
if [[ ! -w "$bin_dir" ]]; then
    command -v sudo >/dev/null || die "sudo is required to install into $bin_dir"
    privileged=(sudo)
fi

stage=''
cleanup() {
    if [[ -n "$stage" ]]; then
        "${privileged[@]}" rm -rf -- "$stage"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A private staging directory on the destination filesystem avoids partial
# replacement and leaves running processes using their existing executable.
stage=$("${privileged[@]}" mktemp -d -- "$bin_dir/.actmon-install.XXXXXX")
"${privileged[@]}" install -m 0755 -Z -- "$binary" "$stage/actmon"
"${privileged[@]}" mv -fT -Z -- "$stage/actmon" "$destination"
printf 'Installed %s at %s\n' "$version" "$destination"
printf 'Running services keep their current executable until restarted.\n'

selected=$(command -v actmon || true)
if [[ -z "$selected" ]]; then
    printf 'Note: %s is not on PATH; shell configuration was not changed.\n' "$bin_dir"
elif [[ $(readlink -f -- "$selected") != "$destination" ]]; then
    printf 'Note: PATH selects %s instead of %s.\n' "$selected" "$destination"
fi
printf 'Check status: %q status\n' "$destination"
