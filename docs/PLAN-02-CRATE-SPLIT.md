# 계획 02 — 크레이트로 나누기 ("어셈블리 분리")

작성: 2026-08-16 · 상태: **계획. 구현 없음.**
선행: [PLAN-01-SESSION-CORE.md](PLAN-01-SESSION-CORE.md) 의 1단계 (경계를 먼저 정해야 한다)

> **1단계는 2026-08-17 에 구현됐다 (`5843087`).** 아래 "지금 상태" 절은 그 이전을 적은
> 것이라 이제 역사다 — 크레이트는 셋이고, `src/main.rs` 는 244줄이 아니라 20줄이다.
> **그리고 1단계의 설계가 바뀌었다**: 자산만 떼면 이득이 0이라(의존 크레이트가 바뀌면
> 종속 크레이트도 재컴파일된다) 본체까지 크레이트로 내리고 자산을 **주입**한다.
> 결과 54.4초 → 2.5초. 2단계 이후는 아직 계획 그대로다.

> "내부 어셈블리? dll? 그런 걸 분리해도 되고"

러스트에는 DLL 경계가 없다(있긴 하지만 ABI 가 불안정해서 안 쓴다). 대신 **워크스페이스
크레이트**가 정확히 같은 일을 한다 — 컴파일 단위가 갈리고, **의존 방향이 컴파일러에
의해 강제된다.** C# 어셈블리 분리로 얻으려는 것(순환 참조 금지, 계층 강제, 빌드 분리)이
그대로 나온다.

---

## 지금 상태

**크레이트 하나. 34개 파일, 40,148줄.** (`Cargo.toml` 에 `[[bin]]` 도 워크스페이스도 없다)

### 그래서 생기는 일 — 실측

| 측정 | 값 |
|---|---|
| `portal.js` 한 글자 고치고 `cargo build --release` | **54.4초** |
| 이유 | `include_str!`(`src/web/assets.rs:19-43`) 이 자산을 바이너리에 용접한다. 자산이 바뀌면 크레이트 전체가 다시 컴파일된다 |

CSS 한 줄 고치는 데 40,148줄을 다시 컴파일한다. 크레이트를 나누는 것만으로 이게
"자산 크레이트 재컴파일 + 링크"로 줄어든다.

### 그리고 계층이 강제되지 않는다

지금 `src/player/coordinator.rs` 가 `crate::web::remote::bot_voice_status_of` 를 부른다.
재생 코드가 웹 모듈을 부르는 것이다. 한 크레이트 안에서는 컴파일러가 못 막는다.
**크레이트가 갈리면 이건 컴파일 에러가 된다** — 그게 목적이다.

---

## 이미 절반은 나뉘어 있다

serenity·songbird 를 import 하는 파일: **34개 중 8개뿐이다.**

| 디스코드를 아는 파일 | 줄 수 |
|---|---|
| `src/web/remote.rs` | 12,750 |
| `src/commands/handlers.rs` | 2,424 |
| `src/player/coordinator.rs` | 1,354 |
| `src/app.rs` | 630 |
| `src/player/side_effects.rs` | 605 |
| `src/commands/embeds.rs` | 496 |
| `src/events.rs` | 278 |
| `src/main.rs` | 244 |
| **합계** | **18,781 (47%)** |

나머지 **21,367줄 (53%) 은 이미 디스코드를 모른다** — `remote/store.rs`(5,668),
`remote/models.rs`(3,273), `player/manager.rs`(1,641), `stats.rs`(1,448), `db.rs`(672),
`media/*`, `models.rs`, `config.rs`, `logging.rs`, `blacklist.rs` 전부.

그리고 47% 중 제일 큰 `web/remote.rs` 의 디스코드 의존은 **얇다** — serenity `Cache`
읽기 ~14곳과 `GuildId::new`/`UserId::new`/`Permissions` 생성자뿐이고, 디스코드 REST 는
serenity 가 아니라 **자체 reqwest 클라이언트**로 부른다. `Songbird` 핸들은 웹에서
**0회** 쓴다.

