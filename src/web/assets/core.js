/* 마참뮤직 리모컨 v2 — 공용 런타임 (core.js)
 *
 * 번들러 없이 <script type="module">로 직접 로드한다. portal.js / console.js가 공유한다.
 * 원칙 3개:
 *   1. innerHTML을 쓰지 않는다. DOM은 전부 h()로 만든다. (XSS 차단)
 *   2. 서버 전체 재조회를 하지 않는다. WS 이벤트를 store에 머지하고, 바뀐 키만 다시 그린다.
 *   3. 화면에 안 보이면(document.hidden) 애니메이션·보간·마퀴를 멈춘다.
 */

/* ───────────────────────── 부트스트랩 컨텍스트 ─────────────────────────
 * 서버가 페이지 셸에 심어준 값. window.MACHAM = { guildId, csrf, buildId, user, tier, ... }
 */
export const ctx = Object.assign(
  { guildId: '', csrf: '', buildId: '', user: null, tier: 'member', permissions: null, intentStatus: null },
  (typeof window !== 'undefined' && window.MACHAM) || {},
);

const API_BASE = () => `/music/api/guilds/${ctx.guildId}`;

/* ───────────────────────── 작은 유틸 ───────────────────────── */

/** 사용자 입력을 화면에 넣기 전에 문자열로 정규화한다. 제어문자는 버린다. */
export function escapeText(value) {
  if (value === null || value === undefined) return '';
  return String(value).replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, '');
}

