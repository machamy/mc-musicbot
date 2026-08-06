/* 마참뮤직 리모컨 v2 — 유저 UI (portal.js)
 *
 * 서버는 빈 셸(#app)과 window.MACHAM만 준다. 화면은 전부 여기서 그린다.
 * 진입: /state/cold 1회 + /state/hot 1회 → 이후는 WebSocket 이벤트만으로 갱신한다.
 * innerHTML은 한 번도 쓰지 않는다. 모든 노드는 core.js의 h()로 만든다.
 */

import {
  ctx, store, connect, api, ApiError, clock, h, frag, list, tooltip, marquee, marqueeRows, mqText,
  toast, sheet, confirmSheet, theme, notify, artColor, fmtTime, fmtAgo, fmtClock, fmtDate,
  parseUtc, escapeText, clear, debounce, prefersReduced,
} from './core.js';

/* ═══════════════════════ 상수 ═══════════════════════ */

const TIERS = {
  owner: { icon: '👑', label: '봇 주인', desc: '모든 서버의 모든 기능을 쓸 수 있어요.' },
  manager: { icon: '🛡', label: '서버 관리자', desc: '이 서버에서 관리 권한이 있어요.' },
  member: { icon: '🎵', label: '일반 멤버', desc: '서버 관리자가 정한 규칙 안에서 조작할 수 있어요.' },
  viewer: { icon: '👀', label: '읽기 전용', desc: '지금은 보기만 할 수 있어요.' },
};

const MODES = {
  score: {
    icon: '⭐', label: '점수제',
    desc: '좋아요를 많이 받은 곡이 먼저 나가요. 기다린 시간도 점수로 쌓여요.',
    formula: '관리자 우선 → (대기 + 👍 + ⭐×2) 높은 순 → 신청 순',
  },
  fifo: {
    icon: '⏱', label: '시간제',
    desc: '먼저 신청한 곡이 먼저 나가요. 좋아요는 표시만 되고 순서를 바꾸지 않아요.',
    formula: '관리자 우선 → 신청 순',
  },
  fair: {
    icon: '⚖', label: '공평제',
    desc: '사람별로 돌아가며 한 곡씩 재생해요. 미리 여러 곡을 넣어도 새치기가 안 되고, 늦게 온 사람도 금방 차례가 와요.',
    formula: '관리자 우선 → 라운드 → 마지막으로 재생된 지 오래된 사람 순',
  },
};

const PERM_LABELS = {
  search: '곡 검색·신청',
  vote: '좋아요·슈퍼 좋아요',
  playback: '재생 / 일시정지 / 스킵',
  seek: '재생 위치 이동',
  volume: '볼륨 조절',
  queueEdit: '대기열 편집',
  chat: '채팅 쓰기·반응·답장',
  library: '보관함·재생목록',
  suggest: '제안 작성·공감',
  autoplaySeed: '자동 재생 기준 곡 편집',
  chatDelete: '남의 채팅 삭제',
  suggestStatus: '제안 상태 변경',
  suspend: '유저 정지·해제',
  sortMode: '정렬 모드 변경',
  console: '서버 관리 콘솔',
  ops: '운영 패널',
};

const RULE_LABELS = {
  guildMember: '모든 멤버',
  sameVoiceChannel: '같은 음성채널',
  configuredRole: '지정 역할',
  administrator: '관리자',
  disabled: '사용 안 함',
  manager: '관리자',
  owner: '봇 주인 전용',
};

const SCOPE_LABELS = { all: '전체', chat: '채팅만', queue: '곡 신청만' };

const QUICK_EMOJI = ['👍', '❤️', '🔥', '😂', '😮', '🎵', '👏', '🙏', '✨', '😭', '🤔', '💜'];

const SIDE_TABS = [
  { id: 'chat', icon: '💬', label: '채팅' },
  { id: 'members', icon: '👥', label: '멤버' },
  { id: 'suggest', icon: '💡', label: '제안' },
  { id: 'recent', icon: '🕘', label: '최근' },
  { id: 'audit', icon: '📜', label: '로그' },
];

const LS = {
  lyrics: 'macham.lyrics.open',
  sideTab: 'macham.side.tab',
  railTab: 'macham.rail.tab',
  layout: 'macham.layout',        // 구버전 키 — 서버 저장이 없던 시절 값을 한 번 물려받는다
  prefs: 'macham.prefs',          // 서버 개인 설정의 로컬 거울
};

/* 화면 배치 3종. 3단·2단은 DOM이 완전히 같고 portal.css의 그리드 배치만 갈린다.
 * 패널형만 도킹 트리를 따로 그린다(패널 노드는 새로 만들지 않고 옮기기만 한다).
 * cells는 프로필 메뉴의 미니 도식 — true인 칸이 '주 영역'이다. */
const LAYOUTS = {
  three: {
    label: '3단',
    desc: '검색·대기열을 늘 왼쪽에 띄워요.',
    extra: '처음이라면 이게 가장 무난해요.',
    hint: '가장 무난해요',
    cells: [false, true, false],
    drawerUnder: 1280,
  },
  two: {
    label: '2단',
    desc: '재생 화면을 넓게 쓰고 채팅을 오른쪽에 고정해요.',
    extra: '노래를 크게 보면서 대화도 놓치지 않아요.',
    cells: [true, false, false],
    drawerUnder: 981,
  },
  panel: {
    label: '패널',
    desc: '창을 원하는 대로 붙이고 나눠요.',
    extra: '탭을 끌어다 붙이면 내 마음대로 배치돼요. 넓은 화면에서 진가가 나와요.',
    cells: [true, false, false, false],
    drawerUnder: 0,
  },
};

/* 패널형에서 다룰 수 있는 창 목록. id는 prefs.panelLayout에 그대로 저장된다. */
const PANELS = {
  now: { icon: '▶', label: '지금 재생' },
  queue: { icon: '📋', label: '대기열' },
  search: { icon: '🔎', label: '검색' },
  library: { icon: '📚', label: '보관함' },
  chat: { icon: '💬', label: '채팅' },
  members: { icon: '👥', label: '멤버' },
  suggest: { icon: '💡', label: '제안' },
  recent: { icon: '🕘', label: '최근' },
  audit: { icon: '📜', label: '로그' },
  lyrics: { icon: '🎤', label: '가사' },
};

/* 크기 조절 한계. 아무리 끌어도 화면이 깨지지 않는 선. */
const SIZE_LIMITS = { rail: { min: 240, max: 560, def: 320 }, side: { min: 300, max: 620, def: 380 } };
const SIDE_DEF_TWO = 390;   // 2단은 채팅이 배치의 절반이라 기본값이 조금 더 넓다

/* ═══════════════════════ 개인 설정 (계정 저장) ═══════════════════════
 * 우선순위: 서버 값 > localStorage 거울 > 기본값.
 * 서버에서 받은 값은 즉시 거울에 적어 다음 첫 페인트가 안 튀게 한다.
 * 쓰기는 300ms 디바운스로 모아 보내고, 드래그 중에는 아예 부르지 않는다(끝난 뒤 한 번).
 */

const PREF_DEFAULTS = {
  layout: null, theme: 'dark', layoutSizes: null, panelLayout: null,
  lyricsOpen: '1', webPlayback: '0', webVolume: '60',
};

let prefsCache = readPrefsMirror();
let prefsRemoteOk = true;               // 서버에 prefs API가 없으면 조용히 로컬 전용으로 내려간다
const prefsPending = new Map();

function readPrefsMirror() {
  let raw = null;
  try { raw = localStorage.getItem(LS.prefs); } catch { /* 시크릿 모드 */ }
  let parsed = null;
  try { parsed = raw ? JSON.parse(raw) : null; } catch { parsed = null; }
  const out = parsed && typeof parsed === 'object' ? Object.assign({}, parsed) : {};
  // 서버 저장이 없던 시절의 낱개 키를 한 번만 물려받는다
  if (out.layout === undefined) {
    try {
      const old = localStorage.getItem(LS.layout);
      if (old) out.layout = old;
    } catch { /* 무시 */ }
  }
  if (out.lyricsOpen === undefined) {
    try {
      const old = localStorage.getItem(LS.lyrics);
      if (old !== null) out.lyricsOpen = old === '1' ? '1' : '0';
    } catch { /* 무시 */ }
  }
  return out;
}

function writePrefsMirror() {
  try { localStorage.setItem(LS.prefs, JSON.stringify(prefsCache)); } catch { /* 시크릿 모드 */ }
}

/** 문자열로 저장된 값. 없으면 기본값. */
function prefGet(key) {
  const value = prefsCache[key];
  if (value === undefined || value === null || value === '') return PREF_DEFAULTS[key] ?? null;
  return String(value);
}

/** JSON 문자열로 저장하는 값(layoutSizes / panelLayout)을 객체로 준다. */
function prefJson(key) {
  const raw = prefGet(key);
  if (!raw) return null;
  try { return JSON.parse(raw); } catch { return null; }
}

/** 값이 실제로 바뀔 때만 거울에 적고 서버로 보낸다. */
function prefSet(key, value) {
  const next = value === null || value === undefined ? null : String(value);
  if (String(prefsCache[key] ?? '') === String(next ?? '')) return;
  if (next === null) delete prefsCache[key];
  else prefsCache[key] = next;
  writePrefsMirror();
  if (next !== null) prefsPending.set(key, next);
  pushPrefs();
}

const pushPrefs = debounce(async () => {
  if (!prefsRemoteOk || !prefsPending.size) { prefsPending.clear(); return; }
  const body = Object.fromEntries(prefsPending);
  prefsPending.clear();
  try {
    await api('/music/api/prefs', { method: 'PUT', body });
  } catch (error) {
    // 서버가 아직 이 API를 모르면 로컬 저장만으로 계속 간다. 화면은 멀쩡해야 한다.
    if (error && (error.status === 404 || error.status === 405 || error.status === 501)) prefsRemoteOk = false;
  }
}, 300);

/** /state/cold 나 GET /music/api/prefs 응답을 받아 서버 값을 최우선으로 반영한다. */
function adoptServerPrefs(serverPrefs) {
  if (!serverPrefs || typeof serverPrefs !== 'object') return false;
  let touched = false;
  for (const [key, value] of Object.entries(serverPrefs)) {
    if (!(key in PREF_DEFAULTS)) continue;
    const next = value === null || value === undefined ? null : String(value);
    if (String(prefsCache[key] ?? '') === String(next ?? '')) continue;
    if (next === null) delete prefsCache[key];
    else prefsCache[key] = next;
    touched = true;
  }
  if (touched) writePrefsMirror();
  return touched;
}

/* ═══════════════════════ 화면 배치 ═══════════════════════
 * 첫 페인트에 배치가 튀면 안 된다. 셸을 만들기 전, 모듈이 로드되는 이 시점에 바로 박는다.
 * CSS는 <html data-layout="...">만 본다.
 */

let layoutChosen = !!LAYOUTS[prefGet('layout')];
let activeLayout = layoutChosen ? prefGet('layout') : 'three';
document.documentElement.dataset.layout = effectiveLayout();
applyLayoutSizes();

/** 680px 이하에서는 패널형(도킹)을 쓰지 않는다. 단일 컬럼 + 하단 탭바로 내려간다. */
function narrowScreen() {
  return window.matchMedia('(max-width: 680px)').matches;
}

function effectiveLayout() {
  if (activeLayout === 'panel' && narrowScreen()) return 'three';
  return LAYOUTS[activeLayout] ? activeLayout : 'three';
}

function panelMode() {
  return effectiveLayout() === 'panel';
}

/* 우측 사이드가 드로어로 빠지는 구간인가. 3단은 1280px 미만, 2단은 981px 미만. */
function drawerActive() {
  if (narrowScreen()) return false;
  const layout = effectiveLayout();
  if (layout === 'panel') return false;
  return window.innerWidth < (LAYOUTS[layout] || LAYOUTS.three).drawerUnder;
}

/* ── 열 너비 (prefs.layoutSizes) ── */

function layoutSizes() {
  const saved = prefJson('layoutSizes');
  return saved && typeof saved === 'object' ? saved : {};
}

function sizeFor(layout, key) {
  const saved = layoutSizes()[layout];
  const raw = saved && Number(saved[key]);
  if (Number.isFinite(raw) && raw > 0) return clampSize(key, raw);
  return key === 'side' && layout === 'two' ? SIDE_DEF_TWO : SIZE_LIMITS[key].def;
}

function clampSize(key, value) {
  const limit = SIZE_LIMITS[key];
  if (!limit) return value;
  return Math.round(Math.min(limit.max, Math.max(limit.min, value)));
}

/** --rail-w / --side-w 를 지금 배치에 맞춰 박는다. 모듈 로드 시점에도 한 번 부른다. */
function applyLayoutSizes() {
  const layout = effectiveLayout();
  const host = document.documentElement;
  if (layout === 'panel' || narrowScreen()) {
    host.style.removeProperty('--rail-w');
    host.style.removeProperty('--side-w');
    return;
  }
  host.style.setProperty('--rail-w', `${sizeFor(layout, 'rail')}px`);
  host.style.setProperty('--side-w', `${sizeFor(layout, 'side')}px`);
  const compose = Number(layoutSizes().chat?.compose);
  if (Number.isFinite(compose) && compose > 0) host.style.setProperty('--compose-h', `${Math.round(compose)}px`);
}

/** 드래그가 끝난 뒤 한 번만 저장한다. */
function saveSize(layout, key, value) {
  const all = layoutSizes();
  const bucket = Object.assign({}, all[layout]);
  bucket[key] = value;
  all[layout] = bucket;
  prefSet('layoutSizes', JSON.stringify(all));
}

/* ═══════════════════════ 작은 헬퍼 ═══════════════════════ */

const el = {};                 // 자주 만지는 노드 참조
let searchResults = [];
let searchedQuery = '';
let searchSource = '';         // browser | server | '' — 결과가 어디서 왔는지
let replyTo = null;            // { id, displayName, preview }
let lastReadId = 0;
let unread = 0;
let libraryTab = 'liked';      // liked | saved | playlists
let auditQuery = '';
let libraryQuery = '';
let lastCurrentId = null;
let acState = null;            // 자동완성 { kind, from, to, items, index }
let serverSkewMs = 0;          // 서버 시계 − 내 시계. 카운트다운을 서버 기준으로 맞추는 데 쓴다
let lastBotState = null;       // presence.bot — WS 이벤트가 안 실어 보내도 잃어버리지 않게 따로 보관
let seedState = null;          // { seeds, max, canEdit } — 없으면 서버가 아직 시드곡을 모른다는 뜻
let seedOpen = false;

function trackKey(track) {
  if (!track) return '';
  return track.cacheKey || `${track.provider || ''}:${track.contentId || ''}`;
}

/** 서버가 artUrl을 주면 그걸 쓰고, 없으면 유튜브 썸네일을 유추한다. */
function artUrl(track) {
  if (!track) return '';
  if (track.artUrl) return track.artUrl;
  if (track.thumbnailUrl) return track.thumbnailUrl;
  const provider = track.provider || '';
  if ((provider === 'YouTube' || provider === 'YouTubeMusic') && track.contentId) {
    return `https://i.ytimg.com/vi/${encodeURIComponent(track.contentId)}/hqdefault.jpg`;
  }
  return '';
}

function trackTitle(track) {
  return (track && (track.title || track.contentId)) || '제목 없음';
}

function trackSub(track) {
  if (!track) return '';
  const bits = [track.artist, track.provider].filter(Boolean);
  const seconds = trackSeconds(track);
  if (seconds) bits.push(fmtTime(seconds));
  return bits.join(' · ');
}

/** duration은 C# TimeSpan 문자열로 올 수 있다. durationSeconds가 있으면 그걸 우선한다. */
function trackSeconds(track) {
  if (!track) return 0;
  if (Number.isFinite(track.durationSeconds)) return track.durationSeconds;
  const raw = track.duration;
  if (typeof raw === 'number') return raw;
  if (typeof raw === 'string' && raw.includes(':')) {
    const parts = raw.split(':').map(Number);
    if (parts.every((n) => Number.isFinite(n))) {
      return parts.reduce((acc, n) => acc * 60 + n, 0);
    }
  }
  return 0;
}

/** 서버가 최종 판정을 하지만, 화면은 즉시 맞춘다.
 *  정지 통보(WS)는 cold 재조회보다 먼저 오므로 여기서 바로 반영해야 UI가 어긋나지 않는다. */
function can(key) {
  const state = store.get();
  if (state.conn === 'down') return false;
  if (state.tier === 'viewer') return false;
  const suspension = state.suspension;
  if (suspension && (suspension.scope === 'all' || matchScope(suspension.scope, key))) return false;
  const permissions = state.permissions;
  if (!permissions || !permissions.can) return false;
  return !!permissions.can[key];
}

function tierOf() {
  return store.get().tier || 'member';
}

/** 왜 못 하는지 한 줄로. 정지 중이면 정지 사유가 이긴다. */
function lockReason(key) {
  const state = store.get();
  if (state.conn === 'down') return '연결이 끊겨서 지금은 조작할 수 없어요. 새로고침해 주세요.';
  const suspension = state.suspension;
  if (suspension && (suspension.scope === 'all' || matchScope(suspension.scope, key))) {
    return `정지 중이라 지금은 못 해요 · ${suspensionRemain(suspension)}`;
  }
  if (state.tier === 'viewer') return '읽기 전용이라 조작할 수 없어요.';
  const entry = (state.permissions?.entries || []).find((row) => row.key === key);
  if (entry && entry.reason) return entry.reason;
  if (entry) return `${RULE_LABELS[entry.rule] || entry.ruleLabel || '관리자'}만 할 수 있어요.`;
  return '권한이 없어요.';
}

function matchScope(scope, key) {
  if (scope === 'chat') return key === 'chat' || key === 'suggest';
  if (scope === 'queue') return key === 'search' || key === 'vote' || key === 'queueEdit';
  return false;
}

