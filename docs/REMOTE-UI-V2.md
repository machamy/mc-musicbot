# 마참뮤직 리모컨 v2 — 구현 사양서

작성: 2026-08-06 · 브랜치 `feat/remote-ui-v2` · 워크트리 `<worktree>`

벤치마크 레퍼런스: `C:\Users\<user>\NAS-Hub\projects\musicbot-web\references\office-jukebox-changelog.md`

> **배포 규칙**: 빌드와 로컬 테스트는 자유. **보조 PC / NAS 반영은 사용자 허락 후에만.**

---

## 0. 확정된 결정

| # | 항목 | 결정 |
|---|------|------|
| 1 | 재작업 범위 | **프론트 전면 재작성, 백엔드 재활용**(`remote.rs`는 마크업 0바이트라 그대로 살림) |
| 2 | 화면 구성 | 유저 UI 신규 · **서버 관리 콘솔 신규** · 운영 패널(8693 `/`)은 **유지**하고 유저 UI 링크만 추가 |
| 3 | 권한 등급 | **봇 주인 / 서버 관리자 / 일반 멤버** 3단계 + 내부 **읽기전용** 등급 |
| 4 | 봇 주인 판정 | 비번 로그인(운영 패널) **+ Discord 유저 ID 등록** → 유저 UI에서도 배지·전용 컨트롤·어드민 이동 |
| 5 | 비멤버 | **403 차단이 기본**. 세션은 살아있는데 길드에서 빠진 경우 등은 **읽기전용**으로 강등 |
| 6 | 접속 표시 | 4종 전부: 리모컨 보는 중 / 음성채널에서 듣는 중 / 길드 멤버 전체 / Discord 온라인 상태 |
| 7 | 특권 인텐트 | 사용자가 개발자 포털에서 **Server Members + Presence 켜둠**. 꺼져 있어도 죽지 않고 자동 축소 |
| 8 | 정렬 모드 | **점수제 / 시간제(FIFO) / 공평제** 3종 · **서버 관리자만 변경** · 각 모드 설명 UI 필수 |
| 9 | 재정렬 주기 | **5초** |
| 10 | 채팅 | **웹 전용 독립**(Discord 채널과 연동 없음) · 반응 · @멘션 · #노래태그 · **인용 답장** |
| 11 | 멘션 대상 | 이 서버에서 리모컨을 써본 사람(채팅했거나 곡을 신청한 사람) |
| 12 | 귓속말 | **없음** |
| 13 | 제안 게시판 | **앱 개선 제안** + 👍 공감 + 관리자 상태(검토중/반영됨/보류) |
| 14 | 유저 정지 | **기능별 + 기간제** (전체/채팅만/신청만 × 5분·30분·3시간·무기한) |
| 15 | 비주얼라이저 | **장식용(가짜)** — 재생 위치 + 곡별 고정 시드 기반. 서버 부담 0 |
| 16 | 테마 | **다크 기본 + 토글**, 선택은 기기에 저장 |
| 17 | 로그인 유지 | **DB 저장** — 봇 재시작·업데이트 후에도 유지 |
| 18 | Discord 명령어 | `/리모컨` — **나만 보이는 응답**(ephemeral) |
| 19 | 시간 규칙 | **없음** (업무시간 볼륨제한·자정정지 미채택) |
| 20 | 렌더링 | **클라이언트 사이드 위주**. 서버는 데이터만. 랙 최소화가 명시 요구사항 |
| 21 | 기기 | **PC 우선**, 모바일도 제대로 |
| 22 | 언어 | 한국어 |

---

## 1. 권한 모델

### 1.1 등급 (`AccessTier`)

```rust
pub enum AccessTier {
    Owner,    // 봇 주인
    Manager,  // 서버 관리자
    Member,   // 일반 길드 멤버
    Viewer,   // 읽기전용 (상호작용 전면 차단)
}
```

판정 순서 (`authorize`):

1. 세션 없음 → 302 `/music/login`
2. 세션의 길드 목록에 없음 → **403**
3. 봇이 그 길드에 없음 → 403
4. **정지 상태 조회** → 전체 정지면 `Viewer`
5. `owner_user_ids`에 포함 → `Owner`
6. Discord `ADMINISTRATOR` / `MANAGE_GUILD` / 길드 소유자 / 지정 역할 → `Manager`
7. 실시간 멤버 재조회 실패(추방·탈퇴) → **`Viewer`로 강등** (기존엔 403 아니면 통과였음 — 실제 구멍)
8. 그 외 → `Member`

