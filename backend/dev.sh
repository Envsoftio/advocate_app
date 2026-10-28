#!/usr/bin/env sh
set -eu
: "${MATTERDRAFT_API_TOKEN=${COURTBUNDLE_API_TOKEN-development-token-change-me}}"
export MATTERDRAFT_API_TOKEN
cd "$(dirname "$0")"
python launcher.py --port "${MATTERDRAFT_PORT:-${COURTBUNDLE_PORT:-8765}}"
