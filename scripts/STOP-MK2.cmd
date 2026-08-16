@echo off
REM 정상 종료를 먼저 부탁한다. 봇이 재생 위치를 저장하고 음성에서 깨끗이 빠진 뒤 내려간다.
REM 예전에는 곧장 taskkill /f 였고, 그래서 §24 의 종료 절차가 한 번도 안 돌았다.
setlocal
set "REQ=%~dp0..\data\SHUTDOWN.request"
echo stop requested> "%REQ%" 2>nul

REM 최대 8초 기다린다. 봇은 1초 안에 파일을 보고, 정리에 최대 6초를 쓴다.
for /l %%i in (1,1,16) do (
    tasklist /fi "imagename eq mc-musicbot.exe" 2>nul | find /i "mc-musicbot.exe" >nul || goto :gone
    timeout /t 1 /nobreak >nul
)

echo mc-musicbot did not stop gracefully; forcing.
taskkill /im mc-musicbot.exe /f >nul 2>&1

:gone
REM 강제로 갔으면 요청 파일이 남는다. 치우지 않으면 다음 기동이 곧바로 스스로 꺼진다.
if exist "%REQ%" del /q "%REQ%" >nul 2>&1
echo mc-musicbot stopped.
endlocal
