#!/bin/bash
# Harmless Ghidra run 5: the shape table helpers, the selector handlers
# and the maths helpers the table builders call.
cd /shared/code/fl-studio-target/ghidra || exit 1
export JAVA_HOME=/home/thc/opt/jdk-21.0.12+8 GHIDRA_HEADLESS_MAXMEM=5G
H=/home/thc/opt/ghidra_12.1.2_PUBLIC/support/analyzeHeadless
PR=/shared/code/fl-studio-target/ghidra/proj
OUT=/shared/code/fl-studio-target/ghidra/out/harmless
FUNCS=0x0077c970,0x0077c920,0x0077c940,0x0077c0f0,0x0060ff50,0x00791c60,0x007921e0,0x00788220,0x00409520,0x0040a0d0,0x007826f0,0x007451e0,0x00745480,0x00744fc0,0x0074bb70
$H $PR harmless -process Harmless_x64.dll -noanalysis -scriptPath /shared/code/fl-rs/tools/ghidra \
  -postScript DecompileAddrs.java "$OUT/run5" "$FUNCS" callees > logs/harmless-r5.log 2>&1
echo "exit $?"
grep -E "ERROR|no function|decompile failed" logs/harmless-r5.log | head -8