> 요약: **경계는 이미 거의 그어져 있다. 크레이트는 그걸 컴파일러에게 알려 주는 일이다.**

---

## 목표 배치

```
mc-musicbot/                     ← 워크스페이스 루트
├── crates/
│   ├── mc-clock/        시각표. 순수. 의존: chrono, serde 뿐
│   ├── mc-domain/       트랙·대기열·설정·점수 타입. 의존: mc-clock
│   ├── mc-store/        SQLite. 의존: mc-domain
│   ├── mc-media/        yt-dlp · ffmpeg · 캐시 · 리졸버. 의존: mc-domain, mc-store
│   ├── mc-engine/       세션·대기열 진행·자동재생. 의존: clock/domain/store/media
│   │                    ★ 여기까지 디스코드를 모른다
│   ├── mc-assets/       JS·CSS·아이콘 바이트 + 버전 해시. 의존: sha2
│   ├── mc-web/          axum. 의존: engine, store, media, domain, assets
│   └── mc-discord/      serenity + songbird. 의존: engine
└── src/main.rs          배선만. 의존: 전부
```

**의존은 위에서 아래로만 흐른다.** `mc-engine` 이 `mc-web` 이나 `mc-discord` 를
`Cargo.toml` 에 적을 수 없다 — 그게 이 계획의 전부다.

> **교차검증에서 고친 것 셋.** 처음 초안은 의존을 더 얇게 그렸는데 실제 코드와 안 맞았다.
>
> | 초안 | 사실 |
> |---|---|
> | `mc-assets` 의존 **없음** | `src/web/assets.rs` 가 axum 응답 타입(`:11-14`)과 `sha2`(`:54`·`:107`)를 직접 쓴다 → **바이트+해시만 크레이트로 내리고, HTTP 응답 만드는 부분은 `mc-web` 에 남긴다** |
> | `mc-media` 는 domain 만 의존 | `CacheManager` 가 `Db`·`LogService` 를, `spawn_auto_update` 가 `App`·`Config` 를 요구한다 → store 의존을 인정하고, `spawn_auto_update` 는 배선 계층(`main.rs`)으로 올린다 |
> | `mc-web` 은 engine·assets 만 | 웹이 `RemoteStore`·`Db`·통계·`media::resolver`·도메인 타입을 **직접** 쓴다(`app.remote` 87회, `app.db` 21회 등) → 그걸 facade 로 감싸는 것은 이 계획의 목표가 아니다. **의존을 정직하게 그린다** |
>
> 마지막 줄이 중요하다. **크레이트를 나눈다고 웹이 얇아지지는 않는다.**
> 얻는 것은 "웹이 엔진을 안 거치고 songbird 를 만질 수 없다" 는 보장이지,
> "웹이 DB 를 안 본다" 가 아니다.

### 크레이트별 성격

| 크레이트 | 성격 | 지금 어디에 있나 |
|---|---|---|
| `mc-clock` | **순수 함수 + 데이터.** I/O 0. 테스트가 시간을 안 기다린다 | `coordinator.rs` 의 `TrackSchedule`·`VirtualSession::position`·`started_utc` 산술 |
| `mc-domain` | 타입만 | `models.rs`, `remote/models.rs` |
| `mc-store` | sqlite | `db.rs`, `remote/store.rs`, `stats.rs` |
| `mc-media` | 자식 프로세스 | `media/*` |
| `mc-engine` | 상태 기계 | `player/manager.rs`, `player/autoplay.rs`, `coordinator.rs` 의 **디스코드 아닌 절반** |
| `mc-assets` | 바이트 | `web/assets.rs` + `web/assets/*` |
| `mc-web` | HTTP·WS | `web/*` |
| `mc-discord` | 게이트웨이·음성 | `commands/*`, `events.rs`, `coordinator.rs` 의 songbird 절반 |