/** 초 → 2:14 / 1:02:03 */
export function fmtTime(seconds) {
  let total = Math.floor(Number(seconds) || 0);
  if (total < 0) total = 0;
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const hrs = Math.floor(total / 3600);
  const pad = (n) => String(n).padStart(2, '0');
  return hrs > 0 ? `${hrs}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/** UTC 문자열 → '방금' / 'N분 전' / '어제 20:14' / '3월 2일' */
export function fmtAgo(utc) {
  const ms = parseUtc(utc);
  if (!ms) return '';
  const diff = Date.now() - ms;
  if (diff < 45_000) return '방금';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}분 전`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}시간 전`;
  if (diff < 172_800_000) return `어제 ${fmtClock(utc)}`;
  const d = new Date(ms);
  const now = new Date();
  const day = `${d.getMonth() + 1}월 ${d.getDate()}일`;
  return d.getFullYear() === now.getFullYear() ? day : `${d.getFullYear()}년 ${day}`;
}

/** UTC 문자열 → 'HH:MM' (로컬) */
export function fmtClock(utc) {
  const ms = parseUtc(utc);
  if (!ms) return '';
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** 서버는 'Z' 없는 로컬-루킹 UTC를 줄 때가 있다. 둘 다 받아준다. */
export function parseUtc(utc) {
  if (!utc) return 0;
  let text = String(utc);
  if (!/[zZ]|[+-]\d{2}:?\d{2}$/.test(text)) text = text.replace(' ', 'T') + 'Z';
  const ms = Date.parse(text);
  return Number.isFinite(ms) ? ms : 0;
}

export function fmtDate(utc) {
  const ms = parseUtc(utc);
  if (!ms) return '';
  const d = new Date(ms);
  return `${d.getFullYear()}. ${d.getMonth() + 1}. ${d.getDate()}.`;
}

export function debounce(fn, wait) {
  let timer = 0;
  const wrapped = (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), wait);
  };
  wrapped.cancel = () => clearTimeout(timer);
  return wrapped;
}

export function prefersReduced() {
  return window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function clear(node) {
  while (node && node.firstChild) node.removeChild(node.firstChild);
  return node;
}

/* ───────────────────────── h() — 초경량 DOM 생성기 ─────────────────────────
 * h('div', { class: ['row', mine && 'is-mine'], onClick: fn, dataset: { id }, tip: '설명' }, child, ...)
 * - 문자열/숫자 자식은 텍스트 노드가 된다. null/false/undefined는 건너뛴다.
 * - html 같은 위험한 prop은 없다.
 */
export function h(tag, props, ...children) {
  const el = tag === 'svg' || tag === 'path' || tag === 'circle'
    ? document.createElementNS('http://www.w3.org/2000/svg', tag)
    : document.createElement(tag);

  if (props) {
    for (const [key, value] of Object.entries(props)) {
      if (value === null || value === undefined || value === false) continue;
      if (key === 'class' || key === 'className') {
        const cls = classNames(value);
        if (cls) el.setAttribute('class', cls);
      } else if (key === 'style' && typeof value === 'object') {
        for (const [k, v] of Object.entries(value)) {
          if (v === null || v === undefined) continue;
          if (k.startsWith('--')) el.style.setProperty(k, String(v));
          else el.style[k] = v;
        }
      } else if (key === 'dataset' && typeof value === 'object') {
        for (const [k, v] of Object.entries(value)) {
          if (v === null || v === undefined || v === false) continue;
          el.dataset[k] = String(v);
        }
      } else if (key === 'tip') {
        el.setAttribute('data-tip', escapeText(value));
      } else if (key === 'text') {
        el.textContent = escapeText(value);
      } else if (key === 'ref' && typeof value === 'function') {
        value(el);
      } else if (key.startsWith('on') && typeof value === 'function') {
        el.addEventListener(key.slice(2).toLowerCase(), value);
      } else if (key === 'value' || key === 'checked' || key === 'disabled' || key === 'hidden' || key === 'indeterminate') {
        el[key] = value === true ? true : value;
        // disabled/hidden은 속성으로도 남겨야 CSS가 잡는다
        if (value === true && (key === 'disabled' || key === 'hidden')) el.setAttribute(key, '');
      } else if (value === true) {
        el.setAttribute(key, '');
      } else {
        el.setAttribute(key, String(value));
      }
    }
  }
  appendAll(el, children);
  return el;
}

function classNames(value) {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.filter(Boolean).join(' ');
  if (value && typeof value === 'object') {
    return Object.entries(value).filter(([, on]) => on).map(([k]) => k).join(' ');
  }
  return '';
}

function appendAll(el, children) {
  for (const child of children) {
    if (child === null || child === undefined || child === false || child === true) continue;
    if (Array.isArray(child)) { appendAll(el, child); continue; }
    if (child instanceof Node) { el.appendChild(child); continue; }
    el.appendChild(document.createTextNode(escapeText(child)));
  }
}

export function frag(...children) {
  const f = document.createDocumentFragment();
  appendAll(f, children);
  return f;
}

/* ───────────────────────── store — 키 단위 구독 저장소 ─────────────────────────
 * 채팅이 바뀌었다고 대기열 렌더러가 돌면 안 된다. 그래서 구독은 키 목록으로 한다.
 * 한 틱 안의 여러 patch는 마이크로태스크로 묶어 한 번만 통지한다.
 */
const STATE = {
  conn: 'connecting',          // live | reconnecting | down | connecting
  buildId: ctx.buildId || '',
  guild: null,
  guilds: [],
  user: ctx.user || null,
  tier: ctx.tier || 'member',
  permissions: ctx.permissions || null,
  intentStatus: ctx.intentStatus || null,
  suspension: null,
  settings: null,
  player: null,
  current: null,
  queue: [],
  queueMode: 'score',
  sortedAt: null,
  presence: { listening: [], viewing: [], online: {} },
  members: [],
  chat: [],
  chatCursor: null,
  chatDelta: null,             // { type:'add'|'react'|'delete', id } — 노드 단위 갱신용
  unread: 0,
  playlists: [],
  liked: [],
  saved: [],
  recent: [],
  audit: [],
  suggestions: [],
  lyrics: null,
  search: null,
  coldAt: 0,
  hotAt: 0,
};

const SUBS = new Map();          // key → Set<fn>
let dirty = null;

export const store = {
  get() { return STATE; },

  /** 얕은 머지. 값이 실제로 바뀐 키만 통지한다. */
  patch(partial) {
    if (!partial) return;
    let changed = null;
    for (const [key, value] of Object.entries(partial)) {
      if (Object.is(STATE[key], value)) continue;
      STATE[key] = value;
      (changed || (changed = [])).push(key);
    }
    if (!changed) return;
    if (!dirty) {
      dirty = new Set();
      queueMicrotask(flush);
    }
    for (const key of changed) dirty.add(key);
  },

  /** subscribe('queue', fn) 또는 subscribe(['queue','player'], fn). 즉시 1회 실행하고 해지 함수를 준다. */
  subscribe(keys, fn) {
    const list = Array.isArray(keys) ? keys : [keys];
    for (const key of list) {
      if (!SUBS.has(key)) SUBS.set(key, new Set());
      SUBS.get(key).add(fn);
    }
    try { fn(STATE); } catch (error) { console.error('[store] 초기 렌더 실패', error); }
    return () => { for (const key of list) SUBS.get(key)?.delete(fn); };
  },
};

function flush() {
  const keys = dirty;
  dirty = null;
  if (!keys) return;
  const seen = new Set();
  for (const key of keys) {
    const subs = SUBS.get(key);
    if (!subs) continue;
    for (const fn of subs) {
      if (seen.has(fn)) continue;
      seen.add(fn);
      try { fn(STATE); } catch (error) { console.error('[store] 렌더 실패', key, error); }
    }
  }
}

/* ───────────────────────── api() — fetch 래퍼 ─────────────────────────
 * - CSRF 헤더 자동
 * - 4xx는 사용자용 한국어 메시지로 바꿔 throw (error.status / error.retryAfter 유지)
 * - 429는 재시도 안내
 */
const STATUS_TEXT = {
  400: '요청 형식이 잘못됐어요.',
  401: '로그인이 풀렸어요. 새로고침하고 다시 로그인해 주세요.',
  403: '권한이 없어요.',
  404: '대상을 찾을 수 없어요.',
  409: '그 사이에 상태가 바뀌었어요. 다시 해 보세요.',
  413: '내용이 너무 길어요.',
  422: '입력값을 확인해 주세요.',
  500: '서버가 처리하지 못했어요.',
  502: '봇에 연결하지 못했어요.',
  503: '봇이 잠깐 바빠요. 조금 뒤에 다시 해 주세요.',
};

export class ApiError extends Error {
  constructor(message, status, retryAfter) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.retryAfter = retryAfter || 0;
  }
}

/** 페이지 셸을 다시 받아 거기 박힌 CSRF 토큰만 꺼낸다. 실패하면 `null`. */
async function refetchCsrf() {
  try {
    const response = await fetch(location.pathname, {
      credentials: 'same-origin',
      headers: { Accept: 'text/html' },
      cache: 'no-store',
    });
    if (!response.ok) return null;
    const html = await response.text();
    // 셸은 `window.MACHAM = {...}` 한 줄로 심는다. JSON 만 떼어 읽는다.
    const match = html.match(/window\.MACHAM\s*=\s*(\{[\s\S]*?\});/);
    if (!match) return null;
    return JSON.parse(match[1])?.csrf || null;
  } catch {
    return null;
  }
}

export async function api(path, opts = {}) {
  const url = /^(https?:)?\/\//.test(path) || path.startsWith('/music') ? path : API_BASE() + path;
  const headers = Object.assign({ 'X-CSRF-Token': ctx.csrf, Accept: 'application/json' }, opts.headers);
  const init = { method: opts.method || 'GET', headers, credentials: 'same-origin', signal: opts.signal };

  if (opts.body !== undefined && opts.body !== null) {
    if (typeof opts.body === 'string' || opts.body instanceof FormData) init.body = opts.body;
    else {
      init.body = JSON.stringify(opts.body);
      if (!headers['Content-Type']) headers['Content-Type'] = 'application/json';
    }
    if (init.method === 'GET') init.method = 'POST';
  }

  let response;
  try {
    response = await fetch(url, init);
  } catch (error) {
    if (error && error.name === 'AbortError') throw error;
    throw new ApiError('서버에 닿지 못했다. 연결 상태를 확인해라.', 0);
  }

  let data = null;
  const type = response.headers.get('content-type') || '';
  if (type.includes('json')) { try { data = await response.json(); } catch { data = null; } }

  if (response.ok) return data;

  // CSRF 가 어긋나면 사람이 할 수 있는 게 없다. 페이지 셸에 박힌 토큰이 서버 것과
  // 다르다는 뜻이라 **몇 번을 눌러도 계속 실패한다.** 셸을 다시 받아 토큰만 갈아 끼우고
  // 한 번 다시 시도한다. 그래도 안 되면 그때 사람에게 말한다.
  if (response.status === 403 && /CSRF/i.test(String(data?.error || '')) && !opts._csrfRetried) {
    const fresh = await refetchCsrf();
    if (fresh && fresh !== ctx.csrf) {
      ctx.csrf = fresh;
      return api(path, Object.assign({}, opts, { _csrfRetried: true }));
    }
  }

  const retryAfter = Number(response.headers.get('retry-after')) || Number(data?.retryAfter) || 0;
  let message = (data && (data.error || data.message)) || STATUS_TEXT[response.status] || `요청 실패 (${response.status})`;
  if (response.status === 429) {
    message = retryAfter > 0 ? `${message} ${Math.ceil(retryAfter)}초 뒤에 다시 눌러 주세요.` : `${message}`;
  }
  throw new ApiError(message, response.status, retryAfter);
}

/* ───────────────────────── clock — 재생 위치 보간 ─────────────────────────
 * 진행바는 서버를 부르지 않는다. playback 이벤트의 positionSeconds + sampledAtUtc를
 * performance.now() 기준으로 이어 붙인다. document.hidden이면 틱을 멈춘다(값은 계속 정확).
 */
export const clock = {
  base: 0,
  baseAt: 0,
  duration: 0,
  paused: true,
  _ticks: new Set(),
  _raf: 0,
  _last: -1,

  /** 봇이 음성에 없어서 실제로는 아무것도 안 나오는 상태 (§36). */
  stopped: false,

  /** 서버 시계와 내 시계의 차이(초). `startedUtc` 를 쓰려면 이걸 먼저 빼야 한다.
   * 기기 시계가 몇 초씩 틀어져 있는 경우가 흔해서, 절대 시각을 그냥 믿으면 그만큼 어긋난다. */
  skew: 0,

  /** `startedUtc` 가 있을 때만 채워진다. 곡의 0초에 해당하는 **내 시계** 기준 epoch(ms). */
  startedAtLocal: 0,

  /** { positionSeconds, sampledAtUtc, isPaused, durationSeconds, startedUtc, stopped } */
  sync(payload) {
    if (!payload) return;
    if (payload.durationSeconds !== undefined && payload.durationSeconds !== null) {
      clock.duration = Number(payload.durationSeconds) || 0;
    }
    /* **봇이 음성에 없으면 아무것도 안 흐른다** (§36).
     * 전에는 `isPaused` 만 봤는데, 봇이 음성에서 빠져도 그 값은 false 라서
     * 진행바가 혼자 계속 갔다. 화면은 재생 중인데 실제로는 아무 소리도 안 나는,
     * 제일 헷갈리는 상태였다. 멈춘 건 멈춘 것으로 보여야 한다. */
    if (payload.stopped !== undefined) clock.stopped = !!payload.stopped;
    if (payload.isPaused !== undefined) clock.paused = !!payload.isPaused;
    if (clock.stopped) clock.paused = true;

    // 서버 표본 시각으로 시계 차이를 계속 다듬는다. 전송 지연도 여기 섞이지만,
    // 지연은 한 방향(서버→나)이라 몇십 ms 수준이고 곡 전체에서 일정하다.
    const sampled = parseUtc(payload.sampledAtUtc);
    if (sampled) clock.skew = Date.now() - sampled;

    /* **절대 시각이 오면 그것만 믿는다** (§31).
     * 예전에는 서버가 "지금 몇 초"를 보내고 각자 지연을 추정해 더했는데, 그 추정이
     * 기기마다 달라서 사람마다 소리가 어긋났다. 0초 지점의 UTC 를 주면 모두가
     * 같은 식으로 계산하므로 곡마다 생기던 미세한 차이가 사라진다. */
    const started = parseUtc(payload.startedUtc);
    if (started) {
      clock.startedAtLocal = started + clock.skew;
      if (!clock.paused) {
        clock.base = Math.max(0, (Date.now() - clock.startedAtLocal) / 1000);
        clock.baseAt = performance.now();
        clock._kick();
        return;
      }
    }

    if (payload.positionSeconds === undefined || payload.positionSeconds === null) { clock._kick(); return; }
    let position = Number(payload.positionSeconds) || 0;
    // 절대 시각이 없는 옛 서버용 폴백. 지연을 0~5초로 잘라 더한다.
    if (sampled) {
      const lag = (Date.now() - sampled) / 1000;
      if (lag > 0 && lag < 5) position += lag;
    }
    clock.base = Math.max(0, position);
    clock.baseAt = performance.now();
    clock._kick();
  },

  /** 곡의 0초에 해당하는 내 시계 시각. 웹 재생이 절대 기준으로 쓴다. 없으면 0. */
  startedAt() {
    return clock.startedAtLocal;
  },

  seekLocal(seconds) {
    clock.base = Math.max(0, Number(seconds) || 0);
    clock.baseAt = performance.now();
    clock._emit();
  },

  position() {
    if (clock.paused) return clock.base;
    const position = clock.base + (performance.now() - clock.baseAt) / 1000;
    return clock.duration > 0 ? Math.min(position, clock.duration) : position;
  },

  progress() {
    return clock.duration > 0 ? Math.min(1, clock.position() / clock.duration) : 0;
  },

  /** 0.25초 이상 값이 변할 때만 호출된다. */
  onTick(fn) {
    clock._ticks.add(fn);
    clock._kick();
    return () => clock._ticks.delete(fn);
  },

  _kick() {
    clock._emit();
    if (clock._raf || document.hidden || clock.paused || !clock._ticks.size) return;
    const loop = () => {
      clock._raf = 0;
      if (document.hidden || clock.paused) { clock._emit(); return; }
      clock._emit();
      clock._raf = requestAnimationFrame(loop);
    };
    clock._raf = requestAnimationFrame(loop);
  },

  _emit() {
    const position = clock.position();
    if (Math.abs(position - clock._last) < 0.2) return;
    clock._last = position;
    for (const fn of clock._ticks) {
      try { fn(position, clock.duration); } catch (error) { console.error('[clock]', error); }
    }
  },
};

document.addEventListener('visibilitychange', () => {
  if (!document.hidden) { clock._last = -1; clock._kick(); }
});

/* ───────────────────────── list() — 키 기반 재조정 + FLIP ─────────────────────────
 * 대기열 순서가 바뀌면 항목이 미끄러져 이동해야 한다.
 * 사라지는 노드는 즉시 제거하고, 남은 노드가 그 자리를 부드럽게 메운다.
 */
const FLIP_EASE = 'cubic-bezier(0.32, 0.72, 0, 1)';

export function list(container, items, keyFn, createFn, updateFn) {
  if (!container) return;
  const prev = container.__mmList instanceof Map ? container.__mmList : new Map();
  const motion = !prefersReduced() && !document.hidden && prev.size > 0;

  // 관리 대상이 아닌 자식(빈 상태 안내 등)은 먼저 치운다
  for (const child of Array.from(container.children)) {
    if (!child.__mmKey) child.remove();
  }

  const before = new Map();
  if (motion) for (const [key, node] of prev) before.set(key, node.getBoundingClientRect());

  const next = new Map();
  const ordered = [];
  items.forEach((item, index) => {
    const key = String(keyFn(item, index));
    let node = prev.get(key);
    const isNew = !node;
    if (isNew) {
      node = createFn(item, index);
      node.__mmKey = key;
    }
    if (updateFn) updateFn(node, item, index);
    node.__mmNew = isNew;
    next.set(key, node);
    ordered.push(node);
  });

  for (const [key, node] of prev) if (!next.has(key)) node.remove();

  let cursor = container.firstChild;
  for (const node of ordered) {
    if (cursor === node) { cursor = cursor.nextSibling; continue; }
    container.insertBefore(node, cursor);
  }

  if (motion) {
    for (const node of ordered) {
      if (node.__mmNew) {
        node.animate(
          [{ opacity: 0, transform: 'translateY(-8px)' }, { opacity: 1, transform: 'none' }],
          { duration: 180, easing: FLIP_EASE },
        );
        continue;
      }
      const from = before.get(node.__mmKey);
      if (!from) continue;
      const to = node.getBoundingClientRect();
      const dx = from.left - to.left;
      const dy = from.top - to.top;
      if (Math.abs(dx) < 1 && Math.abs(dy) < 1) continue;
      node.animate(
        [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }],
        { duration: 240, easing: FLIP_EASE },
      );
    }
  }

  container.__mmList = next;
}

/** 컨테이너를 비우고 리스트 상태도 초기화한다(빈 상태 화면을 넣기 직전에 쓴다). */
list.reset = (container) => {
  if (!container) return;
  clear(container);
  container.__mmList = new Map();
};

/* ───────────────────────── tooltip() — [data-tip] 커스텀 툴팁 ─────────────────────────
 * 네이티브 title=은 지연이 길고 모바일에서 안 뜬다. 호버 350ms, 롱프레스 400ms.
 */
let tipEl = null;
let tipTimer = 0;
let tipTarget = null;
let tipInstalled = false;

export function tooltip() {
  if (tipInstalled) return;
  tipInstalled = true;

  document.addEventListener('pointerover', (event) => {
    if (event.pointerType === 'touch') return;
    const target = event.target.closest?.('[data-tip]');
    if (!target || target === tipTarget) return;
    scheduleTip(target, 350);
  });
  document.addEventListener('pointerout', (event) => {
    if (event.pointerType === 'touch') return;
    const target = event.target.closest?.('[data-tip]');
    if (target && target === tipTarget && !target.contains(event.relatedTarget)) hideTip();
    else if (target) clearTimeout(tipTimer);
  });

  // 터치: 롱프레스
  document.addEventListener('pointerdown', (event) => {
    if (event.pointerType !== 'touch') { hideTip(); return; }
    const target = event.target.closest?.('[data-tip]');
    if (target) scheduleTip(target, 400);
  }, { passive: true });
  const cancelTouch = () => { clearTimeout(tipTimer); if (tipTarget) setTimeout(hideTip, 1200); };
  document.addEventListener('pointerup', cancelTouch, { passive: true });
  document.addEventListener('pointercancel', cancelTouch, { passive: true });

  // 키보드 접근 — 포커스면 지연 없이 보여준다
  document.addEventListener('focusin', (event) => {
    const target = event.target.closest?.('[data-tip]');
    if (target) scheduleTip(target, 0);
  });
  document.addEventListener('focusout', hideTip);

  document.addEventListener('keydown', (event) => { if (event.key === 'Escape') hideTip(); });
  window.addEventListener('scroll', hideTip, true);
  window.addEventListener('resize', hideTip);
  window.addEventListener('blur', hideTip);
}

function scheduleTip(target, delay) {
  clearTimeout(tipTimer);
  tipTimer = setTimeout(() => showTip(target), delay);
}

function showTip(target) {
  const text = target.getAttribute('data-tip');
  if (!text || !target.isConnected) return;
  if (!tipEl) {
    tipEl = h('div', { class: 'tip', role: 'tooltip' });
    document.body.appendChild(tipEl);
  }
  tipTarget = target;
  clear(tipEl);
  tipEl.appendChild(document.createTextNode(escapeText(text)));
  const key = target.getAttribute('data-tip-key');
  if (key) tipEl.appendChild(h('span', { class: 'tip__key' }, key));

  tipEl.style.left = '0px';
  tipEl.style.top = '0px';
  tipEl.dataset.show = '1';

  const anchor = target.getBoundingClientRect();
  const box = tipEl.getBoundingClientRect();
  const gap = 8;
  let left = anchor.left + anchor.width / 2 - box.width / 2;
  left = Math.max(gap, Math.min(left, window.innerWidth - box.width - gap));
  let top = anchor.top - box.height - gap;
  if (top < gap) top = Math.min(anchor.bottom + gap, window.innerHeight - box.height - gap);
  tipEl.style.left = `${Math.round(left)}px`;
  tipEl.style.top = `${Math.round(top)}px`;
}

function hideTip() {
  clearTimeout(tipTimer);
  tipTarget = null;
  if (tipEl) tipEl.dataset.show = '0';
}

/* ───────────────────────── marquee() — 넘치는 텍스트 전광판 ─────────────────────────
 * 구조: <span class="mq"><span class="mq__i">긴 제목</span></span>
 * 행(data-mq-row) 어디에 호버해도 그 행의 마퀴가 흐른다.
 */
export function mqText(text, className) {
  return h('span', { class: className ? `mq ${className}` : 'mq' },
    h('span', { class: 'mq__i' }, escapeText(text)));
}

export function marquee(el) {
  if (!el || !el.isConnected) return;
  const inner = el.firstElementChild;
  if (!inner) return;
  const overflow = inner.scrollWidth - el.clientWidth;
  if (overflow > 4) {
    el.dataset.over = '1';
    el.style.setProperty('--marquee-shift', `${-Math.ceil(overflow + 8)}px`);
    // 초당 약 42px. 짧은 넘침도 너무 빨라 보이지 않게 하한 6초.
    el.style.setProperty('--marquee-dur', `${Math.max(6, Math.round((overflow + 8) / 42 * 2))}s`);
  } else {
    el.dataset.over = '0';
    el.style.removeProperty('--marquee-shift');
  }
}

/** 루트 안의 모든 .mq를 다시 잰다. 렌더 직후에 호출한다. */
marquee.scan = (root) => {
  if (document.hidden) return;
  const scope = root || document;
  for (const el of scope.querySelectorAll('.mq')) marquee(el);
};

let mqInstalled = false;
export function marqueeRows() {
  if (mqInstalled) return;
  mqInstalled = true;

  const setRun = (row, on) => {
    if (!row) return;
    for (const el of row.querySelectorAll('.mq')) {
      if (on) { marquee(el); el.dataset.run = '1'; } else el.removeAttribute('data-run');
    }
  };
  document.addEventListener('pointerover', (event) => {
    const row = event.target.closest?.('[data-mq-row]');
    if (row && !row.contains(event.relatedTarget)) setRun(row, true);
  });
  document.addEventListener('pointerout', (event) => {
    const row = event.target.closest?.('[data-mq-row]');
    if (row && !row.contains(event.relatedTarget)) setRun(row, false);
  });
  document.addEventListener('focusin', (event) => {
    const row = event.target.closest?.('[data-mq-row]');
    if (row) setRun(row, true);
  });
  document.addEventListener('focusout', (event) => {
    const row = event.target.closest?.('[data-mq-row]');
    if (row && !row.contains(event.relatedTarget)) setRun(row, false);
  });

  let resizeTimer = 0;
  window.addEventListener('resize', () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => marquee.scan(), 160);
  });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) marquee.scan(); });
}

/* ───────────────────────── toast / sheet ───────────────────────── */

let toastHost = null;

export function toast(message, kind = 'info') {
  if (!toastHost) {
    toastHost = h('div', { class: 'toasts', role: 'status', 'aria-live': 'polite' });
    document.body.appendChild(toastHost);
  }
  const icon = { ok: '✓', warn: '!', danger: '✕', info: '·' }[kind] || '·';
  const node = h('div', { class: `toast toast--${kind}` },
    h('span', { class: 'toast__icon', 'aria-hidden': 'true' }, icon),
    h('span', { class: 'toast__msg' }, escapeText(message)));

  const dismiss = () => {
    if (node.dataset.out) return;
    node.dataset.out = '1';
    setTimeout(() => node.remove(), 220);
  };
  node.addEventListener('click', dismiss);
  toastHost.appendChild(node);
  while (toastHost.children.length > 4) toastHost.firstElementChild.remove();
  setTimeout(dismiss, kind === 'danger' ? 5200 : 3400);
  return dismiss;
}

/**
 * 모달 시트. body는 노드, actions는 [{ label, kind, value, autofocus }].
 * 반환: { close, result } — result는 사용자가 고른 value로 resolve되는 Promise.
 */
export function sheet({ title, desc, body, actions = [], danger = false, wide = false, dismissValue = null }) {
  const previous = document.activeElement;
  let settle;
  const result = new Promise((resolve) => { settle = resolve; });

  const close = (value) => {
    if (!back.isConnected) return;
    back.dataset.out = '1';
    setTimeout(() => back.remove(), 180);
    document.removeEventListener('keydown', onKey, true);
    if (previous && previous.isConnected) previous.focus();
    settle(value);
  };

  const buttons = actions.map((action) => h('button', {
    type: 'button',
    class: ['btn', action.kind === 'primary' && 'btn--primary', action.kind === 'danger' && 'btn--danger',
      action.kind === 'ghost' && 'btn--ghost'],
    onClick: () => close(action.value !== undefined ? action.value : true),
    ref: (el) => { if (action.autofocus) setTimeout(() => el.focus(), 20); },
  }, action.label));

  const card = h('div', {
    class: ['sheet', wide && 'sheet--wide', danger && 'sheet--danger'],
    role: 'dialog',
    'aria-modal': 'true',
    'aria-label': escapeText(title || '대화상자'),
  },
    title ? h('h2', { class: 'sheet__title' }, title) : null,
    desc ? h('p', { class: 'sheet__desc' }, desc) : null,
    body ? h('div', { class: 'sheet__body' }, body) : null,
    actions.length ? h('div', { class: 'sheet__acts' }, buttons) : null,
    h('button', { type: 'button', class: 'sheet__x', 'aria-label': '닫기', tip: '닫기', onClick: () => close(dismissValue) }, '✕'),
  );

  const back = h('div', { class: 'sheet-back', onPointerdown: (e) => { if (e.target === back) close(dismissValue); } }, card);

  const onKey = (event) => {
    if (event.key === 'Escape') { event.stopPropagation(); close(dismissValue); return; }
    if (event.key !== 'Tab') return;
    const focusables = card.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])');
    if (!focusables.length) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  };

  document.body.appendChild(back);
  document.addEventListener('keydown', onKey, true);
  if (!actions.some((a) => a.autofocus)) setTimeout(() => card.querySelector('button, input, textarea')?.focus(), 20);

  return { close, result, card };
}

/** window.confirm 대체. true/false로 resolve. */
export function confirmSheet({ title, desc, danger = false, confirmText = '확인', cancelText = '취소' }) {
  return sheet({
    title, desc, danger, dismissValue: false,
    actions: [
      { label: cancelText, kind: 'ghost', value: false },
      { label: confirmText, kind: danger ? 'danger' : 'primary', value: true, autofocus: true },
    ],
  }).result;
}

/* ───────────────────────── theme ───────────────────────── */

const THEME_KEY = 'macham.theme';

export const theme = {
  init(fallback = 'dark') {
    let saved = null;
    try { saved = localStorage.getItem(THEME_KEY); } catch { /* 시크릿 모드 */ }
    const mode = saved === 'light' || saved === 'dark' ? saved : fallback;
    theme.apply(mode);
    return mode;
  },
  current() {
    return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
  },
  apply(mode) {
    document.documentElement.dataset.theme = mode;
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', mode === 'light' ? '#f4f6fa' : '#07090f');
  },
  toggle() {
    const next = theme.current() === 'dark' ? 'light' : 'dark';
    theme.apply(next);
    try { localStorage.setItem(THEME_KEY, next); } catch { /* 무시 */ }
    return next;
  },
};

/* ───────────────────────── notify — 브라우저 알림 + 탭 제목 ───────────────────────── */

const BASE_TITLE = document.title || '마참뮤직';

export const notify = {
  granted() {
    return typeof Notification !== 'undefined' && Notification.permission === 'granted';
  },
  supported() {
    return typeof Notification !== 'undefined';
  },
  async ask() {
    if (!notify.supported()) return 'unsupported';
    if (Notification.permission !== 'default') return Notification.permission;
    try { return await Notification.requestPermission(); } catch { return 'denied'; }
  },
  /** 백그라운드일 때만 띄운다. 화면을 보고 있으면 알림이 방해다. */
  push({ title, body, icon, tag, onClick }) {
    if (!document.hidden || !notify.granted()) return null;
    try {
      const item = new Notification(title, { body, icon, tag, silent: false });
      item.onclick = () => { window.focus(); item.close(); onClick?.(); };
      setTimeout(() => item.close(), 8000);
      return item;
    } catch { return null; }
  },
  /** 탭 제목에 미읽음 개수 — (3) 마참뮤직 */
  badge(count) {
    const n = Number(count) || 0;
    document.title = n > 0 ? `(${n > 99 ? '99+' : n}) ${BASE_TITLE}` : BASE_TITLE;
  },
};

/* ───────────────────────── artColor — 앨범아트 대표색 ─────────────────────────
 * canvas로 축소해 샘플링하고 --art-1 / --art-2 / --art-wash 를 갈아끼운다.
 * CORS 등으로 실패하면 조용히 토큰 기본값(보라)으로 되돌린다.
 */
const artCache = new Map();

export function artColor(imgUrl) {
  if (!imgUrl) { resetArt(); return Promise.resolve(null); }
  if (artCache.has(imgUrl)) { applyArt(artCache.get(imgUrl)); return Promise.resolve(artCache.get(imgUrl)); }

  return new Promise((resolve) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    img.onload = () => {
      try {
        const size = 24;
        const canvas = document.createElement('canvas');
        canvas.width = size; canvas.height = size;
        const g = canvas.getContext('2d', { willReadFrequently: true });
        g.drawImage(img, 0, 0, size, size);
        const { data } = g.getImageData(0, 0, size, size);
        const picked = pickColors(data);
        if (!picked) { resetArt(); resolve(null); return; }
        artCache.set(imgUrl, picked);
        if (artCache.size > 40) artCache.delete(artCache.keys().next().value);
        applyArt(picked);
        resolve(picked);
      } catch { resetArt(); resolve(null); }
    };
    img.onerror = () => { resetArt(); resolve(null); };
    img.src = imgUrl;
  });
}

function pickColors(data) {
  const bins = new Map();
  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 200) continue;
    const r = data[i], g = data[i + 1], b = data[i + 2];
    const max = Math.max(r, g, b), min = Math.min(r, g, b);
    const light = (max + min) / 2;
    if (light < 26 || light > 240) continue;          // 거의 검정·흰색은 대표색이 아니다
    const sat = max === min ? 0 : (max - min) / (255 - Math.abs(max + min - 255));
    const key = `${r >> 4}_${g >> 4}_${b >> 4}`;
    const weight = 1 + sat * 3;                        // 채도 높은 픽셀에 가중치
    const bin = bins.get(key) || { r: 0, g: 0, b: 0, n: 0, w: 0 };
    bin.r += r; bin.g += g; bin.b += b; bin.n += 1; bin.w += weight;
    bins.set(key, bin);
  }
  if (!bins.size) return null;
  const sorted = [...bins.values()].sort((a, b) => b.w - a.w);
  const primary = normalize(sorted[0]);
  const secondary = sorted.find((bin) => hueGap(normalize(bin), primary) > 24) || sorted[Math.min(1, sorted.length - 1)];
  return { a: tune(primary, 0), b: tune(normalize(secondary), 12) };
}

function normalize(bin) {
  return [Math.round(bin.r / bin.n), Math.round(bin.g / bin.n), Math.round(bin.b / bin.n)];
}

function hueGap(x, y) {
  return Math.abs(rgbToHsl(x)[0] - rgbToHsl(y)[0]);
}

/** 배경 위에서 읽히도록 채도·명도를 살짝 밀어 올린다. */
function tune(rgb, hueShift) {
  const [hue, sat, light] = rgbToHsl(rgb);
  const h2 = (hue + hueShift + 360) % 360;
  const s2 = Math.min(0.92, Math.max(0.42, sat * 1.25));
  const l2 = Math.min(0.72, Math.max(0.5, light * 0.6 + 0.3));
  return hslToCss(h2, s2, l2);
}

function rgbToHsl([r, g, b]) {
  const rn = r / 255, gn = g / 255, bn = b / 255;
  const max = Math.max(rn, gn, bn), min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let hue;
  if (max === rn) hue = ((gn - bn) / d + (gn < bn ? 6 : 0));
  else if (max === gn) hue = (bn - rn) / d + 2;
  else hue = (rn - gn) / d + 4;
  return [hue * 60, s, l];
}

function hslToCss(hue, sat, light) {
  return `hsl(${Math.round(hue)} ${Math.round(sat * 100)}% ${Math.round(light * 100)}%)`;
}

function applyArt(picked) {
  const root = document.documentElement.style;
  root.setProperty('--art-1', picked.a);
  root.setProperty('--art-2', picked.b);
  root.setProperty('--art-wash', picked.a.replace('hsl(', 'hsl(').replace(')', ' / 12%)'));
}

function resetArt() {
  const root = document.documentElement.style;
  root.removeProperty('--art-1');
  root.removeProperty('--art-2');
  root.removeProperty('--art-wash');
}

/* ───────────────────────── connect() — WebSocket ─────────────────────────
 * {t, d} 이벤트를 store에 머지한다. 전체 재조회는 재연결 때만.
 */
const RECONNECT_MIN = 1000;
const RECONNECT_MAX = 15000;
const CHAT_CAP = 400;

export function connect(guildId, handlers = {}) {
  // connect(guildId, handlers) 와 connect({ guildId, ...handlers }) 둘 다 받는다.
  // 후자로 부르면 guildId 자리에 객체가 들어와 URL이 [object Object]가 된다.
  if (guildId && typeof guildId === 'object') {
    handlers = guildId;
    guildId = handlers.guildId;
  }
  const id = guildId || ctx.guildId;
  let socket = null;
  let attempt = 0;
  let timer = 0;
  let everConnected = false;
  let closed = false;
  let downSince = 0;

  const open = () => {
    if (closed) return;
    clearTimeout(timer);
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    try {
      socket = new WebSocket(`${proto}://${location.host}/music/api/guilds/${id}/events`);
    } catch {
      schedule();
      return;
    }

    socket.onopen = () => {
      attempt = 0;
      downSince = 0;
      store.patch({ conn: 'live' });
      if (everConnected) handlers.onResync?.();
      everConnected = true;
    };

    socket.onmessage = (event) => {
      let payload;
      try { payload = JSON.parse(event.data); } catch { return; }
      if (!payload || typeof payload !== 'object') return;
      // 구버전 서버는 문자열 토픽만 보낸다 — 그때만 재조회로 폴백한다
      if (typeof payload.t !== 'string') return;
      merge(payload.t, payload.d, handlers);
    };

    socket.onerror = () => { /* onclose에서 처리 */ };
    socket.onclose = (event) => {
      if (closed) return;
      // 4403/1008 = 권한·인증 거부. 재시도해도 소용없다.
      if (event.code === 1008 || event.code === 4403) {
        store.patch({ conn: 'down' });
        handlers.onDenied?.(event.reason || '');
        return;
      }
      if (!downSince) downSince = Date.now();
      // 재시작 예고를 받은 직후의 끊김은 사고가 아니다. `down` 으로 바꿔서 빨간 배너를
      // 띄우면 "고장났다" 로 읽힌다 — 돌아올 때까지 안내를 유지한다.
      const expected = store.get().conn === 'restarting' && Date.now() - downSince < 60000;
      if (!expected) {
        store.patch({ conn: Date.now() - downSince > 30000 ? 'down' : 'reconnecting' });
      }
      schedule();
    };
  };

  const schedule = () => {
    if (closed) return;
    attempt += 1;
    const backoff = Math.min(RECONNECT_MAX, RECONNECT_MIN * Math.pow(1.7, attempt - 1));
    const jitter = backoff * (0.8 + Math.random() * 0.4);
    clearTimeout(timer);
    timer = setTimeout(open, jitter);
  };

  // 탭으로 돌아왔는데 끊겨 있으면 백오프를 기다리지 않는다
  document.addEventListener('visibilitychange', () => {
    if (document.hidden || closed) return;
    if (!socket || socket.readyState > WebSocket.OPEN) { attempt = 0; open(); }
  });
  window.addEventListener('online', () => { if (!closed && (!socket || socket.readyState > WebSocket.OPEN)) { attempt = 0; open(); } });

  open();

  return {
    close() { closed = true; clearTimeout(timer); socket?.close(); },
    get state() { return socket ? socket.readyState : -1; },
  };
}

