#!/usr/bin/env bash
# Play every project under "Demo projects" in turn. Closing the window
# starts the next one; Ctrl+C stops the run.
#
# With --rate, each closed window asks how it played (fzf; type your own
# answer if none fits, Esc skips) and records it in a Markdown table,
# .scratch/demo-ratings.md unless --rate=FILE names another. Earlier
# ratings stay for the projects you skip. Each rating records the commit
# the binary was built from, marked -dirty when the tree had uncommitted
# changes outside .scratch.
#
# Usage: tools/play-demos.sh [--rate[=FILE]] ["<FL Studio>/Data"]
# The Data directory defaults to $FL_RS_DATA.
set -euo pipefail

cd "$(dirname "$0")/.."

out=""
data="${FL_RS_DATA:-}"
for arg in "$@"; do
    case "$arg" in
        --rate) out=".scratch/demo-ratings.md" ;;
        --rate=*) out="${arg#--rate=}" ;;
        -*) echo "unknown option $arg" >&2; exit 2 ;;
        *) data="$arg" ;;
    esac
done
if [ -z "$data" ]; then
    echo "usage: $0 [--rate[=FILE]] <FL Studio Data directory>  (or set FL_RS_DATA)" >&2
    exit 2
fi
demos="$data/Demo projects"
if [ ! -d "$demos" ]; then
    echo "no 'Demo projects' directory under $data" >&2
    exit 1
fi
if [ -n "$out" ] && ! command -v fzf >/dev/null; then
    echo "--rate needs fzf" >&2
    exit 1
fi
export FL_RS_DATA="$data"

commit="$(git rev-parse --short HEAD)"
git diff --quiet HEAD -- . ':(exclude).scratch' || commit="$commit-dirty"
cargo build --release -p fl-rs

mapfile -d '' files < <(find "$demos" -name '*.flp' -print0 | sort -z)
total=${#files[@]}

kinds=() names=()
for f in "${files[@]}"; do
    rel="${f#"$demos"/}"
    case "$rel" in
        "Demo songs/"*) kinds+=("song") ;;
        */*) kinds+=("project (${rel%%/*})") ;;
        *) kinds+=("project") ;;
    esac
    names+=("$(basename "$f" .flp)")
done

# Ratings keyed by "kind|name", loaded from the table rows of an earlier run.
declare -A rating rated built
trim() { local s="${1#"${1%%[! ]*}"}"; printf '%s' "${s%"${s##*[! ]}"}"; }
if [ -n "$out" ] && [ -f "$out" ]; then
    while IFS='|' read -r _ kind name r d c _; do
        kind="$(trim "$kind")" name="$(trim "$name")" r="$(trim "$r")" d="$(trim "$d")" c="$(trim "$c")"
        case "$kind" in song|project*) ;; *) continue ;; esac
        [ "$r" = "-" ] && continue
        rating["$kind|$name"]="$r"
        rated["$kind|$name"]="$d"
        built["$kind|$name"]="$c"
    done < "$out"
fi

write_ratings() {
    {
        echo "# Demo project ratings"
        echo
        echo "How each FL Studio demo project plays in fl-rs. Written by \`tools/play-demos.sh --rate\`."
        echo
        echo "| Kind | Name | Rating | Rated | Commit |"
        echo "| --- | --- | --- | --- | --- |"
        for j in "${!files[@]}"; do
            key="${kinds[j]}|${names[j]}"
            echo "| ${kinds[j]} | ${names[j]} | ${rating[$key]:--} | ${rated[$key]:--} | ${built[$key]:--} |"
        done
    } > "$out"
}

ask_rating() {
    local picked
    picked="$(printf '%s\n' smooth "mostly smooth" choppy "wrong sound" silent crashed "won't load" |
        fzf --print-query --height=12 --reverse --prompt="$1 > " \
            --header="How did it play? Type your own answer if none fits, Esc skips." || true)"
    # fzf prints the typed query, then the pick; with no match only the query.
    if [ "$(wc -l <<<"$picked")" -ge 2 ]; then
        sed -n 2p <<<"$picked"
    else
        head -n1 <<<"$picked"
    fi
}

for i in "${!files[@]}"; do
    printf '\n[%d/%d] %s: %s\n' "$((i + 1))" "$total" "${kinds[i]}" "${names[i]}"
    target/release/fl-rs "${files[i]}" </dev/null || echo "exited with status $?" >&2
    [ -z "$out" ] && continue
    key="${kinds[i]}|${names[i]}"
    r="$(ask_rating "${names[i]}" | tr '|' '/')"
    if [ -n "$r" ]; then
        rating["$key"]="$r"
        rated["$key"]="$(date +%F)"
        built["$key"]="$commit"
        write_ratings
        echo "rated: $r"
    fi
done
if [ -n "$out" ]; then
    write_ratings
    echo "ratings in $out"
fi