---

## 벽 넷

### 벽 1. `App` 이 단일 루트다

`src/app.rs:110-178` — 27개 필드에 DB·로그·플레이어·코디네이터·songbird·serenity Http·
Cache·훅 4개가 전부 한 구조체에 들어 있다. **모든 크레이트가 `App` 을 알아야 하면
크레이트를 나눈 의미가 없다.**

→ `App` 을 쪼개서 각 계층이 자기 아래 것만 들게 한다. 마지막에 남는 `App` 은
`src/main.rs` 안의 배선 구조체가 된다.

### 벽 2. 훅 4개가 방향을 거꾸로 만든다

```
src/app.rs:142  on_queue_sorted      코어 → 웹
src/app.rs:145  on_restarting        코어 → 웹
src/app.rs:148  on_chart_prefetch    코어 → 웹
src/app.rs:154  web_listener_count   코어 → 웹  ★ 동기 읽기, 재생 hot path
```

앞의 셋은 **이벤트 방출**이라 쉽다 — `mc-engine` 이 broadcast 채널을 내놓고 `mc-web` 이
구독하면 방향이 맞는다.

넷째가 문제다. `src/player/coordinator.rs:484-488` 이 재생 판단 도중에 웹 메모리를
동기적으로 읽는다. **방향을 뒤집어야 한다** — 리스너 수가 바뀔 때 웹이 엔진에
`set_listener_count(room, n)` 으로 밀어 넣고, 엔진은 자기가 들고 있는 값을 본다.
(그러면 [PLAN-04](PLAN-04-ZERO-DOWNTIME.md) 의 프로세스 분리도 공짜로 가능해진다.)

### 벽 3. 재생 코드가 웹 함수를 부른다

```
src/player/coordinator.rs:359, :441, :758   → crate::web::remote::bot_voice_status_of
src/commands/handlers.rs:11                 → crate::web::remote::{MemberContext, permission_allowed}
```

`bot_voice_status_of` 는 이름과 달리 웹과 상관없다 — serenity `Cache` 를 읽는 함수가
어쩌다 웹 모듈에 앉아 있는 것이다. → `mc-discord` 로 옮긴다.

`permission_allowed` 는 진짜 도메인 규칙이다. → `mc-domain` 으로 옮긴다.
([PLAN-03](PLAN-03-DISCORD-OPTIONAL.md) 이 이걸 다시 다룬다.)

### 벽 4. `web/remote.rs` 12,750줄이 한 파일이다

크레이트를 나누기 전에 파일을 나눠야 한다. 이건 순수하게 기계적인 작업이지만
**테스트 76개가 이 파일 안에 있다** — 옮기면서 잃어버리면 안전망이 사라진다.

---

## 단계

각 단계는 **그 자체로 배포 가능**하고, 끝났을 때 동작이 지금과 **바이트 단위로 같아야**
한다. 크레이트 분리는 기능 변경이 아니다.

### 1단계 — 워크스페이스 껍데기 + `mc-assets` (하루)

제일 값어치 있고 제일 위험이 없다. 자산은 아무것도 의존하지 않는다.

| 할 일 | 주의 |
|---|---|
| `Cargo.toml` 을 워크스페이스로 바꾸고 `crates/mc-assets` 를 만든다 | |
| **바이트 상수 + 버전 해시만** 옮긴다 | `serve_asset` 등 axum 응답 함수는 `mc-web` 에 남긴다 (위 표) |
| `lookup` 화이트리스트도 같이 옮긴다 | 이름→바이트 대응은 자산의 책임이다 |

**완료조건**

