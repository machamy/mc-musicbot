@echo off
taskkill /im mc-musicbot.exe /f >nul 2>&1
echo mc-musicbot stopped.
timeout /t 2 /nobreak >nul
