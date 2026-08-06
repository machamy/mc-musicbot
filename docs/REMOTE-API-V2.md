# 리모컨 v2 — 백엔드 API 계약

`src/web/assets/portal.js`(유저 UI)와 `src/web/assets/console.js`(서버 관리 콘솔)가
**이미 이 계약대로 작성되어 있다.** 백엔드가 여기에 맞춘다. 프런트를 고치지 말고 서버를 맞춰라.

- 베이스: `/music/api/guilds/{guildId}`
- CSRF: 변경 요청은 `X-CSRF-Token` 헤더 필수
- 에러 바디: `{"error":"한국어 문장"}`
- 429: `Retry-After` 헤더 또는 `{"retryAfter": 초}`
- **모든 u64 ID는 JSON에서 문자열**로 보낸다 (JS 숫자 정밀도 손실 방지)
- `repeatMode`는 camelCase 소문자 `"off"|"track"|"queue"` (기존 PascalCase에서 변경)
- **모든 트랙 객체에 `durationSeconds` 숫자를 넣는다.** `duration`은 C# TimeSpan 문자열이라 신뢰 불가

트랙 형태:
```jsonc
{"title","artist","provider","contentId","cacheKey","durationSeconds":245,"artUrl":null}
```

---

## 1. 페이지 셸 부트스트랩

유저 UI (`GET /music/guilds/{id}`):
```js
window.MACHAM = { guildId, csrf, buildId, user:{id,displayName,avatarUrl}, tier, themeDefault }
```
서버 관리 콘솔 (`GET /music/guilds/{id}/admin`):
```js
window.MACHAM = { guildId, csrf, buildId, user, tier, guild, intentStatus }
```

`<head>`에 FOUC 방지 3줄을 **인라인**으로:
```html
<script>try{document.documentElement.dataset.theme=localStorage.getItem('macham.theme')||'dark'}catch(e){}</script>
```
키 이름 `macham.theme` 고정.

에셋 링크 순서: `tokens.css` → `portal.css`(또는 `console.css`).
스크립트는 `<script type="module" src="/music/assets/portal.js?v={build_id}">`.
**`portal.js`/`console.js`가 `./core.js`를 상대경로로 import하므로 셋이 같은 URL 디렉터리에 있어야 한다.**
따라서 캐시버스팅은 경로가 아니라 **쿼리스트링**으로만 한다.

---

## 2. 유저 UI 엔드포인트

### `GET /state/cold` — 진입 시 1회, `settings`/`library`/`suspension` 이벤트 시 재조회

```jsonc
{ "buildId":"", "guild":{"id","name","iconUrl"}, "guilds":[{"id","name","iconUrl"}],
  "user":{"id","displayName","avatarUrl"}, "tier":"owner|manager|member|viewer",
  "viewerReason": null,
  "intentStatus":{"members":true,"presences":true},
  "settings":{"chatEnabled":true,"suggestionEnabled":true,"visualizerEnabled":true,
              "minVolume":0,"maxVolume":200,"sortMode":"fair"},
  "permissions":{
    "can":{"search":true,"vote":true,"chat":true,"playback":true,"seek":true,"volume":true,
           "queueEdit":true,"playlistEdit":true,"library":true,"suggest":true,
           "chatDelete":false,"suggestStatus":false,"suspend":false,"sortMode":false,
           "console":false,"ops":false},
    "entries":[{"key":"search","label":"곡 신청","allowed":true,
                "rule":"guildMember|sameVoiceChannel|configuredRole|administrator|disabled",
                "ruleLabel":"모든 멤버","viaAdmin":false,"reason":null}]
  },
  "suspension": null,
  "playlists":[{"id","name","entryCount","isMine","entries":[{"track":{}}]}],
  "liked":[{"track":{}}], "saved":[{"track":{}}],
  "recent":[{"track":{},"playedUtc","requestedByDisplay"}],
  "members":[{"userId","displayName","avatarUrl","tier"}] }
```

- `can`의 키가 곧 UI 잠금 판정 키다.
- `entries`는 "내 권한" 화면(사양서 §1.3)의 근거다. `viaAdmin: true`면 `← 관리자라 통과`로 표시된다.
- `console`은 서버 관리 콘솔 진입 가능(Manager/Owner), `ops`는 운영 패널 링크 노출(Owner만).
- `suspension`이 있으면 `{"scope":"all|chat|queue","reason","expiresUtc","byDisplayName"}`.

### `GET /state/hot` — 진입 시 1회 + WS 재연결 시에만

