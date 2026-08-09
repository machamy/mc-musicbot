# 웹 재생기 모드 설계안 (4차)

작성: 2026-08-09 · 상태: **제안. 아직 구현 없음**

> "그냥 웹 노래 재생기로도 쓸 수 있으면 좋겠다"
> "**사람마다 반드시 같은 곳, 같은 노래를 들어야만 해**"
> "기존 기능은 해치면 안 된다"

봇이 음성 채널에 없어도 리모컨만으로 음악을 듣게 한다. **여럿이 같은 곡·같은 위치로.**

> 1~3차가 교차검증에서 반려됐다(`[WRONG]` 4·4·5건). 원인이 둘이었다.
> ① 호출 경로와 값의 동일성을 확인 없이 단정했다.
> ② **가상 재생을 물리 재생과 완전히 동등한 시민으로 한 번에 만들려 했다.**
> 4차는 ①을 위해 모든 주장에 근거 파일·줄을 붙였고, ②를 위해 **단계로 쪼갰다.**
> 각 단계는 그 자체로 완결되고, 기존 기능을 안 건드리는 것이 단계마다 완료조건이다.

---

## 이미 있는 것부터 세자

**동기화 장치는 이미 다 있다.** 새로 만들 필요가 없다.

| 있는 것 | 근거 |
|---|---|
| 절대 시각 기준 위치 계산 | `schedule.startedUtc` → `position = now - startedUtc` (§31) |
| 데싱크 보정 | `portal.js:4463` `WEB_SYNC_GAP = 2` · `4909`·`4920` 에서 2초 이상 벌어지면 `seekTo` |
| 유튜브·사운드클라우드 양쪽 처리 | 같은 자리에서 provider 별로 분기 |
| 자동재생 정책 대응 | `webWanted`/`webOn` 분리 |

그래서 **"모두가 같은 곡 같은 위치"는 시각표 하나만 있으면 자동으로 따라온다.**
시각표가 하나면 모든 브라우저가 같은 `startedUtc` 를 보고, 벌어지면 기존 보정이 끌어당긴다.

이 문서가 하는 일은 **없는 시각표를 만들어 주는 것**과, 그것을 막고 있는 장애물을 치우는 것이다.

---

## 벽 넷

### 벽 1. 시각표가 songbird 세션에서만 나온다
```rust
// coordinator.rs:49,73
sessions: Mutex<HashMap<u64, Session>>,      // Session 은 TrackHandle(songbird)을 든다
pub async fn schedule(&self, guild_id) -> Option<TrackSchedule> {
    let sessions = self.sessions.lock().await;   // 없으면 시각표도 없다
```

### 벽 2. 화면이 스스로 멈춘다 — 경로 셋
| 경로 | 근거 |
|---|---|
| WS 갱신 | `core.js:1098` `stopped: data.voiceConnected === false \|\| !data.current` |
| 진입 로드 | `portal.js:8289` 같은 식 (`loadHot`) |
| 적용 | `core.js:388` `if (clock.stopped) clock.paused = true;` |

**서버는 `stopped` 를 안 보낸다**(`remote.rs` 에 그 키 없음, 확인함). 둘 다 클라이언트 파생이다.
§36 에서 일부러 넣은 것 — 판단은 옳고 **기준이 틀렸다**(봇 연결이 아니라 시각표를 봐야 한다).

### 벽 3. 조작이 두 단계에서 막힌다
```rust
// remote.rs:1566 — v4.7
member.same_voice_channel || (!settings.require_voice_for_playback && !member.bot_in_voice)
// remote.rs:4700 부근 — v4.6, 권한 검사 '뒤'의 별도 게이트
if action_requires_voice(&request.action)
   && ctx.settings.require_voice_for_playback
   && !bot_voice_status(...).in_voice() { return 409 }
```
**두 곳 다 열어야 한다.** 한 곳만 열면 409 로 막힌다(3차의 오류).

