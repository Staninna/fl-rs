#!/bin/bash
# Usage: probe.sh <state file> [key]
cd /home/user/fl-rs/.claude/worktrees/agent-a11498b8e8c7e6034 || exit 1
export CARGO_TARGET_DIR=/root/work/morphine/target
export MORPHINE_PROBE="$1"
export MORPHINE_KEY="${2:-60}"
cargo test -j 2 --release -p plugins morphine::tests::probe -- --ignored --nocapture 2>&1 | grep -E "s peak|error|panicked" | head -60