function suspensionRemain(suspension) {
  if (!suspension) return '';
  if (!suspension.expiresUtc) return '무기한';
  const left = parseUtc(suspension.expiresUtc) - Date.now();
  if (left <= 0) return '곧 풀림';
  const minutes = Math.ceil(left / 60000);
  if (minutes < 60) return `${minutes}분 남음`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}시간 ${minutes % 60}분 남음` : `${Math.floor(hours / 24)}일 남음`;
}

/** 권한 없는 버튼은 숨기지 않는다. 비활성 모양 + 이유 툴팁으로 남긴다. */
function setLock(node, locked, reason) {
  if (!node) return node;
  if (node.__tipBase === undefined) node.__tipBase = node.getAttribute('data-tip') || '';
  node.setAttribute('aria-disabled', locked ? 'true' : 'false');
  node.classList.toggle('is-locked', !!locked);
  if (locked) {
    node.dataset.lockReason = reason || '권한이 없어요.';
    node.setAttribute('data-tip', reason || '권한이 없어요.');
  } else {
    delete node.dataset.lockReason;
    if (node.__tipBase) node.setAttribute('data-tip', node.__tipBase);
    else node.removeAttribute('data-tip');
  }
  return node;
}

/** 잠긴 버튼을 눌렀을 때는 이유를 알려준다. 조용히 씹지 않는다. */
function bindAct(node, fn) {
  node.addEventListener('click', (event) => {
    if (node.getAttribute('aria-disabled') === 'true') {
      event.preventDefault();
      event.stopPropagation();
      toast(node.dataset.lockReason || '권한이 없어요.', 'warn');
      return;
    }
    fn(event);
  });
  return node;
}

async function call(fn, okMessage) {
  try {
    const result = await fn();
    if (okMessage) toast(okMessage, 'ok');
    return result;
  } catch (error) {
    if (error instanceof ApiError || error instanceof Error) toast(error.message, 'danger');
    else toast('처리하지 못했어요.', 'danger');
    return null;
  }
}

/** Node.append()는 null을 문자열 "null"로 넣어버린다. 조건부 자식은 항상 이걸로 붙인다. */
function put(node, ...kids) {
  for (const kid of kids) {
    if (kid === null || kid === undefined || kid === false || kid === true) continue;
    if (Array.isArray(kid)) { put(node, ...kid); continue; }
    node.appendChild(kid instanceof Node ? kid : document.createTextNode(escapeText(kid)));
  }
  return node;
}

function avatar(url, name, size) {
  const cls = size === 'sm' ? 'ava ava--sm' : size === 'lg' ? 'ava ava--lg' : 'ava';
  if (url) return h('img', { class: cls, src: url, alt: '', loading: 'lazy' });
  const initial = (escapeText(name) || '?').trim().charAt(0) || '?';
  return h('span', {
    class: cls,
    style: { display: 'grid', placeItems: 'center', fontSize: '11px', color: 'var(--text-3)' },
    'aria-hidden': 'true',
  }, initial);
}

function skeletonRows(count) {
  return frag(...Array.from({ length: count }, () => h('div', { class: 'skel-row' },
    h('div', { class: 'skel' }),
    h('div', null, h('div', { class: 'skel skel--t' }), h('div', { class: 'skel skel--s' })))));
}

function emptyState(icon, title, desc) {
  return h('div', { class: 'empty empty--sm' },
    h('div', { class: 'empty__icon' }, icon),
    h('div', { class: 'empty__title' }, title),
    desc ? h('div', { class: 'empty__desc' }, desc) : null);
}

/* ═══════════════════════ 셸 조립 ═══════════════════════ */

function buildShell() {
  const root = document.getElementById('app') || document.body.appendChild(h('div', { id: 'app' }));
  clear(root);

  el.banners = h('div', { class: 'portal__banners' });
  el.live = h('div', { class: 'sr-only', role: 'status', 'aria-live': 'polite' });

  el.rail = h('aside', { class: 'col rail' }, buildRail());
  el.stage = h('section', { class: 'col stage' }, buildStage());
  el.side = h('aside', { class: 'col side' }, buildSide());

  el.gutterRail = buildGutter('rail', '검색·대기열 열 너비');
  el.gutterSide = buildGutter('side', '채팅 열 너비');

  el.grid = h('main', { class: 'portal__grid' },
    el.rail, el.gutterRail, el.stage, el.gutterSide, el.side);

  // 패널형이 쓰는 도킹 판. 3단·2단일 때는 비어 있고 숨어 있다.
  el.dock = h('main', { class: 'dock', hidden: true });

  el.portal = h('div', {
    class: 'portal',
    dataset: { conn: 'connecting', layout: effectiveLayout() },
  },
    el.banners,
    buildHeader(),
    el.grid,
    el.dock,
    buildMobileTabs(),
    el.live);

  root.appendChild(el.portal);
  document.body.dataset.pane = 'stage';
  rememberPanelHomes();
  watchBannerHeight();
  bindComposeResize();
  watchViewport();
}

/** 2단의 sticky 사이드 높이는 "뷰포트 − 헤더 − 배너"다.
 *  배너는 접속이 끊기거나 정지당하면 생겼다 사라지므로 높이를 계속 알려준다. */
function watchBannerHeight() {
  const sync = () => el.portal.style.setProperty('--banner-h', `${el.banners.offsetHeight || 0}px`);
  if (window.ResizeObserver) new ResizeObserver(sync).observe(el.banners);
  else window.addEventListener('resize', sync);
  sync();
}

/** 창 크기가 바뀌면 패널형↔단일 컬럼 경계를 다시 판정한다. */
function watchViewport() {
  let timer = 0;
  let last = effectiveLayout();
  window.addEventListener('resize', () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const next = effectiveLayout();
      if (next !== last) { last = next; applyLayout(); return; }
      applyLayoutSizes();
      if (panelMode()) layoutDock();
    }, 140);
  });
}

/* ═══════════════════════ 열 크기 조절 (§7.1) ═══════════════════════
 * 열 경계에 손잡이를 둔다. 드래그·더블클릭 초기화·키보드 조절 전부 된다.
 * 저장은 드래그가 끝난 뒤 한 번만 — 끄는 동안 서버를 두드리지 않는다.
 */

function buildGutter(key, label) {
  const node = h('div', {
    class: `gutter gutter--${key}`,
    role: 'separator', 'aria-orientation': 'vertical', tabindex: '0',
    'aria-label': label,
    tip: '끌어서 너비 조절 · 더블클릭하면 기본값 · 화살표 키로도 돼요',
  }, h('span', { class: 'gutter__grip', 'aria-hidden': 'true' }));

  const apply = (value, save) => {
    const layout = effectiveLayout();
    const next = clampSize(key, value);
    document.documentElement.style.setProperty(`--${key}-w`, `${next}px`);
    node.setAttribute('aria-valuenow', String(next));
    // 끄는 동안 문서 전체를 다시 재는 건 낭비다. 저장하는 순간에만 마퀴를 다시 계산한다.
    if (save) { saveSize(layout, key, next); marquee.scan(); }
  };

  let dragging = false;
  let startX = 0;
  let startValue = 0;

  node.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    dragging = true;
    startX = event.clientX;
    startValue = sizeFor(effectiveLayout(), key);
    node.dataset.drag = '1';
    node.setPointerCapture(event.pointerId);
    document.body.dataset.resizing = 'col';
    event.preventDefault();
  });
  node.addEventListener('pointermove', (event) => {
    if (!dragging) return;
    // 좌측 레일은 오른쪽으로 끌면 넓어지고, 우측 사이드는 반대다.
    const delta = key === 'rail' ? event.clientX - startX : startX - event.clientX;
    apply(startValue + delta, false);
  });
  const finish = () => {
    if (!dragging) return;
    dragging = false;
    delete node.dataset.drag;
    delete document.body.dataset.resizing;
    const current = parseInt(document.documentElement.style.getPropertyValue(`--${key}-w`), 10);
    if (Number.isFinite(current)) saveSize(effectiveLayout(), key, clampSize(key, current));
    marquee.scan();
  };
  node.addEventListener('pointerup', finish);
  node.addEventListener('pointercancel', finish);

  node.addEventListener('dblclick', () => {
    const layout = effectiveLayout();
    const def = key === 'side' && layout === 'two' ? SIDE_DEF_TWO : SIZE_LIMITS[key].def;
    apply(def, true);
    toast(`${key === 'rail' ? '왼쪽' : '오른쪽'} 열 너비를 기본값으로 되돌렸어요.`, 'ok');
  });

  node.addEventListener('keydown', (event) => {
    const step = event.shiftKey ? 48 : 16;
    let next = null;
    if (event.key === 'ArrowRight') next = sizeFor(effectiveLayout(), key) + (key === 'rail' ? step : -step);
    else if (event.key === 'ArrowLeft') next = sizeFor(effectiveLayout(), key) - (key === 'rail' ? step : -step);
    else if (event.key === 'Home') next = key === 'side' && effectiveLayout() === 'two' ? SIDE_DEF_TWO : SIZE_LIMITS[key].def;
    if (next === null) return;
    event.preventDefault();
    apply(next, true);
  });

  node.setAttribute('aria-valuemin', String(SIZE_LIMITS[key].min));
  node.setAttribute('aria-valuemax', String(SIZE_LIMITS[key].max));
  return node;
}

/** 채팅 열 안에서 목록과 입력창 사이를 세로로 조절한다. */
function bindComposeResize() {
  const node = h('div', {
    class: 'gutter gutter--row',
    role: 'separator', 'aria-orientation': 'horizontal', tabindex: '0',
    'aria-label': '채팅 입력창 높이',
    tip: '끌어서 입력창 높이 조절 · 더블클릭하면 기본값이에요',
  }, h('span', { class: 'gutter__grip', 'aria-hidden': 'true' }));
  el.composeGutter = node;
  el.compose.parentElement.insertBefore(node, el.compose);

  const apply = (value, save) => {
    const next = Math.round(Math.min(260, Math.max(36, value)));
    document.documentElement.style.setProperty('--compose-h', `${next}px`);
    el.chatInput.style.height = `${next}px`;
    node.setAttribute('aria-valuenow', String(next));
    if (save) {
      const all = layoutSizes();
      all.chat = Object.assign({}, all.chat, { compose: next });
      prefSet('layoutSizes', JSON.stringify(all));
    }
  };

  let dragging = false;
  let startY = 0;
  let startValue = 36;
  node.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    dragging = true;
    startY = event.clientY;
    startValue = el.chatInput.getBoundingClientRect().height || 36;
    node.dataset.drag = '1';
    node.setPointerCapture(event.pointerId);
    document.body.dataset.resizing = 'row';
    event.preventDefault();
  });
  node.addEventListener('pointermove', (event) => { if (dragging) apply(startValue + (startY - event.clientY), false); });
  const finish = () => {
    if (!dragging) return;
    dragging = false;
    delete node.dataset.drag;
    delete document.body.dataset.resizing;
    apply(el.chatInput.getBoundingClientRect().height || 36, true);
  };
  node.addEventListener('pointerup', finish);
  node.addEventListener('pointercancel', finish);
  node.addEventListener('dblclick', () => apply(36, true));
  node.addEventListener('keydown', (event) => {
    const step = event.shiftKey ? 40 : 12;
    const current = el.chatInput.getBoundingClientRect().height || 36;
    if (event.key === 'ArrowUp') { event.preventDefault(); apply(current + step, true); }
    else if (event.key === 'ArrowDown') { event.preventDefault(); apply(current - step, true); }
    else if (event.key === 'Home') { event.preventDefault(); apply(36, true); }
  });

  const saved = Number(layoutSizes().chat?.compose);
  if (Number.isFinite(saved) && saved > 36) apply(saved, false);
}

/* ── 헤더 ── */

function buildHeader() {
  el.guildBtn = h('button', {
    class: 'hdr__guild', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    tip: '다른 서버로 이동',
    onClick: () => toggleMenu(el.guildMenu, el.guildBtn),
  }, h('span', { class: 'hdr__caret' }, '▾'));

  el.guildMenu = h('div', { class: 'dd__menu dd__menu--left', role: 'menu', hidden: true });

  el.presenceBtn = h('button', {
    class: 'hdr__presence', type: 'button',
    tip: '지금 누가 듣고 있는지 보기',
    onClick: () => { openSide('members'); },
  });

  el.themeBtn = h('button', {
    class: 'btn btn--ghost btn--icon', type: 'button', tip: '밝게 / 어둡게',
    'aria-label': '테마 전환',
    onClick: () => {
      const mode = theme.toggle();
      el.themeBtn.textContent = mode === 'dark' ? '🌙' : '☀';
      prefSet('theme', mode);        // 기기가 아니라 계정에 남는다
    },
  }, theme.current() === 'dark' ? '🌙' : '☀');

  el.drawerBtn = h('button', {
    class: 'btn btn--ghost btn--icon hdr__drawer', type: 'button', tip: '채팅·멤버 열기',
    'aria-label': '우측 패널 열기',
    onClick: () => openSide(activeSideTab),
  }, '💬');
  el.drawerBadge = h('span', { class: 'badge', hidden: true }, '0');
  el.drawerBtn.appendChild(el.drawerBadge);

  el.meBtn = h('button', {
    class: 'hdr__me', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    tip: '내 권한 · 설정',
    onClick: () => toggleMenu(el.meMenu, el.meBtn),
  });
  el.meMenu = buildProfileMenu();

  return h('header', { class: 'hdr' },
    h('div', { class: 'hdr__brand' }, '마참뮤직', h('small', null, 'REMOTE')),
    h('div', { class: 'dd' }, el.guildBtn, el.guildMenu),
    h('div', { class: 'hdr__spacer' }),
    el.presenceBtn,
    el.themeBtn,
    el.drawerBtn,
    h('div', { class: 'dd' }, el.meBtn, el.meMenu));
}

function buildProfileMenu() {
  el.meHead = h('div', { class: 'dd__head' });

  const permBtn = h('button', { class: 'dd__item', type: 'button', role: 'menuitem', onClick: () => { closeMenus(); openPermissionSheet(); } },
    h('span', null, '🔐'), h('span', null, '내 권한'));

  el.consoleBtn = h('a', {
    class: 'dd__item', role: 'menuitem', 'data-testid': 'settings-open',
    href: `/music/guilds/${ctx.guildId}/admin`,
  }, h('span', null, '⚙'), h('span', null, '서버 관리 콘솔'));

  el.opsLink = h('a', { class: 'dd__item', role: 'menuitem', href: '/', target: '_blank', rel: 'noreferrer' },
    h('span', null, '🛠'), h('span', null, '운영 패널'));

  el.notifyBtn = h('button', { class: 'dd__item', type: 'button', role: 'menuitem', onClick: askNotify },
    h('span', null, '🔔'), h('span', null, '알림 허용'));

  const logout = h('button', {
    class: 'dd__item dd__item--danger', type: 'button', role: 'menuitem',
    onClick: async () => {
      closeMenus();
      if (!(await confirmSheet({ title: '로그아웃', desc: '이 기기에서 리모컨 로그인을 끊어요.', danger: true, confirmText: '로그아웃' }))) return;
      const form = h('form', { method: 'post', action: '/music/logout' },
        h('input', { type: 'hidden', name: 'csrf', value: ctx.csrf }));
      document.body.appendChild(form);
      form.submit();
    },
  }, h('span', null, '↩'), h('span', null, '로그아웃'));

  return h('div', { class: 'dd__menu dd__menu--wide', role: 'menu', hidden: true },
    el.meHead, permBtn, el.consoleBtn, el.opsLink,
    h('div', { class: 'dd__sep' }),
    buildLayoutPicker(),
    h('div', { class: 'dd__sep' }),
    el.notifyBtn, logout);
}

/* ── 화면 배치 고르기 ── */

function buildLayoutPicker() {
  el.layoutOpts = Object.entries(LAYOUTS).map(([id, def]) => layoutOption(id, def, 'menuitemradio'));

  const reset = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem',
    onClick: () => { closeMenus(); resetPanelLayout(); },
  }, h('span', null, '↺'), h('span', null, '패널 배치를 기본으로 되돌리기'));
  el.panelResetBtn = reset;

  return frag(
    h('div', { class: 'dd__label' }, '화면 배치'),
    h('div', { class: 'lay', role: 'group', 'aria-label': '화면 배치' }, ...el.layoutOpts),
    reset);
}

/** 프로필 메뉴와 첫 진입 시트가 같은 카드를 쓴다. */
function layoutOption(id, def, role) {
  return h('button', {
    class: 'lay__opt', type: 'button', role,
    'aria-checked': String(id === activeLayout),
    dataset: { layout: id },
    onClick: () => setLayout(id),
  },
    h('span', { class: `lay__glyph lay__glyph--${id}`, 'aria-hidden': 'true' },
      ...def.cells.map((main) => h('i', { class: main ? 'is-main' : null }))),
    h('strong', null, def.label, def.hint ? h('span', { class: 'chip chip--accent' }, def.hint) : null),
    h('small', null, def.desc));
}

/** 새로고침 없이 즉시 바꾼다. 3단↔2단은 그리드만, 패널형은 도킹 판으로 갈아 끼운다. */
function setLayout(id, quiet) {
  const next = LAYOUTS[id] ? id : 'three';
  layoutChosen = true;
  if (next === activeLayout) { prefSet('layout', next); syncLayoutOptions(); return; }
  activeLayout = next;
  prefSet('layout', next);
  applyLayout();
  if (!quiet) {
    toast(next === 'panel' && narrowScreen()
      ? '패널 배치를 골랐어요. 화면이 좁아서 지금은 단일 화면으로 보여드려요.'
      : `${LAYOUTS[next].label} 배치로 바꿨어요.`, 'ok');
  }
}

function syncLayoutOptions() {
  for (const node of el.layoutOpts || []) {
    node.setAttribute('aria-checked', String(node.dataset.layout === activeLayout));
  }
  if (el.panelResetBtn) el.panelResetBtn.hidden = activeLayout !== 'panel';
}

function applyLayout() {
  const atEnd = nearChatEnd();
  const layout = effectiveLayout();

  // 전환 순간에 드로어가 슬라이드해 들어오는 등의 잔상이 남지 않게 한 프레임 동안 전환을 끈다
  el.portal.dataset.swap = '1';
  document.documentElement.dataset.layout = layout;
  el.portal.dataset.layout = layout;
  openDrawer(false);
  syncLayoutOptions();

  if (layout === 'panel') mountDock();
  else unmountDock();

  applyLayoutSizes();

  requestAnimationFrame(() => {
    delete el.portal.dataset.swap;
    if (atEnd) scrollChatToEnd(false);
    marquee.scan();
    scheduleViz();
  });
}

/** 서버에서 받은 개인 설정을 화면에 반영한다. 로컬 값과 다르면 서버 쪽이 이긴다. */
function applyServerPrefs() {
  const savedTheme = prefGet('theme');
  if ((savedTheme === 'dark' || savedTheme === 'light') && savedTheme !== theme.current()) {
    theme.apply(savedTheme);
    try { localStorage.setItem('macham.theme', savedTheme); } catch { /* 시크릿 모드 */ }
    if (el.themeBtn) el.themeBtn.textContent = savedTheme === 'dark' ? '🌙' : '☀';
  }

  const savedLyrics = prefGet('lyricsOpen') === '1';
  if (savedLyrics !== lyricsOpen && el.lyricsToggle) {
    lyricsOpen = savedLyrics;
    el.lyricsToggle.setAttribute('aria-expanded', String(lyricsOpen));
    el.lyricsToggle.classList.toggle('btn--primary', lyricsOpen);
    if (!panelMode()) el.lyricsBox.hidden = !lyricsOpen;
    if (lyricsOpen) loadLyrics();
  }

  dockTree = readDockTree();
  const savedLayout = prefGet('layout');
  if (LAYOUTS[savedLayout]) {
    layoutChosen = true;
    if (savedLayout !== activeLayout) { activeLayout = savedLayout; applyLayout(); return; }
  }
  applyLayoutSizes();
  if (panelMode()) renderDock();
}

/* ── 첫 진입 배치 선택 시트 (§3) ── */

let layoutSheetOpen = false;

function openLayoutSheet() {
  if (layoutSheetOpen) return;
  layoutSheetOpen = true;
  let handle = null;

  const cards = Object.entries(LAYOUTS).map(([id, def]) => h('button', {
    class: 'pick__card', type: 'button', dataset: { layout: id },
    onClick: () => { setLayout(id, true); handle?.close(id); },
  },
    h('span', { class: `lay__glyph lay__glyph--${id}`, 'aria-hidden': 'true' },
      ...def.cells.map((main) => h('i', { class: main ? 'is-main' : null }))),
    h('strong', null, def.label, def.hint ? h('span', { class: 'chip chip--accent' }, def.hint) : null),
    h('p', null, def.desc),
    h('small', null, def.extra)));

  handle = sheet({
    title: '화면을 어떻게 볼까요',
    desc: '처음 오셨네요. 셋 중 마음에 드는 걸 고르면 바로 그렇게 보여드려요.',
    wide: true,
    dismissValue: null,
    body: h('div', null,
      h('div', { class: 'pick' }, ...cards),
      h('p', { class: 'hint', style: { marginTop: 'var(--sp-4)' } },
        '나중에 프로필 메뉴에서 언제든 바꿀 수 있어요.')),
    actions: [],
  });

  handle.result.then((value) => {
    layoutSheetOpen = false;
    // ✕나 Esc로 닫아도 지금 배치를 고른 것으로 쳐서 다시 묻지 않는다
    if (!value) setLayout(activeLayout, true);
    toast(`${LAYOUTS[activeLayout].label} 배치로 시작할게요.`, 'ok');
  });
}

/* ═══════════════════════ 패널형 도킹 (§7.2) ═══════════════════════
 * 트리는 { type:'split', dir:'row'|'col', ratio, a, b } 와
 *        { type:'tabs', panels:[...], active } 두 가지뿐이다.
 * 화면을 다시 그릴 때도 패널 알맹이는 **옮기기만** 한다. 새로 만들면 이벤트와 data-testid가 죽는다.
 */

/* 패널 알맹이가 원래 살던 자리. 패널형을 끄면 이 순서대로 되돌린다. */
const HOME_ORDER = {
  stage: ['now', 'lyrics'],
  rail: ['search', 'queue', 'library'],
  side: ['chat', 'members', 'suggest', 'recent', 'audit'],
};

let dockTree = null;
let dockDrag = null;
let dockGid = 0;

function panelNode(id) {
  if (id === 'now') return el.nowCard;
  if (id === 'lyrics') return el.lyricsBox;
  if (el.railPanes && el.railPanes[id]) return el.railPanes[id];
  return el.sidePanes ? el.sidePanes[id] : null;
}

function homeHost(id) {
  if (HOME_ORDER.stage.includes(id)) return { host: el.stageScroll, order: HOME_ORDER.stage };
  if (HOME_ORDER.rail.includes(id)) return { host: el.railBody, order: HOME_ORDER.rail };
  return { host: el.sideBody, order: HOME_ORDER.side };
}

function rememberPanelHomes() {
  // 알맹이가 있어야 할 컨테이너는 buildRail/buildStage/buildSide가 el에 담아 뒀다.
  // 여기서는 존재만 확인한다 — 순서는 HOME_ORDER가 안다.
  for (const id of Object.keys(PANELS)) {
    if (!panelNode(id)) console.warn('[dock] 패널 알맹이를 못 찾았어요:', id);
  }
}

/** 패널 알맹이를 제자리로 돌려보낸다. 순서를 지켜 끼워 넣는다. */
function homePanel(id) {
  const node = panelNode(id);
  if (!node) return;
  const { host, order } = homeHost(id);
  if (!host) return;
  if (node.parentElement === host) return;
  let ref = null;
  for (let i = order.indexOf(id) + 1; i < order.length; i += 1) {
    const later = panelNode(order[i]);
    if (later && later.parentElement === host) { ref = later; break; }
  }
  host.insertBefore(node, ref);
}

/* ── 트리 만들기·검사 ── */

function defaultDockTree() {
  return {
    type: 'split', dir: 'row', ratio: 0.72,
    a: {
      type: 'split', dir: 'col', ratio: 0.56,
      a: { type: 'tabs', panels: ['now'], active: 'now' },
      b: { type: 'tabs', panels: ['queue', 'search', 'library'], active: 'queue' },
    },
    b: { type: 'tabs', panels: ['chat', 'members', 'suggest', 'recent', 'audit'], active: 'chat' },
  };
}

/** 저장된 값은 남이 손댔을 수도 있다. 모르는 건 버리고 중복은 지운다. */
function sanitizeTree(raw, seen = new Set()) {
  if (!raw || typeof raw !== 'object') return null;
  if (raw.type === 'split') {
    const a = sanitizeTree(raw.a, seen);
    const b = sanitizeTree(raw.b, seen);
    if (a && b) {
      const ratio = Number(raw.ratio);
      return {
        type: 'split',
        dir: raw.dir === 'col' ? 'col' : 'row',
        ratio: Number.isFinite(ratio) ? Math.min(0.88, Math.max(0.12, ratio)) : 0.5,
        a, b,
      };
    }
    return a || b;
  }
  if (raw.type !== 'tabs') return null;
  const panels = (Array.isArray(raw.panels) ? raw.panels : [])
    .filter((id) => PANELS[id] && !seen.has(id));
  for (const id of panels) seen.add(id);
  if (!panels.length) return null;
  return { type: 'tabs', panels, active: panels.includes(raw.active) ? raw.active : panels[0] };
}

function readDockTree() {
  return sanitizeTree(prefJson('panelLayout')) || defaultDockTree();
}

function serializeTree(node) {
  if (node.type === 'split') {
    return { type: 'split', dir: node.dir, ratio: Number(node.ratio.toFixed(3)), a: serializeTree(node.a), b: serializeTree(node.b) };
  }
  return { type: 'tabs', panels: node.panels.slice(), active: node.active };
}

function savePanelLayout() {
  if (!dockTree) return;
  prefSet('panelLayout', JSON.stringify(serializeTree(dockTree)));
}

/* ── 트리 조작 ── */

function eachNode(node, fn, parent = null) {
  if (!node) return;
  fn(node, parent);
  if (node.type === 'split') { eachNode(node.a, fn, node); eachNode(node.b, fn, node); }
}

function findGroup(predicate) {
  let hit = null;
  eachNode(dockTree, (node) => { if (!hit && node.type === 'tabs' && predicate(node)) hit = node; });
  return hit;
}

function parentOf(target) {
  let parent = null;
  eachNode(dockTree, (node) => {
    if (node.type === 'split' && (node.a === target || node.b === target)) parent = node;
  });
  return parent;
}

function replaceNode(target, replacement) {
  if (dockTree === target) { dockTree = replacement; return true; }
  const parent = parentOf(target);
  if (!parent) return false;
  if (parent.a === target) parent.a = replacement; else parent.b = replacement;
  return true;
}

function openPanels() {
  const ids = new Set();
  eachNode(dockTree, (node) => { if (node.type === 'tabs') for (const id of node.panels) ids.add(id); });
  return ids;
}

/** 그룹에서 패널 하나를 뺀다. 그룹이 비면 그룹째 접는다(마지막 하나는 남긴다). */
function detachPanel(id) {
  const group = findGroup((node) => node.panels.includes(id));
  if (!group) return null;
  group.panels = group.panels.filter((panel) => panel !== id);
  if (group.active === id) group.active = group.panels[0] || null;
  if (!group.panels.length && dockTree !== group) {
    const parent = parentOf(group);
    if (parent) replaceNode(parent, parent.a === group ? parent.b : parent.a);
  }
  return group;
}

/* ── 화면 붙이기·떼기 ── */

function mountDock() {
  if (!dockTree) dockTree = readDockTree();
  el.grid.hidden = true;
  el.dock.hidden = false;
  renderDock();
}

function unmountDock() {
  if (el.dock.hidden && !el.dock.firstChild) { el.grid.hidden = false; return; }
  for (const id of Object.keys(PANELS)) homePanel(id);
  clear(el.dock);
  el.dock.hidden = true;
  el.grid.hidden = false;
  // 그리드 쪽 탭 상태를 되살린다
  for (const [key, pane] of Object.entries(el.railPanes || {})) pane.hidden = key !== activeRailTab;
  for (const [key, pane] of Object.entries(el.sidePanes || {})) pane.hidden = key !== activeSideTab;
  el.lyricsBox.hidden = !lyricsOpen;
  el.nowCard.hidden = false;
}

function layoutDock() {
  if (!panelMode()) return;
  marquee.scan(el.dock);
}

function renderDock() {
  const scrollWasAtEnd = nearChatEnd();
  clear(el.dock);
  el.dock.appendChild(buildDockNode(dockTree));
  el.dock.appendChild(dropOverlay());

  // 트리에 없는 패널은 원래 자리로 돌려보낸다(패널형을 끌 때 바로 찾을 수 있게)
  const open = openPanels();
  for (const id of Object.keys(PANELS)) if (!open.has(id)) homePanel(id);

  requestAnimationFrame(() => {
    if (scrollWasAtEnd) scrollChatToEnd(false);
    marquee.scan(el.dock);
    scheduleViz();
  });
}

function dropOverlay() {
  if (!el.dockDrop) el.dockDrop = h('div', { class: 'dk-drop', hidden: true, 'aria-hidden': 'true' });
  return el.dockDrop;
}

function buildDockNode(node) {
  if (node.type === 'split') {
    const a = h('div', { class: 'dk-slot', style: { flexGrow: String(node.ratio) } }, buildDockNode(node.a));
    const b = h('div', { class: 'dk-slot', style: { flexGrow: String(1 - node.ratio) } }, buildDockNode(node.b));
    const divider = buildDivider(node);
    const wrap = h('div', { class: `dk-split dk-split--${node.dir}` }, a, divider, b);
    node.__slotA = a;
    node.__slotB = b;
    return wrap;
  }
  return buildDockGroup(node);
}

function buildDivider(split) {
  const node = h('div', {
    class: 'dk-div', role: 'separator', tabindex: '0',
    'aria-orientation': split.dir === 'row' ? 'vertical' : 'horizontal',
    'aria-label': '패널 크기 조절',
    tip: '끌어서 크기 조절 · 더블클릭하면 반반이에요',
  });

  const setRatio = (value) => {
    split.ratio = Math.min(0.88, Math.max(0.12, value));
    if (split.__slotA) split.__slotA.style.flexGrow = String(split.ratio);
    if (split.__slotB) split.__slotB.style.flexGrow = String(1 - split.ratio);
    node.setAttribute('aria-valuenow', String(Math.round(split.ratio * 100)));
  };

  let dragging = false;
  node.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    dragging = true;
    node.dataset.drag = '1';
    node.setPointerCapture(event.pointerId);
    document.body.dataset.resizing = split.dir === 'row' ? 'col' : 'row';
    event.preventDefault();
  });
  node.addEventListener('pointermove', (event) => {
    if (!dragging) return;
    const box = node.parentElement.getBoundingClientRect();
    const ratio = split.dir === 'row'
      ? (event.clientX - box.left) / Math.max(1, box.width)
      : (event.clientY - box.top) / Math.max(1, box.height);
    setRatio(ratio);
  });
  const finish = () => {
    if (!dragging) return;
    dragging = false;
    delete node.dataset.drag;
    delete document.body.dataset.resizing;
    savePanelLayout();     // 끄는 동안이 아니라 끝난 뒤 한 번만
    marquee.scan(el.dock);
  };
  node.addEventListener('pointerup', finish);
  node.addEventListener('pointercancel', finish);
  node.addEventListener('dblclick', () => { setRatio(0.5); savePanelLayout(); });
  node.addEventListener('keydown', (event) => {
    const step = event.shiftKey ? 0.08 : 0.02;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') { event.preventDefault(); setRatio(split.ratio + step); savePanelLayout(); }
    else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') { event.preventDefault(); setRatio(split.ratio - step); savePanelLayout(); }
    else if (event.key === 'Home') { event.preventDefault(); setRatio(0.5); savePanelLayout(); }
  });

  node.setAttribute('aria-valuemin', '12');
  node.setAttribute('aria-valuemax', '88');
  node.setAttribute('aria-valuenow', String(Math.round(split.ratio * 100)));
  return node;
}

function buildDockGroup(group) {
  if (!group.__gid) group.__gid = `g${++dockGid}`;

  const tabs = h('div', { class: 'dk-tabs', role: 'tablist', 'aria-label': '패널 탭' });
  for (const id of group.panels) tabs.appendChild(buildDockTab(group, id));
  tabs.appendChild(h('button', {
    class: 'dk-add', type: 'button', tip: '닫은 패널 다시 열기', 'aria-label': '패널 추가',
    onClick: (event) => openAddPanelMenu(group, event.currentTarget),
  }, '＋'));

  const body = h('div', { class: 'dk-body' });
  for (const id of group.panels) {
    const node = panelNode(id);
    if (!node) continue;
    node.hidden = id !== group.active;
    body.appendChild(node);
  }

  const wrap = h('div', { class: 'dk-group', dataset: { gid: group.__gid } }, tabs, body);
  group.__el = wrap;
  return wrap;
}

function buildDockTab(group, id) {
  const meta = PANELS[id] || { icon: '·', label: id };
  const tab = h('button', {
    class: 'dk-tab', type: 'button', role: 'tab',
    'aria-selected': String(group.active === id),
    dataset: { panel: id },
    onClick: () => {
      // 끌어서 옮긴 직후에 click이 한 번 더 온다. 그건 탭 전환이 아니다.
      if (tab.__afterDrag) { tab.__afterDrag = false; return; }
      activateDockPanel(group, id);
    },
  },
    h('span', { 'aria-hidden': 'true' }, meta.icon),
    h('span', null, meta.label));

  const close = h('button', {
    class: 'dk-x', type: 'button', tip: `${meta.label} 닫기`, 'aria-label': `${meta.label} 닫기`,
    onClick: (event) => { event.stopPropagation(); closeDockPanel(id); },
  }, '✕');
  tab.appendChild(close);

  bindTabDrag(tab, id, close);
  return tab;
}

function activateDockPanel(group, id) {
  if (group.active === id) return;
  group.active = id;
  for (const tab of group.__el.querySelectorAll('.dk-tab')) {
    tab.setAttribute('aria-selected', String(tab.dataset.panel === id));
  }
  for (const panel of group.panels) {
    const node = panelNode(panel);
    if (node) node.hidden = panel !== id;
  }
  savePanelLayout();
  onPanelShown(id);
}

/** 패널을 열면 그 탭이 필요로 하는 데이터도 같이 챙긴다. */
function onPanelShown(id) {
  if (id === 'chat') { markChatRead(); scrollChatToEnd(false); }
  if (id === 'suggest') loadSuggestions();
  if (id === 'audit') loadAudit();
  if (id === 'lyrics' && lyricsOpen) loadLyrics();
  marquee.scan(el.dock);
  scheduleViz();
}

function closeDockPanel(id) {
  if (!dockTree) return;
  if (openPanels().size <= 1) { toast('마지막 패널까지 닫을 수는 없어요.', 'warn'); return; }
  detachPanel(id);
  if (id === 'lyrics') {
    lyricsOpen = false;
    prefSet('lyricsOpen', '0');
    el.lyricsToggle.setAttribute('aria-expanded', 'false');
    el.lyricsToggle.classList.remove('btn--primary');
  }
  renderDock();
  savePanelLayout();
}

function addDockPanel(id, group) {
  if (!dockTree || !PANELS[id]) return;
  if (openPanels().has(id)) { focusDockPanel(id); return; }
  const target = group && dockTree && findGroup((node) => node === group)
    ? group
    : findGroup(() => true);
  if (!target) return;
  target.panels.push(id);
  target.active = id;
  renderDock();
  savePanelLayout();
  onPanelShown(id);
}

/** 이미 열려 있는 패널을 앞으로 끌어온다. 그리드 배치의 setRailTab/openSide 자리다. */
function focusDockPanel(id) {
  if (!dockTree) return false;
  const group = findGroup((node) => node.panels.includes(id));
  if (!group) { addDockPanel(id); return true; }
  activateDockPanel(group, id);
  return true;
}

function openAddPanelMenu(group, anchor) {
  const open = openPanels();
  const closed = Object.entries(PANELS).filter(([id]) => !open.has(id));
  const menu = h('div', { class: 'pop pop--menu' },
    closed.length
      ? closed.map(([id, meta]) => h('button', {
        class: 'dd__item', type: 'button',
        onClick: () => { menu.remove(); addDockPanel(id, group); },
      }, h('span', null, meta.icon), h('span', null, meta.label)))
      : h('div', { class: 'hint', style: { padding: 'var(--sp-2) var(--sp-3)' } }, '전부 열려 있어요.'));

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - box.width - 8))}px`;
  menu.style.top = `${Math.min(rect.bottom + 4, window.innerHeight - box.height - 8)}px`;
  menu.querySelector('button')?.focus();

  const close = (event) => {
    if (menu.contains(event.target)) return;
    menu.remove();
    document.removeEventListener('pointerdown', close, true);
  };
  setTimeout(() => document.addEventListener('pointerdown', close, true), 0);
}

