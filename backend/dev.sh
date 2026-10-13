#!/usr/bin/env sh
set -eu
: "${COURTBUNDLE_API_TOKEN:=development-token-change-me}"
export COURTBUNDLE_API_TOKEN
cd "$(dirname "$0")"
python launcher.py --port "${COURTBUNDLE_PORT:-8765}"
