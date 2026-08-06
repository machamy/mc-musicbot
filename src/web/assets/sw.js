/* 마참뮤직 리모컨 v2 — 서비스워커
 *
 * 반드시 `/music/sw.js` 로 서빙되어야 한다. 스코프가 스크립트 경로를 따라가므로
 * `/music/assets/sw.js` 에 두면 `/music/*` 를 못 잡는다. (사양서 §7.2 B13)
 *
 * 하는 일은 딱 하나: 정적 에셋(`/music/assets/*`)을 stale-while-revalidate 로 캐시한다.
 * 하지 않는 일:
 *   - API 응답(`/music/api/*`)은 절대 캐시하지 않는다. 권한·정지 상태가 섞이면 사고다.
 *   - WebSocket(`/music/api/guilds/{id}/events`)은 fetch 이벤트로 오지도 않지만, 와도 통과시킨다.
 *   - POST/PUT 등 GET 이외의 요청은 손대지 않는다.
 *
 * 버전: 등록할 때 `/music/sw.js?v={build_id}` 로 붙는 v 값을 캐시 이름에 넣는다.
 *       build_id 가 바뀌면 캐시 이름이 바뀌고, activate 에서 옛 캐시를 전부 지운다.
 */

const BUILD_ID = new URL(self.location.href).searchParams.get('v') || 'dev';
const CACHE_NAME = `macham-music-${BUILD_ID}`;
const CACHE_PREFIX = 'macham-music-';

const ASSET_PREFIX = '/music/assets/';
const API_PREFIX = '/music/api/';

/** 오프라인일 때 보여줄 최소 셸. 외부 리소스를 하나도 안 쓴다. */
const OFFLINE_SHELL = `<!doctype html>
<html lang="ko"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<title>연결 없음 · 마참뮤직</title>
<style>
  :root { color-scheme: dark; }
  body { margin:0; height:100vh; display:flex; align-items:center; justify-content:center;
         background:#07090f; color:#b9c2d4;
         font-family:"Pretendard Variable",Pretendard,system-ui,"Segoe UI","Malgun Gothic",sans-serif; }
  .box { max-width:340px; padding:32px 24px; text-align:center; }
  .ico { font-size:34px; }
  h1 { margin:12px 0 6px; color:#f2f5fa; font-size:19px; }
  p { margin:0; font-size:13px; line-height:1.6; color:#7d8899; }
  button { margin-top:20px; height:34px; padding:0 20px; border:none; border-radius:8px;
           background:#8b5cf6; color:#fff; font-size:13px; font-weight:600; cursor:pointer; }
</style></head>
<body><div class="box">
  <div class="ico">📴</div>
  <h1>연결이 끊겼다</h1>
  <p>리모컨은 봇 서버와 실시간으로 이어져 있어야 동작한다.<br>네트워크가 돌아오면 다시 시도해라.</p>
  <button onclick="location.reload()">다시 시도</button>
</div></body></html>`;

/* ── 설치: 즉시 대기 해제. 새 빌드를 오래 붙들고 있지 않는다. ── */
self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then(() => self.skipWaiting()));
});

/* ── 활성화: 이번 빌드 것이 아닌 캐시를 전부 삭제. ── */
self.addEventListener('activate', (event) => {
  event.waitUntil((async () => {
    const names = await caches.keys();
    await Promise.all(names
      .filter((name) => name.startsWith(CACHE_PREFIX) && name !== CACHE_NAME)
      .map((name) => caches.delete(name)));
    await self.clients.claim();
  })());
});

/* ── 페이지가 새 빌드를 감지하면 즉시 교체를 요청할 수 있다. ── */
self.addEventListener('message', (event) => {
  if (event.data === 'skipWaiting') self.skipWaiting();
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;      // 외부 도메인(앨범아트 등)은 손대지 않는다
  if (url.pathname.startsWith(API_PREFIX)) return;      // API·WS 는 절대 캐시하지 않는다

  if (url.pathname.startsWith(ASSET_PREFIX)) {
    event.respondWith(staleWhileRevalidate(request));
    return;
  }

  // 페이지 이동은 항상 네트워크 우선. 오프라인이면 셸만 보여준다.
  if (request.mode === 'navigate') {
    event.respondWith(networkThenShell(request));
  }
});

/**
 * 정적 에셋: 캐시가 있으면 즉시 주고 뒤에서 갱신한다.
 * 에셋 URL 에는 `?v={build_id}` 가 붙으므로 같은 이름이라도 빌드가 다르면 다른 캐시 항목이 된다.
 */
async function staleWhileRevalidate(request) {
  const cache = await caches.open(CACHE_NAME);
  const cached = await cache.match(request);

  const network = fetch(request).then((response) => {
    // opaque/에러 응답을 캐시에 넣으면 다음 로드가 통째로 깨진다.
    if (response && response.ok && response.type === 'basic') {
      cache.put(request, response.clone()).catch(() => {});
    }
    return response;
  }).catch(() => null);

  if (cached) return cached;
  const fresh = await network;
  if (fresh) return fresh;
  return new Response('', { status: 504, statusText: 'offline' });
}

/** 문서 요청: 네트워크가 죽었을 때만 오프라인 셸. 캐시된 HTML 을 되돌려주지 않는다(권한 화면이라 위험). */
async function networkThenShell(request) {
  try {
    return await fetch(request);
  } catch {
    return new Response(OFFLINE_SHELL, {
      status: 200,
      headers: { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' },
    });
  }
}