### 1.2 기능별 권한 매트릭스

`Viewer`는 **모든 쓰기 동작이 서버에서 거부**된다. UI에서 숨기는 것에 의존하지 않는다.

| 기능 | Viewer | Member | Manager | Owner |
|------|:---:|:---:|:---:|:---:|
| 현재 재생·대기열·가사 보기 | ✅ | ✅ | ✅ | ✅ |
| 채팅 **읽기** | ❌ | ✅ | ✅ | ✅ |
| 멤버 목록·접속 상태 보기 | ❌ | ✅ | ✅ | ✅ |
| 활동 로그 보기 | ❌ | ✅ | ✅ | ✅ |
| 곡 검색·신청 | ❌ | 규칙 | ✅ | ✅ |
| 좋아요/슈퍼 좋아요 | ❌ | 규칙 | ✅ | ✅ |
| 재생/일시정지/스킵 | ❌ | 규칙 | ✅ | ✅ |
| 시크·볼륨 | ❌ | 규칙 | ✅ | ✅ |
| 대기열 편집(삭제·순서) | ❌ | 규칙 | ✅ | ✅ |
| 채팅 쓰기·반응·답장 | ❌ | 규칙 | ✅ | ✅ |
| 보관함·재생목록 | ❌ | ✅ | ✅ | ✅ |
| 제안 작성·공감 | ❌ | ✅ | ✅ | ✅ |
| 남의 채팅 삭제 | ❌ | ❌ | ✅ | ✅ |
| 제안 상태 변경 | ❌ | ❌ | ✅ | ✅ |
| 유저 정지/해제 | ❌ | ❌ | ✅ | ✅ |
| **정렬 모드 변경** | ❌ | ❌ | ✅ | ✅ |
| 권한 규칙·제한값 변경 | ❌ | ❌ | ✅ | ✅ |
| 서버 관리 콘솔 진입 | ❌ | ❌ | ✅ | ✅ |
| **운영 패널 진입** | ❌ | ❌ | ❌ | ✅ |
| 관리자 지정 역할 변경 | ❌ | ❌ | ✅ | ✅ |
| 다른 관리자 정지 | ❌ | ❌ | ❌ | ✅ |

"규칙" = 서버 관리자가 정한 `PermissionRule`(멤버 / 같은 음성채널 / 지정 역할 / 관리자 / 사용 안 함).

**버그 수정**: 현재 `permission_allowed`는 `is_admin`이면 `Disabled`까지 통과시킨다.
→ `Disabled`는 **누구도 통과 못 하게** 바꾸고, 관리자 우회는 `Disabled`가 아닌 규칙에만 적용한다.

### 1.3 "내 권한이 뭔지" 화면

유저 UI 우측 상단 프로필 → **내 권한** 패널. 요구사항이므로 반드시 구현.

```
┌─ 내 권한 ────────────────────────────┐
│  🛡  서버 관리자                        │
│      이 서버에서 관리 권한이 있습니다     │
│                                       │
│  할 수 있는 것                          │
│   ✅ 곡 신청          모든 멤버          │
│   ✅ 좋아요           모든 멤버          │
│   ✅ 재생/일시정지     같은 음성채널 ← 관리자라 통과 │
│   ✅ 대기열 편집       같은 음성채널 ← 관리자라 통과 │
│   ✅ 채팅             모든 멤버          │
│   ✅ 남의 채팅 삭제    관리자             │
│   ✅ 정렬 모드 변경    관리자             │
│   ✅ 유저 정지        관리자             │
│                                       │
│  할 수 없는 것                          │
│   ❌ 운영 패널        봇 주인 전용        │
│                                       │
│  [ 서버 관리 콘솔 열기 → ]               │
└───────────────────────────────────────┘
```

- 각 줄에 **왜** 되는지/안 되는지(어떤 규칙 때문인지)를 같이 보여준다.
- 관리자 우회로 통과한 항목은 `← 관리자라 통과`로 명시.
- 정지 중이면 최상단에 사유·남은 시간 배너.

---

## 2. 접속 표시 (4종)

### 2.1 데이터 출처

