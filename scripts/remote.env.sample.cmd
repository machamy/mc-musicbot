@echo off
REM Copy this file to bot\remote.env.cmd on the bot host. Never commit the copy.
set MUSICBOT_DISCORD_CLIENT_ID=PASTE_DISCORD_APPLICATION_ID_HERE
set MUSICBOT_DISCORD_CLIENT_SECRET=PASTE_DISCORD_OAUTH_CLIENT_SECRET_HERE
REM 리모컨(사용자 포털)의 공개 주소다. 관리자 주소가 아니다.
REM START-MK2.cmd 가 이 파일을 나중에 call 하므로, 여기 값이 최종적으로 이긴다.
REM Discord 개발자 포털의 Redirect URI도 https://<리모컨 호스트>/music/oauth/callback 이어야 한다.
set MUSICBOT_PUBLIC_BASE_URL=https://music.example.com

REM 관리자 패널과 리모컨을 서로 다른 호스트명으로 갈라내는 값이다(src/web/mod.rs 의 host_scope_guard).
REM 비워 두면 분리가 꺼져서 리모컨 도메인에서도 /botsettings · /logs 가 열린다.
set MUSICBOT_ADMIN_HOST=musicbot.example.com
set MUSICBOT_REMOTE_HOST=music.example.com