```jsonc
{ "player":{"isPaused":false,"effectiveVolume":85,"repeatMode":"off",
            "shuffleEnabled":false,"voiceChannelId":"…","botOnline":true,
            "minVolume":0,"maxVolume":200},
  "current":{"id","track":{},"durationSeconds":245,"requestedByDisplay","requestedByUserId"},
  "positionSeconds":63.4, "sampledAtUtc":"2026-08-06T12:00:00Z",
  "queueMode":"score|fifo|fair", "sortedAt":"…",
  "queue":[{"id","track":{},"requestedByDisplay","requestedByUserId","isMine":true,
            "myVote":"like|superLike|null","round":1,
            "score":{"waitScore","likeCount","superLikeCount","manualPriority","totalScore"}}],
  "presence":{"listening":["…"],"viewing":["…"],"online":{"userId":"online|idle|dnd|offline"},
              "listeningCount":3,"viewingCount":5} }
```

**`sampledAtUtc`는 `positionSeconds`를 읽은 직후에 찍어야 한다.** 클라이언트가 이 값으로 보간한다.
`round`는 공평제에서 "그 사람의 N번째 곡"(0-based) 표시에 쓴다.

### 나머지 GET

| 경로 | 응답 |
|---|---|
| `GET /chat?before=` | `{"messages":[…],"nextBefore":123}` |
| `GET /audit?before=` | `{"entries":[{id,displayName,action,target,afterValue,failureReason,success,createdUtc}]}` |
| `GET /suggestions` | `{"items":[{id,title,body,userId,displayName,avatarUrl,status,statusNote,votes,votedByMe,createdUtc,isMine}]}` |
| `GET /search?q=&provider=` | `{"results":[track]}` |
| `GET /lyrics` | `{"plainText":null,"syncedLines":[{"startMs":0,"text":"…"}]}` |
| `GET /mention-candidates` | `{"items":[{userId,displayName,avatarUrl}]}` |

채팅 메시지 형태:
```jsonc
{"id":1,"userId":"…","displayName":"…","avatarUrl":"…","content":"…",
 "createdUtc","editedUtc":null,"deletedUtc":null,
 "replyTo":{"id":1,"displayName":"…","preview":"80자"} | null,
 "mentions":["userId"], "mentionNames":["지훈"],
 "tags":[{"cacheKey":"…","track":{}}],
 "reactions":[{"emoji":"👍","count":2,"reactedByMe":false,
               "users":[{"userId","displayName"}]}]}
```

### POST — 전부 `{ok:true, …}` 응답

| 경로 | 바디 |
|---|---|
| `/queue` | `{track}` → `{ok,itemId,queuePosition,playingNow}` |
| `/queue/action` | `{action:"remove"\|"togglePin", itemId}` |
| `/control` | `{action:"pause"\|"resume"\|"skip"\|"seek"\|"volume"\|"repeat"\|"shuffle", value, expectedItemId, mode}` |
| `/vote` | `{itemId, kind:"like"\|"superLike"\|null}` |
| `/library` | `{track, kind:"saved", present:true\|false}` |
| `/playlists/action` | `{action:"enqueue", playlistId}` |
| `/chat` | `{content, replyToMessageId, tags:[{cacheKey,track}]}` |
| `/chat/reaction` | `{messageId, emoji}` |
| `/chat/delete` | `{messageId}` |
| `/chat/read` | `{}` — 멘션 읽음 처리 |
| `/suggestions` | `{title, body}` |
| `/suggestions/vote` | `{suggestionId}` |
| `/suggestions/status` | `{suggestionId, status, note}` |
| `/suspensions` | `{userId, scope, minutes:0=무기한, reason}` |

**`/control`에 `repeat`·`shuffle`이 새로 필요하다.** 현재 백엔드는 pause/resume/skip/seek/volume만 지원한다.
`{action:"repeat", mode:"off|track|queue"}`, `{action:"shuffle", value:1|0}`.

---

## 3. 서버 관리 콘솔 엔드포인트

전부 `/music/api/guilds/{guildId}/admin` 하위. **Manager 이상만.**

### `GET /admin/settings`
```jsonc
{ "settings": {
  "sortMode":"fair", "autoBgmEnabled":true, "repeatMode":"off",
  "defaultVolume":100, "minVolume":0, "maxVolume":200,
  "searchRule":"guildMember","voteRule":"guildMember","chatRule":"guildMember",
  "playbackRule":"sameVoiceChannel","seekRule":"guildMember",
  "volumeRule":"sameVoiceChannel","queueEditRule":"sameVoiceChannel",
  "configuredRoleIds":["123…"],
  "maxQueuePerUser":5,"maxQueuePerGuild":100,"maxTrackSeconds":14400,
  "auditRetentionDays":14,"chatRetentionDays":30,
  "chatEnabled":true,"suggestionEnabled":true
}}
```
**`configuredRoleIds`는 문자열 배열**로 주고받는다. 서버가 파싱한다.

### `PUT /admin/settings/{section}` — `section` ∈ `order|perms|limits|chat`
요청은 그 섹션 키만 담은 부분 객체. 응답 `{"ok":true,"settings":{…정규화…}}`.
`sortMode` 변경은 활동 로그에 남기고 `settings` 이벤트를 broadcast한다.