### 벽 4. 프레임이 안 나간다
`ensure_guild_watcher` 의 서명에 `schedule` 도 `stopped` 도 없다
(`remote.rs:2177-2195`, 확인함: `current_item.id | is_paused | volume | repeat | voice_connected | preview | upcoming`).
→ 가상 세션을 만들어도 **WS 프레임이 안 나가서 화면이 모른다.**

---

## 단계

### 1단계 — 같이 듣기 (핵심)

이것만으로 사용자 요구가 충족된다. 나머지 단계는 이 위에 얹는다.

| 할 일 | 근거·주의 |
|---|---|
| `virtual_sessions: Mutex<HashMap<u64, VirtualSession>>` 를 `Coordinator` 에 추가 | 물리 세션과 나란히 |
| `schedule()` 이 물리 → 가상 순으로 본다 | 화면은 출처를 안 묻는다 |
| `sync_guild` 가 **`bot_voice_status`** 로 분기 | `voice_channel_id` 는 저장 바인딩이지 실제 상태가 아니다 (1차 오류, `HANDOFF` 가 경고한 함정) |
| `stopped` 를 서버가 보낸다 — `playback_payload` **와** `api_state_hot` **둘 다** | 3차 오류: `loadHot` 이 별도 경로다 |
| 클라 `??` 폴백 — `core.js:1098`, `portal.js:8289` | 옛 서버 + 새 화면에서 §36 회귀 방지 |
| watcher 서명에 `schedule 유무` 추가 | 벽 4 |
| 벽 3 의 **두 게이트 모두** `web_player_mode` 를 반영 | 한 곳만 열면 409 |
| 곡 길이만큼 뒤 `advance` → **물리와 같은 절차 재사용** (`advance → ensure_autoplay → sync_guild`, `coordinator.rs:614-628`) | 안 하면 자동재생 보충이 끊긴다 |
| 가상 곡 시작 시 `on_track_started`(`side_effects.rs:44`) 호출 | 안 부르면 프리페치·preview 가 안 돈다 |
| 타이머 `generation` 확인 | 물리 세션이 쓰는 방식과 동일 |
| **리스너 판정 = `presence` ∩ `webPlayback` pref = `1`** | `presence` 만 보면 페이지만 열어 둔 사람까지 센다. pref 는 `prefSet('webPlayback')` 이 서버로 밀어 올린다(`portal.js:287-300`, 확인함) |
| 배선: `app.coordinator.on_listeners_changed(&app, guild_id)` | `PlayerManager` 는 `Coordinator`·`App` 참조가 **없다**(`manager.rs:28-44`). `App` 은 `coordinator` 를 든다(`app.rs:118`) — 3차 오류 |
| `presence_remove` 는 **마지막 소켓**에서만 제거 | 이미 그렇게 되어 있다(`remote.rs:1853-1864`, 확인함). 그 의미를 그대로 쓴다 |
| 가상 `current_position` = `(now - started_utc).max(0)` | `schedule_start_in` 이 `started_utc` 를 미래로 찍는다(`coordinator.rs:88`). `Duration` 은 음수를 못 담는다 |
| 길이 미상이면 **시작하지 않는다** | `0` 으로 두면 즉시 끝난 것으로 처리돼 대기열이 순식간에 비워진다 |
| `web_player_mode: bool` (`serde(default)`, 기본 `false`) + 콘솔 토글 + **리모컨 payload** | 포털이 `requireVoiceForPlayback` 만 보고 잠그면 화면이 계속 막힌다(`api_state_cold` 경로) |

**물리 `current_position` 은 안 바꾼다.** 물리는 `start_offset + handle.position` 이고 `started_utc` 를
아예 안 본다(`coordinator.rs:99`, 확인함). 3차에서 "의미가 같다" 고 한 것이 틀렸다 — 두 계산은 다르고,
**다른 채로 두는 것이 맞다.**

### 2단계 — 인계 (봇이 들어오고 나갈 때)

songbird 시작 위치는 `QueueItem.start_offset` 에서 오고 `started_utc` 도 그 offset 으로 다시 만든다
(`coordinator.rs:302,369`). **가상 세션을 먼저 버리면 위치를 잃는다.**

