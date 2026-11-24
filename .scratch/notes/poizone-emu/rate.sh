#!/bin/bash
# rate.sh <state name> "<events>" frames rate
H=$(cd "$(dirname "$0")" && pwd)
WT=${FL_RS:-$(cd "$H/../../.." && pwd)}
B=$(ls -t $WT/target/debug/deps/plugins-* | grep -v '\.d$' | head -1)
python3 -I $H/emu_poizone.py "$H/states/$1.bin" "$2" $3 64 $4 > $H/rate_ref.txt 2>/dev/null
POIZONE_RATE=$4 POIZONE_STATE="$H/states/$1.bin" POIZONE_EVENTS="$2" POIZONE_FRAMES=$3 POIZONE_OUT=$H/rate_port.txt $B poizone::tests::dump_for_dll --exact > /dev/null 2>&1
python3 -I $H/cmp.py $H/rate_ref.txt $H/rate_port.txt 0