| 표시 | 출처 | 권한 |
|------|------|------|
| 🖥 **리모컨 보는 중** | `WebState.presence`: WS 연결 레지스트리 `(guild_id, user_id) → 연결수` | 불필요 |
| 🎧 **듣는 중** | `App.discord_cache → guild.voice_states`에서 봇이 있는 채널 | `GUILD_VOICE_STATES` (이미 켜짐) |
| 👥 **멤버 전체** | `guild.members` | `GUILD_MEMBERS` (특권) |
| 🟢 **온라인 상태** | `guild.presences` | `GUILD_PRESENCES` (특권) |

### 2.2 우선순위 배지

한 사람에 대해 가장 강한 상태 하나만 대표로 표시:
`듣는 중 🎧` > `보는 중 🖥` > `온라인 🟢` > `자리비움 🌙` > `다른용무 ⛔` > `오프라인 ⚪`

### 2.3 축소 동작 (인텐트가 꺼져 있을 때)

- 앱 시작 시 인텐트 가용 여부를 판정해 `App.intent_status`에 기록.
- 꺼져 있으면 해당 표시를 **숨기고**, 서버 관리 콘솔 상단에 안내 카드:
  > ⚠ Server Members Intent가 꺼져 있어 전체 멤버 목록을 표시할 수 없습니다.
  > Discord 개발자 포털 → 내 봇 → Bot → Privileged Gateway Intents에서 켠 뒤 봇을 재시작하세요.
- **꺼져 있다고 해서 봇이 죽으면 안 된다.** serenity는 특권 인텐트를 요청했는데 포털에서 꺼져 있으면 연결이 거부되므로,
  기동 시 인텐트를 요청했다가 실패하면 **특권 인텐트를 빼고 자동 재시도**한다. (`main.rs` 재연결 루프에 폴백 추가)

---

## 3. 대기열 정렬 모드

### 3.1 3종

```rust
pub enum QueueSortMode { Score, Fifo, Fair }
```

**점수제 (`Score`)** — 기존 동작 유지
`manual_priority DESC → (wait_score + like + super×2) DESC → original_order ASC → id ASC`

**시간제 (`Fifo`)**
`manual_priority DESC → original_order ASC → id ASC`
좋아요는 표시만 되고 순서에 영향 없음.

**공평제 (`Fair`)** — 레퍼런스 v1.32.0
1. 사람별로 자기 곡을 신청 순서대로 줄 세운다 → 각 곡에 `round`(그 사람의 몇 번째 곡인지) 부여
2. `manual_priority DESC → round ASC → (그 사람의 마지막 재생 시각) ASC → original_order ASC`
3. 내 곡이 하나 재생되면 **내 대기 점수는 0으로 초기화**하고 마지막 재생 시각을 갱신
4. 즉 미리 5곡을 넣어도 1라운드에서는 1곡만 나가고, 늦게 온 사람도 다음 차례에 바로 들어온다

`wait_score`는 `Fair`에서 순서에 쓰이지 않지만 화면에는 계속 표시한다(설명용).

### 3.2 필수 UI

- 대기열 헤더에 **현재 모드 뱃지** + ⓘ. 누르면 3종 비교 설명 시트가 열린다(레퍼런스 '예상 재생 순서 ⓘ').
- 각 대기열 항목에 **`누구의 몇 번째 곡`** 표시(레퍼런스 v1.30.0).
- 점수제·공평제에서는 **점수 계산식**을 그대로 노출: `👍3 + ⭐1×2 + 대기 2 = 7`.
- 관리자가 모드를 바꾸면 활동 로그에 남기고 전원에게 토스트로 알린다.

### 3.3 정렬 주기

- 서버: **5초 주기** 재정렬 태스크. 기존 10초 → 5초.
- 버튼 연타 중 순서가 튀지 않도록, 클라이언트는 정렬 결과가 바뀌어도 **FLIP 애니메이션**으로 이동시킨다.
- `shuffle_enabled`일 때 정렬을 건너뛰던 버그(`sort_scored_queue` 조기 반환)를 고친다 — 셔플은 **별도 모드가 아니라** `Fifo`+무작위 `original_order`로 처리.

---

## 4. 화면 구조

### 4.1 유저 UI — `/music/guilds/{guild_id}`

