# 계획 05 — 웹에 우리가 직접 소리를 보내기

작성: 2026-09-10 · 상태: **계획. 구현 없음.** · **선행 조건이 있다 — §0 을 먼저 읽을 것**

> "진짜 송출을 해버려? 디스코드랑 같은 프로토콜로 보내면 되는거라"
> "근데 문제는 내컴에서 모든유저한테 음성파일이나 신호를 보내는건 유저가 많아질수록 힘들다는거야"

결론부터. **걱정한 것(대역폭)은 문제가 아니었고, 노린 것(아이폰 백그라운드)은 될지 모른다.**
그래서 이 계획의 1단계는 코드가 아니라 **반나절짜리 실측**이다.

---

## 0. 실측 — 끝났다 (2026-09-13, 안드로이드)

> **이 절은 원래 '재기 전에는 한 줄도 쓰지 마라' 였다.** 그리고 표에 *"안드로이드가 이미 잘
> 된다 → v4.57 로 충분했다"* 라고 적어 뒀는데 **그게 틀렸다.** 문제는 아이폰이 아니라
> **안드로이드**였고, v4.69 를 배포한 뒤에도 안 고쳐졌다. 아래는 그걸 실제로 잰 결과다.

재는 장비를 만들어 뒀다 — `scripts/Test-AndroidBackgroundAudio.ps1`. 에뮬레이터(API 36,
`google_apis_playstore`)에 실제 크롬을 띄워 두 갈래를 같은 조건으로 돌린다. 남의 폰을
빌리지 않고 고칠 때마다 다시 잴 수 있다.

### 결과

| 갈래 | JS 판정 | 플랫폼 신호 (`dumpsys audio` 의 `state:started`) |
|---|---|---|
| **A** 우리 오리진 `<audio>` (128k opus) | **계속 재생됨** · 벽시계 24.42s / 진행 24.42s (비율 1.000) | 전경 1 → 배경 **1, 1, 1** |
| **B** 유튜브 1×1 iframe (지금 방식) | **멈췄음** · `getPlayerState()` 1→**2**(PAUSED), 시간 동결 | 전경 1 → 배경 **0, 0, 0** |

숨는 순간(`document.hidden`) B 는 곧바로 멈춘다. A 는 25초 내내 1초당 1.00초씩 흐르고
`paused:false` 였다 — JS 조차 안 졸았다(소리가 나는 페이지는 크롬이 덜 조인다).

**대조군이 결론을 지탱한다.** B 가 같은 브라우저·같은 세션에서 멈추는 것을 같이 보지 못하면,
A 가 살아남은 것이 "에뮬레이터가 원래 배경 정책을 안 걸기 때문" 인지 구별할 수 없다.
둘이 갈렸으므로 에뮬레이터는 정책을 걸고 있고, A 의 성공은 진짜다.

### 이 측정이 증명하지 못하는 것

정직하게 적는다. 셋 다 결론을 바꿀 수 있다.

1. ~~누가 멈췄는지는 모른다.~~ **해결됐다 — 유튜브다.** 측정만으로는 "유튜브가 스스로
   멈춘다" 와 "크롬이 숨은 교차 출처 비디오를 특별 취급한다" 를 가를 수 없었는데, 크롬의
   문서가 가려 준다: 크롬은 배경에서 **오디오 트랙이 없는** 미디어만 자동으로 멈추고,
   소리가 있는 미디어는 **비디오 트랙만 끄고 계속 재생한다.** 즉 크롬이라면 멈추지 않았다.
   (§7 참고)
2. **"크롬이 배경 웹오디오를 죽인다" 는 배제됐다.** 이건 확정이다 — A 가 같은 환경에서 살았다.
3. **제조사 절전은 아직 안 쟀다.** 에뮬레이터는 맨 안드로이드다. 삼성 One UI·샤오미는
   프로세스를 더 적극적으로 죽인다. 여기서 통과해도 실기기 확인이 남는다 —
   30분 이상 곡 경계를 넘겨서, 화면 잠금, 절전 모드, 블루투스 끊김, 밤새 한 번.
   **25초 홈 시험은 "즉시 멈추는가" 만 답한다.**

---
## 1. 걱정했던 것 — 대역폭은 문제가 아니다