function resetPanelLayout() {
  dockTree = defaultDockTree();
  savePanelLayout();
  if (panelMode()) renderDock();
  toast('패널 배치를 기본으로 되돌렸어요.', 'ok');
}

/* ── 탭 끌어서 옮기기 ──
 * 드롭 위치는 끌고 있는 동안 반투명 하이라이트로 미리 보여준다. 안 보이면 못 쓴다.
 */

function bindTabDrag(tab, id, closeBtn) {
  tab.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || closeBtn.contains(event.target)) return;
    const startX = event.clientX;
    const startY = event.clientY;
    let armed = false;

    const move = (moveEvent) => {
      if (!armed) {
        if (Math.abs(moveEvent.clientX - startX) + Math.abs(moveEvent.clientY - startY) < 5) return;
        armed = true;
        startTabDrag(id, moveEvent);
      }
      updateTabDrag(moveEvent);
    };
    const up = (upEvent) => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      if (armed) { tab.__afterDrag = true; endTabDrag(upEvent); upEvent.preventDefault(); }
    };
    const cancel = () => {
      document.removeEventListener('pointermove', move);
      document.removeEventListener('pointerup', up);
      document.removeEventListener('pointercancel', cancel);
      cancelTabDrag();
    };
    document.addEventListener('pointermove', move);
    document.addEventListener('pointerup', up);
    document.addEventListener('pointercancel', cancel);
  });
}

function startTabDrag(id, event) {
  const meta = PANELS[id] || { icon: '·', label: id };
  const ghost = h('div', { class: 'dk-ghost' }, h('span', null, meta.icon), h('span', null, meta.label));
  document.body.appendChild(ghost);
  document.body.dataset.dragging = 'tab';
  dockDrag = { id, ghost, target: null, zone: null };
  updateTabDrag(event);
}

function updateTabDrag(event) {
  if (!dockDrag) return;
  dockDrag.ghost.style.left = `${event.clientX + 12}px`;
  dockDrag.ghost.style.top = `${event.clientY + 12}px`;

  dockDrag.ghost.style.visibility = 'hidden';
  const under = document.elementFromPoint(event.clientX, event.clientY);
  dockDrag.ghost.style.visibility = '';
  const groupEl = under?.closest?.('.dk-group');
  if (!groupEl) { hideDropHint(); dockDrag.target = null; return; }

  const group = findGroup((node) => node.__gid === groupEl.dataset.gid);
  if (!group) { hideDropHint(); dockDrag.target = null; return; }

  const box = groupEl.getBoundingClientRect();
  const rx = (event.clientX - box.left) / Math.max(1, box.width);
  const ry = (event.clientY - box.top) / Math.max(1, box.height);
  const edge = 0.24;
  let zone = 'center';
  const nearest = Math.min(rx, 1 - rx, ry, 1 - ry);
  if (nearest < edge) {
    if (nearest === rx) zone = 'left';
    else if (nearest === 1 - rx) zone = 'right';
    else if (nearest === ry) zone = 'top';
    else zone = 'bottom';
  }
  dockDrag.target = group;
  dockDrag.zone = zone;
  showDropHint(groupEl, zone);
}

function showDropHint(groupEl, zone) {
  const overlay = dropOverlay();
  const dockBox = el.dock.getBoundingClientRect();
  const box = groupEl.getBoundingClientRect();
  let left = box.left - dockBox.left;
  let top = box.top - dockBox.top;
  let width = box.width;
  let height = box.height;
  if (zone === 'left') width /= 2;
  else if (zone === 'right') { left += box.width / 2; width /= 2; }
  else if (zone === 'top') height /= 2;
  else if (zone === 'bottom') { top += box.height / 2; height /= 2; }
  overlay.style.left = `${left}px`;
  overlay.style.top = `${top}px`;
  overlay.style.width = `${width}px`;
  overlay.style.height = `${height}px`;
  overlay.dataset.zone = zone;
  overlay.hidden = false;
}

function hideDropHint() {
  if (el.dockDrop) el.dockDrop.hidden = true;
}

function cancelTabDrag() {
  if (!dockDrag) return;
  dockDrag.ghost.remove();
  delete document.body.dataset.dragging;
  hideDropHint();
  dockDrag = null;
}

function endTabDrag() {
  if (!dockDrag) return;
  const { id, target, zone } = dockDrag;
  cancelTabDrag();
  if (!target || !zone) return;

  const source = findGroup((node) => node.panels.includes(id));
  if (source === target && zone === 'center') { activateDockPanel(target, id); return; }
  if (source === target && source.panels.length === 1) return;   // 혼자 있는 패널을 자기 자리에 다시 떨구면 아무 일도 없다

  detachPanel(id);
  // 트리가 접히면서 target이 통째로 사라졌을 수 있다. gid로 다시 찾는다.
  const alive = findGroup((node) => node.__gid === target.__gid);
  if (!alive) { addDockPanel(id); return; }

  if (zone === 'center') {
    alive.panels.push(id);
    alive.active = id;
  } else {
    const fresh = { type: 'tabs', panels: [id], active: id };
    const moved = { type: 'tabs', panels: alive.panels.slice(), active: alive.active, __gid: alive.__gid };
    const first = zone === 'left' || zone === 'top';
    replaceNode(alive, {
      type: 'split',
      dir: zone === 'left' || zone === 'right' ? 'row' : 'col',
      ratio: first ? 0.38 : 0.62,
      a: first ? fresh : moved,
      b: first ? moved : fresh,
    });
  }
  renderDock();
  savePanelLayout();
  onPanelShown(id);
}

async function askNotify() {
  const result = await notify.ask();
  if (result === 'granted') toast('알림을 켰어요. 다른 탭을 보고 있을 때만 울려요.', 'ok');
  else if (result === 'denied') toast('브라우저에서 알림이 막혀 있어요.', 'warn');
  else toast('이 브라우저는 알림을 지원하지 않아요.', 'warn');
  renderProfile();
}

/* ── 메뉴 토글 (공용) ── */

function toggleMenu(menu, button) {
  const open = menu.hidden;
  closeMenus();
  if (!open) return;
  menu.hidden = false;
  button?.setAttribute('aria-expanded', 'true');
  menu.querySelector('button, a')?.focus();
}

function closeMenus() {
  for (const menu of document.querySelectorAll('.dd__menu')) {
    if (!menu.hidden) {
      menu.hidden = true;
      menu.parentElement?.querySelector('[aria-expanded]')?.setAttribute('aria-expanded', 'false');
    }
  }
}

document.addEventListener('pointerdown', (event) => {
  if (!event.target.closest?.('.dd')) closeMenus();
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') closeMenus();
});

/* ═══════════════════════ 좌측 레일 ═══════════════════════ */

let activeRailTab = localStorage.getItem(LS.railTab) || 'queue';

function buildRail() {
  const tabs = [
    { id: 'search', icon: '🔎', label: '검색' },
    { id: 'queue', icon: '📋', label: '대기열' },
    { id: 'library', icon: '📚', label: '보관함' },
  ];
  el.railTabs = tabs.map((tab) => h('button', {
    class: 'tab', type: 'button', role: 'tab', id: `railtab-${tab.id}`,
    'aria-selected': String(tab.id === activeRailTab),
    dataset: { rail: tab.id },
    onClick: () => setRailTab(tab.id),
  }, h('span', { 'aria-hidden': 'true' }, tab.icon), tab.label));

  el.railPanes = {
    search: buildSearchPane(),
    queue: buildQueuePane(),
    library: buildLibraryPane(),
  };
  for (const [id, pane] of Object.entries(el.railPanes)) pane.hidden = id !== activeRailTab;

  el.railBody = h('div', { class: 'pane__body' }, ...Object.values(el.railPanes));
  return h('div', { class: 'pane' },
    h('div', { class: 'pane__tabs', role: 'tablist', 'aria-label': '좌측 패널' }, el.railTabs),
    el.railBody);
}

function setRailTab(id) {
  activeRailTab = id;
  try { localStorage.setItem(LS.railTab, id); } catch { /* 시크릿 모드 */ }
  if (panelMode()) {
    focusDockPanel(id);
    if (id === 'search') el.searchInput?.focus();
    return;
  }
  for (const tab of el.railTabs) tab.setAttribute('aria-selected', String(tab.dataset.rail === id));
  for (const [key, pane] of Object.entries(el.railPanes)) pane.hidden = key !== id;
  if (id === 'search') el.searchInput?.focus();
  if (narrowScreen()) document.body.dataset.pane = 'rail';
  marquee.scan(el.rail);
}

/* ── 검색 ── */

function buildSearchPane() {
  el.searchInput = h('input', {
    class: 'field', type: 'search', 'data-testid': 'search-input',
    placeholder: '곡, 아티스트 또는 링크', autocomplete: 'off', enterkeyhint: 'search',
    onKeydown: (event) => {
      if (event.key === 'Enter') { event.preventDefault(); runSearch(); }
      if (event.key === 'Escape') closeSearch();
    },
    onInput: () => {
      // 검색어가 바뀌면 버튼은 다시 🔎로 돌아온다
      if (el.searchInput.value.trim() !== searchedQuery) syncSearchButton();
    },
  });

  el.searchBtn = bindAct(h('button', {
    class: 'btn btn--primary btn--icon', type: 'button', 'data-testid': 'search-submit',
    tip: '검색', 'aria-label': '검색',
  }, '🔎'), () => {
    if (el.searchBtn.dataset.mode === 'close') { closeSearch(); return; }
    runSearch();
  });

  el.searchProvider = h('select', { class: 'search__provider', tip: '검색할 서비스' },
    h('option', { value: 'YouTube' }, 'YouTube'),
    h('option', { value: 'YouTubeMusic' }, 'YT Music'),
    h('option', { value: 'SoundCloud' }, 'SoundCloud'));

  el.searchResults = h('div', {
    class: 'search__results', 'data-testid': 'search-results', hidden: true,
    role: 'region', 'aria-label': '검색 결과',
  });

  const pane = h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'railtab-search' },
    h('div', { class: 'search' },
      h('div', { class: 'search__row' }, el.searchInput, el.searchProvider, el.searchBtn),
      el.searchResults),
    h('div', { class: 'hint', style: { padding: '0 var(--sp-4)' } },
      '링크를 그대로 붙여넣어도 돼요. 결과는 검색창 바로 아래에 떠요.'));
  return pane;
}

function syncSearchButton() {
  const open = !el.searchResults.hidden && el.searchInput.value.trim() === searchedQuery;
  el.searchBtn.dataset.mode = open ? 'close' : 'search';
  el.searchBtn.textContent = open ? '✕' : '🔎';
  el.searchBtn.setAttribute('data-tip', open ? '결과 닫기' : '검색');
  el.searchBtn.setAttribute('aria-label', open ? '검색 결과 닫기' : '검색');
}

function closeSearch() {
  el.searchResults.hidden = true;
  syncSearchButton();
}

async function runSearch() {
  const query = el.searchInput.value.trim();
  if (!query) { el.searchInput.focus(); return; }
  el.searchResults.hidden = false;
  clear(el.searchResults).appendChild(skeletonRows(4));

  // 브라우저 검색이 가능하면 먼저 시도한다. 실패하면 조용히 서버 검색으로 내려간다.
  if (browserSearchReady(query)) {
    try {
      const results = await youtubeSearch(query, el.searchProvider.value);
      if (results.length) {
        searchResults = results;
        searchedQuery = query;
        searchSource = 'browser';
        renderSearchResults();
        syncSearchButton();
        return;
      }
    } catch (error) {
      console.warn('[search] 브라우저 검색이 실패해서 서버로 넘겨요', error);
    }
  }

  try {
    const data = await api(`/search?q=${encodeURIComponent(query)}&provider=${encodeURIComponent(el.searchProvider.value)}`);
    searchResults = data?.results || [];
    searchedQuery = query;
    searchSource = browserSearchReady(query) ? 'fallback' : 'server';
    renderSearchResults();
  } catch (error) {
    clear(el.searchResults).appendChild(emptyState('⚠', '검색하지 못했어요', error.message));
  }
  syncSearchButton();
}

const SEARCH_SOURCE_NOTE = {
  browser: '이 브라우저에서 바로 찾았어요',
  fallback: '브라우저 검색이 안 돼서 서버에서 찾았어요',
  server: '',
};

function renderSearchResults() {
  clear(el.searchResults);
  const note = SEARCH_SOURCE_NOTE[searchSource] || '';
  el.searchResults.appendChild(h('div', { class: 'search__meta' },
    h('span', null, `${searchResults.length}건`),
    note ? h('span', { class: 'search__src' }, note) : null,
    h('button', { class: 'iconbtn', type: 'button', tip: '닫기', 'aria-label': '검색 결과 닫기', onClick: closeSearch }, '✕')));

  if (!searchResults.length) {
    el.searchResults.appendChild(emptyState('🔍', '결과가 없어요', '다른 단어나 링크로 다시 찾아 보세요.'));
    return;
  }
  for (const track of searchResults) el.searchResults.appendChild(trackRow(track, 'search'));
  marquee.scan(el.searchResults);
}

/* ── 브라우저에서 검색하기 (§6) ──
 * 봇 호스트의 yt-dlp가 느리거나 막히면 검색이 통째로 죽는다. 키가 있으면 브라우저가 직접 묻는다.
 * 키는 어차피 공개되는 값이라 어드민이 HTTP 리퍼러 제한을 걸어 쓰는 전제다.
 */

function searchConfig() {
  return store.get().searchCfg || null;
}

function browserSearchReady(query) {
  const cfg = searchConfig();
  if (!cfg || cfg.mode !== 'browser' || !cfg.youtubeApiKey) return false;
  if (!String(el.searchProvider.value || '').startsWith('YouTube')) return false;
  // 링크는 서버가 그대로 풀어야 한다. 검색 API로는 못 다룬다.
  return !/^https?:\/\//i.test(query);
}

