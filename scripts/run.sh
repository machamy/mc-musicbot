#!/usr/bin/env bash
# Start the bot as-is (no update check). Builds once if not built yet.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -x target/release/musicbot-mk2 ] || cargo build --release
exec target/release/musicbot-mk2
