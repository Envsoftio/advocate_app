#!/usr/bin/env sh
# Format MatterDraft source, or report formatting differences with --check.
set -eu

case "$#:${1-}" in
    0:) frontend_task=format ;;
    1:--check) frontend_task=format:check ;;
    *) printf '%s\n' 'Usage: ./format.sh [--check]' >&2; exit 2 ;;
esac

PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$PROJECT_DIR"

for tool in npm cargo ruff; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'Required formatter tool is missing from PATH: %s\n' "$tool" >&2
        exit 1
    fi
done

if [ ! -x frontend/node_modules/.bin/prettier ]; then
    printf '%s\n' 'Install the frontend dependencies before running this script.' >&2
    exit 1
fi

if ! cargo fmt --version >/dev/null 2>&1; then
    printf '%s\n' 'Install rustfmt for the active Rust toolchain before running this script.' >&2
    exit 1
fi

status=0
npm --prefix frontend run "$frontend_task" || status=1

for manifest in rust-backend/Cargo.toml frontend/src-tauri/Cargo.toml backend/pdf-oxide-worker/Cargo.toml; do
    if [ "$frontend_task" = format:check ]; then
        cargo fmt --manifest-path "$manifest" -- --check || status=1
    else
        cargo fmt --manifest-path "$manifest" || status=1
    fi
done

if [ "$frontend_task" = format:check ]; then
    ruff format --check backend/app backend/scripts backend/tests || status=1
else
    ruff format backend/app backend/scripts backend/tests || status=1
fi

exit "$status"
