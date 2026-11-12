#!/usr/bin/env bash
# Downloads open-source SKY130 and GF180MCU designs and builds ready-to-open IC Studio projects:
# examples/sky130 (caravel_clocking, housekeeping, chip_io; with --caravel also the full chip)
# and examples/gf180 (7-track standard cells and a 512x8 SRAM macro), each with the PDK's
# KLayout layer set imported as its technology. Downloads land in examples/downloads and are
# reused; libraries already in a project are not imported again.
#
#   scripts/fetch-examples.sh [--caravel] [--force] [--dest DIR]
#
#   --caravel  also caravel.gds (52 MB download, 278 MB unpacked)
#   --force    rebuild the projects from the downloads
#
# The `ics` CLI is taken from $ICS, target/release, or `cargo run --release -p ics-cli`.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo/examples"
caravel=0
force=0
while [ $# -gt 0 ]; do
  case "$1" in
    --caravel) caravel=1 ;;
    --force) force=1 ;;
    --dest) dest="$2"; shift ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -n "${ICS:-}" ]; then
  ics_cmd=("$ICS")
elif [ -x "$repo/target/release/ics" ]; then
  ics_cmd=("$repo/target/release/ics")
else
  ics_cmd=(cargo run --quiet --release -p ics-cli --manifest-path "$repo/Cargo.toml" --)
fi

ics() {
  "${ics_cmd[@]}" "$@" || { echo "error: ics $* failed" >&2; exit 1; }
}

fetch() { # url out
  if [ -f "$2" ]; then
    echo "  have   $(basename "$2")"
    return
  fi
  mkdir -p "$(dirname "$2")"
  echo "  fetch  $1"
  if ! curl -fL --retry 2 --connect-timeout 20 -sS -o "$2.part" "$1"; then
    rm -f "$2.part"
    echo "error: download failed: $1" >&2
    echo "  Check the network connection and run the script again; finished downloads are kept." >&2
    exit 1
  fi
  mv "$2.part" "$2"
}

unpack() { # file.gz -> file
  local out="${1%.gz}"
  if [ ! -f "$out" ]; then
    echo "  unpack $(basename "$1")"
    gunzip -c "$1" > "$out.part"
    mv "$out.part" "$out"
  fi
}

build() { # project lyp lyt lib=gds...
  local dir="$1" lyp="$2" lyt="$3"
  shift 3
  echo "project $dir"
  if [ "$force" = 1 ]; then rm -rf "$dir"; fi
  if [ ! -f "$dir/ic-studio.json" ]; then
    ics new "$dir" --template empty
    ics import-tech "$dir" "$lyp" --lyt "$lyt"
  fi
  local have
  have="$(ics info "$dir" | sed -n 's|^ *\([^ ]*\)/$|\1|p')"
  for pair in "$@"; do
    local lib="${pair%%=*}" gds="${pair#*=}"
    if printf '%s\n' "$have" | grep -qx "$lib"; then
      echo "  library $lib already imported"
    else
      echo "  import $lib"
      ics import-gds "$dir" "$gds" --lib "$lib"
    fi
  done
}

downloads="$dest/downloads"

echo "== SKY130 (efabless caravel)"
sky_tech="https://raw.githubusercontent.com/RTimothyEdwards/open_pdks/master/sky130/klayout"
sky="$downloads/sky130"
fetch "$sky_tech/sky130.lyp" "$sky/sky130.lyp"
fetch "$sky_tech/sky130.lyt" "$sky/sky130.lyt"
designs=(caravel_clocking housekeeping chip_io)
if [ "$caravel" = 1 ]; then designs+=(caravel); fi
libs=()
for d in "${designs[@]}"; do
  fetch "https://raw.githubusercontent.com/efabless/caravel/main/gds/$d.gds.gz" "$sky/$d.gds.gz"
  unpack "$sky/$d.gds.gz"
  libs+=("$d=$sky/$d.gds")
done
build "$dest/sky130" "$sky/sky130.lyp" "$sky/sky130.lyt" "${libs[@]}"

echo "== GF180MCU (wafer.space)"
gf_base="https://raw.githubusercontent.com/wafer-space/gf180mcu/main/gf180mcuD"
gf="$downloads/gf180"
fetch "$gf_base/libs.tech/klayout/tech/gf180mcu.lyp" "$gf/gf180mcu.lyp"
fetch "$gf_base/libs.tech/klayout/tech/gf180mcu.lyt" "$gf/gf180mcu.lyt"
libs=()
for pair in \
  "gf180mcu_fd_sc_mcu7t5v0=gf180mcu_fd_sc_mcu7t5v0/gds/gf180mcu_fd_sc_mcu7t5v0.gds" \
  "gf180mcu_fd_ip_sram=gf180mcu_fd_ip_sram/gds/gf180mcu_fd_ip_sram__sram512x8m8wm1.gds"; do
  lib="${pair%%=*}" path="${pair#*=}"
  out="$gf/$(basename "$path")"
  fetch "$gf_base/libs.ref/$path" "$out"
  libs+=("$lib=$out")
done
build "$dest/gf180" "$gf/gf180mcu.lyp" "$gf/gf180mcu.lyt" "${libs[@]}"

echo
echo "Done. Open a project with:"
echo "  cargo run --release -- $dest/sky130"
echo "  cargo run --release -- $dest/gf180"
