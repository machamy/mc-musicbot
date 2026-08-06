# mc-musicbot 인계 문서

마지막 갱신: 2026-07-20

## 기준 저장소와 역할

- Rust MK2 원본: `<workspace>\musicbot-mk2`
- C# 본체/포터블 패키징: `<workspace>\discord-my-music-bot`
- NAS 운영 문서: `C:\Users\<user>\NAS-Hub\projects\musicbot-web\README.md`
- 상세 배포 감사: `C:\Users\<user>\NAS-Hub\docs\musicbot-deployment-audit-20260720.md`

MK2는 봇 호스트 PC에서 실행한다. NAS에서 musicbot 컨테이너를 찾지 않는다. NAS는
`homepage` 다운로드 채널, Cloudflare Tunnel, host-registrar만 담당한다.

## 현재 배포 기준

- Cargo package: `mc-musicbot 0.7.0`
- NAS build ID: `20260720-021334`
- portable manifest: `1241` files
- canonical/portable/NAS `mc-musicbot.exe`
  - size: `26756096` bytes
  - SHA256: `d9c7b0ac83f99edf48e8a31f441a887f973cedfc30a72730bc05a17ee32cf131`

2026-07-20 검증:

- `cargo test`: 성공(정의된 단위 테스트 0개, 미사용 코드 경고 4건)
- `cargo build --release`: 성공
- C# 경량 회귀 테스트: `24/24` 성공
- C# bot/CLI/admin/web 포터블 publish: 성공
- 포터블 updater: NAS build/file count를 읽고 `up to date` 판정
- NAS `homepage`: running, restart 0, OOM false

## 빌드와 배포

```powershell
cd <workspace>\musicbot-mk2
cargo test
cargo build --release

cd <workspace>\discord-my-music-bot
dotnet run --project tests\DiscordMyMusicBot.Tests\DiscordMyMusicBot.Tests.csproj
powershell -ExecutionPolicy Bypass -File scripts\Publish-Windows.ps1
powershell -ExecutionPolicy Bypass -File scripts\Sync-PortableToNas.ps1 -BuildId yyyyMMdd-HHmmss
```

`Publish-Windows.ps1`는 `target\release\mc-musicbot.exe`를
`dist\portable-win-x64\bot-mk2\mc-musicbot.exe`로 복사한다. 실행 파일명은
`musicbot-mk2.exe`가 아니라 `mc-musicbot.exe`다.

동기화 전에 다음을 확인한다.

```powershell
Get-FileHash -Algorithm SHA256 `
  <workspace>\musicbot-mk2\target\release\mc-musicbot.exe
Get-FileHash -Algorithm SHA256 `
  <workspace>\discord-my-music-bot\dist\portable-win-x64\bot-mk2\mc-musicbot.exe
```

두 SHA가 같지 않으면 NAS에 올리지 않는다.

## 자동업데이트 계약

- `UPDATE.cmd` → `scripts\Update-Portable.ps1`
- 기본 LAN manifest:
  `http://<registrar-ip>:8849/downloads/musicbot-files-manifest.json`
- 기본 LAN files:
  `http://<registrar-ip>:8849/downloads/portable`
- bot 설정과 `data/`는 보존한다.
- 변경 파일을 SHA256 검증 후 교체한다.
- 잠긴 실행파일 교체 전 C# bot/web와 `mc-musicbot` 프로세스를 종료한다.

`Sync-PortableToNas.ps1`는 빌드 ID를 해싱 전에 포터블의 `BUILD_ID.txt`에 기록해야 한다.
2026-07-20 이전에는 이 단계가 없어 NAS에 이전 BUILD_ID가 고아 파일로 남을 수 있었고,
현재 스크립트에서 수정했다.

`home.example.com/downloads`는 Cloudflare Access 보호 대상이다. 현재 개인 봇 호스트의
LAN 업데이트는 정상이나, 외부 네트워크에서 인증 없는 무인 업데이트 origin으로 쓸 수는 없다.

## 실행 관계

- `START-MK2.cmd`: Rust/songbird 엔진
- `START.cmd`: C# 엔진
- 두 엔진은 같은 설정/DB/cache/tools와 웹 포트 `8693`을 공유하므로 동시 실행하지 않는다.
- 포터블 샘플의 `bot\botsettings.json`은 `dataRoot=..\data`, `toolsRoot=..\tools`를 쓴다.
- 토큰, 비밀번호, 개인키는 저장소와 이 문서에 기록하지 않는다.

## 작업트리 주의

2026-07-20 기준 MK2 저장소에는 사용자가 수정한 `LICENSE`가 있었고 이를 보존했다.
C# 저장소에는 기능 코드, 문서, 빌드 산출물을 포함한 대규모 미커밋 변경이 있다.
정리·reset·checkout하지 말고 현재 변경의 소유자를 확인한 뒤 이어서 작업한다.

## 알려진 경고와 후속 확인

- MK2: 미사용 메서드/필드 경고 4건. 배포 차단 사항은 아니다.
- C# publish: native DLL에 대한 `MSB3246 PE image does not have metadata` 경고 2건.
  publish는 성공했지만 실제 봇 호스트 기동 후 Discord 음성 접속과 1곡 재생 smoke test가 필요하다.
- MK2 테스트 모듈은 점수 정렬, 대기 점수, 투표/개인 좋아요, 채팅 반응/신고, 가사/권한 테스트 8개를 실행한다.

## 2026-08-06 마참뮤직 서버별 리모컨

- 사용자 포털: `/music`, 길드 화면: `/music/guilds/{guild_id}`
- Discord OAuth2 Authorization Code, 메모리 세션, HttpOnly/SameSite 쿠키와 CSRF 검증
- 현재 길드 멤버·봇 참가·역할·음성 채널 재검증
- 검색, 재생/진행률/가사, 점수 큐, 투표, 채팅/반응/신고, 재생목록, 개인 보관함, 감사 로그
- 기능별 권한, 지정 관리 역할, 볼륨·큐·곡 길이·로그 보존 제한
- SQLite `remote_*` 테이블은 기존 DB에 자동 생성되고 PlayerManager가 최종 재생 상태를 유지한다.
- WebSocket 이벤트와 2초 스냅샷 동기화로 Discord 명령의 상태 변경도 복구한다.
- Rust 테스트 8개, JavaScript 구문 검사, 로컬 HTTP 성공/거부 경로 통합 검사를 통과했다.

운영 OAuth:

- Client ID: `100000000000000001`
- Redirect URI: `https://musicbot.example.com/music/oauth/callback`
- `START-MK2.cmd`가 호스트 전용 `bot\remote.env.cmd`를 선택적으로 호출한다.
- 포터블의 `bot\remote.env.sample.cmd`를 복사해 Client Secret을 입력한다.
- 실제 `remote.env.cmd`는 Git과 NAS 매니페스트에 넣지 않는다.
- Secret이 없으면 관리자 UI는 동작하지만 `/music`은 OAuth 설정 안내를 표시한다.