async function youtubeSearch(query, provider) {
  const key = searchConfig().youtubeApiKey;
  const base = 'https://www.googleapis.com/youtube/v3';
  const searchUrl = `${base}/search?part=snippet&type=video&maxResults=10&videoEmbeddable=true`
    + `&q=${encodeURIComponent(query)}&key=${encodeURIComponent(key)}`;

  const found = await fetchJson(searchUrl);
  const items = Array.isArray(found?.items) ? found.items : [];
  const ids = items.map((item) => item?.id?.videoId).filter(Boolean);
  if (!ids.length) return [];

  // search.list는 길이를 안 준다. videos.list로 한 번 더 물어 ISO8601을 초로 바꾼다.
  let durations = new Map();
  try {
    const detail = await fetchJson(`${base}/videos?part=contentDetails&id=${ids.join(',')}&key=${encodeURIComponent(key)}`);
    for (const item of detail?.items || []) durations.set(item.id, iso8601Seconds(item?.contentDetails?.duration));
  } catch (error) {
    // 길이는 없어도 담을 수 있다. 서버가 나중에 채운다.
    console.warn('[search] 길이를 못 가져왔어요', error);
    durations = new Map();
  }

  return items.map((item) => {
    const videoId = item.id.videoId;
    const snippet = item.snippet || {};
    return {
      title: snippet.title || videoId,
      artist: snippet.channelTitle || '',
      provider: provider === 'YouTubeMusic' ? 'YouTubeMusic' : 'YouTube',
      contentId: videoId,
      cacheKey: `${provider === 'YouTubeMusic' ? 'YouTubeMusic' : 'YouTube'}:${videoId}`,
      durationSeconds: durations.get(videoId) || 0,
      artUrl: snippet.thumbnails?.medium?.url || snippet.thumbnails?.default?.url || null,
    };
  });
}

async function fetchJson(url) {
  const response = await fetch(url, { credentials: 'omit', mode: 'cors' });
  if (!response.ok) {
    let reason = `${response.status}`;
    try {
      const body = await response.json();
      reason = body?.error?.message || reason;
    } catch { /* 본문이 JSON이 아닐 수도 있다 */ }
    throw new Error(reason);
  }
  return response.json();
}

/** PT1H2M3S → 3723 */
function iso8601Seconds(text) {
  const match = /^P(?:\d+D)?T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$/.exec(String(text || ''));
  if (!match) return 0;
  return (Number(match[1]) || 0) * 3600 + (Number(match[2]) || 0) * 60 + (Number(match[3]) || 0);
}

/** 검색결과·최근·보관함이 공유하는 트랙 행 */
function trackRow(track, source, extra) {
  const add = bindAct(h('button', { class: 'iconbtn', type: 'button', tip: '대기열에 담기', 'aria-label': '대기열에 담기' }, '＋'),
    () => enqueue(track));
  setLock(add, !can('search'), lockReason('search'));

  const save = bindAct(h('button', {
    class: 'iconbtn', type: 'button',
    tip: source === 'saved' ? '보관함에서 빼기' : '보관함에 담기',
    'aria-label': source === 'saved' ? '보관함에서 빼기' : '보관함에 담기',
  }, source === 'saved' ? '🗑' : '🔖'), () => toggleSaved(track, source !== 'saved'));
  setLock(save, !can('library'), lockReason('library'));

  return h('div', { class: 'row', dataset: { mqRow: '1' } },
    h('img', { class: 'row__art', src: artUrl(track) || '', alt: '', loading: 'lazy' }),
    h('div', { class: 'row__main' },
      mqText(trackTitle(track), 'row__title'),
      h('div', { class: 'row__sub' }, extra || trackSub(track))),
    h('div', { class: 'row__acts' }, add, seedButton(track), save));
}

async function enqueue(track) {
  const key = trackKey(track);
  const dup = store.get().queue.find((item) => trackKey(item.track) === key);
  if (dup) {
    toast('이미 대기열에 있는 곡이에요.', 'warn');
    setRailTab('queue');
    flashQueueItem(dup.id);
    return;
  }
  const result = await call(() => api('/queue', { body: { track } }));
  if (!result) return;
  toast(result.playingNow ? '바로 재생을 시작했어요.' : `대기열 ${result.queuePosition ?? '?'}번에 담았어요.`, 'ok');
  closeSearch();
}

async function toggleSaved(track, present) {
  await call(() => api('/library', { body: { track, kind: 'saved', present } }),
    present ? '보관함에 담았어요.' : '보관함에서 뺐어요.');
  refetchCold();
}

/* ── 대기열 ── */

function buildQueuePane() {
  el.queueCount = h('span', { class: 'queue__count' });
  el.modeBadge = h('span', { class: 'modebadge' });
  el.sortTick = h('span', {
    class: 'sorttick', hidden: true, role: 'timer',
    tip: '서버가 5초마다 대기열을 다시 정렬해요. 0이 되면 순서가 움직여요.',
  });
  el.queueList = h('div', { class: 'queue__list scroll', 'data-testid': 'queue-list' });

  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'railtab-queue' },
    h('div', { class: 'queue__head' },
      h('h2', null, '대기열'),
      el.queueCount,
      h('span', { class: 'queue__spacer' }),
      el.sortTick,
      el.modeBadge),
    buildSeedBox(),
    el.queueList);
}

function renderQueueHead(state) {
  el.queueCount.textContent = `${state.queue.length}곡`;
  const mode = MODES[state.queueMode] || MODES.score;
  clear(el.modeBadge);
  el.modeBadge.appendChild(document.createTextNode(`${mode.icon} ${mode.label}`));
  el.modeBadge.appendChild(h('button', {
    class: 'modebadge__i', type: 'button', tip: '정렬 방식 3종 비교',
    'aria-label': '정렬 방식 설명', onClick: openModeSheet,
  }, 'ⓘ'));
  renderSortTick();
}

function renderQueue(state) {
  renderQueueHead(state);
  const items = state.queue;
  if (!items.length) {
    list.reset(el.queueList);
    el.queueList.appendChild(state.hotAt
      ? emptyState('🎧', '대기열이 비었어요', '검색해서 다음 곡을 담아 보세요.')
      : skeletonRows(4));
    return;
  }
  const rounds = computeRounds(items);
  list(el.queueList, items, (item) => item.id, createQueueItem, (node, item, index) => updateQueueItem(node, item, index, rounds));
  marquee.scan(el.queueList);
}

/* ── 대기열 갱신 카운트다운 (§5) ──
 * 순서가 갑자기 바뀌면 왜 움직였는지 알 수가 없다. 남은 초를 미리 보여줘서 인과를 눈에 보이게 한다.
 * 기준은 서버 시각(nextSortAt)이다. 클라 타이머만 쓰면 백그라운드에 갔다 오는 순간 어긋난다.
 */

/** 서버 시각 − 내 시계. 서버가 준 표본 시각으로 계속 보정한다. */
function noteServerTime(utc) {
  const ms = parseUtc(utc);
  if (!ms) return;
  const skew = ms - Date.now();
  // 몇 분씩 어긋난 값은 시계가 이상한 것이지 지연이 아니다. 그럴 땐 보정을 포기한다.
  serverSkewMs = Math.abs(skew) < 120000 ? skew : 0;
}

function serverNow() {
  return Date.now() + serverSkewMs;
}

/** 다음 재정렬까지 남은 초. fifo이거나 기준 시각을 모르면 null. */
function sortRemainSeconds() {
  const state = store.get();
  if (state.queueMode === 'fifo') return null;
  if (!state.queue.length) return null;

  let target = parseUtc(state.nextSortAt);
  if (!target) {
    const sorted = parseUtc(state.sortedAt);
    if (!sorted) return null;
    // 서버가 nextSortAt을 아직 안 보내면 5초 주기를 가정해 다음 경계를 만든다
    target = sorted + 5000;
    if (target <= serverNow()) target += Math.ceil((serverNow() - target) / 5000) * 5000;
  }
  const left = target - serverNow();
  if (left > 60000) return null;         // 말도 안 되는 값이면 아예 숨긴다
  return Math.max(0, Math.min(9, Math.ceil(left / 1000)));
}

function renderSortTick() {
  if (!el.sortTick) return;
  const left = sortRemainSeconds();
  if (left === null) { el.sortTick.hidden = true; return; }
  el.sortTick.hidden = false;
  clear(el.sortTick).append(
    h('span', { class: 'sorttick__label' }, '갱신'),
    h('b', null, String(left)));
  el.sortTick.setAttribute('aria-label', `${left}초 뒤에 대기열이 다시 정렬돼요`);
}

function startSortTick() {
  setInterval(() => { if (!document.hidden) renderSortTick(); }, 250);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) renderSortTick(); });
}

/* ── 자동 재생 기준 곡 (§8.5) ──
 * 서버가 이 API를 모르면(404) 섹션과 버튼을 통째로 숨긴다. 새 기능이 실패해도 기본 동작은 살아 있어야 한다.
 */

function buildSeedBox() {
  el.seedCount = h('span', { class: 'chip' }, '0곡');
  el.seedToggle = h('button', {
    class: 'seedbox__head', type: 'button', 'aria-expanded': 'false',
    tip: '자동 재생이 이 곡들과 비슷한 노래를 골라 와요',
    onClick: () => { seedOpen = !seedOpen; renderSeeds(); },
  },
    h('span', { class: 'seedbox__caret', 'aria-hidden': 'true' }, '▸'),
    h('span', null, '📻 자동 재생 기준 곡'),
    h('span', { class: 'queue__spacer' }),
    el.seedCount);
  el.seedBody = h('div', { class: 'seedbox__body', hidden: true });
  el.seedBox = h('section', { class: 'seedbox', hidden: true }, el.seedToggle, el.seedBody);
  return el.seedBox;
}

async function loadSeeds() {
  try {
    const data = await api('/autoplay/seeds');
    seedState = {
      seeds: Array.isArray(data?.seeds) ? data.seeds : [],
      max: Number(data?.max) || 10,
      canEdit: !!data?.canEdit,
    };
  } catch (error) {
    // 아직 없는 기능이면 조용히 접는다. 다른 이유면 마지막으로 받은 목록을 그대로 둔다.
    if (!error || error.status === 404 || error.status === 501) seedState = null;
  }
  renderSeeds();
}

function renderSeeds() {
  if (!el.seedBox) return;
  // 최종 판정은 서버의 canEdit(권한 키 autoplaySeed)이다. 여기서는 화면 상태만 덧대 막는다.
  const state = store.get();
  const blocked = state.conn === 'down' || state.tier === 'viewer'
    || !!(state.suspension && (state.suspension.scope === 'all' || state.suspension.scope === 'queue'));
  const editable = !!(seedState && seedState.canEdit && !blocked);
  el.portal?.setAttribute('data-seeds', editable ? '1' : '0');

  if (!seedState) { el.seedBox.hidden = true; return; }
  el.seedBox.hidden = false;
  el.seedToggle.setAttribute('aria-expanded', String(seedOpen));
  el.seedToggle.dataset.open = seedOpen ? '1' : '0';
  el.seedCount.textContent = `${seedState.seeds.length} / ${seedState.max}곡`;
  el.seedBody.hidden = !seedOpen;
  if (!seedOpen) return;

  clear(el.seedBody);
  if (!seedState.seeds.length) {
    el.seedBody.appendChild(h('p', { class: 'hint' },
      '기준 곡이 없어서 최근에 튼 곡을 참고해요. 곡 옆의 📻를 누르면 여기에 쌓여요.'));
    return;
  }
  for (const seed of seedState.seeds) {
    const track = seed.track || {};
    const remove = seedState.canEdit
      ? bindAct(h('button', {
        class: 'iconbtn iconbtn--danger', type: 'button',
        tip: '기준 곡에서 빼기', 'aria-label': '기준 곡에서 빼기',
      }, '✕'), () => removeSeed(seed.cacheKey))
      : null;
    el.seedBody.appendChild(h('div', { class: 'seedrow', dataset: { mqRow: '1' } },
      h('img', { class: 'row__art', src: artUrl(track) || '', alt: '', loading: 'lazy' }),
      h('div', { class: 'row__main' },
        mqText(trackTitle(track), 'row__title'),
        h('div', { class: 'row__sub' }, [seed.addedByDisplayName, fmtAgo(seed.addedUtc)].filter(Boolean).join(' · '))),
      remove));
  }
  el.seedBody.appendChild(h('p', { class: 'hint' },
    `기준 곡을 돌아가며 참고해서 다음 곡을 골라요. ${seedState.max}곡까지 넣을 수 있어요.`));
  marquee.scan(el.seedBody);
}

/** 대기열·검색 결과에 붙는 '기준으로 삼기'. 권한이 없으면 CSS가 통째로 숨긴다. */
function seedButton(track, wide) {
  if (!track) return null;
  return bindAct(h('button', {
    class: wide ? 'vote seedbtn' : 'iconbtn seedbtn', type: 'button',
    tip: '📻 기준으로 삼기 — 자동 재생이 이 곡과 비슷한 노래를 골라 와요',
    'aria-label': '기준으로 삼기',
  }, wide ? '📻 기준' : '📻'), () => addSeed(track));
}

async function addSeed(track) {
  if (!seedState) return;
  const result = await call(() => api('/autoplay/seeds', { body: { track } }), '자동 재생 기준 곡에 담았어요.');
  if (result) { seedOpen = true; loadSeeds(); }
}

async function removeSeed(cacheKey) {
  const result = await call(() => api('/autoplay/seeds/remove', { body: { cacheKey } }), '기준 곡에서 뺐어요.');
  if (result) loadSeeds();
}

/** 서버가 round를 안 주면 신청자별 순번을 클라이언트가 센다. */
function computeRounds(items) {
  const seen = new Map();
  const rounds = new Map();
  for (const item of items) {
    const who = String(item.requestedByUserId ?? item.requestedByDisplay ?? '');
    const next = (seen.get(who) || 0) + 1;
    seen.set(who, next);
    rounds.set(item.id, item.round || next);
  }
  return rounds;
}

function createQueueItem(item) {
  const node = h('article', { class: 'qitem', 'data-testid': 'queue-item', dataset: { mqRow: '1', id: item.id } });
  node.__parts = {
    rank: h('div', { class: 'qitem__rank' }),
    art: h('img', { class: 'qitem__art', alt: '', loading: 'lazy' }),
    title: mqText('', 'qitem__title'),
    who: h('div', { class: 'qitem__who' }),
    score: h('div', { class: 'score' }),
    acts: h('div', { class: 'qitem__acts' }),
  };
  const p = node.__parts;
  node.append(p.rank, p.art, h('div', { class: 'qitem__main' }, p.title, p.who, p.score), p.acts);

  p.like = bindAct(h('button', { class: 'vote', type: 'button', tip: '좋아요' }), () => vote(node.dataset.id, 'like'));
  p.superLike = bindAct(h('button', { class: 'vote', type: 'button', tip: '슈퍼 좋아요 (2배)' }), () => vote(node.dataset.id, 'superLike'));
  p.save = bindAct(h('button', { class: 'vote', type: 'button', tip: '보관함에 담기', 'aria-label': '보관함에 담기' }, '🔖'),
    () => toggleSaved(node.__item.track, true));
  p.seed = bindAct(h('button', {
    class: 'vote seedbtn', type: 'button',
    tip: '📻 기준으로 삼기 — 자동 재생이 이 곡과 비슷한 노래를 골라 와요', 'aria-label': '기준으로 삼기',
  }, '📻 기준'), () => addSeed(node.__item.track));
  p.pin = bindAct(h('button', { class: 'vote', type: 'button', tip: '관리자 우선으로 올리기', 'aria-label': '관리자 우선' }, '📌'),
    () => call(() => api('/queue/action', { body: { action: 'togglePin', itemId: node.dataset.id } })));
  p.remove = bindAct(h('button', { class: 'vote vote--danger', type: 'button', tip: '대기열에서 빼기', 'aria-label': '대기열에서 빼기' }, '✕'),
    async () => {
      const ok = await confirmSheet({
        title: '이 곡을 뺄까요', desc: trackTitle(node.__item.track), danger: true, confirmText: '빼기',
      });
      if (ok) call(() => api('/queue/action', { body: { action: 'remove', itemId: node.dataset.id } }), '대기열에서 뺐어요.');
    });
  p.acts.append(p.like, p.superLike, p.save, p.seed, p.pin, p.remove);
  return node;
}

function updateQueueItem(node, item, index, rounds) {
  node.__item = item;
  node.dataset.id = item.id;
  const p = node.__parts;
  const state = store.get();
  const mode = state.queueMode;
  const score = item.score || {};

  node.classList.toggle('qitem--mine', !!item.isMine);
  node.classList.toggle('qitem--pinned', score.manualPriority !== null && score.manualPriority !== undefined);

  p.rank.textContent = String(index + 1);
  const art = artUrl(item.track);
  if (p.art.getAttribute('src') !== art) p.art.setAttribute('src', art);

  const titleInner = p.title.firstElementChild;
  const title = trackTitle(item.track);
  if (titleInner.textContent !== title) titleInner.textContent = title;

  const round = rounds.get(item.id) || 1;
  put(clear(p.who),
    h('b', null, item.requestedByDisplay || '알 수 없음'),
    `의 ${round}번째 곡`,
    item.isMine ? h('span', { class: 'chip chip--accent', style: { marginLeft: 'var(--sp-2)' } }, '내 곡') : null);

  renderScore(p.score, score, mode);

  const like = score.likeCount || 0;
  const superLike = score.superLikeCount || 0;
  p.like.setAttribute('aria-pressed', String(item.myVote === 'like'));
  p.superLike.setAttribute('aria-pressed', String(item.myVote === 'superLike'));
  p.like.textContent = `👍 ${like}`;
  p.superLike.textContent = `⭐ ${superLike}`;

  const canVote = can('vote') && !item.isMine;
  setLock(p.like, !canVote, item.isMine ? '내가 신청한 곡에는 좋아요를 누를 수 없어요.' : lockReason('vote'));
  setLock(p.superLike, !canVote, item.isMine ? '내가 신청한 곡에는 좋아요를 누를 수 없어요.' : lockReason('vote'));
  setLock(p.save, !can('library'), lockReason('library'));

  p.pin.hidden = !can('queueEdit') || tierOf() === 'member';
  p.pin.setAttribute('aria-pressed', String(score.manualPriority !== null && score.manualPriority !== undefined));
  const canRemove = item.isMine ? can('queueEdit') || can('search') : can('queueEdit');
  setLock(p.remove, !canRemove, lockReason('queueEdit'));
}

/** 시그니처 — 점수를 숫자 하나로 숨기지 않고 계산식과 막대로 보여준다. */
function renderScore(host, score, mode) {
  const like = score.likeCount || 0;
  const superLike = score.superLikeCount || 0;
  const wait = score.waitScore || 0;
  const total = score.totalScore ?? (wait + like + superLike * 2);

  clear(host);
  host.classList.toggle('score--muted', mode === 'fifo');

  const bar = h('div', { class: 'score__bar', 'aria-hidden': 'true' });
  const sum = Math.max(1, like + superLike * 2 + wait);
  for (const [kind, value] of [['like', like], ['super', superLike * 2], ['wait', wait]]) {
    if (value <= 0) continue;
    bar.appendChild(h('span', { class: `score__seg score__seg--${kind}`, style: { flex: String(value / sum) } }));
  }
  if (!bar.children.length) bar.appendChild(h('span', { class: 'score__seg score__seg--wait', style: { flex: '1', opacity: '0.3' } }));

  const parts = [];
  if (like) parts.push(`👍${like}`);
  if (superLike) parts.push(`⭐${superLike}×2`);
  if (wait) parts.push(`대기${wait}`);

  const text = h('span', { class: 'score__text' });
  // 합계는 절대 잘리면 안 된다. 계산식만 줄어들고 '= 7'은 따로 고정한다.
  if (mode === 'fifo') {
    text.textContent = parts.length ? `${parts.join(' ')} · 순서에는 영향 없어요` : '신청한 순서대로 나가요';
    put(host, bar, text);
  } else if (parts.length) {
    text.textContent = parts.join(' + ');
    put(host, bar, text, h('b', { class: 'score__total' }, `= ${total}`));
  } else {
    text.textContent = '0점 · 방금 담겼어요';
    put(host, bar, text);
  }
  host.setAttribute('data-tip', mode === 'fifo'
    ? '시간제에서는 좋아요가 순서를 바꾸지 않아요.'
    : `총 ${total}점 — 대기 ${wait} + 좋아요 ${like} + 슈퍼 ${superLike}×2`);
}

async function vote(itemId, kind) {
  const item = store.get().queue.find((row) => row.id === itemId);
  const next = item && item.myVote === kind ? null : kind;
  await call(() => api('/vote', { body: { itemId, kind: next } }));
}

function flashQueueItem(itemId) {
  flashNode(el.queueList.querySelector(`[data-id="${CSS.escape(String(itemId))}"]`));
}

/** 어디에 있는지 눈으로 찾게 해준다 — 스크롤 + 잠깐 강조 */
function flashNode(node) {
  if (!node) return;
  node.scrollIntoView({ block: 'center', behavior: prefersReduced() ? 'auto' : 'smooth' });
  node.classList.remove('flash');
  void node.offsetWidth;
  node.classList.add('flash');
  setTimeout(() => node.classList.remove('flash'), 1400);
}

/* ── 보관함 ── */

function buildLibraryPane() {
  el.libFilter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'library-filter', placeholder: '보관함에서 찾기',
    onInput: debounce(() => { libraryQuery = el.libFilter.value.trim(); renderLibrary(store.get()); }, 140),
  });
  el.libSeg = h('div', { class: 'lib__seg' },
    ...[['liked', '좋아요'], ['saved', '보관'], ['playlists', '재생목록']].map(([id, label]) =>
      h('button', {
        class: 'seg', type: 'button', 'aria-pressed': String(id === libraryTab), dataset: { seg: id },
        onClick: () => {
          libraryTab = id;
          for (const btn of el.libSeg.children) btn.setAttribute('aria-pressed', String(btn.dataset.seg === id));
          renderLibrary(store.get());
        },
      }, label)));
  el.libBody = h('div', { class: 'lib__body scroll' });

  return h('div', { class: 'tabpane lib', role: 'tabpanel', 'aria-labelledby': 'railtab-library' },
    h('div', { class: 'lib__filter' }, el.libFilter),
    el.libSeg,
    el.libBody);
}

function renderLibrary(state) {
  clear(el.libBody);
  const needle = libraryQuery.toLowerCase();
  const match = (track) => !needle || [track.title, track.artist, track.provider].join(' ').toLowerCase().includes(needle);

  if (libraryTab === 'playlists') {
    if (!state.playlists.length) {
      el.libBody.appendChild(emptyState('📁', '재생목록이 없어요', '자주 듣는 곡을 묶어두면 한 번에 담을 수 있어요.'));
      return;
    }
    for (const playlist of state.playlists) el.libBody.appendChild(playlistCard(playlist));
    marquee.scan(el.libBody);
    return;
  }

  const items = (libraryTab === 'liked' ? state.liked : state.saved).filter((row) => match(row.track || row));
  if (!items.length) {
    el.libBody.appendChild(emptyState(libraryTab === 'liked' ? '👍' : '🔖',
      libraryQuery ? '찾는 곡이 없어요' : (libraryTab === 'liked' ? '좋아요한 곡이 없어요' : '보관한 곡이 없어요'),
      libraryQuery ? '다른 단어로 찾아 보세요.' : '곡 옆의 🔖를 누르면 여기에 쌓여요.'));
    return;
  }
  for (const row of items) el.libBody.appendChild(trackRow(row.track || row, libraryTab));
  marquee.scan(el.libBody);
}