실측 (봇 호스트 캐시 3,614곡): **평균 3.77 MB**, 최소 0.24 MB, **최대 286.74 MB**, 총 13.32 GB.

```
128 kbps = 16 KB/s = 57.6 MB/시간   (1명당)
```

| 동시 청취자 | 업로드 | 하루 4시간·30일 |
|---|---|---|
| 10명 | 1.28 Mbps | 69 GB |
| 20명 | 2.56 Mbps | 138 GB |
| 30명 | 3.84 Mbps | 207 GB |

오디오 128k 는 1080p 영상(약 5Mbps)의 **1/39** 다. "유저 많아지면 힘들다" 는 영상
스트리밍의 직관이고, 오디오에는 해당하지 않는다.

**참고 — 지금 웹 청취자는 실측 최대 1명이다.** 19일치 로그에서 웹 관련 줄이 9개뿐이다.
이 기능이 거의 안 쓰이고 있다는 뜻이고, 그 이유가 지금 웹 재생이 불편해서일 수도 있다
(백그라운드 안 됨, 곡 사이 무음, 임베드 금지 곡 재생 불가).

### 진짜 위험은 평균이 아니라 몰림이다

**모두가 같은 곡을 같은 지점에서 듣는다.** 그래서 곡이 바뀌는 순간이 **동시에** 온다.
10명이면 그 순간 10 × 3.77MB = **37.7 MB 가 한꺼번에** 나간다.

이건 **미리 받기로 풀린다.** 다음 곡을 앞 곡이 나오는 동안(3~4분) 나눠 받으면
37.7MB / 210초 = 180 KB/s 로 평탄해진다. 즉 §3 의 미리 받기는 편의 기능이 아니라
**몰림 방지 장치**다.

---

## 2. 하지 않기로 한 것 (그리고 그 이유)

### 조각내서 이어 붙이기 — 못 한다, 그리고 할 필요도 없다

"5분 이상이면 3분 단위로 잘라 받자" 는 발상은 문제를 정확히 짚었지만, 두 가지가 걸린다.

**하나. 우리 캐시 형식으로는 안 된다.** 브라우저에서 실측했다.

```
MediaSource.isTypeSupported          ← 조각을 이어 붙일 때 쓰는 것
  audio/ogg;  codecs=opus   →  false    ← 우리 캐시가 이것 (.opus = Ogg)
  audio/webm; codecs=opus   →  true
  audio/mp4;  codecs=mp4a   →  true

<audio> canPlayType                  ← 통째로 재생
  audio/ogg;  codecs=opus   →  probably ← 이건 된다
```

조각 이어 붙이기(MSE)는 Ogg 를 안 받는다. 하려면 캐시 3,614개를 전부 WebM 으로 다시
담아야 한다(`ffmpeg -c copy`, 재인코딩은 아니지만 한 단계가 더 생긴다).

**둘. 브라우저가 이미 그걸 한다.** `<audio src>` 는 전체를 무작정 빨아들이지 않는다.
앞부분을 받아 재생을 시작하고, 버퍼가 차면 네트워크를 멈췄다가 소비되면 Range 로 이어
받는다. 얼마나 앞서 받을지는 회선 속도·화면 상태·데이터 절약 설정까지 보고 정한다 —
**우리가 3분이라고 못 박는 것보다 낫다. 우리는 그 정보를 모른다.**

그래서 **길이별 분기(5분 기준)도 없앤다.** Range 만 제대로 붙이면 4MB 짜리는 한두 번에
끝나고 286MB 짜리는 브라우저가 알아서 나눠 받는다. 분기가 없으면 그 분기의 버그도 없다.

### 파일 해시 — `cache_key` 가 이미 그 일을 한다

`youtube:영상ID` 는 내용이 절대 안 바뀐다. 해시를 계산해 비교할 것 없이

```
Cache-Control: public, max-age=31536000, immutable
```

한 줄이면 브라우저가 두 번 다시 안 물어본다. 계산 비용도 비교 로직도 없다.

### 싱크 포기 — 아낄 것이 없다

`WEB-PLAYER-DESIGN.md:6` 의 최상위 요구가 *"사람마다 반드시 같은 곳, 같은 노래"* 다.
그리고 **싱크는 이미 공짜로 있다** — `startedUtc` 절대시각 + 시계 오차 보정이 그대로
쓰인다. 오히려 `<audio>` 로 바꾸면 **쉬워진다**(§5). 포기할 이유가 없다.

