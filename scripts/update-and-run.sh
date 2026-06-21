#!/usr/bin/env bash
# Pull the latest source, rebuild, then start the bot.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "[update] Pulling latest source..."
git pull --ff-only
echo "[update] Building release binary..."
cargo build --release
exec target/release/musicbot-mk2