function playlistCard(playlist) {
  const entries = (playlist.entries || []).filter((entry) => entry.track);
  const enqueueAll = bindAct(h('button', { class: 'btn btn--sm', type: 'button', tip: '이 목록을 통째로 대기열에 담아요' }, '전체 담기'),
    () => call(() => api('/playlists/action', { body: { action: 'enqueue', playlistId: playlist.id } }), '재생목록을 대기열에 담았어요.'));
  setLock(enqueueAll, !can('search'), lockReason('search'));

  return h('div', { class: 'card plcard', 'data-testid': 'playlist-card', dataset: { mqRow: '1' } },
    h('div', { class: 'plcard__head' },
      h('strong', null, playlist.name),
      h('span', { class: 'chip' }, `${playlist.entryCount ?? entries.length}곡`),
      enqueueAll),
    h('div', { class: 'plcard__entries' },
      entries.slice(0, 5).map((entry) => h('div', { class: 'row__sub' }, `· ${trackTitle(entry.track)}`)),
      entries.length > 5 ? h('div', { class: 'row__sub' }, `· 외 ${entries.length - 5}곡`) : null));
}

/* ═══════════════════════ 중앙 스테이지 ═══════════════════════ */

/** 재생 중인 곡이 없을 때 쓰는 자리표시 아트. 외부 요청 없이 인라인으로 그린다. */
const ART_PLACEHOLDER =
  'data:image/svg+xml;charset=utf-8,' +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 240">' +
      '<rect width="240" height="240" fill="none"/>' +
      '<g fill="currentColor" opacity="0.22" transform="translate(88 78)">' +
      '<path d="M60 0 22 9v46.5a19 19 0 1 0 8 15.5V21l30-7.2z"/>' +
      '</g></svg>',
  );

function buildStage() {
  el.nowArt = h('img', { class: 'now__art', alt: '', loading: 'eager', src: ART_PLACEHOLDER });
  el.nowEyebrow = h('div', { class: 'now__eyebrow' });
  el.nowTitle = mqText('', 'now__title');
  el.nowBy = h('div', { class: 'now__by' });
  el.viz = h('canvas', { class: 'viz', 'aria-hidden': 'true' });

  el.seekFill = h('span', { class: 'seek__fill' });
  el.seekKnob = h('span', { class: 'seek__knob' });
  el.seekTrack = h('div', {
    class: 'seek__track', 'data-testid': 'seek-bar', role: 'slider', tabindex: '0',
    'aria-label': '재생 위치', 'aria-valuemin': '0', 'aria-valuenow': '0',
  }, h('div', { class: 'seek__rail' }, el.seekFill, el.seekKnob));
  el.timeNow = h('span', null, '0:00');
  el.timeEnd = h('span', null, '0:00');

  el.playBtn = bindAct(h('button', {
    class: 'pbtn pbtn--main', type: 'button', 'data-testid': 'play-pause', tip: '재생 / 일시정지', 'data-tip-key': 'Space',
    'aria-label': '재생 / 일시정지',
  }, '▶'), () => control(store.get().player?.isPaused ? 'resume' : 'pause'));

  el.skipBtn = bindAct(h('button', {
    class: 'pbtn', type: 'button', 'data-testid': 'skip', tip: '다음 곡', 'aria-label': '다음 곡',
  }, '⏭'), () => control('skip'));

  el.restartBtn = bindAct(h('button', {
    class: 'pbtn', type: 'button', tip: '처음부터 다시', 'aria-label': '처음부터 다시',
  }, '⏮'), () => control('seek', 0));

  el.repeatBtn = bindAct(h('button', { class: 'pbtn', type: 'button', tip: '반복', 'aria-label': '반복' }, '🔁'),
    () => {
      const order = ['off', 'queue', 'track'];
      const current = String(store.get().player?.repeatMode || 'off').toLowerCase();
      control('repeat', null, { mode: order[(order.indexOf(current) + 1) % order.length] });
    });

  el.shuffleBtn = bindAct(h('button', { class: 'pbtn', type: 'button', tip: '무작위 섞기', 'aria-label': '무작위 섞기' }, '🎲'),
    () => control('shuffle', store.get().player?.shuffleEnabled ? 0 : 1));

  el.volume = h('input', {
    class: 'vol__range', type: 'range', 'data-testid': 'volume', min: '0', max: '200', value: '100',
    'aria-label': '서버 볼륨 (모두에게 적용돼요)',
    onInput: () => { el.volumeLabel.textContent = `${el.volume.value}%`; },
    onChange: () => control('volume', Number(el.volume.value)),
  });
  el.volumeLabel = h('span', { class: 'vol__label' }, '100%');
  el.volumeWrap = h('div', {
    class: 'vol vol--server',
    tip: '서버 볼륨이에요. 바꾸면 Discord로 듣는 모든 사람에게 같이 적용돼요.',
  },
    h('span', { class: 'vol__tag' }, '🔊 서버 볼륨(모두)'),
    el.volume, el.volumeLabel);

  buildWebPlayback();

  el.lyricsToggle = h('button', {
    class: 'btn btn--sm btn--ghost', type: 'button', 'data-testid': 'lyrics-toggle', tip: '가사 열기/접기',
    'aria-expanded': 'false',
  }, '가사');
  el.lyricsToggle.addEventListener('click', toggleLyrics);
  el.lyricsView = h('div', { class: 'lyrics__view' });
  el.lyricsMode = h('button', { class: 'btn btn--sm btn--ghost', type: 'button', tip: '전체 가사 / 따라가기' }, '전체 보기');
  el.lyricsMode.addEventListener('click', () => { lyricsFull = !lyricsFull; el.lyricsMode.textContent = lyricsFull ? '따라가기' : '전체 보기'; renderLyrics(); });
  el.lyricsBox = h('div', { class: 'panel lyrics', hidden: true },
    h('div', { class: 'lyrics__head' }, h('h2', null, '가사'), el.lyricsMode,
      h('button', { class: 'iconbtn', type: 'button', tip: '접기', 'aria-label': '가사 접기', onClick: toggleLyrics }, '✕')),
    el.lyricsView);

  el.nowCard = h('section', { class: 'now', 'data-testid': 'now-playing', dataset: { mqRow: '1' } },
    h('div', { class: 'now__artwrap' }, el.nowArt),
    h('div', { class: 'now__side' },
      el.nowEyebrow,
      el.nowTitle,
      el.nowBy,
      el.viz,
      h('div', { class: 'seek' }, el.seekTrack, h('div', { class: 'seek__times' }, el.timeNow, el.timeEnd)),
      h('div', { class: 'ctrl' },
        el.restartBtn, el.playBtn, el.skipBtn,
        el.repeatBtn, el.shuffleBtn,
        h('span', { class: 'ctrl__spacer' }),
        el.lyricsToggle,
        el.webBtn),
      h('div', { class: 'vols' }, el.volumeWrap, el.webVolWrap, el.webNote)));

  bindSeek();
  el.stageScroll = h('div', { class: 'stage__scroll scroll' }, el.nowCard, el.lyricsBox);
  return el.stageScroll;
}

async function control(action, value, extra) {
  const state = store.get();
  await call(() => api('/control', {
    body: Object.assign({ action, value: value ?? null, expectedItemId: state.current?.id || null }, extra),
  }));
}

/* ═══════════════════════ 웹에서 듣기 (§9) ═══════════════════════
 * 서버는 오디오를 한 바이트도 나르지 않는다. 브라우저가 YouTube에서 직접 받아 재생하고,
 * 위치·곡 정보만 서버에서 받아 봇을 따라간다. 그래서 서버 추가 부담이 0이다.
 * 듣기 전용이라 플레이어 UI는 안 보여준다 — 여기서 조작해도 봇은 꿈쩍하지 않는다.
 */

const WEB_SYNC_GAP = 2;        // 봇과 이만큼 벌어지면 조용히 맞춘다 (초)

/* 브라우저 자동재생 정책 때문에 새로고침 뒤에는 반드시 사용자가 한 번 눌러야 한다.
 * 그래서 "켜 두겠다는 뜻"(webWanted)과 "지금 켜져 있다"(webOn)를 나눠 둔다. */
const webWanted = prefGet('webPlayback') === '1';
let webOn = false;
let webVolume = clampVolume(Number(prefGet('webVolume')));
let ytApiPromise = null;
let ytPlayer = null;
let ytReady = false;
let webVideoId = null;
let webTimer = 0;
let webBlocked = '';           // 외부 스크립트를 못 불러왔을 때의 이유

function clampVolume(value) {
  const n = Number.isFinite(value) ? value : 60;
  return Math.round(Math.min(100, Math.max(0, n)));
}

function buildWebPlayback() {
  el.webBtn = bindAct(h('button', {
    class: 'btn btn--sm btn--ghost webbtn', type: 'button', 'aria-pressed': 'false',
    tip: '이 브라우저에서도 같은 곡을 들어요. 나만 들리고 Discord 쪽은 그대로예요.',
  }, '🔊 웹에서 듣기'), toggleWebPlayback);

  el.webVol = h('input', {
    class: 'vol__range', type: 'range', min: '0', max: '100', value: String(webVolume),
    'aria-label': '내 볼륨 (나에게만 적용돼요)',
    onInput: () => {
      webVolume = clampVolume(Number(el.webVol.value));
      el.webVolLabel.textContent = `${webVolume}%`;
      try { ytPlayer?.setVolume(webVolume); } catch { /* 아직 준비 전 */ }
      prefSet('webVolume', String(webVolume));
    },
  });
  el.webVolLabel = h('span', { class: 'vol__label' }, `${webVolume}%`);
  el.webVolWrap = h('div', {
    class: 'vol vol--me', hidden: true,
    tip: '내 브라우저에서만 적용되는 볼륨이에요. 다른 사람에게는 영향이 없어요.',
  },
    h('span', { class: 'vol__tag' }, '🎧 내 볼륨(나만)'),
    el.webVol, el.webVolLabel);

  el.webNote = h('div', { class: 'webnote', hidden: true, role: 'status' });

  // 숨긴 1×1 플레이어. 화면에 안 보이지만 소리는 난다.
  el.webHost = h('div', { class: 'webhost', 'aria-hidden': 'true' }, h('div', { id: 'macham-yt' }));
  document.body.appendChild(el.webHost);

  window.addEventListener('pagehide', stopWebPlayback);
}

/** 외부 스크립트라 실패할 수 있다. 실패하면 이유를 남기고 토글을 잠근다. */
function loadYouTubeApi() {
  if (ytApiPromise) return ytApiPromise;
  ytApiPromise = new Promise((resolve, reject) => {
    if (window.YT && window.YT.Player) { resolve(window.YT); return; }
    const timer = setTimeout(() => reject(new Error('유튜브 플레이어가 12초 안에 응답하지 않았어요.')), 12000);
    const previous = window.onYouTubeIframeAPIReady;
    window.onYouTubeIframeAPIReady = () => {
      clearTimeout(timer);
      try { previous?.(); } catch { /* 남의 콜백은 남의 사정 */ }
      resolve(window.YT);
    };
    const script = document.createElement('script');
    script.src = 'https://www.youtube.com/iframe_api';
    script.async = true;
    script.onerror = () => {
      clearTimeout(timer);
      reject(new Error('유튜브 스크립트를 불러오지 못했어요. 네트워크나 차단 확장 프로그램을 확인해 주세요.'));
    };
    document.head.appendChild(script);
  });
  return ytApiPromise;
}

function createYtPlayer() {
  return new Promise((resolve, reject) => {
    if (ytPlayer) { resolve(ytPlayer); return; }
    try {
      ytPlayer = new window.YT.Player('macham-yt', {
        height: '1', width: '1',
        playerVars: { autoplay: 0, controls: 0, disablekb: 1, playsinline: 1, rel: 0, origin: location.origin },
        events: {
          onReady: () => {
            ytReady = true;
            try { ytPlayer.setVolume(webVolume); } catch { /* 무시 */ }
            resolve(ytPlayer);
          },
          onError: (event) => onWebError(event?.data),
        },
      });
    } catch (error) {
      reject(error);
    }
  });
}

async function toggleWebPlayback() {
  if (webBlocked) { toast(webBlocked, 'warn'); return; }
  if (webOn) {
    webOn = false;
    prefSet('webPlayback', '0');
    stopWebPlayback();
    syncWebUi();
    toast('웹에서 듣기를 껐어요. Discord 쪽은 그대로 재생 중이에요.', 'ok');
    return;
  }

  // 토글을 누르는 행위 자체가 사용자 제스처다. 자동재생 정책을 통과하는 유일한 타이밍이라 여기서 다 한다.
  webOn = true;
  prefSet('webPlayback', '1');
  syncWebUi();
  setWebNote('플레이어를 준비하고 있어요…');
  try {
    await loadYouTubeApi();
    await createYtPlayer();
  } catch (error) {
    webOn = false;
    prefSet('webPlayback', '0');
    webBlocked = `${error.message || '유튜브 플레이어를 불러오지 못했어요.'} 지금은 Discord로만 들을 수 있어요.`;
    setLock(el.webBtn, true, webBlocked);
    syncWebUi();
    setWebNote(webBlocked);
    toast(webBlocked, 'warn');
    return;
  }
  startWebLoop();
  syncWebNow(true);
  toast('웹에서 듣기를 켰어요. 봇이 있는 위치에 맞춰 재생할게요.', 'ok');
}

function stopWebPlayback() {
  clearInterval(webTimer);
  webTimer = 0;
  webVideoId = null;
  try { ytPlayer?.stopVideo?.(); } catch { /* 이미 정리됨 */ }
}

function startWebLoop() {
  clearInterval(webTimer);
  webTimer = setInterval(webTick, 1500);
}

function webVideoOf(track) {
  if (!track) return null;
  if (!String(track.provider || '').startsWith('YouTube')) return null;
  return track.contentId || null;
}

/** 곡이 바뀌거나 일시정지가 바뀌면 부른다. force면 위치까지 다시 맞춘다. */
function syncWebNow(force) {
  if (!webOn || !ytReady || !ytPlayer) return;
  const state = store.get();
  const current = state.current;
  const videoId = webVideoOf(current?.track);

  if (!current) {
    stopVideoQuietly();
    setWebNote('재생 중인 곡이 없어요. 봇이 곡을 틀면 바로 따라갈게요.');
    return;
  }
  if (!videoId) {
    stopVideoQuietly();
    setWebNote('이 곡은 웹에서 재생할 수 없어요. Discord로 들어 주세요.');
    return;
  }

  setWebNote('');
  const position = Math.max(0, clock.position());
  if (force || videoId !== webVideoId) {
    webVideoId = videoId;
    try {
      ytPlayer.loadVideoById({ videoId, startSeconds: position });
      ytPlayer.setVolume(webVolume);
      if (state.player?.isPaused) setTimeout(() => { try { ytPlayer.pauseVideo(); } catch { /* 무시 */ } }, 500);
    } catch { /* 다음 틱에서 다시 시도한다 */ }
    return;
  }
  try {
    if (state.player?.isPaused) ytPlayer.pauseVideo();
    else ytPlayer.playVideo();
  } catch { /* 무시 */ }
}

function stopVideoQuietly() {
  webVideoId = null;
  try { ytPlayer?.stopVideo?.(); } catch { /* 무시 */ }
}

/** 매 프레임 맞추면 소리가 튄다. 2초 이상 벌어졌을 때만 조용히 옮긴다. */
function webTick() {
  if (!webOn || !ytReady || !ytPlayer || !webVideoId) return;
  if (store.get().player?.isPaused) return;
  let here = 0;
  try { here = Number(ytPlayer.getCurrentTime()) || 0; } catch { return; }
  const there = clock.position();
  if (Math.abs(there - here) > WEB_SYNC_GAP) {
    try { ytPlayer.seekTo(there, true); ytPlayer.playVideo(); } catch { /* 무시 */ }
  }
}

const WEB_ERRORS = {
  2: '영상 주소가 이상해서 웹에서 못 틀어요.',
  5: '이 브라우저에서 재생할 수 없는 영상이에요.',
  100: '영상이 삭제됐거나 비공개예요.',
  101: '이 곡은 다른 사이트에서의 재생이 막혀 있어요. Discord로 들어 주세요.',
  150: '이 곡은 다른 사이트에서의 재생이 막혀 있어요. Discord로 들어 주세요.',
};

function onWebError(code) {
  webVideoId = null;
  // 토글은 켜 둔 채로 다음 곡을 기다린다. 곡 하나 때문에 기능을 꺼버리면 더 헷갈린다.
  setWebNote(`${WEB_ERRORS[code] || '이 곡은 웹에서 재생할 수 없어요.'} 다음 곡부터 다시 따라갈게요.`);
}

function setWebNote(text) {
  if (!el.webNote) return;
  el.webNote.textContent = text || '';
  el.webNote.hidden = !text;
}

function syncWebUi() {
  if (!el.webBtn) return;
  el.webBtn.setAttribute('aria-pressed', String(webOn));
  el.webBtn.classList.toggle('btn--primary', webOn);
  el.webBtn.textContent = webOn ? '🔊 웹에서 듣는 중' : '🔊 웹에서 듣기';
  el.webVolWrap.hidden = !webOn;
  if (!webOn) setWebNote(webBlocked || '');
}

/* ── 진행바 드래그 ── */

let seeking = false;

function bindSeek() {
  const ratioAt = (clientX) => {
    const rect = el.seekTrack.getBoundingClientRect();
    return Math.min(1, Math.max(0, (clientX - rect.left) / Math.max(1, rect.width)));
  };
  const preview = (ratio) => {
    el.seekFill.style.width = `${ratio * 100}%`;
    el.seekKnob.style.left = `${ratio * 100}%`;
    el.timeNow.textContent = fmtTime(ratio * clock.duration);
  };

  el.seekTrack.addEventListener('pointerdown', (event) => {
    if (el.seekTrack.getAttribute('aria-disabled') === 'true') { toast(lockReason('seek'), 'warn'); return; }
    if (!clock.duration) return;
    seeking = true;
    el.seekTrack.dataset.drag = '1';
    el.seekTrack.setPointerCapture(event.pointerId);
    preview(ratioAt(event.clientX));
  });
  el.seekTrack.addEventListener('pointermove', (event) => { if (seeking) preview(ratioAt(event.clientX)); });
  const finish = (event) => {
    if (!seeking) return;
    seeking = false;
    delete el.seekTrack.dataset.drag;
    const seconds = ratioAt(event.clientX) * clock.duration;
    clock.seekLocal(seconds);
    control('seek', seconds);
  };
  el.seekTrack.addEventListener('pointerup', finish);
  el.seekTrack.addEventListener('pointercancel', () => { seeking = false; delete el.seekTrack.dataset.drag; });

  el.seekTrack.addEventListener('keydown', (event) => {
    if (el.seekTrack.getAttribute('aria-disabled') === 'true') return;
    const step = event.shiftKey ? 30 : 5;
    let target = null;
    if (event.key === 'ArrowRight') target = clock.position() + step;
    else if (event.key === 'ArrowLeft') target = clock.position() - step;
    else if (event.key === 'Home') target = 0;
    else if (event.key === 'End') target = Math.max(0, clock.duration - 2);
    if (target === null) return;
    event.preventDefault();
    target = Math.min(clock.duration, Math.max(0, target));
    clock.seekLocal(target);
    control('seek', target);
  });
}

/* ── 지금 재생 ── */

function renderNow(state) {
  const current = state.current;
  const player = state.player || {};
  const online = player.botOnline !== false;
  const connected = !!player.voiceChannelId;

  if (!current) {
    // src 를 지우면 브라우저가 alt 텍스트를 그대로 그려서 깨진 이미지처럼 보인다.
    // 자리표시 SVG를 넣고 alt 는 비워 스크린리더에도 잡히지 않게 한다.
    el.nowArt.setAttribute('src', ART_PLACEHOLDER);
    el.nowArt.setAttribute('alt', '');
    el.nowArt.classList.add('now__art--idle');
    put(clear(el.nowEyebrow), h('span', { class: 'dot dot--offline' }), online ? '대기 중' : '봇 오프라인');
    el.nowTitle.firstElementChild.textContent = online ? '재생 중인 곡이 없어요' : '봇이 꺼져 있어요';
    put(clear(el.nowBy), connected ? '검색해서 첫 곡을 담아 보세요.' : '봇이 음성 채널에 들어오면 재생할 수 있어요.');
    el.timeNow.textContent = '0:00';
    el.timeEnd.textContent = '0:00';
    el.seekFill.style.width = '0%';
    el.seekKnob.style.left = '0%';
    artColor('');
  } else {
    const art = artUrl(current.track);
    el.nowArt.setAttribute('alt', '앨범 아트');
    el.nowArt.classList.remove('now__art--idle');
    if (el.nowArt.getAttribute('src') !== art) {
      el.nowArt.setAttribute('src', art);
      artColor(art).then(readVizColors);
    }
    put(clear(el.nowEyebrow),
      h('span', { class: player.isPaused ? 'dot dot--idle' : 'dot dot--listening' }),
      player.isPaused ? '일시정지' : '재생 중');
    const title = trackTitle(current.track);
    if (el.nowTitle.firstElementChild.textContent !== title) el.nowTitle.firstElementChild.textContent = title;
    put(clear(el.nowBy),
      current.track?.artist ? h('span', null, current.track.artist) : null,
      current.track?.artist ? h('span', { class: 'row__sub' }, '·') : null,
      h('span', null, '신청 '), h('b', null, current.requestedByDisplay || '알 수 없음'),
      current.requestedByUserId && String(current.requestedByUserId) === String(state.user?.id)
        ? h('span', { class: 'chip chip--accent' }, '내 곡') : null);
    el.timeEnd.textContent = fmtTime(current.durationSeconds || trackSeconds(current.track));
  }

  el.playBtn.textContent = player.isPaused ? '▶' : '⏸';
  el.playBtn.setAttribute('aria-label', player.isPaused ? '재생' : '일시정지');

  const repeat = String(player.repeatMode || 'off').toLowerCase();
  el.repeatBtn.setAttribute('aria-pressed', String(repeat !== 'off'));
  el.repeatBtn.textContent = repeat === 'track' ? '🔂' : '🔁';
  el.repeatBtn.setAttribute('data-tip', repeat === 'off' ? '반복 끔' : repeat === 'track' ? '한 곡 반복' : '대기열 반복');
  el.shuffleBtn.setAttribute('aria-pressed', String(!!player.shuffleEnabled));

  if (Number.isFinite(player.effectiveVolume) && document.activeElement !== el.volume) {
    el.volume.min = String(player.minVolume ?? state.settings?.minVolume ?? 0);
    el.volume.max = String(player.maxVolume ?? state.settings?.maxVolume ?? 200);
    el.volume.value = String(player.effectiveVolume);
    el.volumeLabel.textContent = `${player.effectiveVolume}%`;
  }

  const offline = !online || !connected;
  const offlineReason = !online ? '봇이 꺼져 있어요.' : '봇이 음성 채널에 없어요.';
  for (const [node, key] of [[el.playBtn, 'playback'], [el.skipBtn, 'playback'], [el.restartBtn, 'seek'],
    [el.repeatBtn, 'playback'], [el.shuffleBtn, 'queueEdit']]) {
    setLock(node, offline || !can(key), offline ? offlineReason : lockReason(key));
  }
  setLock(el.seekTrack, offline || !can('seek') || !clock.duration, offline ? offlineReason : lockReason('seek'));
  el.volume.disabled = offline || !can('volume');
  el.volumeWrap.setAttribute('data-tip', el.volume.disabled ? (offline ? offlineReason : lockReason('volume')) : '볼륨');
  el.volumeWrap.classList.toggle('is-locked', el.volume.disabled);

  // 곡이 바뀌면 스크린리더에 알리고, 내 신청곡이면 알림도 띄운다
  const id = current?.id || null;
  const changed = id !== lastCurrentId;
  if (changed) {
    lastCurrentId = id;
    if (current) {
      el.live.textContent = `지금 재생: ${trackTitle(current.track)} · 신청 ${current.requestedByDisplay || ''}`;
      if (String(current.requestedByUserId || '') === String(state.user?.id || '')) {
        notify.push({ title: '내 신청곡이 시작됐어요', body: trackTitle(current.track), icon: artUrl(current.track) });
      }
      if (lyricsOpen) loadLyrics();
    }
  }
  // 웹에서 듣기는 봇을 따라간다. 곡이 바뀌면 위치까지 다시 맞춘다.
  syncWebNow(changed);
  scheduleViz();
  marquee.scan(el.nowCard);
}