/** WS 이벤트 → store 머지. 여기서 전체 재조회를 부르지 않는 것이 성능 계약의 핵심이다. */
function merge(type, data, handlers) {
  const state = store.get();
  switch (type) {
    case 'playback': {
      const player = Object.assign({}, state.player, {
        isPaused: data.isPaused,
        currentId: data.currentId,
        effectiveVolume: data.effectiveVolume ?? state.player?.effectiveVolume,
        repeatMode: data.repeatMode ?? state.player?.repeatMode,
        shuffleEnabled: data.shuffleEnabled ?? state.player?.shuffleEnabled,
        voiceChannelId: data.voiceChannelId ?? state.player?.voiceChannelId,
        botOnline: data.botOnline ?? state.player?.botOnline,
        // 서버는 `playback_payload` 에 이 값을 담아 보내는데 여기서 안 읽고 있었다.
        // 그래서 자동 재생을 켜고 꺼도 화면의 📻/🚫 가 영영 그대로였다 —
        // 다른 이유로 전체 재조회가 일어날 때만 우연히 맞았다.
        autoplayEnabled: data.autoplayEnabled ?? state.player?.autoplayEnabled,
      });
      const patch = { player };
      if (data.current !== undefined) patch.current = data.current;
      // 서버가 정한 일정 (§31). 웹 재생과 진행바가 둘 다 이걸 기준으로 움직인다.
      patch.schedule = {
        startedUtc: data.startedUtc ?? null,
        nextStartUtc: data.nextStartUtc ?? null,
        skipLeadMs: Number(data.skipLeadMs) || 0,
        seekLockoutMs: Number(data.seekLockoutMs) || 0,
        webSyncOffsetMs: Number(data.webSyncOffsetMs) || 0,
      };
      store.patch(patch);
      clock.sync({
        positionSeconds: data.positionSeconds,
        sampledAtUtc: data.sampledAtUtc,
        startedUtc: data.startedUtc,
        isPaused: data.isPaused,
        // 봇이 음성에 없으면 실제로는 아무것도 안 나온다 (§36).
        stopped: data.voiceConnected === false || !data.current,
        durationSeconds: data.durationSeconds ?? (data.current ? data.current.durationSeconds : undefined),
      });
      break;
    }
    case 'server.restarting': {
      // 업데이트로 곧 끊긴다는 예고. **이걸 안 받으면 사람은 오류 화면만 본다.**
      // 소켓은 곧 닫히고 기존 백오프 재연결이 알아서 붙는다 — 여기서는 알리기만 한다.
      store.patch({
        conn: 'restarting',
        restartNote: data?.message || '업데이트 중이에요. 곧 다시 연결돼요.',
      });
      break;
    }
    case 'queue.set': {
      // 이 프레임은 모든 사람이 같이 받으므로 서버가 개인화 필드(isMine·myVote)를 비워서 보낸다.
      // 그대로 덮어쓰면 5초마다 "내 곡" 표시와 내 투표가 사라지고, 자기 곡에 투표 버튼이
      // 열려 서버가 403 을 주는 상태가 된다. 그래서 **id 기준으로 병합**한다.
      //
      // 또 서버는 앞 200곡만 보낸다(§18.2). 통째로 갈아치우면 더 불러온 뒤쪽 페이지가 날아가므로,
      // 잘린 프레임이면 앞부분만 교체하고 뒤는 남긴다.
      const incoming = Array.isArray(data.items) ? data.items : [];
      const mine = new Map();
      for (const item of state.queue || []) mine.set(item.id, item);
      const merged = incoming.map((item) => {
        const previous = mine.get(item.id);
        if (!previous) return item;
        return Object.assign({}, item, {
          // null 은 "서버가 안 보냈다"는 뜻이라 예전 값을 지키고,
          // false/문자열처럼 실제 값이 오면 그건 서버 판단이라 따른다.
          isMine: item.isMine ?? previous.isMine,
          myVote: item.myVote === undefined || item.myVote === null ? previous.myVote : item.myVote,
        });
      });
      const tail = data.truncated && (state.queue || []).length > merged.length
        ? state.queue.slice(merged.length).filter((item) => !incoming.some((next) => next.id === item.id))
        : [];
      store.patch({
        queue: merged.concat(tail),
        queueMode: data.mode || state.queueMode,
        sortedAt: data.sortedAt || null,
        nextSortAt: data.nextSortAt || state.nextSortAt || null,
        sortPeriodSeconds: data.sortPeriodSeconds || state.sortPeriodSeconds || null,
        queueTotal: data.total ?? data.queueTotal ?? state.queueTotal,
      });
      break;
    }
    case 'vote': {
      const queue = state.queue.map((item) => (item.id === data.itemId
        ? Object.assign({}, item, {
          score: Object.assign({}, item.score, {
            likeCount: data.like ?? item.score?.likeCount,
            superLikeCount: data.super ?? item.score?.superLikeCount,
            totalScore: data.total ?? item.score?.totalScore,
          }),
          myVote: data.myVote !== undefined ? data.myVote : item.myVote,
        })
        : item));
      store.patch({ queue });
      break;
    }
    case 'chat.add': {
      if (!data || data.id === undefined) break;
      if (state.chat.some((m) => m.id === data.id)) break;
      const chat = state.chat.concat(data);
      store.patch({ chat: chat.length > CHAT_CAP ? chat.slice(chat.length - CHAT_CAP) : chat, chatDelta: { type: 'add', id: data.id } });
      handlers.onChat?.(data);
      break;
    }
    case 'chat.react': {
      const chat = state.chat.map((message) => (message.id === data.messageId
        ? Object.assign({}, message, { reactions: applyReaction(message.reactions, data) })
        : message));
      store.patch({ chat, chatDelta: { type: 'react', id: data.messageId } });
      break;
    }
    case 'chat.delete': {
      const chat = state.chat.map((message) => (message.id === data.messageId
        ? Object.assign({}, message, { deletedUtc: data.deletedUtc || new Date().toISOString(), content: '' })
        : message));
      store.patch({ chat, chatDelta: { type: 'delete', id: data.messageId } });
      break;
    }
    case 'presence':
      store.patch({
        presence: {
          listening: data.listening || [],
          viewing: data.viewing || [],
          online: data.online || {},
        },
      });
      break;
    case 'members':
      store.patch({ members: Array.isArray(data.members) ? data.members : state.members });
      break;
    case 'settings':
      handlers.onRefetch?.('settings');
      break;
    case 'library':
      handlers.onRefetch?.('library');
      break;
    case 'suspension':
      store.patch({ suspension: data && data.scope ? data : null });
      handlers.onRefetch?.('permissions');
      break;
    case 'suggestion.add':
    case 'suggestion.vote':
    case 'suggestion.status':
      handlers.onRefetch?.('suggestions');
      break;
    case 'audit':
      // 서버가 새 항목 하나를 실어 보낸다(§13.5). 그걸 버리고 전체를 다시 불러오면
      // 투표·담기까지 기록 대상이 된 지금은 로그 탭이 열려 있는 내내 재조회가 쏟아진다.
      if (data && data.entry) handlers.onEvent?.('audit', data);
      else handlers.onRefetch?.('audit');
      break;
    case 'lyrics':
      store.patch({ lyrics: data || null });
      break;
    case 'notice':
      if (data?.message) toast(data.message, data.kind || 'info');
      break;
    default:
      handlers.onEvent?.(type, data);
  }
  handlers.onAny?.(type, data);
}

function applyReaction(reactions, data) {
  const next = Array.isArray(reactions) ? reactions.map((r) => Object.assign({}, r)) : [];
  const mine = data.userId && String(data.userId) === String(ctx.user?.id);
  let entry = next.find((r) => r.emoji === data.emoji);
  if (data.added) {
    if (!entry) { entry = { emoji: data.emoji, count: 0, reactedByMe: false, users: [] }; next.push(entry); }
    entry.count += 1;
    if (mine) entry.reactedByMe = true;
    if (data.displayName) entry.users = (entry.users || []).concat({ userId: data.userId, displayName: data.displayName });
  } else if (entry) {
    entry.count = Math.max(0, entry.count - 1);
    if (mine) entry.reactedByMe = false;
    if (entry.users) entry.users = entry.users.filter((u) => String(u.userId) !== String(data.userId));
  }
  return next.filter((r) => r.count > 0);
}
