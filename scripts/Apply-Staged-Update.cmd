@echo off
REM ASCII only in this file. cmd.exe parses batch files with the OEM codepage
REM (CP949 on Korean Windows), so UTF-8 Korean text corrupts the parser.
REM
REM Applies an update that was staged earlier, right before the bot starts.
REM
REM Why here and not at deploy time: swapping the exe while the bot runs means
REM killing it, and a running bot must not be interrupted just because a build
REM is ready. So the deploy script only copies the new exe next to the old one
REM (mc-musicbot.exe.next) and leaves. This runs at the next start, when nothing
REM holds the file, and puts it in place.
REM
REM Nothing staged -> does nothing at all. Safe to call on every start.

REM Delayed expansion: a variable set inside an if-block is only readable with
REM !name! . With %name% cmd expands it when it parses the block, i.e. before the
REM assignment runs, so it reads empty and the version never gets written.
setlocal enabledelayedexpansion
cd /d "%~dp0.."

set "EXE=bot-mk2\mc-musicbot.exe"
set "NEXT=bot-mk2\mc-musicbot.exe.next"
set "PREV=bot-mk2\mc-musicbot.exe.prev"
set "PENDING=bot-mk2\PENDING_BUILD.txt"

if not exist "%NEXT%" goto :eof

echo [update] A staged build is waiting. Applying it now.

REM Never delete the old exe before the new one is in place. If the move fails
REM after the old one is gone there is nothing left to fall back to, and the
REM bot would not start at all. Move it aside instead, and put it back on error.
if exist "%PREV%" del /f /q "%PREV%" >nul 2>&1
if exist "%EXE%" move /y "%EXE%" "%PREV%" >nul 2>&1
if exist "%EXE%" (
    echo [update] Could not set the old build aside. Keeping it and starting as usual.
    goto :eof
)

move /y "%NEXT%" "%EXE%" >nul 2>&1
if not exist "%EXE%" (
    echo [update] Could not put the new build in place. Rolling back.
    if exist "%PREV%" move /y "%PREV%" "%EXE%" >nul 2>&1
    goto :eof
)

REM Only now is the old one safe to drop.
if exist "%PREV%" del /f /q "%PREV%" >nul 2>&1

REM The staged version number becomes the running one. BUILD_ID.txt is what the
REM bot reports in logs and in the web UI, so it has to move together with the exe.
if exist "%PENDING%" (
    for /f "usebackq delims=" %%v in ("%PENDING%") do set "STAGED=%%v"
    if not "!STAGED!"=="" (
        > BUILD_ID.txt echo !STAGED!
        echo [update] Now running !STAGED!.
    )
    del /f /q "%PENDING%" >nul 2>&1
)

endlocal