function renderProgress() {
  const position = clock.position();
  const duration = clock.duration;
  if (!seeking) {
    const ratio = duration > 0 ? Math.min(1, position / duration) : 0;
    el.seekFill.style.width = `${ratio * 100}%`;
    el.seekKnob.style.left = `${ratio * 100}%`;
    el.timeNow.textContent = fmtTime(position);
    el.seekTrack.setAttribute('aria-valuemax', String(Math.round(duration)));
    el.seekTrack.setAttribute('aria-valuenow', String(Math.round(position)));
    el.seekTrack.setAttribute('aria-valuetext', `${fmtTime(position)} / ${fmtTime(duration)}`);
  }
  highlightLyrics(position);
}

/* ── 비주얼라이저 (장식용) ──
 * 오디오 스트림이 없다. positionSeconds + 곡 cache_key 시드로 결정론적인 파형을 그린다.
 * 서버 부담 0. 숨은 탭에서는 멈추고, 일시정지면 가라앉는다.
 */
const VIZ_BARS = 56;
let vizPhase = [];
let vizFreq = [];
let vizLevel = new Float32Array(VIZ_BARS);
let vizSeedKey = '';
let vizColors = ['#8b5cf6', '#9d74f8'];
let vizRaf = 0;

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function hashString(text) {
  let hash = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

function seedViz(key) {
  if (key === vizSeedKey) return;
  vizSeedKey = key;
  const random = mulberry32(hashString(key || 'idle'));
  vizPhase = Array.from({ length: VIZ_BARS }, () => random() * Math.PI * 2);
  vizFreq = Array.from({ length: VIZ_BARS }, () => 0.6 + random() * 3.4);
}

function readVizColors() {
  const style = getComputedStyle(document.documentElement);
  vizColors = [
    style.getPropertyValue('--art-1').trim() || '#8b5cf6',
    style.getPropertyValue('--art-2').trim() || '#9d74f8',
  ];
}

function drawViz() {
  vizRaf = 0;
  const canvas = el.viz;
  if (!canvas || !canvas.isConnected) return;
  const state = store.get();
  seedViz(trackKey(state.current?.track) || 'idle');

  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (!width || !height) { scheduleViz(); return; }
  if (canvas.width !== Math.round(width * dpr) || canvas.height !== Math.round(height * dpr)) {
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
  }
  const g = canvas.getContext('2d');
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, width, height);

  const playing = state.current && !state.player?.isPaused;
  const position = clock.position();
  const beat = 0.55 + 0.45 * Math.sin(position * 3.1);
  const gradient = g.createLinearGradient(0, height, 0, 0);
  gradient.addColorStop(0, vizColors[0]);
  gradient.addColorStop(1, vizColors[1]);
  g.fillStyle = gradient;

  const gap = 2;
  const barWidth = Math.max(1.5, (width - gap * (VIZ_BARS - 1)) / VIZ_BARS);

  for (let i = 0; i < VIZ_BARS; i += 1) {
    const edge = Math.sin((i / (VIZ_BARS - 1)) * Math.PI);      // 가장자리는 낮게
    const wave = Math.abs(Math.sin(position * vizFreq[i] + vizPhase[i]));
    const target = playing ? (0.14 + wave * 0.8 * beat) * (0.35 + edge * 0.65) : 0.06 + edge * 0.04;
    vizLevel[i] += (target - vizLevel[i]) * (playing ? 0.22 : 0.06);
    const barHeight = Math.max(2, vizLevel[i] * height);
    const x = i * (barWidth + gap);
    g.globalAlpha = 0.35 + vizLevel[i] * 0.65;
    roundRect(g, x, height - barHeight, barWidth, barHeight, Math.min(barWidth / 2, 2));
    g.fill();
  }
  g.globalAlpha = 1;

  // 멈춰 있고 파형도 다 가라앉았으면 루프를 끊는다. 재생이 시작되면 renderNow가 다시 깨운다.
  if (!playing) {
    let settled = true;
    for (let i = 0; i < VIZ_BARS; i += 1) {
      if (Math.abs(vizLevel[i] - (0.06 + Math.sin((i / (VIZ_BARS - 1)) * Math.PI) * 0.04)) > 0.004) { settled = false; break; }
    }
    if (settled) return;
  }
  scheduleViz();
}

function roundRect(g, x, y, w, hh, r) {
  g.beginPath();
  if (g.roundRect) { g.roundRect(x, y, w, hh, r); return; }
  g.moveTo(x, y + hh);
  g.lineTo(x, y + r);
  g.quadraticCurveTo(x, y, x + r, y);
  g.lineTo(x + w - r, y);
  g.quadraticCurveTo(x + w, y, x + w, y + r);
  g.lineTo(x + w, y + hh);
  g.closePath();
}

function scheduleViz() {
  if (vizRaf || document.hidden || prefersReduced()) return;
  vizRaf = requestAnimationFrame(drawViz);
}

/* ── 가사 ── */

let lyricsOpen = prefGet('lyricsOpen') === '1';
let lyricsFull = false;
let lyricsLines = [];
let lyricsActive = -1;

function toggleLyrics() {
  lyricsOpen = !lyricsOpen;
  prefSet('lyricsOpen', lyricsOpen ? '1' : '0');
  el.lyricsToggle.setAttribute('aria-expanded', String(lyricsOpen));
  el.lyricsToggle.classList.toggle('btn--primary', lyricsOpen);
  if (panelMode()) {
    // 패널형에서는 가사도 하나의 패널이다. 여닫기 = 패널 추가/닫기.
    if (lyricsOpen) addDockPanel('lyrics');
    else if (openPanels().has('lyrics')) closeDockPanel('lyrics');
  } else {
    el.lyricsBox.hidden = !lyricsOpen;
  }
  if (lyricsOpen) loadLyrics();
}

async function loadLyrics() {
  if (!store.get().current) {
    clear(el.lyricsView).appendChild(emptyState('🎤', '재생 중인 곡이 없어요', null));
    return;
  }
  clear(el.lyricsView).appendChild(h('div', { class: 'hint' }, '가사를 찾는 중이에요…'));
  try {
    const data = await api('/lyrics');
    store.patch({ lyrics: data });
    renderLyrics();
  } catch (error) {
    clear(el.lyricsView).appendChild(emptyState('🎤', '가사를 못 찾았어요', error.message));
  }
}

function renderLyrics() {
  const lyrics = store.get().lyrics;
  clear(el.lyricsView);
  lyricsLines = [];
  lyricsActive = -1;
  if (!lyrics || (!lyrics.plainText && !(lyrics.syncedLines || []).length)) {
    el.lyricsView.appendChild(emptyState('🎤', '이 곡의 가사를 못 찾았어요', null));
    return;
  }
  const synced = lyrics.syncedLines || [];
  if (!lyricsFull && synced.length) {
    for (const line of synced) {
      const node = h('div', { class: 'lyrics__line' }, line.text || ' ');
      node.__ms = line.startMs;
      lyricsLines.push(node);
      el.lyricsView.appendChild(node);
    }
    highlightLyrics(clock.position());
  } else {
    el.lyricsView.appendChild(h('div', { class: 'lyrics__plain' },
      lyrics.plainText || synced.map((line) => line.text).join('\n')));
  }
}

function highlightLyrics(position) {
  if (!lyricsOpen || !lyricsLines.length) return;
  const ms = position * 1000;
  let active = -1;
  for (let i = 0; i < lyricsLines.length; i += 1) {
    if (lyricsLines[i].__ms <= ms) active = i; else break;
  }
  if (active === lyricsActive) return;
  lyricsActive = active;
  lyricsLines.forEach((node, index) => {
    node.classList.toggle('lyrics__line--on', index === active);
    node.classList.toggle('lyrics__line--near', Math.abs(index - active) === 1);
  });
  if (active >= 0 && !document.hidden) {
    lyricsLines[active].scrollIntoView({ block: 'center', behavior: prefersReduced() ? 'auto' : 'smooth' });
  }
}

/* ═══════════════════════ 우측 탭 ═══════════════════════ */

let activeSideTab = localStorage.getItem(LS.sideTab) || 'chat';

function buildSide() {
  el.sideTabs = SIDE_TABS.map((tab) => {
    const badge = h('span', { class: 'badge', hidden: true }, '0');
    const node = h('button', {
      class: 'tab', type: 'button', role: 'tab', id: `sidetab-${tab.id}`,
      'aria-selected': String(tab.id === activeSideTab), dataset: { side: tab.id },
      tip: tab.label,
      onClick: () => openSide(tab.id),
    }, h('span', { 'aria-hidden': 'true' }, tab.icon), h('span', null, tab.label), badge);
    node.__badge = badge;
    return node;
  });

  el.sidePanes = {
    chat: buildChatPane(),
    members: buildMembersPane(),
    suggest: buildSuggestPane(),
    recent: buildRecentPane(),
    audit: buildAuditPane(),
  };
  for (const [id, pane] of Object.entries(el.sidePanes)) pane.hidden = id !== activeSideTab;

  el.sideBody = h('div', { class: 'side__body', 'data-testid': 'tab-body' }, ...Object.values(el.sidePanes));
  return h('div', { class: 'pane' },
    h('div', { class: 'pane__tabs', role: 'tablist', 'aria-label': '우측 패널' }, el.sideTabs),
    el.sideBody);
}

function openSide(id) {
  activeSideTab = id;
  try { localStorage.setItem(LS.sideTab, id); } catch { /* 시크릿 모드 */ }
  if (panelMode()) { focusDockPanel(id); return; }

  for (const tab of el.sideTabs) tab.setAttribute('aria-selected', String(tab.dataset.side === id));
  for (const [key, pane] of Object.entries(el.sidePanes)) pane.hidden = key !== id;

  if (narrowScreen()) document.body.dataset.pane = 'side';
  else if (drawerActive()) openDrawer(true);

  if (id === 'chat') markChatRead();
  if (id === 'suggest') loadSuggestions();
  if (id === 'audit') loadAudit();
  syncMobileTabs();
  marquee.scan(el.side);
}

function openDrawer(open) {
  el.side.dataset.open = open ? '1' : '0';
  if (open && !el.scrim) {
    el.scrim = h('div', { class: 'side-scrim', onClick: () => openDrawer(false) });
    document.body.appendChild(el.scrim);
  } else if (!open && el.scrim) {
    el.scrim.remove();
    el.scrim = null;
  }
}

/* ── 채팅 ── */

function buildChatPane() {
  el.chatLog = h('div', { class: 'chat__log', 'data-testid': 'chat-messages', role: 'log', 'aria-label': '채팅' });
  el.chatJump = h('button', {
    class: 'btn btn--sm btn--primary', type: 'button', hidden: true,
    style: { position: 'absolute', left: '50%', top: '-38px', transform: 'translateX(-50%)' },
    onClick: () => { scrollChatToEnd(true); markChatRead(); },
  }, '새 메시지 ↓');

  el.chatInput = h('textarea', {
    class: 'field', 'data-testid': 'chat-input', maxlength: '2000', rows: '1',
    placeholder: '메시지 입력 — @이름, #곡 이름',
    onInput: () => { autoGrow(el.chatInput); updateAutocomplete(); },
    onKeydown: onChatKeydown,
    onFocus: markChatRead,
  });

  el.chatSend = bindAct(h('button', {
    class: 'btn btn--primary', type: 'button', 'data-testid': 'chat-send', tip: '보내기', 'data-tip-key': 'Enter',
  }, '보내기'), sendChat);

  el.chatReply = h('div', { class: 'compose__reply', hidden: true });
  el.ac = h('div', { class: 'ac', hidden: true, role: 'listbox' });
  el.chatHint = h('div', { class: 'compose__hint' },
    h('span', null, 'Enter 전송 · Shift+Enter 줄바꿈'),
    h('span', null, '@ 이름 · # 곡 제목'));

  el.compose = h('div', { class: 'compose' },
    el.chatJump,
    el.ac,
    el.chatReply,
    h('div', { class: 'compose__box' }, el.chatInput, el.chatSend),
    el.chatHint);

  el.chatLog.addEventListener('scroll', () => {
    if (nearChatEnd()) { el.chatJump.hidden = true; markChatRead(); }
  });

  return h('div', { class: 'tabpane chat', role: 'tabpanel', 'aria-labelledby': 'sidetab-chat' }, el.chatLog, el.compose);
}

/** 입력이 길어지면 늘어난다. 다만 손잡이로 정해 둔 높이보다 작아지지는 않는다. */
function autoGrow(node) {
  const floor = Number(layoutSizes().chat?.compose) || 36;
  node.style.height = 'auto';
  node.style.height = `${Math.min(260, Math.max(floor, node.scrollHeight))}px`;
}

function nearChatEnd() {
  return el.chatLog.scrollHeight - el.chatLog.scrollTop - el.chatLog.clientHeight < 80;
}

function scrollChatToEnd(smooth) {
  el.chatLog.scrollTo({ top: el.chatLog.scrollHeight, behavior: smooth && !prefersReduced() ? 'smooth' : 'auto' });
}

function renderChat(state) {
  const settings = state.settings || {};
  if (settings.chatEnabled === false) {
    list.reset(el.chatLog);
    el.chatLog.appendChild(emptyState('💬', '채팅이 꺼져 있어요', '서버 관리자가 다시 켤 수 있어요.'));
    setLock(el.chatSend, true, '서버 관리자가 채팅을 껐어요.');
    el.chatInput.disabled = true;
    return;
  }
  const messages = state.chat;
  const stick = nearChatEnd();

  if (!messages.length) {
    list.reset(el.chatLog);
    el.chatLog.appendChild(state.coldAt ? emptyState('💬', '아직 대화가 없어요', '첫 메시지를 남겨 보세요.') : skeletonRows(3));
  } else {
    list(el.chatLog, messages, (message) => message.id, createMessage,
      (node, message, index) => updateMessage(node, message, messages[index - 1]));
  }

  const locked = !can('chat');
  setLock(el.chatSend, locked, lockReason('chat'));
  el.chatInput.disabled = locked;
  el.chatInput.placeholder = locked ? lockReason('chat') : '메시지 입력 — @이름, #곡 이름';

  if (stick) scrollChatToEnd(false);
  else if (messages.length) el.chatJump.hidden = false;
  updateUnreadBadges();
}

function createMessage(message) {
  const node = h('article', { class: 'msg', dataset: { id: message.id } });
  node.__parts = {
    ava: avatar(message.avatarUrl, message.displayName),
    main: h('div', null),
    tools: h('div', { class: 'msg__tools' }),
  };
  node.append(node.__parts.ava, node.__parts.main, node.__parts.tools);
  return node;
}

function updateMessage(node, message, previous) {
  const state = store.get();
  const grouped = previous
    && String(previous.userId) === String(message.userId)
    && !message.replyTo
    && parseUtc(message.createdUtc) - parseUtc(previous.createdUtc) < 5 * 60 * 1000;

  const reactSig = (message.reactions || []).map((r) => `${r.emoji}:${r.count}:${r.reactedByMe ? 1 : 0}`).join(',');
  const sig = [message.id, message.content, message.deletedUtc || '', message.editedUtc || '', reactSig, grouped ? 1 : 0].join('|');
  // 반응 하나 눌렀다고 전체를 다시 그리지 않는다 — 바뀐 노드만 갱신한다
  if (node.__sig === sig) return;
  node.__sig = sig;
  node.__message = message;

  const mine = String(message.userId) === String(state.user?.id);
  const mentioned = (message.mentions || []).some((id) => String(id) === String(state.user?.id));
  node.className = `msg ${grouped ? 'msg--run' : 'msg--first'}${mentioned ? ' msg--mention' : ''}`;

  const p = node.__parts;
  const wantGutter = grouped;
  const isGutter = p.ava.classList.contains('msg__gutter');
  if (wantGutter !== isGutter) {
    const next = wantGutter
      ? h('div', { class: 'msg__gutter' }, fmtClock(message.createdUtc))
      : avatar(message.avatarUrl, message.displayName);
    p.ava.replaceWith(next);
    p.ava = next;
  } else if (wantGutter) {
    p.ava.textContent = fmtClock(message.createdUtc);
  }

  clear(p.main);
  if (message.replyTo) {
    p.main.appendChild(h('button', {
      class: 'quote', type: 'button', tip: '원문으로 이동',
      onClick: () => jumpToMessage(message.replyTo.id),
    }, h('b', null, message.replyTo.displayName || '알 수 없음'), h('span', null, message.replyTo.preview || '삭제된 메시지')));
  }
  if (!grouped) {
    p.main.appendChild(h('div', { class: 'msg__head' },
      h('span', { class: 'msg__name' }, message.displayName || '알 수 없음'),
      h('time', { class: 'msg__time', datetime: message.createdUtc || '', tip: fmtAgo(message.createdUtc) }, fmtClock(message.createdUtc))));
  }

  if (message.deletedUtc) {
    p.main.appendChild(h('div', { class: 'msg__body msg__body--gone' }, '삭제된 메시지'));
  } else {
    p.main.appendChild(renderMessageBody(message));
    p.main.appendChild(renderReactions(message));
  }

  clear(p.tools);
  if (!message.deletedUtc) {
    const react = bindAct(h('button', { class: 'iconbtn', type: 'button', tip: '반응 남기기', 'aria-label': '반응 남기기' }, '🙂'),
      (event) => openEmojiPicker(message.id, event.currentTarget));
    setLock(react, !can('chat'), lockReason('chat'));

    const reply = bindAct(h('button', { class: 'iconbtn', type: 'button', tip: '답장', 'aria-label': '답장' }, '↩'),
      () => setReply(message));
    setLock(reply, !can('chat'), lockReason('chat'));

    p.tools.append(react, reply);
    if (mine || can('chatDelete')) {
      p.tools.appendChild(bindAct(h('button', { class: 'iconbtn iconbtn--danger', type: 'button', tip: '삭제', 'aria-label': '삭제' }, '🗑'),
        async () => {
          if (await confirmSheet({ title: '메시지를 지울까요', desc: message.content?.slice(0, 80), danger: true, confirmText: '삭제' })) {
            call(() => api('/chat/delete', { body: { messageId: message.id } }));
          }
        }));
    }
  }
}

/** 이 서버에서 리모컨을 써 본 사람들의 표시 이름 — 멘션 하이라이트 판정에 쓴다. */
function knownNames() {
  const state = store.get();
  const names = new Set();
  for (const member of state.members) if (member.displayName) names.add(member.displayName.toLowerCase());
  for (const message of state.chat) if (message.displayName) names.add(message.displayName.toLowerCase());
  for (const item of state.queue) if (item.requestedByDisplay) names.add(item.requestedByDisplay.toLowerCase());
  return names;
}

/** @멘션과 #노래태그를 노드로 바꾼다. 문자열 연결이 아니라 노드 생성이라 XSS가 없다.
 *  이름과 곡 제목에는 공백이 들어가므로 '가장 긴 것부터' 맞춰본다. */
function renderMessageBody(message) {
  const body = h('div', { class: 'msg__body' });
  const text = escapeText(message.content || '');
  const names = (message.mentionNames || []).length
    ? new Set(message.mentionNames.map((name) => String(name).toLowerCase()))
    : knownNames();
  const tags = (message.tags || []).map((tag) => ({ tag, title: trackTitle(tag.track) }))
    .sort((a, b) => b.title.length - a.title.length);
  const nameList = [...names].sort((a, b) => b.length - a.length);

  let buffer = '';
  const flush = () => { if (buffer) { body.appendChild(document.createTextNode(buffer)); buffer = ''; } };

  let i = 0;
  while (i < text.length) {
    const char = text[i];
    const rest = text.slice(i + 1).toLowerCase();
    if (char === '#') {
      const hit = tags.find((row) => row.title && rest.startsWith(row.title.toLowerCase()));
      if (hit) { flush(); body.appendChild(tagChip(hit.tag)); i += 1 + hit.title.length; continue; }
    }
    if (char === '@') {
      const hit = nameList.find((name) => name && rest.startsWith(name));
      if (hit) {
        flush();
        body.appendChild(h('span', { class: 'msg__mention' }, `@${text.slice(i + 1, i + 1 + hit.length)}`));
        i += 1 + hit.length;
        continue;
      }
    }
    buffer += char;
    i += 1;
  }
  flush();
  if (message.editedUtc) body.appendChild(h('span', { class: 'msg__time' }, ' (수정됨)'));
  return body;
}

function tagChip(tag) {
  const chip = bindAct(h('button', { class: 'tagchip', type: 'button', tip: '이 곡을 대기열에 담기' },
    h('span', { 'aria-hidden': 'true' }, '♪'), h('span', null, trackTitle(tag.track))), () => enqueue(tag.track));
  setLock(chip, !can('search'), lockReason('search'));
  return chip;
}

function renderReactions(message) {
  const wrap = h('div', { class: 'reacts' });
  for (const reaction of message.reactions || []) {
    const who = (reaction.users || []).map((user) => user.displayName).filter(Boolean).slice(0, 8).join(', ');
    const button = bindAct(h('button', {
      class: 'react', type: 'button', 'aria-pressed': String(!!reaction.reactedByMe),
      tip: who ? `${who} 님이 눌렀어요` : `${reaction.emoji} ${reaction.count}`,
    }, h('span', null, reaction.emoji), h('span', null, String(reaction.count))),
      () => call(() => api('/chat/reaction', { body: { messageId: message.id, emoji: reaction.emoji } })));
    setLock(button, !can('chat'), lockReason('chat'));
    wrap.appendChild(button);
  }
  if (!message.deletedUtc) {
    const add = bindAct(h('button', { class: 'react react--add', type: 'button', tip: '반응 고르기', 'aria-label': '반응 고르기' }, '＋'),
      (event) => openEmojiPicker(message.id, event.currentTarget));
    setLock(add, !can('chat'), lockReason('chat'));
    wrap.appendChild(add);
  }
  return wrap;
}

