# mc-musicbot 인계 문서

마지막 갱신: 2026-08-07

## 기준 저장소와 역할

- Rust MK2 원본: `<workspace>\musicbot-mk2` (브랜치 `feat/macham-music-remote`)
- C# 본체/포터블 패키징: `<workspace>\discord-my-music-bot`
- NAS 운영 문서: `C:\Users\<user>\NAS-Hub\projects\musicbot-web\README.md`
- **리모컨 사양(단일 기준)**
  - `docs/REMOTE-UI-V2.md` — 기본 사양(권한 모델·성능 계약·화면 구조)
  - `docs/REMOTE-API-V2.md` — API 계약
  - `docs/REMOTE-API-V3.md` — **v3 추가분. §0 에 전체 체크리스트가 있다**

MK2는 봇 호스트 PC에서 실행한다. NAS에서 musicbot 컨테이너를 찾지 않는다. NAS는
`homepage` 다운로드 채널, Cloudflare Tunnel, host-registrar만 담당한다.

---

## 도메인이 둘로 갈렸다 (2026-08-06)

**이걸 모르면 배포가 조용히 깨진다.**

| 주소 | 무엇 | 문지기 |
|---|---|---|
| `musicbot.example.com` | 봇 주인용 **운영 패널** (대시보드·캐시·블랙리스트·OAuth·로그) | Cloudflare Access + 웹 비밀번호 |
| `music.example.com` | **웹 리모컨** (유저 UI + 서버 관리 콘솔) | Discord OAuth. **Access 없음** |

`src/web/mod.rs` 의 `host_scope_guard` 가 Host 헤더로 갈라낸다.
- 리모컨 도메인에서 `/music/*` 와 `/healthz` 외 경로는 **404**
- 리모컨 도메인의 `/` 는 `/music` 으로
- 관리자 도메인의 `/music/*` 는 리모컨 도메인으로 리다이렉트

**리모컨에 Cloudflare Access 를 걸면 안 된다.** Discord 길드 멤버가 쓰는 곳이라
Access 허용목록으로 이중 차단하면 정작 팀원이 못 들어온다.

### `MUSICBOT_PUBLIC_BASE_URL` = `https://music.example.com`

우선순위가 헷갈리기 쉽다. 위에서부터 이긴다.

1. `data\remote-oauth.json` 의 `publicBaseUrl` — **파일이 있으면 환경변수를 아예 안 본다**
   (`RemoteAuthConfig::load`)
2. `bot\remote.env.cmd` — `START-MK2.cmd` 가 자기 `set` 뒤에 `call` 하므로 여기가 환경변수를 이긴다
3. `START-MK2.cmd` 의 `set`

**이 JSON 을 PowerShell 로 고칠 때 BOM 을 넣지 마라.** `Set-Content -Encoding UTF8` 은 BOM 을 붙이고,
그러면 serde 가 파싱에 실패해 OAuth 설정이 통째로 사라진다(실제로 한 번 겪었다).

```powershell
[IO.File]::WriteAllText($path, $json, (New-Object Text.UTF8Encoding($false)))
```

Discord 개발자 포털의 Redirect URI 도 `https://music.example.com/music/oauth/callback` 이어야 한다.

---

## 현재 배포 기준

- Cargo package: `mc-musicbot 0.7.0`
- 봇 호스트 빌드 ID: `20260807-remote-ui-v3`
- `mc-musicbot.exe` SHA256: `8DAA51057DB8DFE9F0DE6F755796648908F073A93731CB6051985271F6E2C63B`
  (32,197,120 bytes)
- 설치 경로: `<portable-root>\`
- 기동: 예약 작업 `MusicBot Portable` → 루트 `START-MK2.cmd` (로그온 트리거)
- `cargo test`: **179 passed / 0 failed**

2026-08-07 배포 검증:
- `music.example.com` 의 `/login` `/botsettings` `/logs` → **404** (도메인 분리 동작)
- `music.example.com/music` → `/music/login` 303, 에셋 4종 200
- `musicbot.example.com` → Access 302 유지
- OAuth `redirect_uri` = `https://music.example.com/music/oauth/callback`

---

## 리모컨 v3 — 무엇이 들어갔나

### 화면
- **배치 6종**: `three`(기본) `two` `focus`(집중) `dj` `talk`(수다) `panel`(도킹)
  DOM 은 한 벌이고 CSS 그리드 배치와 스크롤 주체만 바뀐다.
  **배치 전용 기능을 만들지 마라** — 만드는 순간 배치 6개가 아니라 앱 6개가 된다.
