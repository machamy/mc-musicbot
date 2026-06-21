# MusicBot

Rust로 만든 셀프호스팅 디스코드 음악봇입니다. [serenity](https://github.com/serenity-rs/serenity) + [songbird](https://github.com/serenity-rs/songbird) 기반이고, **웹 관리자 패널**이 봇과 하나의 프로세스로 함께 돌아갑니다.

- 🎵 YouTube / YouTube Music / SoundCloud 재생 (`yt-dlp` + `ffmpeg`)
- 🔁 대기열 — 반복(없음/한곡/전체), 셔플, **자동추천(autoplay 라디오)**
- 🔎 디스코드 안에서 검색 후 골라 재생 (`/검색`, `/사클검색`)
- 🧠 다운로드 캐시(LRU) — 같은 곡은 다시 받지 않고 즉시 재생
- 🚫 차단 목록(제목/URL, 길드별·전역) · 📜 저장 플레이리스트(전역·길드)
- 🖥️ **웹 관리자** — 대시보드 · 재생설정 · 캐시 · 로그 · 차단목록 · 플레이리스트 · 비밀번호
- 🇰🇷 한국어 + 영어 슬래시 명령

---

## 기존 음악봇과 비교

대형 **호스티드** 음악봇(남의 서버에서 돌아가는 봇)들은 YouTube의 제재로 줄줄이 사라졌습니다 — Rythm·Groovy는 2021년 종료, Hydra는 2023년 음악 기능 제거. 호스티드 봇은 설치가 필요 없지만 **운영 주체가 언제든 닫을 수 있고**, 재생 요청이 제3자 서버를 거칩니다.

그래서 현실적인 비교 대상은 **셀프호스팅 오픈소스** 봇들(JMusicBot, Muse, FredBoat 등)입니다. 이들과 같은 진영에서, 이 봇의 위치는 다음과 같습니다.

| | **이 봇** | JMusicBot | Muse | FredBoat | 호스티드(Jockie 등) |
|---|---|---|---|---|---|
| 셀프호스팅 / 오픈소스 | ✅ / ✅ | ✅ / ✅ | ✅ / ✅ | ✅ / ✅ | ❌ / ❌ |
| 런타임 | **Rust 단일 실행파일** | Java(JVM) | Node.js(TS) | Java + Lavalink | — |
| 웹 관리자 패널 | ✅ **풀 패널** | ❌ (텍스트·DJ롤) | ❌ (슬래시+임베드) | 제한적 | 일부 프리미엄 대시보드 |
| 로컬 다운로드 캐시 | ✅ (LRU) | — | ✅ | — | — |
| 자동추천(라디오) | ✅ | — | — | — | 일부 |
| 소스 | YouTube/YTM/SoundCloud | YouTube/SC/Bandcamp/Twitch… | YouTube/Spotify→YT | YouTube/SC/Bandcamp | YouTube/Spotify/SC |
| 셧다운 리스크 | 없음(내가 운영) | 없음 | 없음 | 없음 | 있음(전례 다수) |

**이 봇만의 차별점**

- **Rust 단일 바이너리(~26MB).** JVM(JMusicBot·FredBoat)이나 Node 런타임이 필요 없고 메모리가 가볍습니다.
- **풀 웹 관리자 패널.** 대부분의 셀프호스트 봇은 명령/DJ롤 중심인데, 이 봇은 브라우저에서 재생설정·캐시 라이브러리·로그·차단목록·플레이리스트·비밀번호까지 관리합니다.
- **자동추천 + 로컬 캐시 내장.** 큐가 비면 라디오식으로 비슷한 곡을 이어주고, 받아둔 곡은 즉시 재생됩니다.
- **운영 안정성은 `yt-dlp`에 위임.** YouTube가 바뀌어 다운로드가 깨지면 `yt-dlp`만 업데이트하면 됩니다(다른 yt-dlp 기반 봇과 동일한 정비 모델).

> 공정을 위해: JMusicBot은 설치가 가장 간단하고 문서가 풍부합니다. Muse는 Spotify 링크를 YouTube로 자동 변환해줍니다. 이 봇은 그 둘이 없는 **웹 관리자 + 자동추천**을 단일 Rust 바이너리로 제공하는 쪽에 강점이 있습니다.

---

## 로컬 세팅 (단계별)

처음부터 끝까지 따라 하면 됩니다. 예시는 Windows 기준이며, Linux/macOS도 도구 설치 명령만 다릅니다.

### 1) Rust 설치

[rustup](https://rustup.rs/)으로 설치합니다.

```sh
# Windows: https://rustup.rs 에서 rustup-init.exe 실행
# Linux/macOS:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

설치 후 `cargo --version`이 나오면 됩니다. (edition 2024 → Rust 1.85 이상 권장)

### 2) ffmpeg 설치 (yt-dlp는 자동)

봇은 `ffmpeg`로 오디오를 디코딩합니다. `PATH`에 두거나 봇 옆 `tools/` 폴더에 넣으세요.

```sh
# Windows
winget install Gyan.FFmpeg          # 또는: scoop install ffmpeg
# Debian/Ubuntu
sudo apt install ffmpeg
# macOS
brew install ffmpeg
```

**`yt-dlp`는 직접 설치하지 않아도 됩니다.** 봇이 처음 켤 때 `yt-dlp`가 `PATH`/`tools/`에 없으면 **GitHub 최신 릴리스에서 `tools/`로 자동 다운로드**하고, 봇이 받은 yt-dlp는 **하루 1회 자동 업데이트**(`yt-dlp -U`)합니다. (YouTube 변경으로 인한 다운로드 실패를 예방 — 웹 설정에서 끌 수 있고, 직접 설치한 `PATH`/시스템 yt-dlp는 건드리지 않습니다.)

> 미리 깔고 싶으면 `winget install yt-dlp.yt-dlp` / `pip install -U yt-dlp` / `brew install yt-dlp`도 됩니다.

### 3) 디스코드 봇 만들기 (토큰 + 인텐트 + 초대)

1. [Discord Developer Portal](https://discord.com/developers/applications) → **New Application** 생성.
2. 좌측 **Bot** 탭 → **Reset Token** → **토큰 복사**(한 번만 보임, 안전하게 보관).
3. 같은 **Bot** 탭에서 **Privileged Gateway Intents** 활성화:
   - **MESSAGE CONTENT INTENT** ✅
   - **SERVER MEMBERS INTENT** ✅
4. 좌측 **OAuth2 → URL Generator**:
   - Scopes: **`bot`** + **`applications.commands`**
   - Bot Permissions: **Connect**, **Speak**, **Send Messages**, **Embed Links**, **Read Message History** (Use Slash Commands 포함)
   - 생성된 URL로 봇을 내 서버에 초대.

### 4) 소스 받고 빌드

```sh
git clone https://github.com/machamy/mc-musicbot.git
cd mc-musicbot
cargo build --release
```

결과물: `target/release/musicbot-mk2` (Windows는 `musicbot-mk2.exe`).

### 5) 설정 파일 만들기

샘플을 복사해 토큰을 채웁니다.

```sh
cp botsettings.sample.json botsettings.json
```

```json
{
  "token": "3단계에서_복사한_봇_토큰",
  "dataRoot": ".musicbot-data",
  "toolsRoot": "tools"
}
```

| 필드 | 기본값 | 설명 |
|------|--------|------|
| `token` | — | 디스코드 봇 토큰 (**필수**) |
| `dataRoot` | `.musicbot-data` | DB·캐시·로그·웹 비밀번호 해시 저장 위치(설정 파일 기준 상대경로) |
| `toolsRoot` | `tools` | `ffmpeg`/`yt-dlp`를 먼저 찾을 폴더(없으면 `PATH` 탐색) |
| `ytDlpPath` / `ffmpegPath` | 자동 | 도구 경로 직접 지정 |
| `registerGuildId` | — | 슬래시 명령을 특정 길드에 **즉시** 등록(전역은 최대 1시간) |
| `botOwnerUserId` | — | 전역 플레이리스트 소유자 |

> `botsettings.json`은 `.gitignore` 대상이라 커밋되지 않습니다. 봇은 실행파일 옆 → `../bot/` → `../` → 현재 디렉터리 순으로 이 파일을 찾습니다.
>
> **명령이 바로 안 뜨면**: 전역 등록은 디스코드 반영까지 최대 1시간 걸립니다. 테스트 중엔 `registerGuildId`에 서버 ID를 넣으면 즉시 등록됩니다.

### 6) 실행

직접 실행:

```sh
./target/release/musicbot-mk2      # Windows: target\release\musicbot-mk2.exe
```

또는 편의 스크립트로 — **그냥 시작** vs **업데이트 후 시작** 두 가지를 둡니다:

| 목적 | Windows | Linux/macOS |
|------|---------|-------------|
| 그냥 시작 (업데이트 확인 X) | `scripts\run.cmd` | `scripts/run.sh` |
| 최신 받아서 시작 (`git pull`→재빌드→시작) | `scripts\update-and-run.cmd` | `scripts/update-and-run.sh` |

콘솔에 `Registered N slash commands` / `Discord bot connected as ...`가 뜨면 성공입니다.

> **봇 업데이트 = 소스 갱신 후 재빌드**입니다. `update-and-run`이 `git pull` + `cargo build --release`를 해줍니다. (자동 정기 업데이트는 없음 — 셀프호스팅이라 직접 갱신합니다. 단, **`yt-dlp`는 봇이 자동으로 최신 유지**합니다.)

### 7) 웹 관리자 — 최초 비밀번호 설정

웹 관리자는 기본 `http://0.0.0.0:8693`에서 동작합니다(`MUSICBOT_WEB_URLS`로 변경 가능).

처음엔 **비밀번호가 없습니다.** 봇이 돌아가는 **그 PC에서** `http://localhost:8693`에 접속하면 설정 페이지가 떠 비밀번호를 정합니다. **보안상 최초 설정은 localhost(같은 PC)에서만** 가능합니다. 이후엔 어디서든 로그인하고, 사이드바 **비밀번호 변경**에서 바꿉니다.

> 운영 시 `MUSICBOT_WEB_PASSWORD` 환경변수로 비밀번호를 고정할 수도 있습니다(저장값보다 우선).

### 8) 외부 노출(선택)

집 밖에서 웹 관리자에 접속하려면 `8693` 포트를 직접 열기보다 **Cloudflare Tunnel**이나 리버스 프록시 뒤에 두고, 가능하면 별도 인증(Access 등)을 한 겹 더 두는 것을 권장합니다. 봇 자체의 비밀번호는 그 안쪽의 방어선입니다.

---

## 슬래시 명령

재생/대기열: `/재생` `/바로재생` `/검색` `/사클검색` `/대기열` `/현재곡` `/스킵` `/지정스킵` `/정지` `/나가기` `/일시정지` `/재개` `/이동시간` `/이전곡` `/다시재생`

대기열 편집: `/이동` `/제거` `/큐비우기` `/셔플` `/반복`

설정/정보: `/볼륨` `/평준화` `/자동추천` `/플레이리스트` `/상태`

영어 별칭(`/play`, `/queue`, `/leave`, `/status` …)도 모두 동작합니다.

---

## 웹 관리자 패널

| 페이지 | 내용 |
|--------|------|
| 메인 대시보드 | 봇 프로세스 · 도구 상태 |
| 진단 / 상태 | 길드별 재생 · 자동추천 · 큐 |
| 재생 설정 | 볼륨 · 평준화 · 자동추천 기본값 · 자동퇴장 · 알림 · 인트로 제거(SponsorBlock) · 비트레이트 |
| 봇 설정 / 공용 설정 | 토큰·명령 등록·소유자·경로 (읽기 전용) |
| 서버 설정 | 길드별 볼륨/자동추천 override |
| 도구 / 캐시 · 캐시 라이브러리 | yt-dlp/ffmpeg 상태 · 받아둔 곡 관리 |
| 플레이리스트 · 차단 목록 · 로그 뷰어 | CRUD · 최근 운영 로그 |
| 비밀번호 변경 | 웹 관리자 비밀번호 |

---

## 라이선스

MIT — [LICENSE](LICENSE) 참고.
