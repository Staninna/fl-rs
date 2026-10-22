#!/usr/bin/env bash
# Render every project under "Demo projects" to a WAV, the same projects
# tools/play-demos.sh plays. The WAVs mirror the folders under "Demo
# projects" in target/demo-renders/<commit>/, marked -dirty when the tree
# had uncommitted changes outside .scratch, unless --out=DIR names another.
#
# Usage: tools/demo-render.sh [--out=DIR] ["<FL Studio>/Data"]
# The Data directory defaults to $FL_RS_DATA.
set -euo pipefail

cd "$(dirname "$0")/.."

commit="$(git rev-parse --short HEAD)"
git diff --quiet HEAD -- . ':(exclude).scratch' || commit="$commit-dirty"

dir="target/demo-renders/$commit"
data="${FL_RS_DATA:-}"
for arg in "$@"; do
    case "$arg" in
        --out=*) dir="${arg#--out=}" ;;
        -*) echo "unknown option $arg" >&2; exit 2 ;;
        *) data="$arg" ;;
    esac
done
if [ -z "$data" ]; then
    echo "usage: $0 [--out=DIR] <FL Studio Data directory>  (or set FL_RS_DATA)" >&2
    exit 2
fi
demos="$data/Demo projects"
if [ ! -d "$demos" ]; then
    echo "no 'Demo projects' directory under $data" >&2
    exit 1
fi
export FL_RS_DATA="$data"

cargo build --release -p fl-rs

mapfile -d '' files < <(find "$demos" -name '*.flp' -print0 | sort -z)
total=${#files[@]}

failed=0
for i in "${!files[@]}"; do
    f="${files[i]}"
    rel="${f#"$demos"/}"
    name="$(basename "$f" .flp)"
    case "$rel" in
        "Demo songs/"*) kind="song" ;;
        */*) kind="project (${rel%%/*})" ;;
        *) kind="project" ;;
    esac
    out="$dir/${rel%.flp}.wav"
    mkdir -p "$(dirname "$out")"
    printf '[%d/%d] %s: %s\n' "$((i + 1))" "$total" "$kind" "$name"
    start=$SECONDS
    if target/release/fl-rs --render "$f" "$out"; then
        echo "    $((SECONDS - start))s"
    else
        echo "    failed" >&2
        failed=$((failed + 1))
    fi
done
echo "$((total - failed)) of $total rendered to $dir"
[ "$failed" -eq 0 ]