```
가상 → 물리                      물리 → 가상
1. pos = 가상 위치                1. pos = current_position()
2. set_current_start_offset(pos)  2. set_current_start_offset(pos)
3. 가상 세션 제거                 3. 물리 세션 취소
4. sync_guild                     4. 가상 세션 생성
```

- **강제 퇴장도 여기 태운다.** 지금 `voice_state_update` 는 `evaluate_auto_leave` 만 부르고 세션을
  안 건드린다(`events.rs:115-118`, 확인함). 그래서 캐시는 미연결인데 물리 세션이 남는다 —
  **그 자체가 기존 버그다.** 봇 자신의 변화면 인계를 태운다.
- **`on_track_started` 중복 방지.** 물리 `play_track` 은 이걸 무조건 부른다(`coordinator.rs:397`).
  인계를 "새 곡 시작" 으로 처리하면 같은 곡에서 프리페치·preview·디스코드 알림이 다시 돈다.
  → 인계 플래그를 두고 그때는 안 부른다.
- **전환 구간이 있다.** `sync_guild` 는 옛 세션을 취소하고 `cache.prepare` 를 await 한 뒤 새 세션을
  등록한다. 그 사이 `schedule()` 이 잠깐 `None` 이다(3차 오류). → 인계 중에는 가상 세션을 **먼저
  만들어 두고** 물리가 붙으면 버린다. 그러면 그 구간에도 시각표가 끊기지 않는다.

### 3단계 — 통계

`stat_track_plays` 는 `(guild_id, cache_key)` 집계 행이고 `plays_user`/`plays_autoplay` 카운터다
(`stats.rs:612-621`). **행 단위 boolean 은 불가능하다**(1차 오류).

```sql
plays_virtual INTEGER NOT NULL DEFAULT 0
```

- **통계 DB 는 마이그레이션 러너가 따로다.** `SCHEMA_VERSION = 1` 이고 `if version >= SCHEMA_VERSION
  { return }` 이라(`stats.rs:36,566`, 확인함) **CREATE TABLE 만 고치면 기존 DB 에 컬럼이 안 생긴다.**
  → `SCHEMA_VERSION` 을 2 로 올리고 `ALTER TABLE` 단계를 추가한다.
- **가상 여부를 어디에 두나.** `GuildPlayerState` 는 매 조작마다 SQLite 에서 새로 만들어지고
  (`manager.rs:173-185`, 확인함) 저장 스키마에 그 칸이 없다. **거기 두면 안 된다**(3차 오류).
  → `Coordinator` 와 `PlayerManager` 가 **같은 `Arc<Mutex<HashSet<u64>>>`** 를 나눠 갖는다.
  `PlayerManager::new(db, remote, log)`(`manager.rs:69`)에 인자 하나를 더한다.
- **다섯 곳 전부가 `PlayerManager`** 다(확인함): `play_now`(283) `cancel_by_id`(428) `skip_to`(515)
  `advance`(593) `skip`(615). 공유 집합을 `record_play` 가 읽으면 다섯 곳이 한 번에 맞는다.
- **웹 skip 은 세션을 먼저 취소한다.** 그래서 집합은 **세션 폐기가 아니라 곡 종료 시점에** 지운다.
- **배타 규칙**: 종료 시점 기준. 한 재생이 두 카운터를 동시에 올리지 않는다.

### 4단계 — 디스코드 카드

`now_playing_embed(state, item, position)`(`embeds.rs:93`)에 가상 인자가 없다.
→ 3단계의 공유 집합을 호출부(`announce_now_playing`)가 읽어 footer 를 붙인다.

```
footer: "웹에서만 재생 중 · 음성 채널에서는 안 들려요"
```
인계로 물리가 되면 다음 갱신에서 사라진다.

### 5단계 — 마무리