function openEmojiPicker(messageId, anchor) {
  const picker = h('div', {
    class: 'pop',
    style: { display: 'grid', gridTemplateColumns: 'repeat(6, 1fr)', gap: '2px' },
  }, ...QUICK_EMOJI.map((emoji) => h('button', {
    class: 'iconbtn', type: 'button', 'aria-label': emoji,
    onClick: () => { picker.remove(); call(() => api('/chat/reaction', { body: { messageId, emoji } })); },
  }, emoji)));

  document.body.appendChild(picker);
  const rect = anchor.getBoundingClientRect();
  const box = picker.getBoundingClientRect();
  picker.style.left = `${Math.max(8, Math.min(rect.left - box.width / 2, window.innerWidth - box.width - 8))}px`;
  picker.style.top = `${Math.max(8, rect.top - box.height - 6)}px`;
  picker.querySelector('button')?.focus();

  const close = (event) => {
    if (picker.contains(event.target)) return;
    picker.remove();
    document.removeEventListener('pointerdown', close, true);
  };
  setTimeout(() => document.addEventListener('pointerdown', close, true), 0);
}

function setReply(message) {
  replyTo = { id: message.id, displayName: message.displayName, preview: (message.content || '').slice(0, 80) };
  clear(el.chatReply).append(
    h('span', null, `${replyTo.displayName}에게 답장: ${replyTo.preview}`),
    h('button', { class: 'iconbtn', type: 'button', tip: '답장 취소', 'aria-label': '답장 취소', onClick: clearReply }, '✕'));
  el.chatReply.hidden = false;
  el.chatInput.focus();
}

function clearReply() {
  replyTo = null;
  el.chatReply.hidden = true;
}

function jumpToMessage(messageId) {
  const node = el.chatLog.querySelector(`[data-id="${CSS.escape(String(messageId))}"]`);
  if (!node) { toast('원문이 오래돼서 화면에 없어요.', 'warn'); return; }
  flashNode(node);
}

function onChatKeydown(event) {
  if (acState && ['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(event.key)) {
    if (event.key === 'Escape') { closeAutocomplete(); return; }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      acState.index = (acState.index + (event.key === 'ArrowDown' ? 1 : -1) + acState.items.length) % acState.items.length;
      paintAutocomplete();
      return;
    }
    event.preventDefault();
    applyAutocomplete(acState.items[acState.index]);
    return;
  }
  if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); sendChat(); }
  if (event.key === 'Escape' && replyTo) clearReply();
}

async function sendChat() {
  const content = el.chatInput.value.trim();
  if (!content) return;
  const tags = collectTags(content);
  const payload = { content, replyToMessageId: replyTo?.id || null, tags };
  el.chatInput.value = '';
  autoGrow(el.chatInput);
  clearReply();
  closeAutocomplete();
  try {
    await api('/chat', { body: payload });
    scrollChatToEnd(true);
  } catch (error) {
    el.chatInput.value = content;   // 실패하면 쓴 글을 돌려준다
    toast(error.message, 'danger');
  }
}

/** #제목 토큰을 대기열·최근에서 실제 트랙으로 묶어 보낸다. 제목의 공백까지 맞춰본다. */
function collectTags(content) {
  const state = store.get();
  const pool = [
    ...state.queue.map((item) => item.track),
    ...state.recent.map((row) => row.track || row),
    state.current?.track,
  ].filter(Boolean).map((track) => ({ track, title: trackTitle(track) }))
    .sort((a, b) => b.title.length - a.title.length);

  const tags = [];
  for (let i = 0; i < content.length; i += 1) {
    if (content[i] !== '#') continue;
    const rest = content.slice(i + 1).toLowerCase();
    const hit = pool.find((row) => row.title && rest.startsWith(row.title.toLowerCase()));
    if (!hit) continue;
    const key = trackKey(hit.track);
    if (!tags.some((tag) => tag.cacheKey === key)) tags.push({ cacheKey: key, track: hit.track });
    i += hit.title.length;
  }
  return tags;
}

/* ── 자동완성 (@멘션 / #노래태그) ── */

function updateAutocomplete() {
  const value = el.chatInput.value;
  const caret = el.chatInput.selectionStart ?? value.length;
  const before = value.slice(0, caret);
  const match = /([@#])([^\s@#]*)$/.exec(before);
  if (!match) { closeAutocomplete(); return; }

  const [, kind, needle] = match;
  const lower = needle.toLowerCase();
  const state = store.get();
  let items = [];

  if (kind === '@') {
    const seen = new Set();
    const people = [...state.members, ...state.chat.map((m) => ({ userId: m.userId, displayName: m.displayName, avatarUrl: m.avatarUrl }))];
    for (const person of people) {
      const name = person.displayName || '';
      if (!name || seen.has(String(person.userId))) continue;
      if (lower && !name.toLowerCase().includes(lower)) continue;
      seen.add(String(person.userId));
      items.push({ kind, text: name, label: name, icon: person.avatarUrl, sub: '멤버' });
      if (items.length >= 8) break;
    }
  } else {
    const pool = [...state.queue.map((item) => item.track), ...state.recent.map((row) => row.track || row)];
    const seen = new Set();
    for (const track of pool) {
      const title = trackTitle(track);
      if (!title || seen.has(trackKey(track))) continue;
      if (lower && !title.toLowerCase().includes(lower)) continue;
      seen.add(trackKey(track));
      items.push({ kind, text: title, label: title, icon: artUrl(track), sub: track.artist || track.provider || '' });
      if (items.length >= 8) break;
    }
  }

  if (!items.length) { closeAutocomplete(); return; }
  acState = { kind, from: caret - needle.length - 1, to: caret, items, index: 0 };
  paintAutocomplete();
}

function paintAutocomplete() {
  clear(el.ac);
  acState.items.forEach((item, index) => {
    el.ac.appendChild(h('button', {
      class: 'ac__item', type: 'button', role: 'option', 'aria-selected': String(index === acState.index),
      onMousedown: (event) => { event.preventDefault(); applyAutocomplete(item); },
    },
      item.icon ? h('img', { class: 'ava ava--sm', src: item.icon, alt: '' }) : h('span', null, acState.kind),
      h('span', null, item.label),
      item.sub ? h('span', { class: 'row__sub', style: { marginLeft: 'auto' } }, item.sub) : null));
  });
  el.ac.hidden = false;
}

function applyAutocomplete(item) {
  if (!item || !acState) return;
  const value = el.chatInput.value;
  const inserted = `${acState.kind}${item.text} `;
  el.chatInput.value = value.slice(0, acState.from) + inserted + value.slice(acState.to);
  const caret = acState.from + inserted.length;
  el.chatInput.setSelectionRange(caret, caret);
  closeAutocomplete();
  el.chatInput.focus();
  autoGrow(el.chatInput);
}

function closeAutocomplete() {
  acState = null;
  el.ac.hidden = true;
}

/* ── 미읽음 ── */

function markChatRead() {
  const messages = store.get().chat;
  if (messages.length) lastReadId = messages[messages.length - 1].id;
  unread = 0;
  updateUnreadBadges();
}

function updateUnreadBadges() {
  const tab = el.sideTabs.find((node) => node.dataset.side === 'chat');
  if (tab) {
    tab.__badge.textContent = unread > 99 ? '99+' : String(unread);
    tab.__badge.hidden = unread === 0;
  }
  el.drawerBadge.textContent = unread > 99 ? '99+' : String(unread);
  el.drawerBadge.hidden = unread === 0;
  const mobile = el.mobileTabs?.find((node) => node.dataset.pane === 'side');
  if (mobile) { mobile.__badge.textContent = String(unread); mobile.__badge.hidden = unread === 0; }
  notify.badge(unread);
}

function onChatArrived(message) {
  const state = store.get();
  const mine = String(message.userId) === String(state.user?.id);
  const active = activeSideTab === 'chat' && !document.hidden && !el.sidePanes.chat.hidden;
  if (!mine && !active) {
    unread += 1;
    updateUnreadBadges();
  }
  const mentioned = (message.mentions || []).some((id) => String(id) === String(state.user?.id));
  if (mentioned && !mine) {
    notify.push({ title: `${message.displayName}님이 불렀어요`, body: message.content, icon: message.avatarUrl });
    if (!active) toast(`${message.displayName}님이 나를 불렀어요`, 'info');
  }
}

/* ── 멤버 ── */

function buildMembersPane() {
  el.memberBody = h('div', { class: 'scroll', style: { flex: '1', minHeight: '0' } });
  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'sidetab-members' }, el.memberBody);
}

function renderMembers(state) {
  clear(el.memberBody);
  const intents = state.intentStatus || {};
  if (intents.members === false) {
    el.memberBody.appendChild(h('div', { class: 'banner banner--warn' },
      h('span', { class: 'banner__icon' }, '⚠'),
      h('div', { class: 'banner__text' }, '멤버 목록 권한(Server Members Intent)이 꺼져 있어서 전체 목록은 못 보여드려요. 지금 듣거나 보고 있는 사람만 나와요.')));
  }

  const presence = state.presence || {};
  const bot = presence.bot || null;
  const listening = new Set((presence.listening || []).map(String));
  const otherVoice = new Set((presence.inOtherVoice || []).map(String));
  const viewing = new Set((presence.viewing || []).map(String));
  const online = presence.online || {};

  // 봇이 음성에 없으면 '이 채널에서 듣는 중'은 있을 수 없다. 서버가 실수로 채워 보내도 여기서 막는다.
  const botInVoice = bot ? !!bot.inVoice : true;
  if (!botInVoice) listening.clear();

  if (bot && bot.inGuild !== false) {
    el.memberBody.appendChild(h('div', { class: 'mnote' }, botInVoice
      ? `봇은 지금 ${bot.voiceChannelName ? `'${bot.voiceChannelName}'` : '음성 채널'}에 있어요.`
      : '봇이 음성 채널에 없어서 같이 듣는 사람도 없어요.'));
  }

  const buckets = { listening: [], otherVoice: [], viewing: [], online: [], offline: [] };
  const members = state.members.length ? state.members : synthesizeMembers(state, listening, otherVoice, viewing);
  for (const member of members) {
    const id = String(member.userId ?? member.id);
    const status = online[id] || 'offline';
    if (listening.has(id)) buckets.listening.push({ member, status: 'listening' });
    else if (otherVoice.has(id)) buckets.otherVoice.push({ member, status: 'othervoice' });
    else if (viewing.has(id)) buckets.viewing.push({ member, status: 'viewing' });
    else if (status !== 'offline') buckets.online.push({ member, status });
    else buckets.offline.push({ member, status: 'offline' });
  }

  const groups = [
    ['listening', '🎧 이 채널에서 듣는 중', buckets.listening],
    ['otherVoice', '🔈 다른 채널에 있어요', buckets.otherVoice],
    ['viewing', '🖥 리모컨 보는 중', buckets.viewing],
    ['online', '🟢 온라인', buckets.online],
    ['offline', '⚪ 오프라인', buckets.offline],
  ];
  let any = false;
  for (const [key, title, rows] of groups) {
    if (!rows.length) continue;
    if (key === 'online' && intents.presences === false) continue;
    any = true;
    const group = h('div', { class: 'mgroup' },
      h('div', { class: 'mgroup__title' }, title, h('span', { class: 'count' }, String(rows.length))));
    for (const row of rows) group.appendChild(memberRow(row.member, row.status));
    el.memberBody.appendChild(group);
  }
  if (!any) el.memberBody.appendChild(emptyState('👥', '표시할 멤버가 없어요', null));
}

/** 멤버 목록 인텐트가 꺼져 있어도 접속 중인 사람은 보여준다. */
function synthesizeMembers(state, listening, otherVoice, viewing) {
  const map = new Map();
  const add = (id, name, avatarUrl) => {
    if (!id || map.has(String(id))) return;
    map.set(String(id), { userId: id, displayName: name || `사용자 ${String(id).slice(-4)}`, avatarUrl });
  };
  for (const message of state.chat) add(message.userId, message.displayName, message.avatarUrl);
  for (const item of state.queue) add(item.requestedByUserId, item.requestedByDisplay);
  if (state.current) add(state.current.requestedByUserId, state.current.requestedByDisplay);
  add(state.user?.id, state.user?.displayName, state.user?.avatarUrl);
  for (const id of [...listening, ...otherVoice, ...viewing]) add(id, null, null);
  return [...map.values()];
}

function memberRow(member, status) {
  const id = String(member.userId ?? member.id);
  const row = h('div', { class: `member member--${status}` },
    avatar(member.avatarUrl, member.displayName, 'sm'),
    h('span', { class: `dot dot--${status}` }),
    h('span', { class: 'member__name' }, member.displayName || '알 수 없음'),
    member.tier && member.tier !== 'member'
      ? h('span', { class: `tier tier--${member.tier}` }, TIERS[member.tier]?.icon || '') : null);

  if (can('suspend') && id !== String(store.get().user?.id)) {
    row.appendChild(h('div', { class: 'member__acts' },
      bindAct(h('button', { class: 'iconbtn iconbtn--danger', type: 'button', tip: '이 사람 정지', 'aria-label': '정지' }, '⛔'),
        () => openSuspendSheet(member))));
  }
  return row;
}

async function openSuspendSheet(member) {
  let scope = 'chat';
  let minutes = 30;
  const reason = h('input', { class: 'field', placeholder: '사유 (선택)' });

  const scopeRow = h('div', { class: 'lib__seg', style: { padding: '0' } },
    ...Object.entries(SCOPE_LABELS).map(([id, label]) => h('button', {
      class: 'seg', type: 'button', 'aria-pressed': String(id === scope), dataset: { seg: id },
      onClick: (event) => {
        scope = id;
        for (const node of scopeRow.children) node.setAttribute('aria-pressed', String(node.dataset.seg === id));
        event.preventDefault();
      },
    }, label)));

  const durationRow = h('div', { class: 'lib__seg', style: { padding: '0' } },
    ...[[5, '5분'], [30, '30분'], [180, '3시간'], [0, '무기한']].map(([value, label]) => h('button', {
      class: 'seg', type: 'button', 'aria-pressed': String(value === minutes), dataset: { seg: String(value) },
      onClick: () => {
        minutes = value;
        for (const node of durationRow.children) node.setAttribute('aria-pressed', String(Number(node.dataset.seg) === value));
      },
    }, label)));

  const ok = await sheet({
    title: `${member.displayName} 정지`,
    desc: '기능별로, 정해진 기간 동안만 막아요. 언제든 풀 수 있어요.',
    body: h('div', { style: { display: 'grid', gap: 'var(--sp-4)' } },
      h('div', null, h('div', { class: 'hint' }, '무엇을 막을지'), scopeRow),
      h('div', null, h('div', { class: 'hint' }, '얼마나'), durationRow),
      reason),
    danger: true,
    dismissValue: false,
    actions: [{ label: '취소', kind: 'ghost', value: false }, { label: '정지', kind: 'danger', value: true }],
  }).result;

  if (!ok) return;
  await call(() => api('/suspensions', {
    body: { userId: member.userId ?? member.id, scope, minutes, reason: reason.value.trim() || null },
  }), `${member.displayName}님을 정지했어요.`);
}

/* ── 제안 ── */

function buildSuggestPane() {
  el.suggestBody = h('div', { class: 'scroll', style: { flex: '1', minHeight: '0' } });
  const title = h('input', { class: 'field', placeholder: '제안 한 줄 요약', maxlength: '80' });
  const body = h('textarea', { class: 'field', placeholder: '어떻게 바뀌면 좋을지 적어 주세요.', maxlength: '1000' });
  const submit = bindAct(h('button', { class: 'btn btn--primary', type: 'button', tip: '제안 올리기' }, '올리기'), async () => {
    if (!title.value.trim()) { title.focus(); return; }
    const result = await call(() => api('/suggestions', { body: { title: title.value.trim(), body: body.value.trim() } }), '제안을 올렸어요.');
    if (result) { title.value = ''; body.value = ''; loadSuggestions(); }
  });
  el.suggestForm = h('div', { class: 'filterbar', style: { display: 'grid', gap: 'var(--sp-2)' } },
    title, body, h('div', { style: { textAlign: 'right' } }, submit));
  el.suggestSubmit = submit;

  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'sidetab-suggest' }, el.suggestForm, el.suggestBody);
}

async function loadSuggestions() {
  clear(el.suggestBody).appendChild(skeletonRows(3));
  try {
    const data = await api('/suggestions');
    store.patch({ suggestions: data?.items || [] });
  } catch (error) {
    clear(el.suggestBody).appendChild(emptyState('💡', '제안을 못 불러왔어요', error.message));
  }
}

const SUGGEST_STATUS = {
  open: ['접수됨', 'chip'],
  reviewing: ['검토 중', 'chip chip--info'],
  planned: ['반영 예정', 'chip chip--accent'],
  done: ['반영됨', 'chip chip--ok'],
  declined: ['보류', 'chip chip--warn'],
};

function renderSuggestions(state) {
  setLock(el.suggestSubmit, !can('suggest'), lockReason('suggest'));
  clear(el.suggestBody);
  const items = [...state.suggestions].sort((a, b) => (b.votes || 0) - (a.votes || 0) || parseUtc(b.createdUtc) - parseUtc(a.createdUtc));
  if (!items.length) {
    el.suggestBody.appendChild(emptyState('💡', '아직 제안이 없어요', '불편한 걸 적어두면 반영될 수도 있어요.'));
    return;
  }
  for (const item of items) {
    const [label, chipClass] = SUGGEST_STATUS[item.status] || SUGGEST_STATUS.open;
    const voteBtn = bindAct(h('button', {
      class: 'vote', type: 'button', 'aria-pressed': String(!!item.votedByMe), tip: '공감',
    }, `👍 ${item.votes || 0}`), () => call(async () => {
      await api('/suggestions/vote', { body: { suggestionId: item.id } });
      loadSuggestions();
    }));
    setLock(voteBtn, !can('suggest'), lockReason('suggest'));

    const statusBtn = can('suggestStatus')
      ? bindAct(h('button', { class: 'iconbtn', type: 'button', tip: '상태 바꾸기', 'aria-label': '상태 바꾸기' }, '⋯'),
        () => openStatusSheet(item))
      : null;

    el.suggestBody.appendChild(h('article', { class: 'sug' },
      h('div', { class: 'sug__head' },
        h('span', { class: chipClass }, label),
        h('h3', { class: 'sug__title' }, item.title)),
      item.body ? h('p', { class: 'sug__body' }, item.body) : null,
      h('div', { class: 'sug__foot' },
        avatar(item.avatarUrl, item.displayName, 'sm'),
        h('span', null, item.displayName || '알 수 없음'),
        h('span', null, fmtDate(item.createdUtc)),
        h('span', { class: 'grow' }),
        voteBtn, statusBtn),
      item.statusNote ? h('p', { class: 'hint' }, `관리자: ${item.statusNote}`) : null));
  }
}

async function openStatusSheet(item) {
  let picked = item.status || 'open';
  const note = h('input', { class: 'field', placeholder: '한 줄 메모 (선택)', value: item.statusNote || '' });
  const row = h('div', { class: 'lib__seg', style: { padding: '0', flexWrap: 'wrap' } },
    ...Object.entries(SUGGEST_STATUS).map(([id, [label]]) => h('button', {
      class: 'seg', type: 'button', 'aria-pressed': String(id === picked), dataset: { seg: id },
      onClick: () => { picked = id; for (const node of row.children) node.setAttribute('aria-pressed', String(node.dataset.seg === id)); },
    }, label)));

  const ok = await sheet({
    title: '제안 상태', desc: item.title, body: h('div', { style: { display: 'grid', gap: 'var(--sp-3)' } }, row, note),
    dismissValue: false,
    actions: [{ label: '취소', kind: 'ghost', value: false }, { label: '저장', kind: 'primary', value: true }],
  }).result;
  if (!ok) return;
  await call(async () => {
    await api('/suggestions/status', { body: { suggestionId: item.id, status: picked, note: note.value.trim() || null } });
    loadSuggestions();
  }, '상태를 바꿨어요.');
}

/* ── 최근 ── */

function buildRecentPane() {
  el.recentBody = h('div', { class: 'scroll', style: { flex: '1', minHeight: '0', padding: '0 var(--sp-2) var(--sp-3)' } });
  el.recentAll = bindAct(h('button', { class: 'btn btn--sm', type: 'button', tip: '최근 목록을 한 번에 대기열로' }, '한번에 다시 담기'),
    async () => {
      const tracks = store.get().recent.slice(0, 20).map((row) => row.track || row);
      if (!tracks.length) return;
      const ok = await confirmSheet({ title: `${tracks.length}곡을 담을까요`, desc: '최근 재생한 곡을 순서대로 대기열에 넣어요.', confirmText: '담기' });
      if (!ok) return;
      let done = 0;
      for (const track of tracks) {
        try { await api('/queue', { body: { track } }); done += 1; } catch { /* 중복·한도는 건너뛴다 */ }
      }
      toast(done ? `${done}곡을 담았어요.` : '담긴 곡이 없어요. 이미 대기열에 있거나 한도에 걸렸어요.', done ? 'ok' : 'warn');
    });
  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'sidetab-recent' },
    h('div', { class: 'filterbar', style: { display: 'flex', justifyContent: 'flex-end' } }, el.recentAll),
    el.recentBody);
}

function renderRecent(state) {
  setLock(el.recentAll, !can('search'), lockReason('search'));
  clear(el.recentBody);
  if (!state.recent.length) {
    el.recentBody.appendChild(emptyState('🕘', '최근 재생 기록이 없어요', null));
    return;
  }
  for (const row of state.recent) {
    const track = row.track || row;
    el.recentBody.appendChild(trackRow(track, 'recent',
      [fmtAgo(row.playedUtc), row.requestedByDisplay].filter(Boolean).join(' · ')));
  }
  marquee.scan(el.recentBody);
}

/* ── 활동 로그 ── */

function buildAuditPane() {
  el.auditFilter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'audit-filter', placeholder: '사람 · 동작 · 곡 제목으로 거르기',
    onInput: debounce(() => { auditQuery = el.auditFilter.value.trim().toLowerCase(); renderAudit(store.get()); }, 140),
  });
  el.auditBody = h('div', { class: 'scroll', style: { flex: '1', minHeight: '0' } });
  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'sidetab-audit' },
    h('div', { class: 'filterbar' }, el.auditFilter),
    el.auditBody);
}

async function loadAudit() {
  if (store.get().audit.length) return;
  clear(el.auditBody).appendChild(skeletonRows(4));
  try {
    const data = await api('/audit');
    store.patch({ audit: data?.entries || data || [] });
  } catch (error) {
    clear(el.auditBody).appendChild(emptyState('📜', '활동 로그를 못 불러왔어요', error.message));
  }
}

