#!/usr/bin/env sh
# Direct Tauri IPC: no Python process and no Rust HTTP server.
set -eu
PROJECT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "$#:${1-}" in
    0:|1:--vault) export MATTERDRAFT_BACKEND=rust
        MATTERDRAFT_DEV_SCOPE='isolated protected store and scoped native file selection' ;;
    1:--synthetic) export MATTERDRAFT_BACKEND=rust-synthetic
        MATTERDRAFT_DEV_SCOPE='isolated synthetic store and the pinned PDF fixture' ;;
    *) echo 'Usage: ./dev-rust.sh [--vault | --synthetic]' >&2; exit 2 ;;
esac
if [ -z "${MATTERDRAFT_CMYK_PROFILE+x}" ] && command -v shasum >/dev/null 2>&1; then
    for PROFILE_CANDIDATE in \
        /opt/homebrew/opt/ghostscript/share/ghostscript/iccprofiles/default_cmyk.icc \
        /usr/local/opt/ghostscript/share/ghostscript/iccprofiles/default_cmyk.icc
    do
        if [ -f "$PROFILE_CANDIDATE" ] && PROFILE_DIGEST=$(shasum -a 256 "$PROFILE_CANDIDATE" 2>/dev/null); then
            if [ "${PROFILE_DIGEST%% *}" = 8472fa1493a024b800b67dee9424835ec0c41ab79490200ae8ec4a689fd1b9a9 ]; then
                export MATTERDRAFT_CMYK_PROFILE="$PROFILE_CANDIDATE"
                break
            fi
        fi
    done
fi
node -e 'const net=require("node:net");const server=net.createServer();server.once("error",()=>{console.error("Port 1420 is occupied. Stop the existing development frontend before starting Rust IPC mode.");process.exit(1)});server.listen(1420,"127.0.0.1",()=>server.close());'
printf '%s\n' "MatterDraft Rust development mode ($MATTERDRAFT_BACKEND) uses an $MATTERDRAFT_DEV_SCOPE."
cd "$PROJECT_DIR/frontend"
npm run tauri:dev:hot
