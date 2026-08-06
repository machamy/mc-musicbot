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
set MUSICBOT_PUBLIC_BASE_URL=https://music.example.com
REM Keep OAuth secrets outside the update manifest. Create bot\remote.env.cmd from
REM bot\remote.env.sample.cmd on the host; updates preserve the local file.
if exist "bot\remote.env.cmd" call "bot\remote.env.cmd"
if "%MUSICBOT_DISCORD_CLIENT_ID%"=="" set MUSICBOT_DISCORD_CLIENT_ID=100000000000000001
if "%MUSICBOT_DISCORD_CLIENT_SECRET%"=="" echo [i] Discord OAuth Secret can be configured in the admin UI or bot\remote.env.cmd.

REM Register both the admin and remote hostnames to this PC. The host-local
REM data\registrar.json is preserved by updates and is never in the manifest.
if exist "scripts\Register-RemoteHosts.ps1" if exist "data\registrar.json" powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\Register-RemoteHosts.ps1" -ConfigPath "data\registrar.json"

start "MusicBot" bot-mk2\mc-musicbot.exe

echo.
echo ============================================================
echo  MusicBot (Rust / songbird) started.
echo    Web admin: http://localhost:8693
echo    Admin: https://musicbot.example.com
echo    Remote: https://music.example.com/music
echo    First run: open the web admin to set your password.
echo  Stop: STOP-MK2.cmd
echo ============================================================
timeout /t 5 /nobreak >nul
start "" "http://localhost:8693"
endlocal