function renderAudit(state) {
  clear(el.auditBody);
  const rows = state.audit.filter((entry) => !auditQuery
    || [entry.displayName, entry.action, entry.target, entry.afterValue, entry.failureReason]
      .join(' ').toLowerCase().includes(auditQuery));
  if (!rows.length) {
    el.auditBody.appendChild(emptyState('📜', auditQuery ? '조건에 맞는 기록이 없어요' : '기록이 없어요', null));
    return;
  }
  for (const entry of rows) {
    el.auditBody.appendChild(h('div', { class: `logrow${entry.success === false ? ' logrow--fail' : ''}` },
      h('time', { datetime: entry.createdUtc || '', tip: entry.createdUtc || '' }, fmtClock(entry.createdUtc)),
      h('div', null,
        h('b', null, entry.displayName || '시스템'),
        document.createTextNode(' '),
        h('span', null, entry.action || ''),
        h('p', null, entry.failureReason || entry.target || entry.afterValue || ''))));
  }
}

/* ═══════════════════════ 모바일 하단 탭바 ═══════════════════════ */

function buildMobileTabs() {
  const defs = [
    { id: 'search', icon: '🔎', label: '검색', pane: 'rail', rail: 'search' },
    { id: 'queue', icon: '📋', label: '대기열', pane: 'rail', rail: 'queue' },
    { id: 'now', icon: '▶', label: '재생', pane: 'stage' },
    { id: 'chat', icon: '💬', label: '채팅', pane: 'side', side: 'chat' },
    { id: 'more', icon: '⋯', label: '더보기', pane: 'side' },
  ];
  el.mobileTabs = defs.map((def) => {
    const badge = h('span', { class: 'badge', hidden: true }, '0');
    const node = h('button', {
      class: 'mtab', type: 'button', role: 'tab', dataset: { pane: def.pane, tab: def.id },
      'aria-selected': 'false',
      onClick: () => {
        if (def.id === 'more') { openMoreSheet(); return; }
        document.body.dataset.pane = def.pane;
        if (def.rail) setRailTab(def.rail);
        if (def.side) openSide(def.side);
        syncMobileTabs();
      },
    }, h('em', { 'aria-hidden': 'true' }, def.icon), def.label, badge);
    node.__badge = badge;
    node.__def = def;
    return node;
  });
  return h('nav', { class: 'mtabs', role: 'tablist', 'aria-label': '화면 전환' }, el.mobileTabs);
}

function syncMobileTabs() {
  const pane = document.body.dataset.pane;
  for (const node of el.mobileTabs) {
    const def = node.__def;
    const on = def.pane === pane
      && (!def.rail || def.rail === activeRailTab)
      && (!def.side || def.side === activeSideTab);
    node.setAttribute('aria-selected', String(on));
  }
}

async function openMoreSheet() {
  let handle = null;
  const body = h('div', { style: { display: 'grid', gap: 'var(--sp-1)' } },
    ...SIDE_TABS.filter((tab) => tab.id !== 'chat').map((tab) => h('button', {
      class: 'dd__item', type: 'button',
      onClick: () => handle?.close(tab.id),
    }, h('span', null, tab.icon), h('span', null, tab.label))));

  handle = sheet({ title: '더 보기', body, dismissValue: null, actions: [] });
  const id = await handle.result;
  if (id) { document.body.dataset.pane = 'side'; openSide(id); syncMobileTabs(); }
}

/* ═══════════════════════ 시트들 ═══════════════════════ */

function openModeSheet() {
  const current = store.get().queueMode;
  sheet({
    title: '대기열 정렬 방식',
    desc: '지금 이 서버는 아래 방식으로 순서를 정해요. 바꾸는 건 서버 관리자만 할 수 있어요.',
    wide: true,
    body: h('div', { class: 'modecmp' }, ...Object.entries(MODES).map(([id, mode]) => h('div', {
      class: 'modecmp__card', dataset: { active: id === current ? '1' : '0' },
    },
      h('h3', null, h('span', { 'aria-hidden': 'true' }, mode.icon), mode.label,
        id === current ? h('span', { class: 'chip chip--accent' }, '지금 이 방식') : null),
      h('p', null, mode.desc),
      h('code', null, mode.formula)))),
    actions: can('sortMode')
      ? [{ label: '닫기', kind: 'ghost', value: false },
        { label: '관리 콘솔에서 바꾸기', kind: 'primary', value: 'admin', autofocus: true }]
      : [{ label: '닫기', kind: 'primary', value: false, autofocus: true }],
  }).result.then((value) => {
    if (value === 'admin') location.href = `/music/guilds/${ctx.guildId}/admin#queue`;
  });
}

function openPermissionSheet() {
  const state = store.get();
  const tier = TIERS[state.tier] || TIERS.member;
  const entries = state.permissions?.entries || fallbackEntries();
  const allowed = entries.filter((entry) => entry.allowed);
  const denied = entries.filter((entry) => !entry.allowed);

  const rowOf = (entry) => h('div', { class: `perm__row${entry.allowed ? '' : ' perm__row--no'}` },
    h('span', { 'aria-hidden': 'true' }, entry.allowed ? '✅' : '❌'),
    h('span', { class: 'label' }, entry.label || PERM_LABELS[entry.key] || entry.key),
    h('span', { class: 'why' },
      entry.ruleLabel || RULE_LABELS[entry.rule] || '',
      Array.isArray(entry.roleNames) && entry.roleNames.length ? ` (${entry.roleNames.join(', ')})` : '',
      entry.viaAdmin ? h('em', null, ' ← 관리자라 통과') : null,
      entry.reason && !entry.allowed ? ` · ${entry.reason}` : ''));

  const body = h('div', null,
    state.suspension ? h('div', { class: 'banner banner--danger', style: { borderRadius: 'var(--r-md)', marginBottom: 'var(--sp-4)' } },
      h('span', { class: 'banner__icon' }, '⛔'),
      h('div', { class: 'banner__text' },
        h('b', null, `${SCOPE_LABELS[state.suspension.scope] || '전체'} 정지 중`),
        document.createTextNode(` · ${suspensionRemain(state.suspension)}`),
        state.suspension.reason ? h('div', { class: 'hint' }, `사유: ${state.suspension.reason}`) : null)) : null,
    h('div', { class: 'perm__tier' },
      h('span', { class: 'big', 'aria-hidden': 'true' }, tier.icon),
      h('div', null, h('strong', null, tier.label), h('p', null, tier.desc))),
    allowed.length ? h('div', { class: 'perm__group' }, h('h3', null, '할 수 있는 것'), ...allowed.map(rowOf)) : null,
    denied.length ? h('div', { class: 'perm__group' }, h('h3', null, '할 수 없는 것'), ...denied.map(rowOf)) : null);

  sheet({
    title: '내 권한', body,
    actions: can('console')
      ? [{ label: '닫기', kind: 'ghost', value: false }, { label: '서버 관리 콘솔 열기 →', kind: 'primary', value: 'admin', autofocus: true }]
      : [{ label: '닫기', kind: 'primary', value: false, autofocus: true }],
  }).result.then((value) => {
    if (value === 'admin') location.href = `/music/guilds/${ctx.guildId}/admin`;
  });
}

/** 서버가 entries를 안 주면 can 맵으로라도 화면을 채운다. */
function fallbackEntries() {
  const permissions = store.get().permissions?.can || {};
  return Object.keys(PERM_LABELS).map((key) => ({
    key, label: PERM_LABELS[key], allowed: !!permissions[key], rule: null, ruleLabel: '',
  }));
}

/* ═══════════════════════ 배너 ═══════════════════════ */

function renderBanners(state) {
  clear(el.banners);
  el.portal.dataset.conn = state.conn;

  if (state.conn === 'reconnecting' || state.conn === 'down') {
    el.banners.appendChild(h('div', { class: `banner banner--${state.conn === 'down' ? 'danger' : 'warn'}`, role: 'status' },
      h('span', { class: 'banner__icon' }, state.conn === 'down' ? '⛔' : '⏳'),
      h('div', { class: 'banner__text' }, state.conn === 'down'
        ? '연결이 끊겼어요. 지금은 조작이 막혀요. 새로고침하면 다시 붙어요.'
        : '실시간 갱신이 끊겼어요. 다시 붙는 중이라 화면이 잠깐 옛날 것일 수 있어요.'),
      state.conn === 'down'
        ? h('button', { class: 'btn btn--sm', type: 'button', onClick: () => location.reload() }, '새로고침')
        : null));
  }

  // 봇이 이 서버에 아예 없으면 접속 요약이 아니라 배너로 크게 알린다
  const bot = state.presence?.bot;
  if (bot && bot.inGuild === false) {
    el.banners.appendChild(h('div', { class: 'banner banner--danger', role: 'status' },
      h('span', { class: 'banner__icon' }, '🤖'),
      h('div', { class: 'banner__text' },
        h('b', null, '봇이 이 서버에 없어요'),
        document.createTextNode(' · 봇을 다시 초대해야 재생과 대기열이 움직여요. 지금은 보기만 돼요.'))));
  }

  if (state.tier === 'viewer') {
    el.banners.appendChild(h('div', { class: 'banner banner--warn' },
      h('span', { class: 'banner__icon' }, '👀'),
      h('div', { class: 'banner__text' },
        h('b', null, '읽기 전용'),
        document.createTextNode(` · ${state.viewerReason || '이 서버에서 조작 권한이 없어요. 보기는 그대로 돼요.'}`))));
  }

  if (state.suspension) {
    el.banners.appendChild(h('div', { class: 'banner banner--danger' },
      h('span', { class: 'banner__icon' }, '⛔'),
      h('div', { class: 'banner__text' },
        h('b', null, `${SCOPE_LABELS[state.suspension.scope] || '전체'} 정지 중`),
        document.createTextNode(` · ${suspensionRemain(state.suspension)}`),
        state.suspension.reason ? document.createTextNode(` · ${state.suspension.reason}`) : null)));
  }

  if (state.intentStatus && state.intentStatus.presences === false && state.tier !== 'member') {
    el.banners.appendChild(h('div', { class: 'banner banner--info' },
      h('span', { class: 'banner__icon' }, 'ℹ'),
      h('div', { class: 'banner__text' }, '온라인 상태 권한(Presence Intent)이 꺼져 있어서 접속 상태 일부가 안 보여요.')));
  }
}

/* 새 버전 안내 — buildId가 다르면 알린다 */
let versionNagged = false;
function checkVersion(state) {
  if (versionNagged || !state.buildId || !ctx.buildId) return;
  if (state.buildId === ctx.buildId) return;
  versionNagged = true;
  confirmSheet({
    title: '새 버전이 올라왔어요',
    desc: '지금 보고 있는 화면은 이전 버전이에요. 새로고침하면 최신 화면으로 바뀌어요.',
    confirmText: '새로고침', cancelText: '나중에',
  }).then((ok) => { if (ok) location.reload(); });
}

/* ═══════════════════════ 프로필 / 헤더 렌더 ═══════════════════════ */

function renderProfile() {
  const state = store.get();
  const tier = TIERS[state.tier] || TIERS.member;

  clear(el.meBtn).append(
    avatar(state.user?.avatarUrl, state.user?.displayName),
    h('strong', null, state.user?.displayName || '나'),
    h('span', { class: `tier tier--${state.tier}` }, tier.icon),
    h('span', { class: 'hdr__caret' }, '▾'));

  clear(el.meHead).append(
    avatar(state.user?.avatarUrl, state.user?.displayName, 'lg'),
    h('div', null,
      h('strong', null, state.user?.displayName || '나'),
      h('span', { class: `tier tier--${state.tier}` }, `${tier.icon} ${tier.label}`)));

  setLock(el.consoleBtn, !can('console'), '서버 관리자만 열 수 있어요.');
  el.opsLink.hidden = !can('ops');
  el.notifyBtn.hidden = notify.granted() || !notify.supported();
  syncLayoutOptions();
}

function renderGuild(state) {
  put(clear(el.guildBtn),
    state.guild?.iconUrl ? h('img', { src: state.guild.iconUrl, alt: '' }) : null,
    h('span', null, state.guild?.name || '서버'),
    h('span', { class: 'hdr__caret' }, '▾'));

  clear(el.guildMenu);
  const guilds = state.guilds || [];
  if (!guilds.length) {
    el.guildMenu.appendChild(h('div', { class: 'hint', style: { padding: 'var(--sp-3)' } }, '다른 서버가 없어요.'));
    return;
  }
  for (const guild of guilds) {
    el.guildMenu.appendChild(h('a', {
      class: 'dd__item', role: 'menuitem', 'data-testid': 'guild-card',
      href: `/music/guilds/${guild.id}`,
      'aria-current': String(String(guild.id) === String(ctx.guildId)),
    },
      guild.iconUrl ? h('img', { class: 'ava ava--sm', src: guild.iconUrl, alt: '' }) : h('span', null, '🎵'),
      h('span', null, guild.name)));
  }
}

function renderPresenceSummary(state) {
  const presence = state.presence || {};
  const bot = presence.bot || null;
  const viewing = presence.viewingCount ?? (presence.viewing || []).length;
  const viewChip = h('span', { class: 'pcount pcount--view' },
    h('span', { 'aria-hidden': 'true' }, '🖥'), '보는중 ', h('b', null, String(viewing)));

  // 봇이 음성에 없으면 '듣는중 0'은 거짓말에 가깝다. 왜 0인지를 그대로 말해 준다.
  if (bot && bot.inGuild === false) {
    clear(el.presenceBtn).append(h('span', { class: 'pcount pcount--off' }, '🤖 봇이 서버에 없어요'), viewChip);
    el.presenceBtn.setAttribute('aria-label', `봇이 서버에 없어요. 리모컨 ${viewing}명`);
    el.presenceBtn.setAttribute('data-tip', '봇을 다시 초대해야 재생할 수 있어요.');
    return;
  }
  if (bot && bot.inVoice === false) {
    clear(el.presenceBtn).append(h('span', { class: 'pcount pcount--off' }, '🎧 봇이 음성 채널에 없어요'), viewChip);
    el.presenceBtn.setAttribute('aria-label', `봇이 음성 채널에 없어요. 리모컨 ${viewing}명`);
    el.presenceBtn.setAttribute('data-tip', '봇을 음성 채널로 부르면 같이 들을 수 있어요.');
    return;
  }

  const listening = bot && Number.isFinite(bot.listenerCount)
    ? bot.listenerCount
    : (presence.listeningCount ?? (presence.listening || []).length);
  clear(el.presenceBtn).append(
    h('span', { class: 'pcount pcount--listen' }, h('span', { 'aria-hidden': 'true' }, '🎧'), '듣는중 ', h('b', null, String(listening))),
    viewChip);
  el.presenceBtn.setAttribute('aria-label', `음성채널 ${listening}명, 리모컨 ${viewing}명`);
  el.presenceBtn.setAttribute('data-tip', bot?.voiceChannelName
    ? `'${bot.voiceChannelName}' 채널에서 ${listening}명이 같이 듣고 있어요`
    : '지금 누가 듣고 있는지 보기');
}

/* ═══════════════════════ 데이터 로드 ═══════════════════════ */

async function loadCold() {
  const data = await api('/state/cold');
  // 서버 개인 설정이 최우선이다. 받자마자 거울에 적어 다음 첫 페인트가 안 튀게 한다.
  if (adoptServerPrefs(data.prefs)) applyServerPrefs();
  store.patch({
    searchCfg: data.search || null,
    buildId: data.buildId || ctx.buildId,
    guild: data.guild || null,
    guilds: data.guilds || [],
    user: data.user || ctx.user,
    tier: data.tier || ctx.tier,
    viewerReason: data.viewerReason || null,
    permissions: data.permissions || null,
    intentStatus: data.intentStatus || null,
    settings: data.settings || null,
    suspension: data.suspension || null,
    playlists: data.playlists || [],
    liked: data.liked || [],
    saved: data.saved || [],
    recent: data.recent || [],
    members: data.members || [],
    coldAt: Date.now(),
  });
}

async function loadHot() {
  const data = await api('/state/hot');
  // 카운트다운은 서버 시각 기준으로 센다. 표본 시각으로 시계 차이를 맞춰 둔다.
  noteServerTime(data.sampledAtUtc || data.sortedAt);
  if (data.presence && data.presence.bot !== undefined) lastBotState = data.presence.bot;
  store.patch({
    player: data.player || null,
    current: data.current || null,
    queue: data.queue || [],
    queueMode: data.queueMode || data.mode || 'score',
    sortedAt: data.sortedAt || null,
    nextSortAt: data.nextSortAt || null,
    presence: data.presence || store.get().presence,
    hotAt: Date.now(),
  });
  clock.sync({
    positionSeconds: data.positionSeconds,
    sampledAtUtc: data.sampledAtUtc,
    isPaused: data.player?.isPaused,
    durationSeconds: data.current?.durationSeconds ?? trackSeconds(data.current?.track),
  });
}

async function loadChat() {
  const data = await api('/chat');
  const messages = data?.messages || data || [];
  store.patch({ chat: messages, chatCursor: data?.nextBefore || null });
  markChatRead();
  requestAnimationFrame(() => scrollChatToEnd(false));
}

const refetchCold = debounce(() => { loadCold().catch(() => {}); }, 400);
const refetchHot = debounce(() => { loadHot().catch(() => {}); }, 400);

/* ═══════════════════════ 부팅 ═══════════════════════ */

async function boot() {
  theme.init(prefGet('theme') || ctx.themeDefault || 'dark');
  buildShell();
  tooltip();
  marqueeRows();
  if (panelMode()) mountDock();

  // 초기 그리기 — 데이터가 오기 전에도 뼈대는 보인다
  store.subscribe(['conn', 'tier', 'suspension', 'intentStatus', 'presence'], renderBanners);
  store.subscribe(['user', 'tier', 'permissions'], renderProfile);
  store.subscribe(['guild', 'guilds'], renderGuild);
  store.subscribe(['presence'], renderPresenceSummary);
  store.subscribe(['presence', 'members', 'intentStatus'], renderMembers);
  store.subscribe(['queue', 'queueMode', 'permissions', 'suspension', 'tier', 'conn', 'hotAt'], renderQueue);
  store.subscribe(['current', 'player', 'permissions', 'suspension', 'tier', 'settings', 'conn'], renderNow);
  store.subscribe(['chat', 'chatDelta', 'permissions', 'suspension', 'tier', 'conn', 'settings', 'coldAt'], renderChat);
  store.subscribe(['liked', 'saved', 'playlists', 'permissions', 'suspension', 'tier'], renderLibrary);
  store.subscribe(['recent', 'permissions', 'suspension', 'tier'], renderRecent);
  store.subscribe(['suggestions', 'permissions', 'suspension', 'tier'], renderSuggestions);
  store.subscribe(['audit'], renderAudit);
  store.subscribe(['lyrics'], () => { if (lyricsOpen) renderLyrics(); });
  store.subscribe(['buildId'], checkVersion);
  store.subscribe(['permissions', 'suspension', 'tier'], renderSeeds);
  store.subscribe(['queueMode', 'sortedAt', 'nextSortAt', 'queue'], renderSortTick);

  if (!panelMode()) el.lyricsBox.hidden = !lyricsOpen;
  el.lyricsToggle.setAttribute('aria-expanded', String(lyricsOpen));
  el.lyricsToggle.classList.toggle('btn--primary', lyricsOpen);
  syncWebUi();
  if (webWanted) setWebNote('지난번에 웹에서 듣기를 켜 두셨어요. 🔊 웹에서 듣기를 한 번 눌러 주세요.');

  clock.onTick(renderProgress);
  scheduleViz();
  startSortTick();
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden) { scheduleViz(); marquee.scan(); if (activeSideTab === 'chat') markChatRead(); }
  });

  bindShortcuts();
  syncMobileTabs();

  // 진입 로드 — cold/hot/chat 한 번씩. 이후는 WS만.
  try {
    await Promise.all([loadCold(), loadHot(), loadChat()]);
  } catch (error) {
    toast(error.message || '초기 데이터를 못 불러왔어요.', 'danger');
  }
  if (lyricsOpen) loadLyrics();
  loadSeeds();

  // 한 번도 배치를 고른 적이 없으면 여기서 물어본다. 두 번째 진입부터는 안 뜬다.
  if (!layoutChosen) openLayoutSheet();

  connect(ctx.guildId, {
    onResync: () => { loadHot().catch(() => {}); loadCold().catch(() => {}); loadSeeds(); },
    onRefetch: (what) => {
      if (what === 'library' || what === 'settings' || what === 'permissions') refetchCold();
      if (what === 'suggestions' && activeSideTab === 'suggest') loadSuggestions();
      if (what === 'audit') { store.patch({ audit: [] }); if (activeSideTab === 'audit') loadAudit(); }
    },
    onChat: onChatArrived,
    onEvent: (type) => { if (type === 'autoplay') loadSeeds(); },
    // core.js의 merge()는 계약에 없던 필드를 흘려보낸다. 여기서 원본 payload를 다시 주워 담는다.
    onAny: (type, data) => {
      if (type === 'presence' && data) {
        // core.js가 이미 presence를 {listening,viewing,online}으로 갈아 끼운 뒤라
        // 직전 값에서 bot을 되찾을 수 없다. 마지막으로 본 봇 상태는 따로 들고 있는다.
        if (data.bot !== undefined) lastBotState = data.bot;
        store.patch({
          presence: {
            listening: data.listening || [],
            inOtherVoice: data.inOtherVoice || [],
            viewing: data.viewing || [],
            online: data.online || {},
            listeningCount: data.listeningCount,
            viewingCount: data.viewingCount,
            bot: lastBotState,
          },
        });
      }
      if (type === 'queue.set' && data) {
        noteServerTime(data.sortedAt);
        store.patch({ nextSortAt: data.nextSortAt || null });
      }
      if (type === 'playback' && data) noteServerTime(data.sampledAtUtc);
    },
    onDenied: (reason) => {
      store.patch({ tier: 'viewer', viewerReason: reason || '접근 권한이 사라졌어요.' });
      toast(reason || '접근 권한이 사라졌어요. 새로고침해 보세요.', 'danger');
    },
  });

  // 정지 남은 시간 카운트다운. 만료되면 스스로 풀고, 서버 판정은 다음 요청에서 맞춰진다.
  setInterval(() => {
    const suspension = store.get().suspension;
    if (!suspension) return;
    if (suspension.expiresUtc && parseUtc(suspension.expiresUtc) <= Date.now()) {
      store.patch({ suspension: null });
      refetchCold();
      toast('정지가 풀렸어요.', 'ok');
      return;
    }
    renderBanners(store.get());
  }, 30000);
}

/** 키보드만으로 주요 조작이 가능해야 한다. 입력 중일 때는 가로채지 않는다. */
function bindShortcuts() {
  document.addEventListener('keydown', (event) => {
    const tag = document.activeElement?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
    if (document.activeElement?.isContentEditable) return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    // 버튼/링크에 포커스가 있을 때 Space는 그 버튼을 눌러야 한다. 가로채지 않는다.
    if (event.key === ' ' && (tag === 'BUTTON' || tag === 'A')) return;
    switch (event.key) {
      case ' ': event.preventDefault(); el.playBtn.click(); break;
      case 'n': el.skipBtn.click(); break;
      case '/': event.preventDefault(); setRailTab('search'); el.searchInput.focus(); break;
      case 'l': el.lyricsToggle.click(); break;
      case 'c': openSide('chat'); el.chatInput.focus(); break;
      case 't': el.themeBtn.click(); break;
      default: break;
    }
  });
}

if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
else boot();