---

## 3. 만들 것

### 서버

| 파일 | 무엇 | 대략 |
|---|---|---|
| `crates/mc-app/src/web/stream.rs` (신규) | Range 파서 + 파일 응답. `Accept-Ranges`·`Content-Range`·206·`Content-Type: audio/ogg`·`immutable` | 200줄 |
| `crates/mc-app/Cargo.toml` | `tokio-util` features `io` (`ReaderStream`). `tower-http::ServeDir` 는 쓰지 않는다 — 경로 노출 통제를 우리가 해야 한다 | 1줄 |
| `web/remote.rs` | 라우트 `GET /music/api/guilds/{id}/stream/{cache_key}`. **반드시 `/music/api/` 아래** — `sw.js:75` 가 그 경로만 통과시킨다. 다른 자리에 두면 서비스워커가 가로채 Range 가 깨진다 | 60줄 |
| 〃 | 권한: 길드 멤버십 + **그 곡이 지금 그 길드 큐/미리보기에 있는지**. `cache_key` 로 아무 캐시나 긁는 것을 막는다 | 60줄 |
| 〃 | 슬롯 관리 (§4) | 80줄 |
| 〃 | 상태 payload 에 `streamUrl`·`streamSlots` 추가 — **세 곳 전부** (`playback_payload`·hot·cold) + watcher 서명. 한 곳이라도 빠지면 프레임이 안 나간다 | 60줄 |
| `player/coordinator.rs` | `reconcile_virtual` 이 **현재 곡도** `cache.prepare` 하도록. 지금은 다음 2곡만 받아서 웹 전용 첫 곡이 비어 있다 | 60줄 |
| `remote/models.rs` | `web_stream_enabled: bool`(기본 false), `web_stream_max_listeners: u32`(기본 10) | 20줄 |

### 클라이언트

| 파일 | 무엇 | 대략 |
|---|---|---|
| `portal.js` | `<audio>` **두 개** 교대(이중 버퍼). `currentTime` 동기 + `playbackRate` 미세보정. 실패 시 iframe 으로 폴백 | 300줄 |
| 〃 | `🔊 직접 받기` 토글 (§4) | 60줄 |
| 〃 | MediaSession 을 `<audio>` 기준으로 다시 검 | 50줄 |
| `console.js` | 관리 콘솔에 켜기/인원 상한 | 40줄 |
| `sw.js` | **변경 없음** (`/music/api/*` 통과 확인함) | 0 |

**합계 약 900줄.** 되돌리기는 토글 하나다.

---

## 4. 인원 제한 — 어떻게 걸 것인가

### 사람이 직접 켠다

`웹에서 듣기` 안에 **`🔊 직접 받기`** 를 따로 둔다. 기본은 꺼짐(= 지금처럼 유튜브 임베드).
**켠 사람 몫만 대역폭이 나간다.** 안 켠 사람은 지금과 완전히 같다.

### 상한은 전역이다

대역폭은 길드가 아니라 **호스트의 자원**이다. 길드별로 걸면 길드 셋이 각각 10명씩
차지해 30명이 된다. 기본값 **10명**, 설정으로 조절.

### 꽉 찼을 때가 이 설계의 핵심이다

**거절하지 않는다. 원래 방식으로 되돌린다.**

```
자리 있음  →  <audio> 로 우리가 보낸다
자리 없음  →  유튜브 임베드로 듣는다 (지금과 같음)  + "지금은 자리가 없어 유튜브로 들어요"
```

실패 모드가 "못 듣는다" 가 아니라 **"예전처럼 듣는다"** 다. 그래서 상한을 낮게 잡아도
안전하고, 낮게 잡는 것이 맞다.

### 슬롯을 언제 놓는가

이게 안 되면 유령 슬롯이 쌓여 아무도 못 켠다. 이미 있는 장치를 그대로 쓴다 —
`WEB_LISTENER_GRACE`(90초, `remote.rs:5348`)와 같은 스위퍼에 얹는다. 소켓이 끊기고
90초가 지나면 슬롯도 같이 반납된다.

---

## 5. 덤으로 풀리는 것

이 전환은 아이폰 백그라운드 때문에 하는 것이지만, 곁다리로 셋이 같이 풀린다.

