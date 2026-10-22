#!/bin/bash
# run.sh <state name (in states/, without .bin)> <key> <frames> [release]
H=$(cd "$(dirname "$0")" && pwd)
D=${SAKURA_WORK:?set SAKURA_WORK to a directory with states/ in it}
st="$D/states/$1.bin"
tag=$(echo "$1" | tr -c 'A-Za-z0-9\n' '_')
rel=${4:--1}
id="${tag}_$2_$3_$rel"
mkdir -p "$D/out"
if [ ! -s "$D/out/ref_$id.txt" ]; then
  SAKURA_INNER="$D/out/refin_$id" ${PYTHON:-python3} -I "$H/emu_sakura.py" "$st" "$2" "$3" 512 44100 "$rel" > "$D/out/ref_$id.txt"
fi
envrel=()
[ "$rel" != "-1" ] && envrel=(SAKURA_RELEASE="$rel")
BIN=${BIN:-$(ls -t "${CARGO_TARGET_DIR:-target}"/release/deps/plugins-* | grep -v "\.d$" | head -1)}
env SAKURA_STATE="$st" SAKURA_KEY="$2" SAKURA_FRAMES="$3" "${envrel[@]}" SAKURA_OUT="$D/out/port_$id.txt" SAKURA_INNER="$D/out/portin_$id" \
  "$BIN" sakura::tests::dump_for_dll --exact >/dev/null 2>&1
# Stages (set STAGES) need the port to write them too; see ../sakura.md.
for k in ${STAGES-}; do echo stage $k:; ${PYTHON:-python3} -I "$H/cmp.py" "$D/out/refin_$id.$k" "$D/out/portin_$id.$k" "${TOL:-1e-6}" | head -${LINES:-8}; done
echo out:
${PYTHON:-python3} -I "$H/cmp.py" "$D/out/ref_$id.txt" "$D/out/port_$id.txt" "${TOL:-1e-6}" | head -${LINES:-8}