### 나머지

| 경로 | 응답/바디 |
|---|---|
| `GET /admin/roles` | `{"roles":[{id,name,color,memberCount}]}` — 실패해도 빈 배열 |
| `GET /admin/queue-preview?mode=fair` | `{mode,totalCount,items:[{itemId,title,requestedBy,roundLabel,score,currentPosition,previewPosition,delta}]}` |
| `GET /admin/permission-preview?rule=&roleIds=1,2` | `{rule,passCount,memberCount,managerBypassCount,note,sample:[{userId,displayName,avatarUrl,bypass}]}` |
| `GET /admin/participants` | `{"members":[{userId,displayName,avatarUrl,tier,presence,lastSeenUtc,queueCount,chatCount,suspensions:[{scope,expiresUtc}]}]}` |
| `GET /admin/suspensions` | `{"items":[{userId,displayName,avatarUrl,scope,reason,byUserId,byDisplayName,createdUtc,expiresUtc}]}` |
| `POST /admin/suspensions` | `{userId,scope,minutes:5\|30\|180\|null,reason}` — **대상이 manager/owner인데 호출자가 Owner가 아니면 403** |
| `POST /admin/suspensions/lift` | `{userId,scope}` |
| `GET /admin/reports?status=open` | `{"items":[{id,messageId,messageAuthor,messageContent,reporterDisplayName,reason,createdUtc}]}` |
| `POST /admin/reports/{id}/resolve` | `{action:"delete"\|"dismiss"}` |
| `GET /admin/suggestions?limit=50` | `{"items":[{id,title,body,displayName,createdUtc,voteCount,status,statusNote}]}` |
| `POST /admin/suggestions/{id}/status` | `{status,note}` |
| `GET /admin/audit?limit=&before=&q=` | `{"items":[…],"nextCursor":941\|null}` |
| `GET /admin/diagnostics` | `{bot:{online,voiceConnected,voiceChannelName,gatewayLatencyMs},buildId,schemaVersion,uptimeSeconds}` |

**`permission-preview`에서 `rule=disabled`면 `passCount`는 반드시 0이다(관리자 포함).**
사양서 §7.1 S3(관리자가 `Disabled`를 통과하는 버그) 수정과 같은 판정 함수를 써야 화면이 거짓말을 안 한다.

---

## 4. WebSocket — `GET /music/api/guilds/{id}/events`

프레임은 전부 `{"t": 토픽, "d": 데이터}`. **payload 없는 문자열 토픽은 클라이언트가 무시한다.**

| `t` | `d` |
|---|---|
| `playback` | `{isPaused,positionSeconds,sampledAtUtc,currentId,current?,durationSeconds?,effectiveVolume?,repeatMode?,shuffleEnabled?,voiceChannelId?,botOnline?}` |
| `queue.set` | `{items,mode,sortedAt}` — `items`는 hot의 `queue`와 같은 형태 |
| `vote` | `{itemId,like,super,total,myVote?}` |
| `chat.add` | 채팅 메시지 전체 |
| `chat.react` | `{messageId,emoji,userId,displayName,added}` |
| `chat.delete` | `{messageId,deletedUtc}` |
| `presence` | `{listening:[],viewing:[],online:{},listeningCount,viewingCount}` |
| `members` | `{members:[]}` |
| `settings` / `library` / `suspension` | cold 재조회 트리거 (`suspension`은 객체 또는 null) |
| `suggestion.add\|vote\|status` / `audit` | 해당 탭이 열려 있을 때만 재조회 |
| `lyrics` | 가사 재조회 |
| `notice` | `{message,kind}` — 토스트 |

**보안**: 이제 WS가 실제 데이터를 나르므로 `api_events`는 세션·길드 확인만으로는 부족하다.
전체 `authorize` 경로 + Origin 허용목록을 태워야 한다 (사양서 §7.1 S4).

**정지·거부**: 서버가 접근을 끊을 때 close code `1008` 또는 `4403`을 쓴다. 클라이언트는 재시도하지 않는다.

**presence는 DB를 쓰지 않는다.** 메모리 레지스트리 + Discord 캐시에서 만들고, 변경 시에만
**최대 초당 1회로 코얼레싱**해서 보낸다.

---

## 5. 성능 계약 (사양서 §5.2 재확인)

- 유휴 상태(아무 조작 없음): **탭당 SQLite 쿼리 0회/초**
- 곡 전환 1회: 쿼리 10회 이하
- 채팅 1건: 쿼리 2회(insert + 멘션)
- 탭 10개가 붙어도 재생 경로 지연 없음

이를 위해 `/state` 단일 스냅샷 2초 폴링을 **폐기**하고 hot/cold 분리 + 타입드 WS push로 간다.
기존 `GET /state`는 당분간 유지해도 되지만 새 프런트는 쓰지 않는다.