- `crates/mc-assets/Cargo.toml` 의 의존이 `sha2` 하나
- `portal.js` 한 글자 수정 후 `cargo build --release` **10초 이하** (지금 54.4초)
- 다음 응답이 **바이트 단위로 동일** — 분리 전 저장해 둔 것과 `sha256sum` 비교:
  `/music/assets/portal.js` · `portal.css` · `core.js` · `console.js` · `console.css` ·
  `tokens.css` · `apidoc.css` · `sw.js` · `manifest.webmanifest` · `favicon.svg` ·
  `icon-192.png` · `icon-512.png` · `icon-180.png`
- 자산 버전 해시 문자열이 분리 전과 **같다**
- `remote_page.rs` 가 셸에 박는 `?v=` 값이 위 해시와 일치한다

> 이 단계 하나로 일상 개발 속도가 바뀐다. 나머지 단계가 미뤄져도 이건 남는다.

**반증 시험**: 옮기는 김에 `portal.js` 끝에 개행 하나를 더해 보면 위 SHA 비교가
**실패해야** 한다. 안 실패하면 비교 대상을 잘못 잡은 것이다.

### 2단계 — `mc-clock` 을 떼어낸다 (PLAN-01 1단계와 같은 일)

시각표 계산은 이미 거의 순수하다. 순수한 것만 골라 크레이트로 옮기면
**컴파일러가 그 순수성을 영구히 지켜 준다** — `mc-clock` 의 `Cargo.toml` 에
tokio 도 rusqlite 도 songbird 도 없으므로 누가 나중에 I/O 를 넣으려 해도 못 넣는다.

정확히 무엇이 들어가는지는 [PLAN-01](PLAN-01-SESSION-CORE.md) 이 정한다.

| 완료조건 |
|---|
| `mc-clock` 의 의존이 `chrono`·`serde` 뿐이다 |
| 위치 계산 테스트가 **실제 시간을 안 기다린다** (`now` 를 인자로 받는다) |
| 기존 재생 동작이 변하지 않는다 |

### 3단계 — `mc-domain` + `mc-store`

타입과 저장소. 이미 디스코드를 모르므로 옮기기만 하면 된다.
**주의**: `remote/store.rs` 의 마이그레이션(`PRAGMA user_version`, 현재 v22)이
같이 따라와야 하고, 스키마 버전은 **건드리지 않는다.**

| 완료조건 |
|---|
| 운영 DB 사본으로 마이그레이션 dry-run 이 v22 에서 **아무것도 안 한다** |
| 테스트 90개(store 55 + models 35)가 그대로 통과 |

### 4단계 — `mc-media`

`media/*` 를 옮긴다. **`spawn_auto_update`(`src/media/tools.rs`)는 같이 안 옮긴다** —
`App` 전체를 요구하므로 배선 계층(`src/main.rs`)으로 올린다.

**완료조건**

| # | 시나리오 | 기대 |
|---|---|---|
| 1 | 캐시에 있는 곡 재생 | yt-dlp 실행 **0회**, 재생 시작 |
| 2 | 캐시에 없는 곡 | yt-dlp 1회, `cache_entries` 행 +1, 파일 존재 |
| 3 | 캐시 용량 초과 | LRU 정리가 돌고 **재생 중인 파일은 안 지운다** |
| 4 | ffmpeg 경로가 잘못된 설정 | 분리 전과 **같은 오류 메시지** |
| 5 | 기동 | yt-dlp 자동 업데이트 태스크가 여전히 뜬다 (로그로 확인) |

### 5단계 — `mc-engine` (제일 큰 공사)

`coordinator.rs` 를 **시각표 쪽과 songbird 쪽으로 가른다.** 이게 이 계획의 심장이다.

> **[PLAN-01](PLAN-01-SESSION-CORE.md) 2단계와 "같은 일" 이 아니다.** 처음에 그렇게
> 적었는데 교차검증이 반박했고 맞다 — PLAN-01 2단계는 `current_position`·일시정지·
> 물리 종료 기준을 **의도적으로 바꾼다.** 이 단계는 이사다.
>
> **순서: PLAN-01 2단계를 먼저 끝내고, 그 결과를 이사한다.** 동작 변경과 이사를
> 같은 커밋에 섞으면 회귀 원인을 못 가린다 (아래 "하면 안 되는 것" 첫 줄).