**하나. 곡 사이 무음.** 지금 코드가 스스로 적어 뒀다 —
> `portal.js:6589` — "유튜브 플레이어가 **하나뿐**이라, 거기에 다음 곡을 cue 하는 순간 지금 곡이 내려간다. 진짜로 하려면 숨은 플레이어를 하나 더 두고 곡이 바뀔 때 둘을 바꿔 끼워야 한다(이중 버퍼). 그건 별도 작업이라..."

`<audio>` 는 둘 만드는 게 `new Audio()` 두 번이다.

**둘. 싱크가 쉬워진다.** 지금 `seekTo(t, true)` 는 **네트워크 재요청**을 유발해서
되먹임을 만든다(v4.55 가 쿨다운 6초로 땜질했다). 로컬 `<audio>` 는 `currentTime = t` 가
버퍼 안에서 즉시 끝난다. 게다가 `playbackRate = 1.002` 로 **seek 없이** 서서히 맞출 수
있다(0.2% = 3.5센트, 귀에 안 들린다). 지금 구조로는 불가능한 방법이다.

**셋. 임베드 금지 곡.** 실측으로 확인했다 — `ERROR 150`(업로더가 임베드를 막음)이 나면
지금은 웹에서 못 듣는다(`portal.js:6658-6659`). 우리가 보내면 들린다.
**다만 이건 §6 의 문제이기도 하다.**

---

## 6. 감수해야 하는 것

### 클라우드플레어

실측으로 확인했다 — 리모컨은 클라우드플레어 엣지를 지난다.

```
server: cloudflare · cf-ray: ...-HKG · cf-cache-status: DYNAMIC
```

터널과 CDN 이 별개 제품인 것은 맞지만 **바이트는 그들 망을 지나간다.** CDN 약관에
*"유료 서비스 없이 video, audio files, 또는 큰 파일을 서빙하면 제한할 권리"* 조항이
있고, 월 수백 GB 의 오디오는 문언상 해당한다. 실제 제재 여부는 규모·재량이라 예측 불가다.

피하려면 오디오만 별도 서브도메인을 회색 구름(프록시 끔)으로 두고 직결해야 하는데,
그러면 **집 IP 가 노출되고** 포트를 직접 열어야 한다 — `README.md:159` 의 권장과 정반대다.
**지금 판단: 상한 10명이면 월 69GB 수준이라 일단 진행하되, 늘릴 때 다시 본다.**

### 유튜브 약관 — **여기가 가장 무거운 항목이다**

2026-09-13 조사(서브에이전트 + Codex)로 확인했다. 사실만 적는다.

**① 지금 쓰는 1×1 임베드가 이미 규정 위반이다.** 이건 계획과 무관하게 지금 그렇다.