- **테마 7종**: 다크(기본)·라이트·미드나잇·그레이·노르드·베이지·레트로 + `시스템 따라가기`
  전 테마가 `tokens.css` 의 토큰을 **빠짐없이** 재정의한다. 하나라도 빠지면 다크 값이 새어 나온다.
- 크기 조절(드래그 핸들), 우클릭 메뉴, 사람 카드(닉네임 좌클릭), 제안 모달

### 기능
- 정렬 3종(점수·시간·공평) + 5초 재정렬(500곡 초과 시 15초) + 갱신 카운트다운
- 투표 점수 4종 설정화, 싫어요, 붐따, 투표 스킵, 슈퍼 좋아요 쿨타임·하루 제한
- 자동재생 방식 3종(시드/최근N/장르) + 정책 4종 + 아티스트 쿨다운 · 이력 감쇠 · 막힌 후보 기억
- 차트: 외부 22장 + **우리 차트**(서버/봇 전체 × 많이 튼 곡/많이 사랑받은 곡)
- 개인 재생목록, 보관함, 활동 로그 피드, 개인 통계, 웹에서 듣기, 브라우저 검색
- 채팅: 반응 · @멘션 · #노래태그 · 인용 답장

### 권한 10종 + 관리자
`search` `vote` `chat` `playback` `skip` `seek` `volume` `queueEdit` `autoplay` `bulkEnqueue`

관리자 지정 역할(`managerRoleIds`)은 **별도**다. 예전에는 권한용 역할과 겸해서
"검색 역할을 줬더니 관리자가 되는" 문제가 있었다.

### 등급
`Owner`(봇 주인, Discord ID 등록) > `Manager`(서버 관리자) > `Member` > `Viewer`(읽기 전용)

---

## 데이터베이스

| 파일 | 무엇 | 스키마 |
|---|---|---|
| `<dataRoot>/musicbot.sqlite` | 봇 설정·큐·캐시·리모컨. **C# 엔진과 공유** | `remote_*` 는 `PRAGMA user_version` **13** |
| `<dataRoot>/musicbot-stats.sqlite` | **개인 통계와 우리 차트 전용** | `user_version` **1** |

**통계를 본 DB에 넣지 마라.** 본 DB는 C# 엔진과 공유하고 재생 경로가 같은 뮤텍스를 물고 있는데,
통계는 제일 빨리 부푸는 데이터라 거기 두면 통계 쓰기가 재생 쿼리와 락을 다툰다.
통계 DB가 깨져도 봇은 계속 돈다(`Stats::open` 이 `None` 을 주고 통계만 꺼진다).

**통계 쓰기는 재생 경로를 막지 않는다.** `Stats::record()` 로 `mpsc` 에 던지고 즉시 돌아간다.
전용 태스크가 1초 또는 200건마다 트랜잭션 하나로 반영한다. 채널이 차면 **버린다** —
통계 한 줄 때문에 음악이 밀리면 본말전도다.

**레거시 테이블(`settings` `playlists` `playlist_entries` `guild_states` `guild_queue`
`cache_entries` `blacklist` `guild_metadata`)을 건드리지 마라.** C# 엔진과 공유하는 스키마다.
새 테이블은 전부 `remote_` 접두사를 쓴다.

---

## 프런트엔드

**Rust 문자열이 아니라 진짜 파일이다.** `src/web/assets/` 에 있고
`src/web/assets.rs` 가 `include_str!` / `include_bytes!` 로 컴파일 시 임베드한다.

```
tokens.css   디자인 토큰(테마 7종). portal/console 이 공유한다. 색은 여기서만 정의한다.
core.js      WS · 상태 저장소 · API · 렌더 유틸. portal/console 이 공유한다.
portal.js    유저 UI          portal.css
console.js   서버 관리 콘솔     console.css
sw.js        서비스워커        manifest.webmanifest · icon-*.png · favicon.svg
```

`ServeDir` 을 쓰지 마라. 포터블 1241파일 매니페스트와 "exe 하나 SHA 하나" 계약을 깨뜨린다.

### 캐시 — 셋이 겹치면 "배포했는데 화면이 그대로"가 된다

1. 에셋 버전은 `BUILD_ID` 가 아니라 **에셋 내용 해시**로 계산한다(`assets::version()`).
   `BUILD_ID.txt` 는 포터블 배포본에만 있어 개발 중에는 비고, 빈 `?v=` + `immutable` 은 영구 캐시가 된다.
2. `portal.js` 가 `./core.js` 를 정적 import 해서 그 요청에는 `?v=` 가 안 붙는다.
   그래서 `?v=` 가 현재 버전과 **정확히 일치할 때만** `immutable` 을 주고 나머지는 ETag 재검증(304)이다.