**여기서도 분리해야 할 것**: `src/player/side_effects.rs` 는 엔진 상태 변경과
serenity 메시지 전송·임베드 생성을 **한 파일에서** 한다(`announce_text`,
`on_track_started`). 엔진 쪽과 디스코드 쪽을 갈라야 `mc-engine` 에서 serenity 가 빠진다.

벽 2·벽 3 을 여기서 해결한다:
- 훅 4개 → broadcast 채널 + `set_listener_count` 밀어넣기
- `bot_voice_status_of` → `mc-discord` 로 이사
- `permission_allowed` → `mc-domain` 으로 이사

| 완료조건 |
|---|
| `crates/mc-engine/Cargo.toml` 에 **serenity 도 songbird 도 없다** |
| `mc-engine` 이 `mc-web` 을 참조하지 않는다 (컴파일러가 보증) |
| 재생·스킵·시크·자동재생 동작이 지금과 같다 |

### 6단계 — `mc-web` · `mc-discord` 분리

남은 것을 두 소비자로 가른다. `src/main.rs` 는 배선만 남는다.

| 완료조건 |
|---|
| `mc-discord` 를 `Cargo.toml` 에서 빼도 **`mc-web` 이 빌드된다** ← [PLAN-03](PLAN-03-DISCORD-OPTIONAL.md) 의 입구 |

---

## 하면 안 되는 것

| 금지 | 이유 |
|---|---|
| 크레이트를 나누면서 **동작을 같이 고치는 것** | 회귀가 났을 때 이사 때문인지 수정 때문인지 못 가린다. 이사와 수정은 별도 커밋 |
| `remote.rs` 를 나누면서 테스트를 옮기지 않는 것 | 76개가 유일한 안전망이다 |
| 스키마 버전을 올리는 것 | 이 계획에 DB 변경은 없다 |
| 크레이트 경계를 먼저 정하고 코드를 거기 맞추는 것 | 반대다. **지금 이미 나뉜 선**(디스코드를 아는 8개 파일)을 따라간다 |

---

## "동작이 같다" 를 무엇으로 확인하나

각 단계 완료조건에 "지금과 같다" 가 나오면 **아래 넷 중 무엇으로 비교하는지 반드시
지정한다.** HTTP 응답만 비교하면 재생·DB·디스코드 부수효과는 아무도 안 본다.

| 대상 | 방법 |
|---|---|
| HTTP | 분리 전 응답을 파일로 저장 → `sha256sum` 비교 |
| DB | 시나리오 실행 후 `sqlite3 .dump` 를 비교 (타임스탬프 컬럼은 제외) |
| 재생 | 곡 id · 위치 · 전환 횟수 · 재생 세대를 로그로 찍어 비교 |
| 디스코드 | 보낸 메시지·임베드 수와 임베드 제목을 로그로 찍어 비교 |

---

## 이 계획이 끝나면

- JS/CSS 수정 재빌드: **54초 → 10초 이하**
- 시각표 코드가 디스코드·DB·HTTP 를 **컴파일러 수준에서** 모른다
- [PLAN-03](PLAN-03-DISCORD-OPTIONAL.md)(디스코드 선택화)이 **가능해진다**

**하지만 웹 프로세스 분리는 아직 안 된다.** 크레이트 경계가 웹의 61개
`PlayerManager`/`Coordinator` **구체 호출을 명령 트레이트로 바꾸지 않기 때문**이다.
[PLAN-04](PLAN-04-ZERO-DOWNTIME.md) 3단계가 "트레이트 구현만 IPC 로 교체하면 된다" 고
적었던 것은 틀렸다 — 그 트레이트를 만드는 일이 **별도 단계로 남는다.**