PC 기준 3열. 1280px 이상에서 좌/중/우, 980px 이하 2열, 680px 이하 단일 + 하단 탭바.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ 마참뮤직  [서버이름 ▾]      🎧듣는중 3  🖥보는중 5   🌓  [프로필 ▾]        │
├────────────────┬─────────────────────────────────┬───────────────────────┤
│ 검색           │  ┌───────────────────────────┐  │ [채팅][멤버][제안]    │
│ ┌────────────┐ │  │      앨범아트 (대)         │  │ [최근][로그]          │
│ │🔎 곡 검색  │ │  │                           │  │                       │
│ └────────────┘ │  │  곡 제목 (길면 전광판)     │  │  ┌──────────────────┐ │
│                │  │  아티스트 · 신청: 민수     │  │  │ 민수  20:14      │ │
│ 대기열 12곡    │  └───────────────────────────┘  │  │ 이거 좋다        │ │
│ [공평제 ⓘ]    │  ▁▂▃▅▇▅▃▂▁  비주얼라이저        │  │  👍2  🙂         │ │
│                │  ──────●───────────  2:14/4:02  │  ├──────────────────┤ │
│ ┌────────────┐ │  ⏮  ⏯  ⏭   🔊────  🔁  🎲     │  │ ┌ 민수: 이거 좋다│ │
│ │1 아이브     │ │                                 │  │ │ 지훈  20:15    │ │
│ │  민수 1번째 │ │  ┌─ 가사 ──────────────────┐   │  │ │ ㅇㅈ           │ │
│ │  👍3 ⭐1 대2│ │  │  이전 줄                 │   │  └──────────────────┘ │
│ │  = 7점      │ │  │ ▶현재 줄 (강조)          │   │                       │
│ │  👍 ⭐ 🔖 ✕│ │  │  다음 줄                 │   │  [@ #  🙂 ]  [보내기] │
│ └────────────┘ │  └─────────────────────────┘   │                       │
└────────────────┴─────────────────────────────────┴───────────────────────┘
```

우측 탭: **채팅 / 멤버 / 제안 / 최근 / 로그** (+ 보관함은 좌측 검색 패널의 탭)

### 4.2 서버 관리 콘솔 — `/music/guilds/{guild_id}/admin`

현재의 "구린 설정 모달"을 대체. 좌측 섹션 네비 + 우측 폼.

```
┌──────────────────────────────────────────────────────────────┐
│ ← 리모컨으로 돌아가기        서버 관리 · 마참서버             │
├──────────────┬───────────────────────────────────────────────┤
│ 순서와 재생  │  대기열 정렬 방식                              │
│ 권한         │  ┌─────────┬─────────┬─────────┐              │
│ 제한값       │  │ 점수제  │ 시간제  │ ●공평제 │              │
│ 유저 관리    │  └─────────┴─────────┴─────────┘              │
│ 채팅과 제안  │  사람별로 돌아가며 한 곡씩 재생합니다.          │
│ 활동 기록    │  미리 여러 곡을 넣어도 새치기가 안 되고,        │
│ 진단         │  늦게 온 사람도 금방 차례가 옵니다.            │
│              │                                               │
│              │  ┌─ 지금 대기열에 적용하면 ────────────┐      │
│              │  │ 1. 아이브   민수 1번째  (지금 1위)  │      │
│              │  │ 2. 다플     지훈 1번째  (지금 3위↑) │      │
│              │  │ 3. 뉴진스   민수 2번째  (지금 2위↓) │      │
│              │  └────────────────────────────────────┘      │
│              │                        [되돌리기] [저장]      │
└──────────────┴───────────────────────────────────────────────┘
```

**"구림" 해소 기준 — 아래를 전부 만족해야 함**

1. 항목마다 **한 줄 설명**이 붙는다. 라벨만 있는 필드 금지.
2. 관련 항목은 **섹션으로 묶고** 각 섹션에 목적 설명.
3. 권한 드롭다운은 고르는 즉시 **"지금 이 서버에서 몇 명이 통과하는지"** 를 보여준다.
4. 정렬 모드는 위처럼 **현재 대기열에 적용한 미리보기**를 보여준다.
5. **변경된 항목만 강조**되고, 저장 전 `되돌리기` 가능. 저장 안 하고 나가면 확인.
6. 숫자 입력은 슬라이더 + 직접입력 + 단위 + 허용범위 표시.
7. 저장은 부분 저장 가능(섹션 단위), 저장 결과 토스트.
8. 모바일에서 좌측 네비가 상단 가로 스크롤 탭으로 바뀐다. (운영 패널의 820px 네비 실종 문제를 반복하지 않음)

### 4.3 운영 패널 — `/` (기존 유지)

- 디자인 재작성 **안 함**. 단, 상단에 **`리모컨 열기 →`** 링크를 추가한다 (어드민 → 유저 방향 이동 요구사항).
- 봇 주인 Discord ID 등록 필드를 `/botsettings`에 추가한다.

### 4.4 이동 방향

```
운영 패널 ──[리모컨 열기]──▶ 유저 UI
                              │
서버 관리 콘솔 ◀──[관리 콘솔]─┤ (Manager/Owner에게만 버튼 노출)
      │                       │
      └──[← 리모컨으로]───────▶
      
유저 UI ──▶ 운영 패널 : Owner에게만 링크 노출. Owner가 아니면 링크도 없고 URL 직접 입력해도 비번 로그인에 막힘.
```

---

## 5. 성능 계약 (명시 요구사항: "랙 최대한 없도록, 클라 렌더링, 서버 부담 최소")

### 5.1 현재 문제

탭 하나당 **2초마다 `/state` 전체 재조회** → 채팅 100건의 반응을 건건이 쿼리(N+1) 포함 **SQLite 왕복 약 110~120회**.
이 커넥션은 **재생 경로와 같은 뮤텍스**를 쓴다. 탭 5개면 초당 ~300 쿼리.

### 5.2 v2 계약

**A. 상태를 hot / cold로 분리**

| 엔드포인트 | 내용 | 갱신 |
|---|---|---|
| `GET .../state/hot` | player, position, queue(+score), 접속 요약 | WS 이벤트 시에만 |
| `GET .../state/cold` | settings, permissions, playlists, library, 멤버 목록 | 진입 시 1회 + `settings`/`library` 이벤트 시 |
| `GET .../chat?before=` | 채팅 페이지네이션 (기본 50) | 진입 시 1회 + WS push로 증분 |
| `GET .../audit?before=` | 활동 로그 | 탭 열 때만 |
| `GET .../suggestions` | 제안 | 탭 열 때만 |

**B. WS에 실제 payload를 싣는다**

```jsonc
{"t":"chat.add","d":{ ...메시지 전체... }}
{"t":"chat.react","d":{"messageId":1,"emoji":"👍","userId":"…","added":true}}
{"t":"chat.delete","d":{"messageId":1}}
{"t":"queue.set","d":{"items":[…],"mode":"fair","sortedAt":"…"}}
{"t":"playback","d":{"isPaused":false,"positionSeconds":12.3,"sampledAtUtc":"…","currentId":"…"}}
{"t":"presence","d":{"listening":["…"],"viewing":["…"],"online":{…}}}
{"t":"vote","d":{"itemId":"…","like":3,"super":1,"total":7}}
{"t":"suggestion.*"}, {"t":"settings"}, {"t":"library"}, {"t":"suspension"}
```

→ **채팅 한 줄 쓰는데 전체 재조회가 일어나지 않는다.** 클라이언트는 이벤트를 자기 상태에 머지한다.
→ `refetch` 트리거는 `settings` / `library` / 재연결 시로 한정.

**C. 진행바는 서버를 안 부른다**
`playback` 이벤트에 `positionSeconds` + **`sampledAtUtc`** 를 같이 실어서, 클라이언트가 `performance.now()`로 보간한다.
(현재는 `serverTimeUtc`를 position **이후에** 읽어서 오차가 있고, 지연 보정도 없다.)

**D. N+1 제거**
`list_chat_messages`의 메시지별 반응 조회를 `WHERE message_id IN (...)` **한 방 쿼리 + 그룹핑**으로 교체.

**E. 접속 표시는 DB를 안 쓴다**
전부 메모리(`WebState.presence`) + Discord 캐시. 변경 시에만 broadcast, **최대 초당 1회로 코얼레싱**.

**F. 렌더링은 전부 클라이언트**
서버는 HTML 셸(빈 컨테이너 + 부트스트랩 JSON)만 준다. 목록·카드·채팅은 전부 JS가 그린다.
채팅 반응 추가 시 **해당 메시지 노드만** 갱신(레퍼런스 v1.26.0의 "전체 리렌더 안 됨").

**G. 백그라운드 탭**
`document.hidden`이면 비주얼라이저·진행바 보간·마퀴를 멈추고, WS는 유지하되 렌더는 미룬다.

**H. 목표 수치**
- 유휴 상태(아무도 조작 안 함): 탭당 **SQLite 쿼리 0회/초**
- 곡 전환 1회: 쿼리 10회 이하
- 채팅 1건: 쿼리 2회 (insert + 멘션)
- 탭 10개 붙어도 재생 경로 지연 없음

---

## 6. 스키마

### 6.1 마이그레이션 러너 (선행 필수)

현재 `CREATE TABLE IF NOT EXISTS` 뿐이라 **기존 DB에 컬럼 추가가 조용히 무시된다.**
`RemoteStore::open`에 `PRAGMA user_version` 기반 러너를 넣는다. 각 단계는 트랜잭션. 레거시(C# 공용) 테이블은 건드리지 않는다.

```
v0 → v1  : 기존 remote_* 테이블 보장
v1 → v2  : remote_chat_messages 에 reply_to_message_id, edited_utc
v2 → v3  : remote_chat_mentions, remote_chat_tags
v3 → v4  : remote_suggestions, remote_suggestion_votes
v4 → v5  : remote_user_suspensions
v5 → v6  : remote_web_sessions
v6 → v7  : remote_queue_scores 에 round, last_played_utc (공평제)
v7 → v8  : 인덱스 보강 + 보존기간 정리
```

### 6.2 신규 테이블

```sql
CREATE TABLE remote_chat_mentions (
  message_id INTEGER NOT NULL, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
  read_utc TEXT, PRIMARY KEY (message_id, user_id));
CREATE INDEX idx_chat_mentions_unread ON remote_chat_mentions(guild_id, user_id, read_utc);

CREATE TABLE remote_chat_tags (
  message_id INTEGER NOT NULL, cache_key TEXT NOT NULL,
  track_json TEXT NOT NULL, PRIMARY KEY (message_id, cache_key));

CREATE TABLE remote_suggestions (
  id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL,
  user_id INTEGER NOT NULL, display_name TEXT NOT NULL, avatar_url TEXT,
  title TEXT NOT NULL, body TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',        -- open|reviewing|planned|done|declined
  status_note TEXT, status_by_user_id INTEGER, status_utc TEXT,
  created_utc TEXT NOT NULL, deleted_utc TEXT);
CREATE INDEX idx_suggestions_guild ON remote_suggestions(guild_id, id DESC);

CREATE TABLE remote_suggestion_votes (
  suggestion_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
  created_utc TEXT NOT NULL, PRIMARY KEY (suggestion_id, user_id));

CREATE TABLE remote_user_suspensions (
  guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
  scope TEXT NOT NULL,                         -- all|chat|queue
  reason TEXT, by_user_id INTEGER NOT NULL,
  created_utc TEXT NOT NULL, expires_utc TEXT, -- NULL = 무기한
  PRIMARY KEY (guild_id, user_id, scope));

CREATE TABLE remote_web_sessions (
  token_hash TEXT PRIMARY KEY,                 -- 토큰 원문은 저장하지 않음
  user_id INTEGER NOT NULL, display_name TEXT NOT NULL, avatar_url TEXT,
  guilds_json TEXT NOT NULL, access_token TEXT, refresh_token TEXT,
  expires_utc TEXT NOT NULL, refreshed_utc TEXT, created_utc TEXT NOT NULL);
CREATE INDEX idx_web_sessions_expiry ON remote_web_sessions(expires_utc);
```

### 6.3 기존 테이블 변경

- `remote_chat_messages` + `reply_to_message_id INTEGER NULL`, `edited_utc TEXT NULL`
- `remote_queue_scores` + `round INTEGER NOT NULL DEFAULT 0`, `last_played_utc TEXT NULL`
- 길드 설정 JSON(`settings` 키 `remote_guild_settings:{id}`)에 추가 — `#[serde(default)]`라 마이그레이션 불필요:
  `sort_mode`, `chat_retention_days`(기본 30), `suggestion_enabled`, `owner_visible`, `theme_default`

### 6.4 보존 정리 (지금은 전부 무제한)

기동 시 + 하루 1회: 채팅 30일(설정 가능) · 최근재생 500건 · 활동로그 기존 설정값 · 가사 캐시 실패건 7일 TTL.
`prune_audit`의 문자열 비교 버그(`'T'` vs `' '`)를 `julianday()` 비교로 교체.

---

## 7. 백엔드 작업 목록

### 7.1 보안·정합성 (재작성 전에 반드시)

| # | 위치 | 문제 | 조치 |
|---|------|------|------|
| S1 | `remote.rs` `api_library` | 권한 규칙이 **아예 없음** | `authorize`에 규칙 적용 |
| S2 | `remote.rs` `api_chat_delete` | `chat_enabled`/`chat_rule` 검사 누락 | 다른 채팅 라우트와 동일하게 |
| S3 | `remote.rs` `permission_allowed` | 관리자가 `Disabled`까지 통과 | `Disabled`는 전원 차단 |
| S4 | `remote.rs` `api_events` (WS) | 세션·길드만 확인. CSRF·Origin·봇존재·정지 미확인 | 전체 `authorize` 경로 + Origin 허용목록. **payload를 싣기 때문에 필수** |
| S5 | `remote.rs:865` | POST마다 `reqwest::Client::new()` 새로 생성 | `App`에 공유 클라이언트 |
| S6 | `remote.rs` `verify_csrf` | 비상수시간 `==` | `subtle` 또는 수동 상수시간 비교 |
| S7 | 쿠키 `Secure` | `public_base_url` 문자열로만 판정 | 프록시 헤더도 고려, 기본값을 안전한 쪽으로 |
| S8 | `oauth_states` | 스위퍼 없음 → 누수 | 주기적 정리 태스크 |

### 7.2 신규

| # | 내용 |
|---|------|
| B1 | 마이그레이션 러너 (`PRAGMA user_version`) |
| B2 | `AccessTier` + 정지 검사 + `Viewer` 강등 |
| B3 | 세션 DB 영속화 + refresh token 요청·저장·갱신 |
| B4 | 접속 레지스트리 (`WebState.presence`) + 음성/멤버/온라인 병합 |
| B5 | 인텐트 폴백 기동 (`main.rs`) + `App.intent_status` |
| B6 | `QueueSortMode` 3종 + 5초 재정렬 + 셔플 조기반환 제거 |
| B7 | hot/cold 상태 분리 + 타입드 WS payload |
| B8 | N+1 반응 쿼리 제거 + 채팅 커서 페이지네이션 |
| B9 | 답장 / 멘션 / 노래태그 |
| B10 | 제안 게시판 CRUD + 공감 + 상태 |
| B11 | 유저 정지 (기능별·기간제) |
| B12 | 정적 에셋 라우트 (`include_str!`/`include_bytes!`, `/music/assets/{name}.{build_id}.{ext}`) |
| B13 | PWA manifest + service worker (`/music/sw.js` — 스코프 때문에 반드시 이 경로) |
| B14 | `/리모컨` 슬래시 명령 (ephemeral) + `App`에 공개 URL `OnceLock` |
| B15 | 봇 주인 Discord ID 설정 (`/botsettings`) |
| B16 | 보존 정리 태스크 + `prune_audit` 날짜 비교 수정 |
| B17 | 가사 실패 negative-cache TTL |

### 7.3 직렬화 함정 (프론트 계약)

- `TrackRef.duration`은 **C# TimeSpan 문자열**(`"00:03:25"`)로 직렬화된다. v2 JSON에서는 `durationSeconds` 숫자를 **모든** 트랙에 추가한다.
- `RepeatMode`만 PascalCase(`"Off"|"Track"|"Queue"`) — camelCase로 통일.
- `positionSeconds`에 `sampledAtUtc`를 짝지어 보낸다.

---

## 8. 프론트엔드

### 8.1 파일 배치 (번들러 없음, 단일 exe 유지)

```
src/web/assets/
  tokens.css        디자인 토큰 (다크/라이트 양쪽)
  portal.css        유저 UI
  console.css       서버 관리 콘솔
  core.js           WS·상태스토어·API·렌더 유틸 (공용)
  portal.js         유저 UI
  console.js        서버 관리 콘솔
  sw.js             서비스워커
  manifest.webmanifest
  icon-192.png / icon-512.png / favicon.svg
```

`src/web/assets.rs`에서 `include_str!`/`include_bytes!`로 컴파일 시 임베드.
라우트는 `/music/assets/{name}` + `?v={build_id}` 캐시버스팅, `Cache-Control: immutable`.
**`tower-http`/`ServeDir` 금지** — 포터블 1241파일 매니페스트와 "exe 하나 SHA 하나" 계약을 깨뜨린다.

### 8.2 디자인 시스템

- **다크 기본**. `:root` 다크 토큰, `[data-theme="light"]` 오버라이드, 토글은 `localStorage`.
- 폰트: Pretendard를 **실제로 임베드하지 않는다**(용량). `system-ui`/`Malgun Gothic` 스택으로 가고, 로컬에 있으면 사용.
- 앨범아트 중심 레이아웃. 강조색은 앨범아트에서 추출한 색으로 은은하게 물들인다(canvas 1픽셀 샘플링, 클라이언트).
- 모션: 200ms 이하, `prefers-reduced-motion` 존중.
- 모든 버튼에 **커스텀 툴팁**(터치는 롱프레스). 네이티브 `title=` 금지 — 지연이 길고 모바일에서 안 뜬다.

### 8.3 필수 상호작용 디테일

| 항목 | 요구 |
|---|---|
| 긴 제목 | 행 어디에든 호버하면 전광판. 대기열·보관함·최근 전부 (레퍼런스 v1.30.0) |
| 검색 | 결과가 **검색창 바로 아래** 드롭다운. 열려 있으면 버튼이 ✕, 검색어 바뀌면 다시 🔎 |
| 중복 곡 | 이미 대기열에 있으면 추가 대신 안내 + 해당 항목으로 스크롤 |
| 대기열 | `누구의 몇 번째 곡` + 점수 계산식. 순서 변경은 FLIP 애니메이션 |
| 채팅 | 연속 메시지는 닉네임 1회. 호버 시 🙂 답장 ⋯ . 반응은 해당 노드만 갱신 |
| 답장 | 인용 프리뷰(작성자+80자) + 클릭 시 원문으로 스크롤·하이라이트. 원문이 창 밖이면 서버에서 스텁만 조회 |
| @멘션 | 자동완성. 불리면 탭 뱃지 + 백그라운드면 브라우저 알림 + 탭 제목 개수 |
| #노래태그 | 대기열·최근에서 자동완성. 칩으로 렌더되고 클릭 시 대기열 담기 |
| 가사 | 현재 줄 강조 + 따라 스크롤. 접기 상태 저장 |
| 비주얼라이저 | `positionSeconds` + `cache_key` 시드 PRNG. 일시정지 시 가라앉음, 숨은 탭이면 정지 |
| 알림 | 내 신청곡 시작 / 멘션 / 답장 |
| 버전 | 서버 `build_id`와 다르면 새로고침 유도 팝업 + 변경이력 |
| 오프라인 | WS 끊기면 상단에 "연결 끊김 · 재연결 중" 바, 조작 버튼 비활성 |

### 8.4 접근성·품질 기준

- 키보드만으로 전 기능 도달. 포커스 링 유지.
- 대비 4.5:1 이상.
- `aria-live`로 곡 전환·토스트 알림.
- **`data-testid` 24개 전부 유지** — 외부 Playwright 하니스가 물고 있다.
  `audit-filter chat-input chat-messages chat-send dev-login discord-login guild-card library-filter lyrics-toggle music-portal now-playing play-pause playlist-card queue-item queue-list search-input search-results search-submit seek-bar settings-open settings-save skip tab-body volume`
  `settings-open`/`settings-save`는 이제 서버 관리 콘솔로 가는 버튼/저장 버튼에 붙인다.

---

## 9. Discord 명령어

```
/리모컨   (별칭 /remote)
```

- **ephemeral**(나만 보임)
- 임베드: 서버 이름 · 지금 재생 중인 곡 · 대기열 수 · 접속 인원 · 링크
- Manager/Owner면 `서버 관리 콘솔` 링크도 같이
- 공개 URL은 `RemoteAuthConfig.public_base_url`. `App`에 `OnceLock<String>`을 추가해 `web::serve()`에서 주입(기존 `songbird`/`http`/`discord_cache` 패턴과 동일). 미설정이면 안내 문구.
- 링크 자체에 접근 토큰을 넣지 않는다 — 항상 Discord 로그인을 거친다.

삽입 지점: `commands/catalog.rs`의 `ALL`에 `CommandDef` 추가 → `handlers::dispatch`에 arm 추가.
`handle_command`가 이미 `defer`했으므로 `create_response`가 아니라 `respond_text`/`create_followup`을 쓴다.

---

## 10. 검증

빌드·로컬 테스트까지만 수행하고, 보조 PC/NAS 반영은 별도 승인.

1. `cargo build --release` (경고 0 목표)
2. `cargo test` — 신규: 정렬 3종, 권한 매트릭스, 정지 만료, 멘션 파서, 마이그레이션 v0→최신
3. 로컬 기동 + `MUSICBOT_DEV_LOGIN=1`로 4등급 시나리오 수동 확인
4. 쿼리 카운터로 §5.2 H 목표 수치 측정
5. 인텐트 OFF 상태에서 기동 → 폴백 동작 확인
6. Playwright `data-testid` 24개 존재 확인