> *"Embedded players must have a viewport that is at least 200px by 200px"*
> — [required-minimum-functionality](https://developers.google.com/youtube/terms/required-minimum-functionality)

**② 배경 재생 자체가 명시적으로 금지돼 있다.** 즉 우리가 "고치려는" 동작은 버그가 아니라
의도된 차단이다.

> *"create, include, or promote features that play content, including audio or video
> components, from a background player, meaning a player that is not displayed in the page,
> tab, or screen that the user is viewing"* — 개발자 정책 §III.F.3
> — [developer-policies](https://developers.google.com/youtube/terms/developer-policies)

**③ 오디오만 떼어내는 것도 금지 조항에 그대로 있다.** 이 계획의 핵심이 바로 그것이다.

> *"separate, isolate, or modify the audio or video components of any YouTube audiovisual
> content"* — 같은 문서 §III.I

**④ 유튜브가 2026년 2월에 실제로 조이기 시작했다.** 구글이 공식으로 확인한 발언이다.
우회로가 점점 막히는 방향이라는 뜻이다.

> *"Background playback is a feature intended to be exclusive for YouTube Premium members. …
> we have updated the experience to ensure consistency across all our platforms."*
> — [9to5Google (2026-02-02)](https://9to5google.com/2026/02/02/youtube-background-playback-workarounds-not-working-third-party-browsers/)

**⑤ 이 사유로 앱이 플레이 스토어에서 실제로 내려갔다.** 개발자가 배경 재생 API 를 부르지도
않았고 유튜브 임베드 기본 컨트롤만 있었는데도 심사에서 걸렸다.

- [react-native-youtube-iframe #72](https://github.com/LonelyCpp/react-native-youtube-iframe/issues/72)
  — *"your app violates the Device and Network Abuse policy by enabling background play of YouTube videos"*
- 안드로이드용 유튜브 플레이어 라이브러리도 같은 경고를 문서에 박아 뒀다:
  *"you won't be able to publish your app on the Play Store"*
  — [android-youtube-player wiki](https://github.com/PierfrancescoSoffritti/android-youtube-player/wiki/Useful-info)

### 그래서 이 계획이 바꾸는 것은 무엇인가

**"약관을 안 지키던 것을 지키게 되는" 변화가 아니다.** 위반의 성격이 옮겨 간다.

| | 지금 | 이 계획 뒤 |
|---|---|---|
| 위반 조항 | §III.F.3 (배경 플레이어) · 200×200 | §III.I (오디오 분리) · 다운로드·캐싱 |
| 누가 재배포하나 | 각 브라우저가 유튜브에서 직접 받는다 | **우리 서버가 파일을 내려준다** |
| 노출 | 숨은 임베드 하나 | 서버가 오디오를 서빙하는 엔드포인트 |

디스코드 재생도 같은 다운로드를 하지만, 그건 **청취자 수와 무관한 한 스트림이고 파일이
사용자에게 넘어가지 않는다.** 이 계획은 그 선을 넘는다 — 파일이 사용자 브라우저로 간다.
그게 실질적인 차이다.

**판단은 이 문서가 하지 않는다.** 위험을 감수할지는 봇 주인이 정할 일이고, 이 절은 그 결정에
필요한 사실을 갖춰 두기 위해 있다. 다만 **기술적으로는 이 길밖에 없다는 것**이 §7 에서
확인됐으므로, 선택지는 "이 계획" 과 "안드로이드 배경 재생을 포기" 둘이다.
### 캐시 미스 지연

웹 전용 모드의 첫 곡은 캐시에 없다. v4.56 이 기록한 "통째로 받아 재인코딩" 지연이
그대로 온다. `reconcile_virtual` 에 현재 곡 `prepare` 를 넣는 것이 **필수**다(§3).

### 캐시 정리가 재생 중인 파일을 지울 수 있다

`cache.rs` 의 LRU 정리는 지금 "재생 중"을 모른다. 스트리밍 중인 파일을 지우면 그 사람의
소리가 끊긴다. 슬롯이 잡고 있는 `cache_key` 는 정리 대상에서 빼야 한다.

---

## 7. 왜 다른 길이 없는가 (2026-09-13 조사)

"우리 오리진 `<audio>` 로 옮기는 것" 이 유일한 길인지 확인했다. **전부 막혀 있다.**

### 멈추는 주체는 크롬이 아니라 유튜브다

크롬의 배경 미디어 정책은 **오디오 트랙이 없는** 미디어만 멈춘다. 소리가 있으면 비디오
트랙만 끄고 계속 재생한다. 그래서 크롬 탓이라면 우리 임베드는 안 멈췄어야 한다.

> *"Chrome now disables video tracks when the video is played in the background … If the video
> doesn't contain any audio tracks, the video will be automatically paused when played in the
> background."* — [Media updates in Chrome 61](https://developer.chrome.com/blog/media-updates-in-chrome-61)

유튜브 플레이어는 **Page Visibility API** 로 배경 전환을 감지해 스스로 멈춘다. 이걸 무력화
하는 유저스크립트들이 전부 `document.hidden` 을 `false` 로, `visibilityState` 를 `visible` 로
고정하는 방식인 것이 그 증거다 —
[video-bg-play-userscript](https://github.com/Delphox/video-bg-play-userscript)

**그래서 고치려면 유튜브 문서 안에서 코드가 돌아야 한다.** 교차 출처 `www.youtube.com/embed`
안으로는 손이 안 들어간다. 이걸 끄는 파라미터·playerVar·`allow=` 토큰은 없다.

### 시도해 볼 만해 보였던 것들 — 전부 아니오

| 방법 | 되나 | 왜 |
|---|---|---|
| `allow="autoplay"` | ✘ | 자동재생 권한만 위임한다. 정지와 무관 |
| 부모에서 PiP 호출 | ✘ | `requestPictureInPicture()` 는 **내 문서의** `<video>` 에만 부를 수 있다. IFrame API 에 PiP 메서드도 없다 |
| Document PiP (iframe 을 PiP 창에) | ✘ | **안드로이드 크롬 미지원** |
| MediaSession 핸들러로 다시 `playVideo()` | ✘ | 다시 멈춘다. 반복해서 싸우는 건 금지 조항 우회 시도다 |
| Web Worker · 서비스워커 | ✘ | 미디어 요소를 소유하지 않는다. 페이지 가시성을 바꿀 수 없다 |
| Wake Lock · 무음 오디오 · 소켓 심박 | ✘ | 배경 자격을 주지 않는다 |
| PWA 로 설치 | ✘ | 설치는 임베드의 재생 계약을 바꾸지 않는다 |

PiP 는 명세가 못 박아 준다 — 알고리즘이 `HTMLVideoElement` 에 정의돼 있고 호출 문서의
transient activation 을 요구한다. 교차 출처 프레임에는 둘 다 없다.
[W3C Picture-in-Picture](https://www.w3.org/TR/picture-in-picture/)

### APK 는 답이 아니다 — 이 일의 **하류**다

| | 결과 |
|---|---|
| **TWA** | WebView 가 아니라 **크롬**을 쓴다(Custom Tabs). 유튜브 정지를 그대로 물려받는다 |
| **WebView 껍데기** (Capacitor·Cordova) | **크롬보다 나쁘다.** WebView 는 창 가시성이 바뀌면 미디어를 멈춘다 — 크롬에 없는 WebView 고유 동작이다. `onWindowVisibilityChanged` 를 네이티브로 덮어 억지로 살릴 수는 있는데, 그러면 **알림·잠금화면 컨트롤이 사라지고** 스토어에서 내려간다 |
| **네이티브 플레이어** (Media3/ExoPlayer + `MediaSessionService` 포그라운드 서비스) | 확실히 튼튼하다. **그런데 재생할 스트림이 먼저 있어야 한다** |

마지막 줄이 핵심이다. 네이티브 앱도 유튜브를 직접 못 튼다 — 우리 서버가 파일을 내려줘야
한다. 즉 **1단계(`stream.rs`)를 건너뛰는 길이 아니라, 그 위에 얹는 추가 공사다.**
[Android 배경 재생](https://developer.android.com/media/media3/session/background-playback)

### 우리 오리진 `<audio>` 는 문서로 보장된 길이다

크롬 안드로이드는 `<audio>`/`<video>` 재생에 알림·잠금화면 컨트롤을 띄우고, 그게 배경
재생을 지탱한다. `display: standalone` PWA 에서도 된다.
[미디어 알림](https://developer.chrome.com/blog/media-notifications)

조건이 몇 개 붙는데, 구현할 때 그대로 지켜야 한다.

- **5초보다 긴 미디어만** 알림이 뜬다.
- **Web Audio 만으로 소리를 내면 알림이 없다.** 반드시 `<audio>` 요소를 거쳐야 한다.
- `<audio>` 요소를 **항상 DOM 에 살려 둔다.** 붙였다 떼면 재생 권한이 흐트러진다.
- `metadata` 와 `play`/`pause`/`previoustrack`/`nexttrack` 핸들러를 채워 둔다.

### 남은 진짜 위험은 제조사 절전이다

API 문제가 아니라 **기기 설정 문제**다. 샤오미·삼성·화웨이는 맨 안드로이드 위에 자체
절전을 얹어 배경 앱을 얼린다(삼성은 기본으로 사흘 안 쓴 앱을 잠재운다).
[dontkillmyapp.com](https://dontkillmyapp.com/problem)

사용자 쪽 해결은 한 줄이다 — **설정 → 앱 → 크롬(또는 설치한 PWA) → 배터리 사용량 →
"제한 없음".** 이걸 안내 문구로 넣어야 한다.

오래 배경에 있으면 결국 멈추는 크로미움 버그도 열려 있다 —
[issue 375973479](https://issues.chromium.org/issues/375973479)

---

## 8. 구현할 때 틀리기 쉬운 것 (Codex 검토)

계획 초안에 있던 낙관을 고친다.

- **`<audio>` 두 개로 진짜 무간격은 안 된다.** `ended`·`timeupdate`·타이머는 샘플 단위가
  아니고, Opus 의 pre-skip/end padding 과 디코더 기동 시간이 있다. B 를 미리 완전히 준비해
  A 가 끝나기 조금 전에 시작하고 크로스페이드하면 **주관적으로** 촘촘해진다. 코덱 수준
  무간격은 약속하지 마라.
- **Range 만으로 탐색 정확도가 보장되지 않는다.** 컨테이너 타임스탬프·pre-skip·연속성을
  서버에서 검증해야 한다. 바이트 범위는 바이트만 준다.
- **`immutable` 보다 내용 주소가 먼저다.** 순서가 바뀌면 잘못 만든 파일이 캐시에 박혀
  안 빠진다. `cache_key` 가 이미 내용 주소이므로 그걸 URL 에 넣고 나서 `immutable` 을 건다.
- **`206`·`Content-Range`·`Accept-Ranges`·MIME·검증자(ETag)·CORS 를 제대로 준다.**
- **MediaSession 은 문서 단위다.** 두 `<audio>` 사이에 "인계" 같은 것은 없다. 핸들러 한 벌을
  두고 어느 쪽이 주인이 됐을 때 `metadata`·`positionState` 만 갱신한다.
- **두 요소가 동시에 소리를 내지 않게 한다**(의도한 크로스페이드 구간 제외). 안 그러면
  크롬·시스템의 미디어 상태 판정이 흐려진다.
- **첫 재생은 탭이 직접 `play()` 를 불러야 한다.** 사이에 `await` 를 끼우면 사용자 활성화가
  소모돼 거절된다. 불러오기·디코딩·MediaSession 설정은 활성화를 만족시키지 않는다.
- **브라우저 캐시는 할당량 관리 대상이라 언제든 비워진다.** 미리받기는 최적화일 뿐
  저장소로 취급하면 안 된다.

---
## 단계

| # | 무엇 | 선행 | 크기 |
|---|---|---|---|
| **0** | **§0 실측 — 아이폰/안드로이드 PWA 백그라운드** | 없음 | 반나절, 코드 0줄 |
| 1 | `stream.rs` — Range + 권한 + `immutable` | 0 통과 | 서버 320줄 |
| 2 | payload 세 곳 + watcher 서명 | 1 | 60줄 |
| 3 | `reconcile_virtual` 현재 곡 prepare | 1 | 60줄 |
| 4 | 클라 `<audio>` 이중 버퍼 + 폴백 | 2 | 300줄 |
| 5 | `🔊 직접 받기` 토글 + 슬롯 | 4 | 140줄 |
| 6 | 다음 곡 미리 받기 (**크기 상한 필수**) | 4 | 60줄 |
| 7 | 캐시 정리에서 재생 중 파일 제외 | 1 | 30줄 |
| 8 | `REMOTE-API-V3.md` §9.1 "서버 추가 부담: 0" 정정 | — | 문서 |

**8번을 빠뜨리지 마라.** 그 문장은 지금 이 저장소의 헌법처럼 읽힌다. 뒤집는다면 왜
뒤집는지를 같은 문서에 적어야 다음 사람이 같은 반려를 반복하지 않는다.

---

## 하지 않기로 한 것

| | 왜 |
|---|---|
| 3분 단위 직접 조각내기 | MSE 가 Ogg 를 안 받는다(실측). 그리고 브라우저가 이미 한다 |
| 5분 기준 길이 분기 | Range 면 한 경로로 끝난다. 분기가 없으면 그 버그도 없다 |
| 파일 해시 | `cache_key` 가 이미 내용 주소다. `immutable` 한 줄로 끝 |
| 64k 로 낮춰 보내기 | 절대량이 이미 작아 이득이 작고, 곡마다 재인코딩 + 캐시 2배가 든다. 모바일 데이터 절약 옵션으로는 뜻이 있다 |
| 백그라운드일 때만 전환 | iOS 는 백그라운드에서 JS 가 멈춰 전환 코드가 안 돈다. 안드로이드도 전환 순간 끊긴다 |
| 전면 교체 (iframe 제거) | 폴백이 없어진다. 임베드는 남겨 두는 것이 상한·실패의 안전망이다 |
| 싱크 포기 | 최상위 요구와 충돌하고, 아낄 것도 없다 |
