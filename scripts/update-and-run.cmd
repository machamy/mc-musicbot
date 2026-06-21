@echo off
REM Pull the latest source, rebuild, then start the bot.
cd /d "%~dp0\.."
echo [update] Pulling latest source...
git pull --ff-only
echo [update] Building release binary...
cargo build --release || exit /b 1
"target\release\mc-musicbot.exe"