- `guild_delete`(`events.rs:103-113`)에서 가상 세션·리스너·타이머 정리
- 모드를 끄면 도는 세션 즉시 정리
- 모드를 켜는 순간 **이미 접속 중인 사람**으로 시작 (리스너 콜백은 경계에서만 울린다)
- 길이 미상 알림을 WS `notice` 로. `Coordinator` 는 `emit` 에 못 닿으므로(`emit` 은 `WebState`),
  `App` 에 훅을 두고 `web/mod.rs` 가 채운다 — `on_restarting` 과 같은 방식(`app.rs:145`)
- 종료·재기동: 가상 재생 중 종료되면 재기동 후 자동 복구하지 않는다. `resume_after_restart` 는
  물리 음성 복귀 경로다. 사람이 다시 켠다.

---

## 위험

1. **아무도 안 듣는데 대기열이 돈다** → 리스너 판정이 `presence ∩ pref`. 비면 멈춘다
2. **길이 미상** → 시작하지 않는다. 대기열은 안 건드린다
3. **구동기 두 벌** → 길드마다 하나. 2단계의 순서로 넘긴다
4. **타이머 세대** → `generation` 확인
5. **모바일 백그라운드** → 임베드 정책이라 이 설계로도 안 풀린다

---

## 검증

### 공통 — 기존 기능 보호 (단계마다 매번 확인)

| # | 무엇이 참이어야 하는가 |
|---|---|
| R1 | **모드가 꺼져 있으면 모든 동작이 지금과 같다.** 봇이 음성에 없을 때 시각표가 안 생기고 대기열도 안 넘어간다 |
| R2 | 물리 재생 중 `stopped`·`current_position`·정렬·통계·카드가 지금과 같다 |
| R3 | `cargo test` 전부 통과 (착수 시점 232개에서 줄지 않는다) |

### 1단계

| # | |
|---|---|
| 1 | 모드 On + 리스너 있음 + 봇 음성 없음 → `schedule.startedUtc` 가 생긴다 |
| 2 | **브라우저 둘이 같은 곡을 틀고, 두 위치 차이가 2초 이내로 유지된다** (핵심 요구) |
| 3 | 서버가 `playback_payload` **와** `api_state_hot` 둘 다 `stopped` 를 보낸다 |
| 4 | **새로고침해도** 계속 흐른다 (`loadHot` 경로) |
| 5 | 가상 세션 생성·제거가 WS 프레임을 유발한다 (watcher 서명) |
| 6 | 곡 끝 시점에 정확히 한 번 넘어가고, `ensure_autoplay` 가 이어 돈다 |
| 7 | 가상 곡 시작 시 `on_track_started` 가 돌아 다음 곡 프리페치·preview 가 채워진다 |
| 8 | 길이 미상이면 시작하지 않고 `current_item`·`upcoming` 이 그대로다 |
| 9 | **모드 On 이면 기본 설정에서도 pause·seek·skip 이 된다** — 권한·API 게이트·화면 잠금 셋 다 |
| 10 | 리스너 판정이 `presence ∩ webPlayback=1` 이다. 페이지만 열어 둔 사람만 있으면 안 돈다 |
| 11 | 마지막 리스너가 나가면 멈추고, 다시 들어오면 멈춘 위치에서 잇는다 (2초 이내) |
| 12 | 가상 `current_position` 이 시작 전(미래 `started_utc`)에 0 이고 패닉하지 않는다 |
| 13 | 모드를 끄면 도는 세션이 즉시 정리된다 |

### 2단계

| # | |
|---|---|
| 14 | 봇 합류 시 물리 하나만 남고 위치가 인계 직전과 **2초 이내**로 이어진다 |
| 15 | 봇 이탈(**강제 퇴장 포함**) 시 가상이 그 위치에서 이어받는다 (2초 이내) |
| 16 | 인계 중 `schedule()` 이 한 번도 `None` 이 되지 않는다 (전환 구간 포함) |
| 17 | 인계로 `on_track_started` 가 **다시 불리지 않는다** — 같은 곡에서 디스코드 알림·프리페치가 재실행되지 않는다 |

