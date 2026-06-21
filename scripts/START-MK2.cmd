@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

REM ASCII only in this file. cmd.exe parses batch files with the OEM codepage
REM (CP949 on Korean Windows), so UTF-8 Korean text corrupts the parser.

if not exist "bot\botsettings.json" (
    echo [!] bot\botsettings.json not found. Fill in the Discord bot token and run again.
    if exist "bot\botsettings.sample.json" (
        copy "bot\botsettings.sample.json" "bot\botsettings.json" >nul
        notepad "bot\botsettings.json"
    )
    pause
    exit /b 1
)

REM Web admin bind address. On the FIRST launch, open http://localhost:8693
REM and set the admin password (first-time setup is allowed from localhost only).
set MUSICBOT_WEB_URLS=http://0.0.0.0:8693

start "MusicBot" bot-mk2\mc-musicbot.exe

echo.
echo ============================================================
echo  MusicBot (Rust / songbird) started.
echo    Web admin: http://localhost:8693
echo    First run: open the web admin to set your password.
echo  Stop: STOP-MK2.cmd
echo ============================================================
timeout /t 5 /nobreak >nul
start "" "http://localhost:8693"
endlocal
