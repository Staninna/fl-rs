#!/bin/sh
# Check an FL Studio Data/ directory against corpus/manifest.sha256, the
# 165 shipped projects the tests and docs are pinned to.
#
# Usage: tools/check-corpus.sh "<FL Studio>/Data"
set -eu

if [ $# -ne 1 ]; then
    echo "usage: $0 <FL Studio Data directory>" >&2
    exit 2
fi
manifest="$(cd "$(dirname "$0")/.." && pwd)/corpus/manifest.sha256"
cd "$1"
if command -v sha256sum >/dev/null; then
    sha256sum --quiet -c "$manifest"
else
    shasum -a 256 --quiet -c "$manifest"
fi
echo "all $(wc -l < "$manifest" | tr -d ' ') projects match corpus/manifest.sha256"