3. 페이지 셸은 `no-store` 다. 셸에 CSRF 토큰 · 로그인 정보 · 현재 에셋 버전이 박혀 있다.

서비스워커의 에셋 전략은 **네트워크 우선**이다. stale-while-revalidate 로 두면
배포해도 옛 화면이 계속 나온다.

---

## 빌드와 배포

```powershell
cd <workspace>\musicbot-mk2
cargo test              # 179 passed 여야 한다
cargo build --release
```

봇 호스트(보조 PC `bot-host-pc`, ssh 별칭 `bot-host`)에 올리기:

```powershell
# 반드시 PowerShell 로. Git Bash 의 ssh 는 Windows ssh-agent 를 못 봐서 publickey 거부가 난다.
$root = "<portable-root>"
ssh bot-host "powershell -NoProfile -Command \"Stop-ScheduledTask -TaskName 'MusicBot Portable'; Get-Process mc-musicbot -EA SilentlyContinue | Stop-Process -Force\""
scp "<workspace>\musicbot-mk2\target\release\mc-musicbot.exe" "bot-host:$root/bot-mk2/mc-musicbot.exe"
# 양쪽 SHA256 을 대조한 뒤에만 기동한다
ssh bot-host "powershell -NoProfile -Command \"Start-ScheduledTask -TaskName 'MusicBot Portable'\""
```

**exe 를 덮어쓰기 전에 프로세스를 반드시 죽인다.** 잠긴 파일은 복사도 `cargo build` 도 실패한다.

`scripts\Register-RemoteHosts.ps1` 이 `data\registrar.json` 을 읽어
`musicbot.example.com` 과 `music.example.com` 을 현재 LAN 주소로 등록한다.
설정 파일이 없으면 조용히 건너뛰므로 봇 기동은 막지 않는다.

---

## 자주 밟는 함정

- **`.or(player_channel)`** — 봇이 지금 어디 있는지는 **Discord 캐시의 `voice_states` 만** 본다.
  저장된 `player.voice_channel_id` 는 "다음에 어디로 들어갈까"에만 쓴다.
  같은 실수가 두 번 났고(원래 코드 한 번, 주석까지 달아 놓고 마지막 줄에서 뒤집은 게 한 번),
  지금은 `authoritative_voice_channel(cache, stored)` 가 `stored` 를 의도적으로 버려서 막아 둔다.
- **`0 = 무제한`** — 숫자 설정은 전부 0이 무제한이다. `.max(1)` 같은 클램프를 넣으면 규약이 깨진다.
  `RemoteGuildSettings::sanitize()` 가 저장·로드 양쪽에서 강제한다.
- **`color-scheme`** — Chromium 은 문서의 color-scheme 을 첫 페인트 때 확정하고,
  이후 `data-theme` 이 바뀌어도 CSS 선언을 다시 읽지 않는다.
  테마를 바꿀 때 `style.colorScheme` 을 인라인으로 같이 박아야 스크롤바·폼 위젯이 따라온다.
- **자동재생 판정** — `requester.is_none()` 으로 자동재생을 판정하지 마라.
  `/이전곡` 처럼 사람이 시켰는데 신청자 ID 가 없는 항목이 있어 차트가 조용히 오염된다.
  `StatEvent::played_from_item` 이 `request_kind` 로만 판정한다.
- **`queue.set` 은 개인화 필드를 비워서 나간다.** 모두가 같이 받는 프레임이라 그게 맞고,
  클라이언트가 id 기준으로 `isMine`/`myVote` 를 **병합**해서 지킨다.
  그대로 덮어쓰면 5초마다 내 곡 표시와 내 투표가 사라진다.
- **테스트가 프로덕션 경로를 안 탈 수 있다.** 실제로 "구현은 3벌인데 작동하는 건 0벌"인 채로
  테스트만 초록불인 상태가 있었다. 기능을 넣으면 **화면을 열어 눈으로 확인한다.**

---

## 토큰·비밀값

토큰, Client Secret, 비밀번호, API 키, 개인키는 **어떤 문서에도 기록하지 않는다.**
값이 아니라 어디에 있는지만 적는다.

- Discord 봇 토큰: `bot\botsettings.json` 의 **`token`** (필드 이름이 `discordToken` 이 아니다)
- OAuth Client ID/Secret, 공개 URL, 봇 주인 ID: `data\remote-oauth.json`
- host-registrar 시크릿: `data\registrar.json` (`scripts\registrar.sample.json` 을 복사해 만든다)
- 웹 비밀번호 해시: `data\web-auth.hash`
