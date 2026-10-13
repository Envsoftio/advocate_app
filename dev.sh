#!/usr/bin/env sh
set -eu

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BACKEND_DIR="$PROJECT_DIR/backend"
FRONTEND_DIR="$PROJECT_DIR/frontend"
DEV_TOKEN="development-token-change-me"
DEV_PORT="8765"

if [ -x "$BACKEND_DIR/.venv312/bin/python" ]; then
  PYTHON="$BACKEND_DIR/.venv312/bin/python"
elif command -v python3.12 >/dev/null 2>&1; then
  python3.12 -m venv "$BACKEND_DIR/.venv312"
  PYTHON="$BACKEND_DIR/.venv312/bin/python"
else
  echo "Python 3.12 is required. Install it, then run ./dev.sh again." >&2
  exit 1
fi

if ! "$PYTHON" -c 'import fastapi, uvicorn, pymupdf' >/dev/null 2>&1; then
  "$PYTHON" -m pip install -r "$BACKEND_DIR/requirements-dev.txt"
fi

if [ ! -d "$FRONTEND_DIR/node_modules" ]; then
  (cd "$FRONTEND_DIR" && npm install)
fi

cleanup() {
  if [ -n "${BACKEND_PID:-}" ]; then
    kill "$BACKEND_PID" 2>/dev/null || true
    wait "$BACKEND_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM HUP

(
  cd "$BACKEND_DIR"
  COURTBUNDLE_API_TOKEN="$DEV_TOKEN" exec "$PYTHON" -m uvicorn app.main:create_app \
    --factory --reload --host 127.0.0.1 --port "$DEV_PORT"
) &
BACKEND_PID=$!

cd "$FRONTEND_DIR"
COURTBUNDLE_API_TOKEN="$DEV_TOKEN" \
COURTBUNDLE_DEV_BACKEND_URL="http://127.0.0.1:$DEV_PORT" \
npm run tauri:dev:hot