### 3단계

| # | |
|---|---|
| 18 | 통계 DB `SCHEMA_VERSION` 이 올라가고 **기존 DB 에 `plays_virtual` 컬럼이 실제로 생긴다** |
| 19 | 가상 재생은 `plays_virtual` 만 올린다. `plays_user` 는 안 오른다 |
| 20 | `play_now`·`cancel_by_id`·`skip_to`·`skip` 으로 끝내도 분류가 맞는다 (웹 skip 의 취소-후-호출 순서 포함) |
| 21 | 양방향 인계 후에는 종료 시점 기준으로 **한쪽만** 오른다 |

### 4·5단계

| # | |
|---|---|
| 22 | 가상 중 카드 footer 에 `웹에서만 재생 중 · 음성 채널에서는 안 들려요` 가 있고, 물리로 바뀌면 사라진다 |
| 23 | 봇이 쫓겨나면(`guild_delete`) 도는 세션이 정리된다 |
| 24 | 모드를 켜는 순간 이미 접속해 있던 사람으로 시작된다 |
| 25 | 길이 미상 시 WS `notice` 가 나간다 |

### 실측

| # | |
|---|---|
| 26 | **브라우저 실측**: 봇이 음성에 없는 상태에서 웹 재생을 켜고, 고정 곡(provider `youtube`, contentId 는 착수 시 확정)으로 사용자 클릭 후 10초 안에 `YT.PlayerState.PLAYING` 관측, 3초 간격 두 표본에서 위치가 2초 이상 증가 |
| 27 | **동기 실측**: 같은 길드에 브라우저 둘을 붙이고 30초 관측 — 두 위치 차이가 항상 2초 이내 |

R1·R2·R3 는 **단계마다** 다시 본다. 2·27 이 사용자 요구의 핵심이고, 9 가 없으면 켜져도 아무도 못 쓴다.

---

## 1~3차가 틀렸던 것 (기록)

| 차수 | 틀린 주장 | 사실 |
|---|---|---|
| 1 | 화면은 안 고쳐도 된다 | 클라이언트가 스스로 멈춘다 |
| 1 | `voice_channel_id` 로 음성 판단 | 저장 바인딩이다. `bot_voice_status` 를 봐야 한다 |
| 1 | 합류 시 가상 세션 먼저 버림 | 위치를 `start_offset` 에 먼저 옮겨야 한다 |
| 1 | 행마다 `virtual: bool` | 집계 행이라 불가능. 카운터 컬럼이어야 한다 |
| 1 | 접속자가 있을 때만 돈다 | 호출 경로가 없었다 |
| 2 | `stopped` 두 곳 | 세 곳이다 (`loadHot`) |
| 2 | 물리 의미는 그대로 | 강제 퇴장 시 달라진다 — 그 자체가 버그 |
| 2 | 가상 위치 = `now - started_utc` | 미래일 수 있다. `Duration` 은 음수 불가 |
| 2 | `app.player.on_listeners_changed` | `PlayerManager` 는 `Coordinator`·`App` 참조가 없다 |
| 2 | (누락) `on_track_started`·권한 상호작용 | 없으면 프리페치·조작이 전부 죽는다 |
| 3 | `same_voice_satisfied` 만 열면 됨 | 권한 뒤에 별도 게이트가 또 있다 |
| 3 | 물리도 미래 `started_utc` 전엔 0 | 물리는 `started_utc` 를 아예 안 본다 |
| 3 | `GuildPlayerState.virtual_playback` | 매 조작마다 SQLite 에서 새로 만들어진다 |
| 3 | presence 면 위험이 사라진다 | pref 까지 봐야 한다 |
| 3 | (누락) 통계 DB 별도 마이그레이션·watcher 서명 | 컬럼이 안 생기고 프레임이 안 나간다 |

**공통점**: 확인 없이 단정했고, 한 번에 다 만들려 했다.
4차는 주장마다 근거를 붙였고 단계로 쪼갰다.
