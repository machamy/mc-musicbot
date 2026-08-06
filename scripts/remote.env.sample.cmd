@echo off
REM Copy this file to bot\remote.env.cmd on the bot host. Never commit the copy.
set MUSICBOT_DISCORD_CLIENT_ID=100000000000000001
set MUSICBOT_DISCORD_CLIENT_SECRET=PASTE_DISCORD_OAUTH_CLIENT_SECRET_HERE
REM 리모컨(사용자 포털)의 공개 주소다. 관리자 주소(musicbot.example.com)가 아니다.
REM START-MK2.cmd 가 이 파일을 나중에 call 하므로, 여기 값이 최종적으로 이긴다.
REM Discord 개발자 포털의 Redirect URI도 https://music.example.com/music/oauth/callback 이어야 한다.
set MUSICBOT_PUBLIC_BASE_URL=https://music.example.com
