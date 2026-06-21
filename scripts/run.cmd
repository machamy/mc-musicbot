@echo off
REM Start the bot as-is (no update check). Builds once if not built yet.
cd /d "%~dp0\.."
if not exist "target\release\mc-musicbot.exe" (
    echo [run] First run - building release binary...
    cargo build --release || exit /b 1
)
"target\release\mc-musicbot.exe"
