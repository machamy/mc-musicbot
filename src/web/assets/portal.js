/* 마참뮤직 리모컨 v2 — 유저 UI (portal.js)
 *
 * 서버는 빈 셸(#app)과 window.MACHAM만 준다. 화면은 전부 여기서 그린다.
 * 진입: /state/cold 1회 + /state/hot 1회 → 이후는 WebSocket 이벤트만으로 갱신한다.
 * innerHTML은 한 번도 쓰지 않는다. 모든 노드는 core.js의 h()로 만든다.
 */

import {
  ctx, store, connect, api, ApiError, clock, h, frag, list, tooltip, marquee, marqueeRows, mqText,
  toast, sheet, confirmSheet, notify, artColor, fmtTime, fmtAgo, fmtClock, fmtDate,
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

/* 서버(remote.rs permissions_json)가 내려주는 키를 그대로 따른다.
 * §10.5에서 `skip`이 `playback`에서 떨어져 나왔으니 라벨도 따로 가진다 —
 * 라벨이 합쳐져 있으면 "재생 권한은 있는데 왜 스킵이 안 되지"를 화면이 설명하지 못한다. */
const PERM_LABELS = {
  search: '곡 검색·신청',
  vote: '좋아요·슈퍼 좋아요·싫어요',
  playback: '재생 / 일시정지',
  skip: '곡 넘기기',
  seek: '재생 위치 이동',
  volume: '볼륨 조절',
  queueEdit: '대기열 편집',
  chat: '채팅 쓰기·반응·답장',
  autoplay: '자동 재생 켜고 끄기·기준 곡',
  bulkEnqueue: '재생목록·차트 전부 담기',
  library: '보관함·재생목록',
  suggest: '제안 작성·공감',
  autoplaySeed: '자동 재생 기준 곡 편집',
  stats: '기록 보기',
  chatDelete: '남의 채팅 삭제',
  suggestStatus: '제안 상태 변경',
  suspend: '유저 정지·해제',
  sortMode: '정렬 모드 변경',
  blacklist: '차단 목록 관리',
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

/* 제안은 §11에서 탭을 떠나 헤더 버튼 + 모달로 갔다. 그래서 여기에 없다. */
const SIDE_TABS = [
  { id: 'chat', icon: '💬', label: '채팅' },
  { id: 'members', icon: '👥', label: '멤버' },
  { id: 'recent', icon: '🕘', label: '최근' },
  { id: 'audit', icon: '📜', label: '로그' },
];

const LS = {
  lyrics: 'macham.lyrics.open',
  sideTab: 'macham.side.tab',
  railTab: 'macham.rail.tab',
  layout: 'macham.layout',        // 구버전 키 — 서버 저장이 없던 시절 값을 한 번 물려받는다
  prefs: 'macham.prefs',          // 서버 개인 설정의 로컬 거울
  theme: 'macham.theme',          // FOUC 방지 인라인 스크립트가 읽는 키
  seenChangelog: 'macham.changelog.seen',  // 어느 빌드까지 패치노트를 봤는지 (§30)
};

/* ── 테마 7종 + 시스템 따라가기 (§17) ──
 * 스와치 색은 tokens.css의 --surface-1 / --accent / --text-1 을 그대로 옮겨 적은 것이다.
 * 적용해 보기 전에는 그 테마의 토큰을 읽을 방법이 없어서 여기 한 벌 둔다.
 * tokens.css를 고치면 여기도 같이 고쳐야 색이 안 어긋난다. */
/* 밝은 테마 목록 — color-scheme 을 light 로 줘야 스크롤바·폼 위젯이 같이 밝아진다. */
const LIGHT_THEMES = new Set(['light', 'sepia']);

const THEMES = {
  auto: { label: '시스템 따라가기', desc: '기기가 밝으면 라이트, 어두우면 다크로 따라가요', swatch: null },
  dark: { label: '다크', desc: '거의 검정 배경에 남보라 강조', swatch: ['#0d111a', '#8b5cf6', '#f2f5fa'] },
  light: { label: '라이트', desc: '흰 배경. 밝은 방에서 잘 보여요', swatch: ['#ffffff', '#7c3aed', '#0f1622'] },
  midnight: { label: '미드나잇', desc: '짙은 곤색에 하늘색 강조', swatch: ['#1a1b26', '#7aa2f7', '#c8d3f5'] },
  slate: { label: '그레이', desc: '채도가 낮아 눈이 가장 덜 피곤해요', swatch: ['#22272e', '#539bf5', '#cdd9e5'] },
  sepia: { label: '베이지', desc: '따뜻한 종이색. 밝은 방에서 좋아요', swatch: ['#fdf6e3', '#9c5a2d', '#3a2f24'] },
  retro: { label: '레트로', desc: '옛날 CRT 터미널처럼 앰버 단색이에요', swatch: ['#12100a', '#ffb000', '#ffc457'] },
  nord: { label: '노르드', desc: '차가운 청회색 팔레트', swatch: ['#2e3440', '#88c0d0', '#eceff4'] },
};

/** 스와치 3색. `auto`는 고정 색이 없으니 지금 시스템이 가리키는 테마 것을 빌려 온다 —
 *  다크 색을 박아 두면 밝은 기기에서도 "시스템 따라가기"가 늘 어둡게 보인다. */
function themeSwatch(id) {
  const meta = THEMES[id];
  if (meta && meta.swatch) return meta.swatch;
  return THEMES[resolveTheme(id)]?.swatch || THEMES.dark.swatch;
}

/* 모바일 주소창 색. 각 테마의 --surface-0 이다. */
const THEME_META = {
  dark: '#07090f', light: '#f4f6fa', midnight: '#16161e', slate: '#1c2128',
  sepia: '#efe6d3', retro: '#0a0803', nord: '#242933',
};

/* 활동 로그 분류 (§13.4). 기본은 곡 + 재생목록만 켠다 — 투표까지 켜면 다른 게 안 보인다. */
const AUDIT_KINDS = {
  song: { icon: '🎵', label: '곡' },
  vote: { icon: '👍', label: '투표' },
  playback: { icon: '▶', label: '재생' },
  playlist: { icon: '📃', label: '재생목록' },
  moderation: { icon: '🛡', label: '관리' },
  admin: { icon: '⚙', label: '설정' },
};
const AUDIT_DEFAULT = ['song', 'playlist'];

/* 차트 분류 카드 (§15.3). 서버가 주는 순서를 우선하고, 이건 아이콘·설명 사전이다. */
const CHART_CATEGORIES = {
  ours: { icon: '⭐', label: '우리 차트', desc: '우리가 실제로 많이 튼 곡' },
  popular: { icon: '🔥', label: '인기', desc: '지금 많이 듣는 곡' },
  region: { icon: '🌏', label: '나라별', desc: '미국·일본·영국' },
  genre: { icon: '🎸', label: '장르', desc: 'K-Pop·힙합·록·R&B' },
  karaoke: { icon: '🎤', label: '노래방', desc: 'TJ·금영 장르별' },
  soundcloud: { icon: '☁', label: 'SoundCloud', desc: '사운드클라우드 인기곡' },
};
const CHART_PERIODS = [['week', '이번 주'], ['month', '이번 달'], ['all', '전체']];

/* 화면 배치 6종 (§7.2). DOM은 한 벌이다.
 * 배치가 바꾸는 건 CSS 그리드 배치와 "누가 스크롤하는가"뿐이고, 기능은 6종 전부에서 똑같이 된다.
 * 패널형만 도킹 트리를 따로 그린다(패널 노드는 새로 만들지 않고 옮기기만 한다).
 * cells는 미니 도식 — true인 칸이 '주 영역'이다. */
const LAYOUTS = {
  three: {
    label: '3단',
    desc: '검색·대기열을 늘 왼쪽에 띄워요.',
    extra: '처음이라면 이게 가장 무난해요.',
    when: '뭘 고를지 모르겠으면 이걸 고르세요.',
    hint: '가장 무난해요',
    cells: [false, true, false],
    drawerUnder: 1280,
  },
  two: {
    label: '2단',
    desc: '재생 화면을 넓게 쓰고 채팅을 오른쪽에 고정해요.',
    extra: '노래를 크게 보면서 대화도 놓치지 않아요.',
    when: '노래를 크게 보면서 대화도 같이 볼 때 좋아요.',
    cells: [true, false, false],
    drawerUnder: 981,
  },
  focus: {
    label: '집중',
    desc: '재생 화면만 크게 띄우고 나머지는 접어 둬요.',
    extra: '검색·채팅은 가장자리에서 잠깐 열었다 닫아요.',
    when: '음악만 틀어놓고 볼 때 좋아요.',
    cells: [true],
    drawerUnder: 99999,     // 언제나 가장자리 오버레이로 연다
  },
  dj: {
    label: 'DJ',
    desc: '왼쪽에 넓은 대기열과 검색, 오른쪽에 작은 재생 화면과 채팅이에요.',
    extra: '곡을 계속 고르고 넣기 좋아요.',
    when: '곡을 계속 고르고 넣을 때 좋아요.',
    cells: [true, false, false],
    drawerUnder: 1180,
  },
  talk: {
    label: '수다',
    desc: '오른쪽 채팅을 가장 넓게 쓰고, 왼쪽은 작은 재생 바와 대기열이에요.',
    extra: '대화가 주인공일 때 좋아요.',
    when: '채팅이 주인공일 때 좋아요.',
    cells: [false, true],
    drawerUnder: 900,
  },
  panel: {
    label: '패널',
    desc: '창을 원하는 대로 붙이고 나눠요.',
    extra: '탭을 끌어다 붙이면 내 마음대로 배치돼요. 넓은 화면에서 진가가 나와요.',
    when: '내 맘대로 짜고 싶을 때 좋아요.',
    cells: [true, false, false, false],
    drawerUnder: 0,
  },
};

/* 패널형에서 다룰 수 있는 창 목록. id는 prefs.panelLayout에 그대로 저장된다.
 * 제안은 §11에서 모달로 빠졌으므로 패널 목록에 없다. */
const PANELS = {
  now: { icon: '▶', label: '지금 재생' },
  queue: { icon: '📋', label: '대기열' },
  search: { icon: '🔎', label: '검색' },
  charts: { icon: '📈', label: '차트' },
  library: { icon: '📚', label: '보관함' },
  chat: { icon: '💬', label: '채팅' },
  members: { icon: '👥', label: '멤버' },
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
  lyricsOpen: '1', webPlayback: '0', webVolume: '60', webOffset: '0',
  auditFilter: null,          // JSON 배열 문자열. 없으면 곡+재생목록
  notify: null,               // {"song":1,"mention":1,"reply":1}
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

/* ═══════════════════════ 테마 (§17) ═══════════════════════
 * tokens.css가 [data-theme="X"]로 토큰을 통째로 갈아 끼운다. 여기서는 값 하나만 박으면 된다.
 * `auto`는 값이 아니라 규칙이라 여기서도 풀어 준다(remote_page.rs의 FOUC 스크립트도 auto를 푼다 —
 * 두 곳이 같은 규칙을 따라야 첫 페인트와 이후 페인트가 안 어긋난다).
 */

let themePreview = null;        // 미리보기 중 되돌릴 원래 값

function systemTheme() {
  return window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

/** 저장된 선택값. `auto`도 그대로 돌려준다. */
function themeChoice() {
  const saved = prefGet('theme');
  if (saved && THEMES[saved]) return saved;
  let local = null;
  try { local = localStorage.getItem(LS.theme); } catch { /* 시크릿 모드 */ }
  return local && THEMES[local] ? local : 'dark';
}

/** 실제로 화면에 박히는 값. `auto`는 여기서 풀린다. */
function resolveTheme(choice) {
  return choice === 'auto' ? systemTheme() : (THEMES[choice] ? choice : 'dark');
}

/** 화면에만 적용한다(미리보기). 저장은 하지 않는다. */
function paintTheme(choice) {
  const value = resolveTheme(choice);
  const root = document.documentElement;
  root.dataset.theme = value;
  // Chromium 은 문서의 color-scheme 을 **첫 페인트 때 확정**하고, 이후 data-theme 이 바뀌어도
  // CSS 의 color-scheme 선언을 다시 읽지 않는다. 그래서 스크롤바와 폼 기본 위젯만
  // 반대 색으로 남는다. 인라인 스타일은 바로 먹으므로 여기서 같이 박아 준다.
  root.style.colorScheme = LIGHT_THEMES.has(value) ? 'light' : 'dark';
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute('content', THEME_META[value] || THEME_META.dark);
  if (el.themeBtn) el.themeBtn.textContent = themeIcon(choice);
  readVizColors();
  scheduleViz();
}

function themeIcon(choice) {
  if (choice === 'auto') return '🌗';
  return resolveTheme(choice) === 'light' ? '☀' : '🌙';
}

/** 고른 값을 확정한다. 계정(prefs)과 로컬 거울 양쪽에 남긴다. */
function commitTheme(choice) {
  const value = THEMES[choice] ? choice : 'dark';
  themePreview = null;
  paintTheme(value);
  prefSet('theme', value);
  try { localStorage.setItem(LS.theme, value); } catch { /* 시크릿 모드 */ }
}

/* 서버 셸의 FOUC 스크립트는 저장값을 그대로 박는다. `auto`만 여기서 한 번 풀어 준다.
 * el 이 아직 없는 시점이라 paintTheme 대신 최소한만 만진다. */
if (themeChoice() === 'auto') {
  const boot = systemTheme();
  document.documentElement.dataset.theme = boot;
  document.documentElement.style.colorScheme = LIGHT_THEMES.has(boot) ? 'light' : 'dark';
}

/* 시스템 따라가기를 골라 둔 사람은 OS 설정이 바뀌면 같이 바뀌어야 한다. */
if (window.matchMedia) {
  const media = window.matchMedia('(prefers-color-scheme: light)');
  const onChange = () => { if (themeChoice() === 'auto' && !themePreview) paintTheme('auto'); };
  if (media.addEventListener) media.addEventListener('change', onChange);
  else if (media.addListener) media.addListener(onChange);
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

/* 우측 사이드가 드로어로 빠지는 구간인가. 3단은 1280px 미만, 2단은 981px 미만,
 * 집중 배치는 폭과 무관하게 언제나(가장자리 오버레이). */
function drawerActive() {
  if (narrowScreen()) return false;
  const layout = effectiveLayout();
  if (layout === 'panel') return false;
  return window.innerWidth < (LAYOUTS[layout] || LAYOUTS.three).drawerUnder;
}

/* 좌측 레일도 오버레이로 빠지는 배치는 집중 하나뿐이다. */
function railDrawerActive() {
  return !narrowScreen() && effectiveLayout() === 'focus';
}

function openRailDrawer(open) {
  if (!el.rail) return;
  el.rail.dataset.open = open ? '1' : '0';
  el.railDrawerBtn?.setAttribute('aria-expanded', String(!!open));
  syncScrim();
  if (open) marquee.scan(el.rail);
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
let autoplaySheet = null;      // 열려 있는 자동 재생 시트 { handle, body } — 갱신이 오면 안을 다시 그린다
let autoplayState = null;      // { mode, recentCount, genres, policy, genreOptions, canEdit }
let myScore = null;            // 마참 점수 (§22.4)
let chartState = null;         // 차트 화면 상태 — 없으면 서버가 차트를 모른다는 뜻
let suggestUnread = false;
let queueTotal = 0;            // 서버가 잘라 보냈을 때의 전체 곡 수 (§18.2)
let queueTruncated = false;

/* ── 재정렬 주기 (§5 · §18.2 (3)) ──
 * **서버가 정한다.** 대기열이 길어지면 서버가 5초에서 15초로 늦추는데(`sortPeriodSeconds`),
 * 화면이 5초를 세면 카운트다운이 세 번 헛돌고 헤더도 거짓말을 한다.
 * 그래서 하드코딩된 5·15 는 여기 한 곳에서만 "서버가 아직 안 알려줬을 때의 기본값"으로 쓴다. */
const SORT_PERIOD_DEFAULT = 5;
let sortPeriodSec = SORT_PERIOD_DEFAULT;

/** 서버가 알려 준 재정렬 주기(초). 이상한 값은 기본값으로 되돌린다. */
function sortPeriodSeconds() {
  const value = Number(sortPeriodSec);
  if (!Number.isFinite(value) || value < 1 || value > 600) return SORT_PERIOD_DEFAULT;
  return Math.round(value);
}

function noteSortPeriod(value) {
  if (Number.isFinite(Number(value)) && Number(value) >= 1) sortPeriodSec = Math.round(Number(value));
}

/** 알림은 앱 스위치 → 종류 스위치 → 브라우저 권한을 다 통과해야 울린다 (§16 B3). */
function pushNotify(kind, payload) {
  if (!notifyOn(kind)) return null;
  return notify.push(payload);
}

/** 0 = 무제한 (§23.1). 숫자 설정을 화면에 쓸 때는 반드시 이걸로 통과시킨다. */
function fmtLimit(value, unit) {
  const n = Number(value);
  if (!Number.isFinite(n) || n <= 0) return '무제한';
  return `${n}${unit || ''}`;
}

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

/** 왜 못 하는지 + 누가 되는지 한 줄로 (§23.3). 정지 중이면 정지 사유가 이긴다.
 *  `권한 없음`은 답이 아니다. 조건을 그대로 말하고, 통과하는 대상(역할 이름 + 인원수)을 덧붙인다.
 *  사람 이름은 절대 나열하지 않는다 — 그러면 그 사람들이 부탁 받는 창구가 된다. */
function lockReason(key) {
  const state = store.get();
  if (state.conn === 'down') return '연결이 끊겨서 지금은 조작할 수 없어요. 새로고침해 주세요.';
  const suspension = state.suspension;
  if (suspension && (suspension.scope === 'all' || matchScope(suspension.scope, key))) {
    return `${SCOPE_LABELS[suspension.scope] || '전체'}가 정지돼 있어요 · ${suspensionRemain(suspension)}`;
  }
  if (state.tier === 'viewer') return '읽기 전용이라 조작할 수 없어요.';
  const entry = (state.permissions?.entries || []).find((row) => row.key === key);
  return [whyBlocked(entry, key), whoCan(entry), whereToChange()].filter(Boolean).join(' · ');
}

/** 1) 왜 안 되는지 — 조건을 그대로. */
function whyBlocked(entry, key) {
  if (!entry) return '이 기능을 쓸 권한이 없어요';
  if (entry.rule === 'disabled') return '이 기능은 서버에서 꺼 뒀어요';
  if (entry.reason) return String(entry.reason).replace(/\.$/, '');
  if (entry.rule === 'sameVoiceChannel') return '봇과 같은 음성 채널에 있어야 눌러요';
  if (entry.rule === 'administrator' || entry.rule === 'manager') return '서버 관리자만 할 수 있어요';
  if (entry.rule === 'owner') return '봇 주인만 할 수 있어요';
  if (entry.rule === 'configuredRole') {
    const roles = (entry.roleNames || []).map((name) => `@${name}`).join(' · ');
    return roles ? `${roles} 역할이 있어야 눌러요` : '지정된 역할이 있어야 눌러요';
  }
  // `모든 멤버만 할 수 있어요` 같은 비문을 만들면 안 된다 — 규칙 라벨을 그대로 문장에 끼우지 않는다 (§23.3)
  if (entry.rule === 'guildMember') return '이 서버의 멤버여야 눌러요';
  const label = entry.ruleLabel || RULE_LABELS[entry.rule];
  if (label) return `${label} 조건이라 지금은 눌러지지 않아요`;
  return '지금은 이 기능을 쓸 조건이 아니에요';
}

/** 2) 누구는 되는지 — 역할 이름 + 인원수까지만. */
function whoCan(entry) {
  if (!entry) return '';
  const names = entry.allowedRoleNames || entry.roleNames || [];
  const count = Number(entry.allowedCount);
  const who = names.length ? names.map((name) => `@${name}`).join(' · ') : '';
  if (who && Number.isFinite(count) && count > 0) return `지금은 ${who} 이 쓸 수 있어요 (${count}명)`;
  if (who) return `지금은 ${who} 이 쓸 수 있어요`;
  if (Number.isFinite(count) && count > 0) return `${count}명이 쓸 수 있어요`;
  return '';
}

/** 4) 어디서 바뀌는지 — 못 들어가는 사람에게 링크를 보여주면 놀리는 거다. */
function whereToChange() {
  return can('console') ? '관리 콘솔에서 바꿀 수 있어요' : '';
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

/** `권한 없음`은 답이 아니다 (§23.3). 이유를 안 넘긴 호출부가 있어도 화면이 그 말을 하면 안 되므로
 *  최후의 문구도 조건을 가리키는 쪽으로 둔다. */
const LOCK_FALLBACK = '지금은 이 버튼을 쓸 조건이 아니에요. 서버 관리자가 정한 규칙을 따라요.';

/** 권한 없는 버튼은 숨기지 않는다. 비활성 모양 + 이유 툴팁으로 남긴다. */
function setLock(node, locked, reason) {
  if (!node) return node;
  if (node.__tipBase === undefined) node.__tipBase = node.getAttribute('data-tip') || '';
  node.setAttribute('aria-disabled', locked ? 'true' : 'false');
  node.classList.toggle('is-locked', !!locked);
  if (locked) {
    node.dataset.lockReason = reason || LOCK_FALLBACK;
    node.setAttribute('data-tip', reason || LOCK_FALLBACK);
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
      toast(node.dataset.lockReason || LOCK_FALLBACK, 'warn');
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
  // 드래그하기 전에도 지금 값을 읽을 수 있어야 한다. min/max 만 있으면 스크린리더가
  // "몇 픽셀인지"를 영영 못 말한다 (§20 · role=separator 규약).
  node.setAttribute('aria-valuenow', String(clampSize(key, sizeFor(effectiveLayout(), key))));
  return node;
}

/** 채팅 열 안에서 목록과 입력창 사이를 세로로 조절한다. */
function bindComposeResize() {
  const node = h('div', {
    class: 'gutter gutter--row',
    role: 'separator', 'aria-orientation': 'horizontal', tabindex: '0',
    'aria-label': '채팅 입력창 높이',
    // 세로 손잡이도 값 범위를 말해야 한다. apply() 가 aria-valuenow 를 이어서 갱신한다.
    'aria-valuemin': '36', 'aria-valuemax': '260', 'aria-valuenow': '36',
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
    tip: '눌러서 지금 누가 듣고 보고 있는지 확인해요',
    onClick: () => { openWhoSheet(); },
  });

  el.themeBtn = h('button', {
    class: 'btn btn--ghost btn--icon', type: 'button', tip: '테마 고르기 · 7가지가 있어요',
    'aria-label': '테마 고르기', 'aria-haspopup': 'menu',
    onClick: (event) => openThemeMenu(event.currentTarget),
  }, themeIcon(themeChoice()));

  el.suggestBtn = h('button', {
    class: 'btn btn--ghost btn--icon hdr__suggest', type: 'button',
    tip: '제안 게시판 열기 · 불편한 걸 적어 두면 반영될 수도 있어요',
    'aria-label': '제안 게시판',
    onClick: openSuggestModal,
  }, '💡');
  el.suggestDot = h('span', { class: 'hdr__dot', hidden: true, 'aria-hidden': 'true' });
  el.suggestBtn.appendChild(el.suggestDot);

  // 집중 배치에서만 보이는 왼쪽 가장자리 손잡이. 검색·대기열을 잠깐 열었다 닫는다.
  el.railDrawerBtn = h('button', {
    class: 'btn btn--ghost btn--icon hdr__raildrawer', type: 'button',
    tip: '검색·대기열 열기', 'aria-label': '검색·대기열 열기',
    onClick: () => openRailDrawer(el.rail.dataset.open !== '1'),
  }, '📋');

  el.drawerBtn = h('button', {
    class: 'btn btn--ghost btn--icon hdr__drawer', type: 'button', tip: '채팅·멤버 열기',
    'aria-label': '우측 패널 열기',
    onClick: () => openSide(activeSideTab),
  }, '💬');
  el.drawerBadge = h('span', { class: 'badge', hidden: true }, '0');
  el.drawerBtn.appendChild(el.drawerBadge);

  el.meBtn = h('button', {
    class: 'hdr__me', type: 'button', 'aria-haspopup': 'menu', 'aria-expanded': 'false',
    tip: '내 권한 · 내 기록 · 설정',
    onClick: () => toggleMenu(el.meMenu, el.meBtn),
  });
  el.meMenu = buildProfileMenu();

  return h('header', { class: 'hdr' },
    h('div', { class: 'hdr__brand' }, '마참뮤직', h('small', null, 'REMOTE')),
    el.railDrawerBtn,
    h('div', { class: 'dd' }, el.guildBtn, el.guildMenu),
    h('div', { class: 'hdr__spacer' }),
    el.presenceBtn,
    el.suggestBtn,
    el.themeBtn,
    el.drawerBtn,
    h('div', { class: 'dd' }, el.meBtn, el.meMenu));
}

/* ── 테마 고르기 (§17.3) ──
 * 이름만 있으면 뭐가 뭔지 모른다. 스와치 세 색을 나란히 보여주고, 올리면 바로 미리보기로 적용한다.
 * 고르지 않고 닫으면 원래 테마로 되돌린다 — 실수로 바뀌어 있으면 그게 제일 당황스럽다.
 */
function openThemeMenu(anchor) {
  const original = themeChoice();
  themePreview = original;
  let done = false;

  const menu = h('div', { class: 'pop pop--menu themepop', role: 'menu', 'aria-label': '테마' });
  for (const [id, meta] of Object.entries(THEMES)) {
    const row = h('button', {
      class: 'themeopt', type: 'button', role: 'menuitemradio',
      'aria-checked': String(id === original),
      tip: meta.desc,
      onMouseenter: () => paintTheme(id),
      onFocus: () => paintTheme(id),
      onClick: () => { done = true; commitTheme(id); close(); toast(`${meta.label} 테마로 바꿨어요.`, 'ok'); },
    },
      h('span', { class: 'themeopt__sw', 'aria-hidden': 'true' },
        ...themeSwatch(id).map((color) => h('i', { style: { background: color } }))),
      h('span', { class: 'themeopt__main' },
        h('strong', null, meta.label),
        h('small', null, meta.desc)),
      id === original ? h('span', { class: 'themeopt__on', 'aria-hidden': 'true' }, '✓') : null);
    if (id === 'auto') row.classList.add('themeopt--auto');
    menu.appendChild(row);
  }

  const close = () => {
    if (!menu.isConnected) return;
    menu.remove();
    document.removeEventListener('pointerdown', onOutside, true);
    document.removeEventListener('keydown', onKey, true);
    if (!done) paintTheme(original);       // 확정 안 했으면 원래대로
    themePreview = null;
  };
  const onOutside = (event) => { if (!menu.contains(event.target) && event.target !== anchor) close(); };
  const onKey = (event) => { if (event.key === 'Escape') { event.stopPropagation(); close(); } };

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  const box = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(rect.right - box.width, window.innerWidth - box.width - 8))}px`;
  menu.style.top = `${Math.min(rect.bottom + 6, Math.max(8, window.innerHeight - box.height - 8))}px`;
  menu.addEventListener('mouseleave', () => { if (!done) paintTheme(themePreview || original); });
  menu.querySelector('button')?.focus();
  setTimeout(() => {
    document.addEventListener('pointerdown', onOutside, true);
    document.addEventListener('keydown', onKey, true);
  }, 0);
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

  // 서버 승인 (§26.2) — 봇 주인만. 대기 중인 서버가 있으면 개수를 배지로 붙인다.
  el.approvalBtn = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem', hidden: true,
    tip: '봇을 초대한 서버를 승인하거나 막아요',
    onClick: () => { closeMenus(); openApprovalSheet(); },
  }, h('span', null, '🛡'), h('span', null, '서버 승인'), el.approvalCount = h('span', { class: 'count', hidden: true }));

  el.statsBtn = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem',
    tip: '담은 곡·재생·받은 반응을 모아 봐요',
    onClick: () => { closeMenus(); openStatsModal(null); },
  }, h('span', null, '📊'), h('span', null, '내 기록'));

  // 버전 버튼을 누르면 역대 패치노트가 뜬다 (§30).
  el.changelogBtn = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem',
    tip: '무엇이 바뀌었는지 모아 봐요',
    onClick: () => { closeMenus(); openChangelog(); },
  }, h('span', null, '📝'), h('span', null, '패치노트'),
    h('span', { class: 'count' }, ctx.buildId || ''));

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
    el.meHead, permBtn, el.statsBtn, el.changelogBtn, el.consoleBtn, el.approvalBtn, el.opsLink,
    h('div', { class: 'dd__sep' }),
    buildLayoutPicker(),
    h('div', { class: 'dd__sep' }),
    buildNotifyBox(),
    logout);
}

/* ── 알림 켜고 끄기 (§16 B3) ──
 * 브라우저 권한은 JS로 철회할 수 없다. 그래서 앱이 자기 스위치를 따로 가진다.
 * 꺼 두면 브라우저 알림도, 탭 제목 숫자도 안 띄운다.
 */

const NOTIFY_KINDS = [
  ['song', '내 신청곡이 시작될 때'],
  ['mention', '나를 부를 때 (@멘션)'],
  ['reply', '내 메시지에 답장이 올 때'],
];

function notifySettings() {
  const raw = prefGet('notify');
  let parsed = null;
  try { parsed = raw ? JSON.parse(raw) : null; } catch { parsed = null; }
  const base = { on: 1, song: 1, mention: 1, reply: 1 };
  if (parsed && typeof parsed === 'object') Object.assign(base, parsed);
  return base;
}

function saveNotifySettings(next) {
  prefSet('notify', JSON.stringify(next));
  renderNotifyBox();
  notify.badge(titleBadgeOn() ? unread : 0);
}

/** 탭 제목의 숫자를 띄울지. **브라우저 알림 권한과는 무관하다** — 제목 숫자는 브라우저 알림이
 *  아니라 이 화면이 직접 쓰는 값이라 권한이 필요 없다. 판단 기준은 앱 스위치 하나뿐이고,
 *  두 군데가 서로 다른 기준을 쓰면 켰다 껐다에 따라 숫자가 오락가락한다 (§16 B3). */
function titleBadgeOn() {
  return !!notifySettings().on;
}

/** 앱 스위치 + 종류별 스위치 + 브라우저 권한을 모두 통과해야 울린다. */
function notifyOn(kind) {
  if (!notify.granted()) return false;
  const settings = notifySettings();
  if (!settings.on) return false;
  return kind ? !!settings[kind] : true;
}

function buildNotifyBox() {
  el.notifyMain = h('button', {
    class: 'dd__item dd__item--switch', type: 'button', role: 'menuitemcheckbox',
    onClick: onNotifyMainClick,
  },
    h('span', null, '🔔'),
    h('span', { class: 'dd__grow' }, '알림'),
    h('span', { class: 'sw', 'aria-hidden': 'true' }, h('i')));

  el.notifyKinds = NOTIFY_KINDS.map(([key, label]) => h('button', {
    class: 'dd__item dd__item--sub', type: 'button', role: 'menuitemcheckbox',
    tip: `${label} 알려 드릴게요`,
    onClick: () => {
      const next = notifySettings();
      next[key] = next[key] ? 0 : 1;
      saveNotifySettings(next);
    },
  },
    h('span', null, ''),
    h('span', { class: 'dd__grow' }, label),
    h('span', { class: 'sw sw--sm', 'aria-hidden': 'true' }, h('i'))));

  el.notifyBox = h('div', { class: 'notifybox' }, el.notifyMain, ...el.notifyKinds);
  return el.notifyBox;
}

async function onNotifyMainClick() {
  if (!notify.supported()) { toast('이 브라우저는 알림을 지원하지 않아요.', 'warn'); return; }
  if (Notification.permission === 'denied') {
    toast('브라우저 설정에서 알림이 막혀 있어요. 주소창 옆 자물쇠에서 풀 수 있어요.', 'warn');
    return;
  }
  if (Notification.permission === 'default') {
    const result = await notify.ask();
    if (result !== 'granted') { toast('알림 권한을 받지 못했어요.', 'warn'); renderNotifyBox(); return; }
    const next = notifySettings();
    next.on = 1;
    saveNotifySettings(next);
    toast('알림을 켰어요. 다른 탭을 보고 있을 때만 울려요.', 'ok');
    return;
  }
  const next = notifySettings();
  next.on = next.on ? 0 : 1;
  saveNotifySettings(next);
  toast(next.on ? '알림을 켰어요.' : '알림을 껐어요. 탭 제목의 숫자도 안 띄워요.', 'ok');
}

function renderNotifyBox() {
  if (!el.notifyMain) return;
  const supported = notify.supported();
  const permission = supported ? Notification.permission : 'denied';
  const settings = notifySettings();
  const on = supported && permission === 'granted' && !!settings.on;

  const label = !supported ? '알림을 못 써요'
    : permission === 'denied' ? '알림이 막혀 있어요'
      : permission === 'default' ? '알림 켜기'
        : on ? '알림 켜짐' : '알림 꺼짐';
  el.notifyMain.children[1].textContent = label;
  el.notifyMain.setAttribute('aria-checked', String(on));
  el.notifyMain.dataset.on = on ? '1' : '0';
  setLock(el.notifyMain, !supported || permission === 'denied',
    !supported ? '이 브라우저는 알림을 지원하지 않아요'
      : '브라우저 설정에서 알림이 막혀 있어요');
  if (supported && permission !== 'denied') {
    el.notifyMain.setAttribute('data-tip', permission === 'default'
      ? '누르면 브라우저에 알림 권한을 물어봐요'
      : on ? '지금은 알림이 켜져 있어요 · 누르면 꺼요' : '지금은 알림이 꺼져 있어요 · 누르면 켜요');
  }

  el.notifyKinds.forEach((node, index) => {
    const key = NOTIFY_KINDS[index][0];
    node.hidden = !on;
    node.setAttribute('aria-checked', String(!!settings[key]));
    node.dataset.on = settings[key] ? '1' : '0';
  });
}

/* ── 화면 배치 고르기 ── */

function buildLayoutPicker() {
  el.layoutOpts = Object.entries(LAYOUTS).map(([id, def]) => layoutOption(id, def, 'menuitemradio'));

  const reset = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem',
    tip: '기본 배치로 되돌려요. 저장해 둔 슬롯은 그대로예요',
    onClick: () => { closeMenus(); resetPanelLayout(); },
  }, h('span', null, '↺'), h('span', null, '패널 배치를 기본으로 되돌리기'));
  el.panelResetBtn = reset;

  // 슬롯·공유는 패널형에서만 의미가 있다. 다른 배치에서는 숨긴다.
  const slots = h('button', {
    class: 'dd__item', type: 'button', role: 'menuitem',
    tip: '배치를 여러 개 저장해 두거나, 코드로 남과 주고받아요',
    onClick: () => { closeMenus(); openSlotSheet(); },
  }, h('span', null, '▦'), h('span', null, '배치 슬롯 · 공유'));
  el.panelSlotsBtn = slots;

  return frag(
    h('div', { class: 'dd__label' }, '화면 배치'),
    h('div', { class: 'lay', role: 'group', 'aria-label': '화면 배치' }, ...el.layoutOpts),
    slots,
    reset);
}

/** 프로필 메뉴와 첫 진입 시트가 같은 카드를 쓴다. 6개라 3×2로 놓인다. */
function layoutOption(id, def, role) {
  return h('button', {
    class: 'lay__opt', type: 'button', role,
    'aria-checked': String(id === activeLayout),
    dataset: { layout: id },
    tip: def.when,
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
  if (el.panelSlotsBtn) el.panelSlotsBtn.hidden = activeLayout !== 'panel';
}

function applyLayout() {
  const atEnd = nearChatEnd();
  const layout = effectiveLayout();

  // 전환 순간에 드로어가 슬라이드해 들어오는 등의 잔상이 남지 않게 한 프레임 동안 전환을 끈다
  el.portal.dataset.swap = '1';
  document.documentElement.dataset.layout = layout;
  el.portal.dataset.layout = layout;
  openDrawer(false);
  openRailDrawer(false);
  el.railDrawerBtn.hidden = !railDrawerActive();
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
  if (THEMES[savedTheme] && resolveTheme(savedTheme) !== document.documentElement.dataset.theme) {
    paintTheme(savedTheme);
    try { localStorage.setItem(LS.theme, savedTheme); } catch { /* 시크릿 모드 */ }
  }
  syncAuditChips();
  renderNotifyBox();

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
    tip: def.when,
    onClick: () => { setLayout(id, true); handle?.close(id); },
  },
    h('span', { class: `lay__glyph lay__glyph--${id}`, 'aria-hidden': 'true' },
      ...def.cells.map((main) => h('i', { class: main ? 'is-main' : null }))),
    h('strong', null, def.label, def.hint ? h('span', { class: 'chip chip--accent' }, def.hint) : null),
    h('p', null, def.desc),
    h('small', null, def.extra)));

  handle = sheet({
    title: '화면을 어떻게 볼까요',
    desc: '처음 오셨네요. 여섯 가지 중 마음에 드는 걸 고르면 바로 그렇게 보여드려요.',
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
  rail: ['search', 'charts', 'queue', 'library'],
  side: ['chat', 'members', 'recent', 'audit'],
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
      b: { type: 'tabs', panels: ['queue', 'search', 'charts', 'library'], active: 'queue' },
    },
    b: { type: 'tabs', panels: ['chat', 'members', 'recent', 'audit'], active: 'chat' },
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

/* ═══════════════════════ 배치 슬롯과 공유 (패널형 전용) ═══════════════════════
 * 슬롯은 계정(prefs.panelSlots)에 저장한다. 기기를 바꿔도 따라온다.
 * 공유는 서버를 거치지 않는다 — 트리를 그대로 문자열로 만들어 주고받는다.
 * 남의 코드를 붙여넣는 자리라 sanitizeTree 로 반드시 걸러서 쓴다.
 */

const MAX_SLOTS = 5;

function readSlots() {
  const raw = prefJson('panelSlots');
  if (!Array.isArray(raw)) return [];
  return raw
    .map((slot) => {
      const tree = sanitizeTree(slot && slot.tree);
      if (!tree) return null;
      return { name: escapeText(String(slot.name || '이름 없음')).slice(0, 24), tree };
    })
    .filter(Boolean)
    .slice(0, MAX_SLOTS);
}

function writeSlots(slots) {
  prefSet('panelSlots', JSON.stringify(slots.slice(0, MAX_SLOTS)));
}

function saveSlot(name) {
  if (!dockTree) return;
  const slots = readSlots();
  const clean = escapeText(String(name || '').trim()).slice(0, 24) || `배치 ${slots.length + 1}`;
  const tree = serializeTree(dockTree);
  const at = slots.findIndex((slot) => slot.name === clean);
  if (at >= 0) slots[at] = { name: clean, tree };
  else if (slots.length >= MAX_SLOTS) { toast(`슬롯은 ${MAX_SLOTS}개까지예요. 하나를 덮어써 주세요.`, 'warn'); return; }
  else slots.push({ name: clean, tree });
  writeSlots(slots);
  toast(`"${clean}" 에 저장했어요.`, 'ok');
}

function loadSlot(index) {
  const slot = readSlots()[index];
  if (!slot) return;
  dockTree = slot.tree;
  savePanelLayout();
  if (panelMode()) renderDock();
  toast(`"${slot.name}" 배치를 불러왔어요.`, 'ok');
}

function deleteSlot(index) {
  const slots = readSlots();
  if (!slots[index]) return;
  const [gone] = slots.splice(index, 1);
  writeSlots(slots);
  toast(`"${gone.name}" 슬롯을 지웠어요.`, 'ok');
}

/** 트리 → 공유 코드. 사람이 실수로 자르기 쉬운 문자를 피하려고 base64url 로 만든다. */
function encodeLayout(tree) {
  const json = JSON.stringify(serializeTree(tree));
  const bytes = new TextEncoder().encode(json);
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return 'MCM1' + btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** 공유 코드 → 트리. **반드시 sanitizeTree 를 통과시킨다** — 남이 준 문자열이다. */
function decodeLayout(code) {
  const raw = String(code || '').trim().replace(/\s+/g, '');
  if (!raw.startsWith('MCM1')) return null;
  try {
    const b64 = raw.slice(4).replace(/-/g, '+').replace(/_/g, '/');
    const binary = atob(b64 + '='.repeat((4 - (b64.length % 4)) % 4));
    const bytes = Uint8Array.from(binary, (ch) => ch.charCodeAt(0));
    return sanitizeTree(JSON.parse(new TextDecoder().decode(bytes)));
  } catch {
    return null;
  }
}

async function copyLayoutCode() {
  if (!dockTree) return;
  const code = encodeLayout(dockTree);
  try {
    await navigator.clipboard.writeText(code);
    toast('배치 코드를 복사했어요. 붙여넣어 공유하세요.', 'ok');
  } catch {
    // 클립보드가 막힌 환경(비 HTTPS 등)에서는 직접 고를 수 있게 보여준다.
    sheet({
      title: '배치 코드',
      desc: '아래 코드를 전체 복사해서 공유해 주세요.',
      body: h('textarea', {
        class: 'field', rows: '4', readonly: true, value: code,
        ref: (node) => setTimeout(() => { node.focus(); node.select(); }, 20),
      }),
      actions: [{ label: '닫기', kind: 'primary', value: true, autofocus: false }],
    });
  }
}

async function importLayoutSheet() {
  const input = h('textarea', {
    class: 'field', rows: '4', placeholder: 'MCM1... 로 시작하는 코드를 붙여넣어 주세요',
    ref: (node) => setTimeout(() => node.focus(), 20),
  });
  const ok = await sheet({
    title: '배치 코드 가져오기',
    desc: '남이 준 코드를 붙여넣으면 그 사람 배치를 그대로 써요. 지금 배치는 덮어써요.',
    body: input,
    actions: [
      { label: '취소', kind: 'ghost', value: false },
      { label: '가져오기', kind: 'primary', value: true },
    ],
  });
  if (!ok) return;
  const tree = decodeLayout(input.value);
  if (!tree) { toast('코드를 알아볼 수 없어요. 전체를 복사했는지 확인해 주세요.', 'danger'); return; }
  dockTree = tree;
  savePanelLayout();
  if (panelMode()) renderDock();
  toast('배치를 가져왔어요.', 'ok');
}

/** 슬롯 목록 시트 — 저장·불러오기·삭제·공유를 한 자리에 모은다. */
async function openSlotSheet() {
  const slots = readSlots();
  const nameInput = h('input', {
    class: 'field', type: 'text', maxlength: '24',
    placeholder: `지금 배치 이름 (예: 노래고를때)`,
    ref: (node) => setTimeout(() => node.focus(), 20),
  });
  const list = h('div', { class: 'slotlist' },
    slots.length
      ? slots.map((slot, index) => h('div', { class: 'slotrow' },
        h('button', {
          class: 'slotrow__load', type: 'button', tip: `"${slot.name}" 배치로 바꿔요`,
          onClick: () => { closeSheets(); loadSlot(index); },
        }, h('span', null, '▦'), h('span', null, slot.name)),
        h('button', {
          class: 'btn btn--sm btn--ghost', type: 'button', tip: '지금 배치로 덮어써요',
          onClick: () => { saveSlot(slot.name); },
        }, '덮어쓰기'),
        h('button', {
          class: 'btn btn--sm btn--danger', type: 'button', tip: '이 슬롯을 지워요',
          onClick: (event) => { deleteSlot(index); event.currentTarget.closest('.slotrow').remove(); },
        }, '✕')))
      : h('p', { class: 'hint' }, '저장한 배치가 아직 없어요.'));

  const action = await sheet({
    title: '배치 슬롯',
    desc: `배치를 ${MAX_SLOTS}개까지 저장해 두고 골라 쓸 수 있어요. 계정에 저장돼서 다른 기기에서도 그대로예요.`,
    body: h('div', { class: 'slotbox' }, list, nameInput),
    wide: true,
    actions: [
      { label: '닫기', kind: 'ghost', value: 'close' },
      { label: '📋 코드 복사', kind: 'ghost', value: 'copy' },
      { label: '📥 코드 가져오기', kind: 'ghost', value: 'import' },
      { label: '＋ 지금 배치 저장', kind: 'primary', value: 'save' },
    ],
  });
  if (action === 'save') saveSlot(nameInput.value);
  else if (action === 'copy') copyLayoutCode();
  else if (action === 'import') importLayoutSheet();
}

/** 시트 안의 버튼이 시트를 닫아야 할 때 쓴다. */
function closeSheets() {
  document.querySelectorAll('.sheet-back').forEach((back) => back.remove());
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

  // 닫은 패널을 되살리는 유일한 입구다. 아이콘만 두면 패널을 닫은 사람이
  // "되돌릴 방법이 없다"고 느낀다. 라벨을 붙이고, 닫아 둔 게 있으면 개수까지 보여준다.
  const closedCount = Object.keys(PANELS).length - openPanels().size;
  tabs.appendChild(h('button', {
    class: 'dk-add', type: 'button',
    dataset: { closed: closedCount > 0 ? '1' : '0' },
    tip: closedCount > 0
      ? `닫아 둔 창 ${closedCount}개를 여기서 다시 열어요`
      : '창을 추가해요. 지금은 전부 열려 있어요',
    'aria-label': '패널 추가',
    onClick: (event) => openAddPanelMenu(group, event.currentTarget),
  }, h('span', { 'aria-hidden': 'true' }, '＋'), h('span', null, '창'),
     closedCount > 0 ? h('span', { class: 'dk-add__n' }, String(closedCount)) : null));

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

/* 탭 하나는 "전환"과 "닫기" 두 조작을 가진다. `<button>` 안에 `<button>` 을 넣으면 HTML이
 * 깨지고(브라우저가 파싱 단계에서 풀어 버린다) 보조기술에서 두 버튼이 하나로 뭉친다.
 * 그래서 바깥은 role=tab 을 단 div 로 두고 안쪽 ✕ 만 진짜 버튼으로 남긴다.
 * div 는 키보드가 안 먹으므로 Enter/Space 를 직접 받는다. */
function buildDockTab(group, id) {
  const meta = PANELS[id] || { icon: '·', label: id };
  const pick = () => {
    // 끌어서 옮긴 직후에 click이 한 번 더 온다. 그건 탭 전환이 아니다.
    if (tab.__afterDrag) { tab.__afterDrag = false; return; }
    activateDockPanel(group, id);
  };
  const tab = h('div', {
    class: 'dk-tab', role: 'tab', tabindex: '0',
    'aria-selected': String(group.active === id),
    tip: `${meta.label} 창을 앞으로 가져와요`,
    dataset: { panel: id },
    onClick: pick,
    onKeydown: (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      pick();
    },
  },
    h('span', { 'aria-hidden': 'true' }, meta.icon),
    h('span', null, meta.label));

  // 마지막 하나 남은 창은 못 닫는다. 눌러도 아무 일이 없는 버튼을 그냥 두면 고장으로 보이니
  // 미리 비활성으로 알려 준다. (§23.3 — 막힌 컨트롤에는 이유가 붙는다)
  const isLast = openPanels().size <= 1;
  const close = h('button', {
    class: ['dk-x', isLast && 'is-locked'],
    type: 'button',
    tip: isLast ? '창이 하나뿐이라 닫을 수 없어요' : `${meta.label} 닫기`,
    'aria-label': isLast ? '창이 하나뿐이라 닫을 수 없어요' : `${meta.label} 닫기`,
    'aria-disabled': isLast ? 'true' : null,
    onClick: (event) => { event.stopPropagation(); if (!isLast) closeDockPanel(id); },
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
  if (id === 'audit') loadAudit();
  if (id === 'charts') loadCharts();
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
  // 탭이 4개가 되니 아이콘 + 짧은 라벨로 (§15.3)
  const tabs = [
    { id: 'search', icon: '🔎', label: '검색', tip: '곡을 찾아 대기열에 담아요' },
    { id: 'charts', icon: '📈', label: '차트', tip: '인기·나라·장르·노래방 차트에서 곡을 담아요' },
    { id: 'queue', icon: '📋', label: '대기열', tip: '다음에 나갈 곡들이에요' },
    { id: 'library', icon: '📚', label: '보관함', tip: '좋아요·담아둔 곡·재생목록이에요' },
  ];
  el.railTabs = tabs.map((tab) => h('button', {
    class: 'tab', type: 'button', role: 'tab', id: `railtab-${tab.id}`,
    'aria-selected': String(tab.id === activeRailTab),
    dataset: { rail: tab.id }, tip: tab.tip,
    onClick: () => setRailTab(tab.id),
  }, h('span', { 'aria-hidden': 'true' }, tab.icon), tab.label));

  el.railPanes = {
    search: buildSearchPane(),
    charts: buildChartsPane(),
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
  if (narrowScreen()) document.body.dataset.pane = 'rail';
  else if (railDrawerActive()) openRailDrawer(true);
  if (id === 'search') el.searchInput?.focus();
  if (id === 'charts') loadCharts();
  syncMobileTabs();
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
    const kind = provider === 'YouTubeMusic' ? 'YouTubeMusic' : 'YouTube';
    return {
      title: snippet.title || videoId,
      artist: snippet.channelTitle || '',
      provider: kind,
      contentId: videoId,
      // **서버의 TrackRef 는 sourceUrl 이 필수다.** 이게 빠지면 본문 해석이 실패해서
      // 곡을 담을 때 422 가 나고 화면에는 "입력값을 확인해 주세요" 만 뜬다.
      // 서버 검색 경로는 Rust 가 채워 주지만 브라우저 검색은 여기서 채워야 한다.
      sourceUrl: kind === 'YouTubeMusic'
        ? `https://music.youtube.com/watch?v=${videoId}`
        : `https://www.youtube.com/watch?v=${videoId}`,
      cacheKey: `${kind}:${videoId}`,
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

/** 검색결과·차트·최근·보관함이 공유하는 트랙 행 */
function trackRow(track, source, extra, opts) {
  const inQueue = store.get().queue.some((item) => trackKey(item.track) === trackKey(track));
  const add = inQueue
    ? h('span', { class: 'chip', tip: '이미 대기열에 있는 곡이에요' }, '담김')
    : setLock(bindAct(h('button', { class: 'iconbtn', type: 'button', tip: '대기열에 담기', 'aria-label': '대기열에 담기' }, '＋'),
      () => enqueue(track)), !can('search'), lockReason('search'));

  const toList = bindAct(h('button', {
    class: 'iconbtn', type: 'button', tip: '재생목록에 넣기', 'aria-label': '재생목록에 넣기',
  }, '📃'), (event) => openPlaylistPicker(track, event.currentTarget));

  const save = bindAct(h('button', {
    class: 'iconbtn', type: 'button',
    tip: source === 'saved' ? '보관함에서 빼기' : '보관함에 담기',
    'aria-label': source === 'saved' ? '보관함에서 빼기' : '보관함에 담기',
  }, source === 'saved' ? '🗑' : '🔖'), () => toggleSaved(track, source !== 'saved'));
  setLock(save, !can('library'), lockReason('library'));

  const row = h('div', { class: 'row', dataset: { mqRow: '1' } },
    opts && opts.rank ? h('span', { class: 'row__rank', tip: `이 차트에서 ${opts.rank}위예요` }, String(opts.rank)) : null,
    h('img', { class: 'row__art', src: artUrl(track) || '', alt: '', loading: 'lazy' }),
    h('div', { class: 'row__main' },
      mqText(trackTitle(track), 'row__title'),
      h('div', { class: 'row__sub' }, extra || trackSub(track))),
    h('div', { class: 'row__acts' }, add, seedButton(track), toList, save));
  bindContextTarget(row, () => trackMenu(track, { source }));
  return row;
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
  // 툴팁 문구는 renderSortTick()이 서버 주기(sortPeriodSeconds)로 매번 다시 쓴다.
  el.sortTick = h('span', {
    class: 'sorttick', hidden: true, role: 'timer',
    tip: `${sortPeriodSeconds()}초마다 순서를 다시 정해요`,
  });

  // 5곡 이상일 때만 뜬다. 4곡 이하면 눈으로 봐도 알아서 굳이 안 띄운다 (§10.4)
  el.whyOrder = h('button', {
    class: 'whyorder', type: 'button', hidden: true,
    tip: '지금 순서가 왜 이렇게 정해졌는지 보여드려요',
    onClick: openModeSheet,
  }, '왜 이 순서인가요? ⓘ');

  el.queueClear = bindAct(h('button', {
    class: 'iconbtn iconbtn--danger', type: 'button', hidden: true,
    tip: '대기열을 통째로 비워요', 'aria-label': '대기열 비우기',
  }, '🧹'), clearQueue);

  el.queueList = h('div', { class: 'queue__list scroll', 'data-testid': 'queue-list' });
  el.queueList.addEventListener('scroll', onQueueScroll, { passive: true });

  const head = h('div', { class: 'queue__head' },
    h('h2', null, '대기열'),
    el.queueCount,
    h('span', { class: 'queue__spacer' }),
    el.queueClear,
    el.sortTick,
    el.modeBadge);
  bindContextTarget(head, () => queueHeadMenu());

  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'railtab-queue' },
    head,
    el.whyOrder,
    buildSeedBox(),
    el.queueList);
}

function renderQueueHead(state) {
  const shown = state.queue.length;
  const total = queueTotal || shown;
  // 대기열이 길어지면 서버가 정렬 주기를 늘린다. 왜 갑자기 느려졌는지 알 수 있어야 한다 (§18.3).
  // **판단 기준은 서버가 준 주기 하나뿐이다** — `> 500`·`15초` 를 화면이 따로 알고 있으면
  // 서버가 실제로 늦추지 않았을 때 헤더만 거짓말을 한다.
  const period = sortPeriodSeconds();
  el.queueCount.textContent = period > SORT_PERIOD_DEFAULT
    ? `${total}곡 · 정렬은 ${period}초마다`
    : (queueTruncated ? `${total}곡 (앞 ${shown}곡 표시)` : `${total}곡`);
  el.queueCount.setAttribute('data-tip', queueTruncated
    ? `대기열이 길어서 앞 ${shown}곡만 받아 왔어요. 아래로 내리면 더 불러와요`
    : `지금 대기열에 ${total}곡이 있어요`);

  const mode = MODES[state.queueMode] || MODES.score;
  clear(el.modeBadge);
  el.modeBadge.appendChild(document.createTextNode(`${mode.icon} ${mode.label}`));
  el.modeBadge.setAttribute('data-tip', mode.desc);
  el.modeBadge.appendChild(h('button', {
    class: 'modebadge__i', type: 'button', tip: '정렬 방식 3종 비교',
    'aria-label': '정렬 방식 설명', onClick: openModeSheet,
  }, 'ⓘ'));

  el.whyOrder.hidden = total < 5;
  el.queueClear.hidden = !can('queueEdit') || tierOf() === 'member' || !total;
  setLock(el.queueClear, !can('queueEdit'), lockReason('queueEdit'));
  renderSortTick();
}

async function clearQueue() {
  const total = queueTotal || store.get().queue.length;
  const ok = await confirmSheet({
    title: '대기열을 비울까요',
    desc: `${total}곡이 전부 지워져요. 되돌릴 수 없어요.`,
    danger: true, confirmText: `${total}곡 비우기`,
  });
  if (!ok) return;
  await call(() => api('/queue/action', { body: { action: 'clear' } }), '대기열을 비웠어요.');
}

/* ── 개인화 필드 보존 (§10.4 · §18.2 (1)) ──
 * `queue.set` 브로드캐스트는 접속자 전원이 같은 프레임을 받으므로 서버가 `isMine`/`myVote` 를
 * 일부러 `null` 로 비워 보낸다(remote.rs `broadcast_queue`). 옳은 설계다 —
 * 대신 **클라이언트가 되붙여야** 한다. 안 붙이면 재정렬이 한 번 돌 때마다
 *   · `내 곡` 칩이 사라지고
 *   · 내 투표의 `aria-pressed` 가 풀려 취소를 못 하고
 *   · `canVote = can('vote') && !item.isMine` 이 `null`(falsy)을 통과해 자기 곡 투표 버튼이 열린다(서버는 403).
 * 그래서 "개인화된 프레임"(cold/hot/`GET /queue`)을 볼 때마다 id별로 기억해 두고 다시 얹는다.
 */
const QUEUE_PERSONAL_CAP = 4000;
const queuePersonal = new Map();      // itemId → { isMine, myVote }

function notePersonalFields(items) {
  for (const item of items || []) {
    if (!item || item.id === undefined || item.id === null) continue;
    // `isMine` 이 채워져 있으면 그 프레임은 개인화된 것이다. 브로드캐스트 프레임은 여기서 걸러진다.
    if (item.isMine === null || item.isMine === undefined) continue;
    queuePersonal.set(item.id, { isMine: !!item.isMine, myVote: item.myVote ?? null });
  }
  if (queuePersonal.size > QUEUE_PERSONAL_CAP) {
    // 오래된 것부터 버린다(Map은 삽입 순서를 지킨다). 대기열에서 빠진 곡의 잔재를 모아 두지 않는다.
    const drop = queuePersonal.size - QUEUE_PERSONAL_CAP;
    let index = 0;
    for (const key of queuePersonal.keys()) {
      if (index++ >= drop) break;
      queuePersonal.delete(key);
    }
  }
}

/* ── 지금 재생 중인 곡의 투표자 (§10.4) ──
 * "제일 궁금한 곡"인데 서버의 `current` 페이로드에는 `score` 가 없다(`current_json`). 재생이 시작되면
 * 그 곡은 `upcoming` 에서 빠져 점수 조회 대상이 아니기 때문이다. 그래서 **대기열에 있던 동안의
 * 점수를 id 로 기억해 뒀다가** 재생으로 넘어가는 순간 그대로 쓴다. 서버가 나중에 `current.score` 를
 * 실어 주면 그쪽이 이긴다. */
const SCORE_CACHE_CAP = 600;
const queueScoreCache = new Map();    // itemId → score

function noteQueueScores(items) {
  for (const item of items || []) {
    if (!item || !item.score) continue;
    queueScoreCache.set(item.id, item.score);
  }
  if (queueScoreCache.size > SCORE_CACHE_CAP) {
    const drop = queueScoreCache.size - SCORE_CACHE_CAP;
    let index = 0;
    for (const key of queueScoreCache.keys()) {
      if (index++ >= drop) break;
      queueScoreCache.delete(key);
    }
  }
}

function scoreForCurrent(current) {
  if (!current) return null;
  return current.score || queueScoreCache.get(current.id) || null;
}

function notePersonalVote(itemId, myVote) {
  const saved = queuePersonal.get(itemId);
  queuePersonal.set(itemId, { isMine: saved ? saved.isMine : false, myVote: myVote ?? null });
}

function applyPersonalFields(items) {
  if (!Array.isArray(items)) return [];
  return items.map((item) => {
    if (!item) return item;
    if (item.isMine !== null && item.isMine !== undefined) return item;   // 서버가 이미 채웠으면 건드리지 않는다
    const saved = queuePersonal.get(item.id);
    if (!saved) return item;
    return Object.assign({}, item, { isMine: saved.isMine, myVote: saved.myVote });
  });
}

/* ── 이미 불러온 뒤쪽 페이지 (§18.2 (1)) ──
 * 브로드캐스트는 언제나 앞 200곡뿐이다. 스크롤해서 `GET /queue?offset=200` 으로 받아 둔 뒷부분을
 * 매번 버리면 (1) 목록이 300곡→200곡으로 줄어 스크롤이 튀고 (2) 다시 스크롤 → 다시 요청이라
 * 사실상 5초 주기 폴링이 된다(§23.2 위반). 그래서 앞 200곡에 없는 항목만 뒤에 이어 붙여 둔다.
 * 뒤쪽의 **정확한 순서**는 서버만 아는 값이라 다음 전체 로드(`loadHot`) 때 버린다. */
let queueTail = [];

function keepQueueTail(head) {
  if (!queueTail.length) return head;
  const seen = new Set(head.map((item) => item.id));
  const room = Math.max(0, (queueTotal || 0) - head.length);
  queueTail = queueTail.filter((item) => item && !seen.has(item.id)).slice(0, room);
  return queueTail.length ? head.concat(queueTail) : head;
}

/** `queue.set` 프레임 하나를 화면에 쓸 대기열로. core.js의 통째 교체를 여기서 되돌린다. */
function mergeQueueFrame(items) {
  return keepQueueTail(applyPersonalFields(items));
}

/* ── 가상 스크롤 (§18.2) ──
 * 5000곡을 전부 그리면 브라우저가 죽는다. 보이는 만큼만 노드를 만들고
 * 위아래는 **컨테이너 패딩**으로 채워 스크롤 길이를 맞춘다. FLIP도 화면에 보이는 항목에만 걸린다.
 *
 * 빈 상자(자식 노드)로 채우면 안 된다 — core.js의 `list()` 는 `__mmKey` 없는 자식을 **먼저 제거**한 뒤
 * `getBoundingClientRect()` 로 레이아웃을 강제한다. 그 순간 컨테이너 높이가 슬라이스 높이로 줄어
 * 브라우저가 `scrollTop` 을 잘라 버리고, 상자를 다시 붙여도 스크롤 위치는 안 돌아온다(§18.2 (2)).
 * 패딩은 자식이 아니라서 `list()` 가 손댈 수 없고, 그래서 잘림 자체가 일어나지 않는다.
 */
const VIRT_THRESHOLD = 80;      // 이 아래로는 통째로 그리는 게 더 빠르다
const VIRT_ROW_DEFAULT = 74;    // 항목 하나의 대략 높이(px) — 실제 값은 그린 뒤에 재서 갱신한다
const VIRT_OVERSCAN = 8;

let virtWindow = { from: 0, to: 0 };
// 좁은 레일에서는 `.qitem__acts { flex-wrap: wrap }` 로 행이 두 줄이 된다. 고정 74px 로 계산하면
// 스페이서 총합과 실제 높이가 어긋나 스크롤바 길이와 위치가 같이 틀어진다. 그래서 재서 쓴다.
let virtRow = VIRT_ROW_DEFAULT;
let virtRemeasureQueued = false;

function virtualizing() {
  return store.get().queue.length > VIRT_THRESHOLD;
}

function computeVirtWindow(items) {
  if (!virtualizing()) return { from: 0, to: items.length };
  const row = virtRow;
  const top = el.queueList.scrollTop;
  const height = el.queueList.clientHeight || 480;
  const from = Math.max(0, Math.floor(top / row) - VIRT_OVERSCAN);
  const to = Math.min(items.length, Math.ceil((top + height) / row) + VIRT_OVERSCAN);
  return { from, to };
}

/** 그려 놓은 항목의 실제 간격을 재서 다음 계산에 쓴다. 여기서 다시 그리면 재귀가 되므로
 *  값이 크게 달라졌을 때만 한 프레임 뒤에 딱 한 번 다시 그린다. */
function measureVirtRow() {
  const nodes = el.queueList.querySelectorAll('.qitem');
  if (nodes.length < 2) return;
  const first = nodes[0];
  const last = nodes[nodes.length - 1];
  const pitch = (last.offsetTop - first.offsetTop) / (nodes.length - 1);
  if (!Number.isFinite(pitch) || pitch < 32 || pitch > 400) return;
  if (Math.abs(pitch - virtRow) < 1) return;
  virtRow = pitch;
  if (virtRemeasureQueued || !virtualizing()) return;
  virtRemeasureQueued = true;
  requestAnimationFrame(() => {
    virtRemeasureQueued = false;
    if (virtualizing()) renderQueue(store.get());
  });
}

let virtRaf = 0;
function onQueueScroll() {
  if (!virtualizing() || virtRaf) return;
  virtRaf = requestAnimationFrame(() => {
    virtRaf = 0;
    const next = computeVirtWindow(store.get().queue);
    if (next.from === virtWindow.from && next.to === virtWindow.to) {
      maybeLoadMoreQueue();
      return;
    }
    renderQueue(store.get());
    maybeLoadMoreQueue();
  });
}

/** 서버가 앞 200곡만 보냈으면 바닥 근처에서 다음 쪽을 불러온다. 평소에는 아예 안 일어난다. */
let queueLoadingMore = false;
async function maybeLoadMoreQueue() {
  if (!queueTruncated || queueLoadingMore) return;
  const list_ = el.queueList;
  if (list_.scrollHeight - list_.scrollTop - list_.clientHeight > 400) return;
  queueLoadingMore = true;
  try {
    const state = store.get();
    const data = await api(`/queue?offset=${state.queue.length}&limit=200`);
    const more = data?.items || data?.queue || [];
    if (more.length) {
      // 이 응답은 개인화된 프레임이다. 브로드캐스트가 비워 보낼 값을 여기서 기억해 둔다.
      notePersonalFields(more);
      const seen = new Set(state.queue.map((item) => item.id));
      queueTail = queueTail.concat(more.filter((item) => !seen.has(item.id)));
      store.patch({ queue: state.queue.concat(more) });
      if (Number.isFinite(data?.queueTotal)) queueTotal = data.queueTotal;
      queueTruncated = state.queue.length + more.length < queueTotal;
    } else {
      queueTruncated = false;
    }
  } catch {
    queueTruncated = false;      // 서버가 이 경로를 모르면 더 조르지 않는다
  }
  queueLoadingMore = false;
}

function renderQueue(state) {
  renderQueueHead(state);
  const items = state.queue;
  noteQueueScores(items);
  if (!items.length) {
    list.reset(el.queueList);
    setVirtPad(0, 0);
    clear(el.queueList).appendChild(state.hotAt
      ? emptyState('🎧', '대기열이 비었어요', '검색해서 다음 곡을 담아 보세요.')
      : skeletonRows(4));
    return;
  }

  const rounds = computeRounds(items);
  // 2단처럼 바깥이 스크롤하는 배치에서는 목록이 자기 스크롤을 가져야 가상화가 먹는다
  el.queueList.dataset.virt = virtualizing() ? '1' : '0';
  const win = computeVirtWindow(items);
  virtWindow = win;
  const slice = win.from === 0 && win.to === items.length ? items : items.slice(win.from, win.to);

  // 위아래 패딩으로 스크롤 길이를 맞춘다. list()보다 **먼저** 정해 둬야 레이아웃이 한 번도 줄지 않는다.
  setVirtPad(win.from * virtRow, Math.max(0, (items.length - win.to) * virtRow));

  // 그래도 남을 수 있는 잘림에 대비해 스크롤 위치를 붙잡아 둔다 (§18.2 (2)).
  const keepScroll = el.queueList.scrollTop;
  list(el.queueList, slice, (item) => item.id, createQueueItem,
    (node, item, index) => updateQueueItem(node, item, win.from + index, rounds));
  if (virtualizing() && el.queueList.scrollTop !== keepScroll) el.queueList.scrollTop = keepScroll;

  measureVirtRow();
  marquee.scan(el.queueList);
}

/** 가상 스크롤의 위아래 여백. 자식 노드가 아니라 CSS 변수라서 list()가 지울 수 없다. */
function setVirtPad(top, bottom) {
  el.queueList.style.setProperty('--virt-pad-top', `${Math.max(0, Math.round(top))}px`);
  el.queueList.style.setProperty('--virt-pad-bottom', `${Math.max(0, Math.round(bottom))}px`);
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

/** 다음 재정렬까지 남은 초. fifo이거나 기준 시각을 모르면 null.
 *
 *  기준 시각(`nextSortAt`)은 서버가 준다. 문제는 그 시각이 **지나가도 다음 값이 안 오는 경우**가
 *  있다는 것이다 — `queue.set` 은 순서가 실제로 바뀐 길드에만 나가고, 곡이 1~2곡이면
 *  서버는 아예 재정렬을 시도하지 않는다. 그때 롤오버가 없으면 `갱신 0` 이 영구히 박힌다.
 *  그래서 지난 기준 시각은 **주기 단위로 앞으로 굴린다**. 주기도 서버가 준 값(§18.2 (3))을 쓴다.
 *
 *  `now` 를 받는 건 회귀 테스트에서 시계를 고정하기 위해서다.
 */
function sortRemainFrom(state, nowMs, periodSec) {
  if (!state || state.queueMode === 'fifo') return null;
  if (!state.queue || !state.queue.length) return null;

  const period = Math.max(1, Math.round(periodSec || SORT_PERIOD_DEFAULT));
  const periodMs = period * 1000;
  let target = parseUtc(state.nextSortAt) || null;
  if (!target) {
    const sorted = parseUtc(state.sortedAt);
    if (!sorted) return null;
    target = sorted + periodMs;
  }
  // 이미 지난 기준 시각은 주기만큼 굴려서 다음 경계로 옮긴다. 여기가 `갱신 0` 고착의 원인이었다.
  if (target <= nowMs) target += Math.ceil((nowMs - target + 1) / periodMs) * periodMs;

  const left = target - nowMs;
  // 롤오버를 거쳤으니 남은 시간은 한 주기를 넘을 수 없다. 넘으면 기준 시각이 미래로 크게 어긋난
  // 경우라 세는 시늉을 하지 않는다.
  if (left > periodMs + 1000) return null;
  return Math.max(0, Math.min(period, Math.ceil(left / 1000)));
}

function sortRemainSeconds() {
  return sortRemainFrom(store.get(), serverNow(), sortPeriodSeconds());
}

function renderSortTick() {
  if (!el.sortTick) return;
  const left = sortRemainSeconds();
  if (left === null) { el.sortTick.hidden = true; return; }
  const period = sortPeriodSeconds();
  el.sortTick.hidden = false;
  clear(el.sortTick).append(
    h('span', { class: 'sorttick__label' }, '갱신'),
    h('b', null, String(left)));
  el.sortTick.setAttribute('aria-label', `${left}초 뒤에 대기열이 다시 정렬돼요`);
  // 툴팁도 서버 주기를 따라간다 — `5초마다` 로 박아 두면 15초 서버에서 화면이 거짓말을 한다 (§20)
  el.sortTick.setAttribute('data-tip', `${period}초마다 순서를 다시 정해요`);
}

function startSortTick() {
  // 백그라운드 탭에서는 아무것도 그리지 않는다 (§23.2)
  let superTick = 0;
  setInterval(() => {
    if (document.hidden) return;
    renderSortTick();
    // 슈퍼 좋아요 쿨타임은 1초에 한 번만 다시 그린다
    superTick += 1;
    if (superTick % 4 === 0) tickSuperButtons();
  }, 250);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) renderSortTick(); });
}

/** 쿨타임 카운트다운은 ⭐ 버튼의 글자 하나만 바뀐다. 여기서 `renderQueue(전체)` 를 부르면
 *  쿨타임이 도는 내내 1초마다 대기열 전체가 다시 그려진다 (§23.2 "전체 재렌더 금지").
 *  쿨타임이 **풀리는 순간** 한 번만 정식 렌더로 잠금·툴팁을 다시 계산한다. */
let superCooling = false;
function tickSuperButtons() {
  if (!el.queueList) return;
  const info = superLikeInfo();
  if (info.coolLeft <= 0) {
    if (superCooling) { superCooling = false; renderQueue(store.get()); }
    return;
  }
  superCooling = true;
  const label = fmtTime(info.coolLeft);
  for (const node of el.queueList.querySelectorAll('.qitem')) {
    const button = node.__parts?.superLike;
    const item = node.__item;
    if (!button || !item) continue;
    if (item.myVote === 'superLike') continue;      // 취소용 버튼에는 쿨타임을 안 씌운다
    button.textContent = `⭐ ${label}`;
    if (button.getAttribute('aria-disabled') === 'true' && !item.isMine) {
      setLock(button, true, `슈퍼 좋아요는 ${label} 뒤에 다시 쓸 수 있어요`);
    }
  }
}

/* ── 자동 재생 (§8) ──
 * 서버가 이 API를 모르면(404) 막대와 📻 버튼을 통째로 숨긴다. 새 기능이 실패해도 기본 동작은 살아 있어야 한다.
 *
 * 설정은 **대기열 탭 안이 아니라 별도 시트**에 있다. 예전에는 대기열 위 접이식 상자 하나에
 * 방식·정책·기준 곡·최근 곡·빼 둔 곡을 전부 욱여넣었는데, 폭이 레일 하나뿐이라
 * 무엇 하나 제대로 안 보였다(칩 12개로 잘린 목록, 260px 짜리 속 스크롤).
 * 시트는 배치 6종 어디서나 같은 크기로 열리고 좁은 화면에서는 화면 전체를 쓴다 —
 * 배치별로 다른 화면을 만들지 않고 자리를 넓히는 유일한 방법이다.
 * 대기열 탭에는 "지금 무엇을 근거로 고르는지" 한 줄 요약만 남긴다.
 */

function buildSeedBox() {
  el.seedSummary = h('span', { class: 'seedbar__sum' });
  el.seedCount = h('span', { class: 'chip' }, '0곡');
  // 막대 자체는 잠그지 않는다. 읽기 전용이어도 **무엇을 근거로 고르는지는 볼 수 있어야** 한다.
  // 못 바꾸는 것은 시트 안에서 버튼별로 비활성 + 이유로 알려 준다 (§23.3).
  el.seedBox = h('section', { class: 'seedbar', hidden: true },
    bindAct(h('button', {
      class: 'seedbar__btn', type: 'button', 'aria-haspopup': 'dialog',
      tip: '자동 재생이 무엇을 근거로 다음 곡을 고르는지 보고 바꿔요',
    },
      h('span', { class: 'seedbar__icon', 'aria-hidden': 'true' }, '📻'),
      h('span', { class: 'seedbar__label' }, '자동 재생'),
      el.seedSummary,
      h('span', { class: 'queue__spacer' }),
      el.seedCount,
      h('span', { class: 'seedbar__go', 'aria-hidden': 'true' }, '⚙')), openAutoplaySheet));
  return el.seedBox;
}

async function loadSeeds() {
  // 새 API(/autoplay)가 있으면 방식·정책까지 한 번에 받고, 없으면 예전 시드 목록만 받는다.
  try {
    const data = await api('/autoplay');
    autoplayState = {
      mode: data?.mode || 'recent',
      recentCount: Number(data?.recentCount) || 5,
      genres: Array.isArray(data?.genres) ? data.genres : [],
      genreOptions: Array.isArray(data?.genreOptions) ? data.genreOptions : [],
      policy: data?.policy || 'balanced',
      canEdit: !!data?.canEdit,
      basket: data?.basket || null,
    };
    seedState = {
      seeds: Array.isArray(data?.seeds) ? data.seeds : [],
      max: Number(data?.max) || 10,
      canEdit: !!data?.canEdit,
    };
    renderSeeds();
    return;
  } catch (error) {
    if (!(error && (error.status === 404 || error.status === 501))) {
      // 권한·네트워크 문제면 아래 예전 경로로 한 번 더 시도한다
    }
    autoplayState = null;
  }

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

const AUTOPLAY_MODES = [
  ['seed', '📻 기준 곡', '내가 고른 곡들과 비슷한 노래를 골라 와요'],
  ['recent', '🕘 최근 튼 곡', '최근에 튼 곡 몇 개를 참고해서 골라 와요'],
  ['genre', '🎸 장르', '고른 장르 차트에서 골라 와요'],
];

const AUTOPLAY_POLICIES = [
  ['similar', '비슷하게', '분위기를 유지해요'],
  ['balanced', '적당히', '비슷하되 매번 달라요'],
  ['explore', '새롭게', '예상 못 한 곡이 나와요'],
  ['popular', '무난하게', '많이 들은 곡 위주예요'],
];

async function saveAutoplay(patch) {
  const result = await call(() => api('/autoplay', { method: 'PUT', body: patch }), '자동 재생 설정을 바꿨어요.');
  if (result) loadSeeds();
}

async function resetBasket(scope, confirmText) {
  if (!window.confirm(confirmText)) return;
  const result = await call(() => api('/autoplay/reset', { body: { scope } }));
  if (result) {
    toast(result.message || '비웠어요.', 'ok');
    loadSeeds();
  }
}

function autoplayModeMeta(id) {
  return AUTOPLAY_MODES.find((row) => row[0] === id) || AUTOPLAY_MODES[1];
}

function autoplayPolicyMeta(id) {
  return AUTOPLAY_POLICIES.find((row) => row[0] === id) || AUTOPLAY_POLICIES[1];
}

/** 자동 재생을 지금 바꿀 수 있는가. 서버의 canEdit 과 권한(`autoplay`)은 다른 문제라 둘 다 본다.
 *  최종 판정은 언제나 서버다 — 여기서 규칙을 다시 구현하지 않는다. */
function autoplayEditable() {
  return !!autoplayState?.canEdit && canAutoplay();
}

/* ── 자동 재생 시트 (§8.7) ──
 *
 * 자동재생이 **무엇을 근거로** 곡을 고르는지 안 보이면, 이상한 곡이 나왔을 때 손댈 데가 없다.
 * 방식·정책·기준 곡·최근 튼 곡·빼 둔 곡을 한 화면에 전부 펼쳐 두고, 지금 쓰이지 않는 칸도
 * 지우지 않고 흐리게만 둔다 — 없애 버리면 방식을 바꿨을 때 그게 어디서 나타나는지 알 수가 없다.
 *
 * 시트는 배치와 무관하다. 3단이든 패널이든 모바일이든 같은 노드가 같은 크기로 열린다.
 */

function openAutoplaySheet() {
  if (autoplaySheet) { return; }        // 이미 열려 있으면 한 장 더 띄우지 않는다
  const body = h('div', { class: 'ap' });
  const handle = sheet({
    title: '📻 자동 재생',
    desc: '대기열이 비면 봇이 알아서 곡을 골라요. 무엇을 근거로 고를지 여기서 정해요.',
    wide: true,
    body,
    dismissValue: null,
    actions: [{ label: '닫기', kind: 'primary', value: null }],
  });
  autoplaySheet = { handle, body };
  renderAutoplaySheet();
  handle.result.then(() => { autoplaySheet = null; });
}

/** 서버 갱신(`loadSeeds`)이나 WS `autoplay` 이벤트가 올 때마다 시트 속을 통째로 다시 그린다.
 *  시트는 한 화면짜리라 부분 갱신을 아껴 봐야 얻는 게 없고, 어긋난 화면이 훨씬 비싸다. */
function renderAutoplaySheet() {
  if (!autoplaySheet) return;
  const body = autoplaySheet.body;
  clear(body);
  if (!seedState) {
    body.appendChild(emptyState('📻', '이 서버는 아직 자동 재생 설정을 몰라요',
      '봇을 최신 버전으로 올리면 여기서 바꿀 수 있어요.'));
    return;
  }
  put(body,
    apModeSection(),
    apSeedSection(),
    apRecentSection(),
    apGenreSection(),
    apBlockedSection(),
    apPolicySection());
}

/** 시트 안 카드 한 장. `used === false` 면 지금 방식이 참고하지 않는 칸이라 흐리게 둔다. */
function apSection(opts) {
  return h('section', { class: `ap__sec${opts.used === false ? ' ap__sec--idle' : ''}` },
    h('div', { class: 'ap__head' },
      h('span', { class: 'ap__icon', 'aria-hidden': 'true' }, opts.icon),
      h('h3', { class: 'ap__title' }, opts.name),
      opts.count === undefined || opts.count === null
        ? null
        : h('span', { class: 'ap__count' }, String(opts.count)),
      h('span', { class: 'queue__spacer' }),
      opts.action || null),
    opts.desc ? h('p', { class: 'ap__note' }, opts.desc) : null,
    opts.used === false
      ? h('p', { class: 'ap__note ap__note--idle' }, '지금 방식에서는 참고하지 않아요')
      : null,
    ...(opts.children || []).filter(Boolean));
}

/** 기준 곡으로 보낼 수 있는 모양인가. 서버의 TrackRef 는 sourceUrl(또는 provider+contentId)이
 *  없으면 본문 해석 자체가 실패해서 422 가 돌아온다. 바구니의 최근 목록처럼 요약만 온 항목은
 *  담기 버튼을 아예 안 만든다 — 눌러도 "입력값을 확인해 주세요" 밖에 안 나온다. */
function seedableTrack(track) {
  if (!track || typeof track !== 'object') return false;
  return !!(track.sourceUrl || (track.provider && track.contentId));
}

/** 곡 한 줄. 시트 안에서는 버튼이 hover 로 나타나면 안 된다(터치 화면에서 찾을 수가 없다). */
function apRow(track, sub, acts, idle, idleTip) {
  const attrs = { class: `aprow${idle ? ' aprow--idle' : ''}` };
  // 빈 tip 을 달면 툴팁 대상이 되어 아무 글자도 없는 말풍선이 뜬다. 있을 때만 붙인다.
  if (idle && idleTip) attrs.tip = idleTip;
  return h('div', attrs,
    h('img', { class: 'row__art aprow__art', src: artUrl(track) || '', alt: '', loading: 'lazy' }),
    h('div', { class: 'aprow__main' },
      h('div', { class: 'aprow__title' }, trackTitle(track)),
      h('div', { class: 'row__sub' }, sub || trackSub(track))),
    (acts || []).filter(Boolean).length ? h('div', { class: 'aprow__acts' }, ...acts.filter(Boolean)) : null);
}

/** 칸 하나를 통째로 비우는 버튼. 개별 삭제가 되는 칸에도 남겨 둔다 — 20곡을 하나씩 지우게 하면 안 된다. */
function apWipe(scope, name, count, confirmText) {
  const editable = autoplayEditable();
  const reason = lockReason('autoplay');
  const button = bindAct(h('button', {
    class: 'btn btn--sm btn--ghost btn--danger', type: 'button',
    tip: `${name}을(를) 한 번에 비워요.`,
  }, '전부 비우기'), () => resetBasket(scope, confirmText));
  return setLock(button, !editable || !count, !editable ? reason : `${name}이(가) 이미 비어 있어요.`);
}

/** 세그먼트 한 줄. 잠겨도 숨기지 않는다 — 무엇을 고를 수 있는 화면인지는 보여야 한다. */
function apSeg(label, tip, options, current, onPick) {
  const editable = autoplayEditable();
  const reason = lockReason('autoplay');
  const row = h('div', {
    class: 'lib__seg lib__seg--wrap', style: { padding: '0' },
    role: 'group', 'aria-label': label,
  },
    ...options.map(([id, text, hint]) => setLock(bindAct(h('button', {
      class: 'seg', type: 'button', 'aria-pressed': String(id === current), dataset: { seg: id },
      tip: hint || text,
    }, text), () => onPick(id)), !editable, reason)));
  return h('div', { class: 'autoplay__row' }, h('div', { class: 'hint', tip }, label), row);
}

function apModeSection() {
  if (!autoplayState) return null;
  const meta = autoplayModeMeta(autoplayState.mode);
  return apSection({
    icon: '🎛', name: '무엇을 기준으로 고를까요', desc: meta[2],
    children: [apSeg('추천 방식', '자동 재생이 무엇을 기준으로 곡을 고를지 정해요',
      AUTOPLAY_MODES, autoplayState.mode, (id) => saveAutoplay({ mode: id }))],
  });
}

function apPolicySection() {
  if (!autoplayState) return null;
  const meta = autoplayPolicyMeta(autoplayState.policy);
  return apSection({
    icon: '🎚', name: '어떤 느낌으로 고를까요', desc: meta[2],
    children: [apSeg('추천 정책', '같은 기준에서도 얼마나 비슷한 곡을 집을지 정해요',
      AUTOPLAY_POLICIES, autoplayState.policy, (id) => saveAutoplay({ policy: id }))],
  });
}

/* ── 기준 곡 ──
 * 예전에는 대기열·검색에서 📻 를 누르는 길 하나뿐이라, 설정을 열어 놓고도
 * "여기서 담을 수가 없네" 로 끝났다. 지금 재생 중인 곡과 대기열 앞쪽을 시트 안에서 바로 담는다.
 * 순서는 라운드로빈 순번이라(§8.2) 눈에 보여야 하고, 기존 `/autoplay/seeds/reorder` 로 바꾼다.
 */
function apSeedSection() {
  const editable = seedEditable();
  const reason = lockReason('autoplay');
  const seeds = seedState.seeds || [];
  const max = Number(seedState.max) || 0;
  const full = max > 0 && seeds.length >= max;

  const rows = seeds.map((seed, index) => {
    const track = seed.track || {};
    const up = setLock(bindAct(h('button', {
      class: 'iconbtn', type: 'button', tip: '앞으로 옮기기', 'aria-label': '앞으로 옮기기',
    }, '↑'), () => moveSeed(index, -1)), !editable || index === 0,
    !editable ? reason : '맨 앞이에요');
    const down = setLock(bindAct(h('button', {
      class: 'iconbtn', type: 'button', tip: '뒤로 옮기기', 'aria-label': '뒤로 옮기기',
    }, '↓'), () => moveSeed(index, 1)), !editable || index === seeds.length - 1,
    !editable ? reason : '맨 뒤예요');
    const remove = setLock(bindAct(h('button', {
      class: 'iconbtn iconbtn--danger', type: 'button',
      tip: '이 곡만 기준에서 빼요', 'aria-label': '기준 곡에서 빼기',
    }, '✕'), () => removeSeed(seed.cacheKey)), !editable, reason);
    const sub = [seed.addedByDisplayName, fmtAgo(seed.addedUtc)].filter(Boolean).join(' · ');
    const row = apRow(track, sub || trackSub(track), [up, down, remove]);
    row.insertBefore(h('span', {
      class: 'aprow__no', tip: `${index + 1}번째로 참고해요`,
    }, String(index + 1)), row.firstChild);
    return row;
  });

  // 담을 수 있는 후보. 이미 담긴 곡과 중복은 뺀다 — 눌러 봐야 중복이라고 400 이 돌아온다.
  const have = new Set(seeds.map((seed) => String(seed.cacheKey)));
  const state = store.get();
  const picks = [];
  if (state.current?.track) picks.push(['지금 곡', state.current.track]);
  for (const item of state.queue.slice(0, 8)) if (item.track) picks.push([null, item.track]);
  const pickable = picks.filter(([, track]) => {
    const key = trackKey(track);
    if (!seedableTrack(track) || have.has(key)) return false;
    have.add(key);                       // 지금 곡이 대기열에도 있으면 칩이 두 개 생긴다
    return true;
  });

  const pickBox = pickable.length
    ? h('div', { class: 'ap__pick' },
      h('span', { class: 'hint' }, '바로 담기'),
      ...pickable.slice(0, 8).map(([tag, track]) => setLock(bindAct(h('button', {
        class: 'ap__chip', type: 'button',
        tip: `📻 ${trackTitle(track)} 을(를) 기준 곡으로 담아요`,
      }, `📻 ${tag || trackTitle(track)}`), () => addSeed(track)),
      !editable || full, !editable ? reason : `기준 곡은 ${max}곡까지예요. 하나 빼고 담아 주세요.`)))
    : null;

  // "지금 이 칸을 보고 있는가" 는 서버가 바구니에 담아 준다. 방식이 `seed` 여도 시드가 없으면
  // 폴백 사슬(§8.2)을 타고 최근 곡으로 내려가므로, 모드만 보고 판단하면 화면이 거짓말을 한다.
  const basket = autoplayState?.basket;
  const usesSeeds = basket ? !!basket.usesSeeds : (!autoplayState || autoplayState.mode === 'seed');

  return apSection({
    icon: '📻', name: '기준 곡',
    used: usesSeeds,
    count: `${seeds.length} / ${fmtLimit(max, '곡')}`,
    action: apWipe('seeds', '기준 곡', seeds.length, '담아 둔 기준 곡을 전부 뺄까요?'),
    desc: seeds.length
      ? '위에서부터 돌아가며 참고해요. ↑↓ 로 순서를, ✕ 로 한 곡씩 뺄 수 있어요.'
      : '아직 담은 곡이 없어요. 곡 옆의 📻 를 누르거나 아래에서 바로 담아 보세요.',
    children: [rows.length ? h('div', { class: 'ap__rows' }, ...rows) : null, pickBox],
  });
}

/* ── 최근 튼 곡 ──
 * 한 줄만 빼는 것과 칸을 통째로 비우는 것을 둘 다 둔다. 20곡을 하나씩 지우게 하면 안 되고,
 * 반대로 한 곡이 거슬려서 이력 전체를 날리게 해서도 안 된다.
 *
 * 여기에 더해 "몇 곡까지 참고하는지"를 목록 위에 붙이고 범위 밖 줄은 흐리게 만든다 —
 * 지우지 않고도 **무엇이 지금 영향을 주고 있는지**가 눈에 보여야 한다.
 */
function apRecentSection() {
  const basket = autoplayState?.basket;
  if (!basket) return null;
  const recent = Array.isArray(basket.recent) ? basket.recent : [];
  const editable = seedEditable();
  const reason = lockReason('autoplay');
  const limit = Number(autoplayState.recentCount) || 0;
  const have = new Set((seedState.seeds || []).map((seed) => String(seed.cacheKey)));

  const input = h('input', {
    class: 'field field--num', type: 'number', min: '0', max: '20',
    value: String(autoplayState.recentCount ?? 5),
    'aria-label': '최근 몇 곡을 참고할지',
    onChange: () => saveAutoplay({ recentCount: Number(input.value) || 0 }),
  });
  setLock(input, !autoplayEditable(), lockReason('autoplay'));

  const rows = recent.slice(0, 12).map((item, index) => {
    const track = item.track || item;
    const idle = limit > 0 && index >= limit;
    const seedable = seedableTrack(track) && !have.has(trackKey(track));
    const toSeed = seedable
      ? setLock(bindAct(h('button', {
        class: 'iconbtn', type: 'button',
        tip: '📻 이 곡을 기준 곡으로 담아요', 'aria-label': '기준 곡으로 담기',
      }, '📻'), () => addSeed(track)), !editable, reason)
      : null;
    // 한 줄만 빼기. 서버가 준 행 id 가 있어야 지목할 수 있다 — 없는 옛 응답이면 버튼을 안 단다.
    const drop = Number.isFinite(Number(item.id))
      ? setLock(bindAct(h('button', {
        class: 'iconbtn', type: 'button',
        tip: '이 기록만 참고에서 빼요', 'aria-label': '이 기록만 빼기',
      }, '✕'), () => removeRecent(Number(item.id))), !editable, reason)
      : null;
    const sub = [fmtAgo(item.playedUtc), item.artist || track.artist].filter(Boolean).join(' · ');
    return apRow(track, sub, [toSeed, drop], idle, `최근 ${limit}곡만 참고해서 이 곡은 지금 영향이 없어요`);
  });

  return apSection({
    icon: '🕘', name: '최근 튼 곡',
    used: !!basket.usesRecent,
    count: `${recent.length}곡`,
    action: apWipe('recent', '최근 재생 기록', recent.length,
      '최근 재생 기록을 비울까요?\n추천만 초기화되고 통계와 차트는 그대로예요.'),
    desc: '자동 재생이 이 목록에서 골라요. 한 줄씩 뺄 수도 있고, 참고 범위를 줄여도 돼요.',
    children: [
      h('div', { class: 'autoplay__row' },
        h('div', { class: 'hint', tip: '0을 넣으면 최근에 튼 곡 전부를 참고해요' }, '최근 몇 곡을 참고할까요'),
        h('div', { class: 'autoplay__num' }, input,
          h('span', { class: 'hint' }, fmtLimit(autoplayState.recentCount, '곡')))),
      rows.length ? h('div', { class: 'ap__rows' }, ...rows) : h('p', { class: 'ap__note' }, '아직 튼 곡이 없어요.'),
      recent.length > 12 ? h('p', { class: 'ap__note' }, `외 ${recent.length - 12}곡`) : null,
    ],
  });
}

function apGenreSection() {
  if (!autoplayState) return null;
  const editable = autoplayEditable();
  const reason = lockReason('autoplay');
  const options = autoplayState.genreOptions || [];
  // 장르 목록이 비어도 칸을 통째로 숨기지 않는다 — `🎸 장르` 를 골랐는데 아무것도 안 나오면
  // 고장인지 내가 뭘 잘못한 건지 알 수가 없다 (§23.3).
  const box = options.length
    ? h('div', { class: 'lib__seg lib__seg--wrap', style: { padding: '0' } },
      ...options.map((option) => {
        const on = autoplayState.genres.includes(option.key);
        return setLock(bindAct(h('button', {
          class: 'seg', type: 'button', 'aria-pressed': String(on), dataset: { seg: option.key },
          tip: `${option.label} 차트에서 곡을 골라 와요`,
        }, option.label), () => saveAutoplay({
          genres: on
            ? autoplayState.genres.filter((key) => key !== option.key)
            : autoplayState.genres.concat(option.key),
        })), !editable, reason);
      }))
    : h('p', {
      class: 'ap__note', tip: '장르 차트가 준비되면 여기에 고를 수 있는 장르가 나와요',
    }, '고를 수 있는 장르가 아직 없어요. 관리 콘솔에서 장르 차트를 켜면 여기에 나와요.');

  return apSection({
    icon: '🎸', name: '장르',
    used: autoplayState.mode === 'genre',
    count: `${autoplayState.genres.length}개`,
    children: [box],
  });
}

function apBlockedSection() {
  const basket = autoplayState?.basket;
  if (!basket) return null;
  const blocked = Array.isArray(basket.blocked) ? basket.blocked : [];
  const editable = autoplayEditable();
  const reason = lockReason('autoplay');
  const rows = blocked.slice(0, 12).map((item) => {
    const track = item.track || item;
    const title = item.title || track.title || item.cacheKey || '알 수 없는 곡';
    // 한 곡만 풀기. 전부 풀기는 옆에 그대로 두되, 하나만 되돌리고 싶을 때가 더 흔하다.
    const free = item.cacheKey
      ? setLock(bindAct(h('button', {
        class: 'iconbtn', type: 'button',
        tip: '이 곡만 다시 추천에 나오게 해요', 'aria-label': '이 곡만 풀기',
      }, '↩'), () => unblockCandidate(String(item.cacheKey))), !editable, reason)
      : null;
    return h('div', { class: 'aprow aprow--flat' },
      h('div', { class: 'aprow__main' },
        h('div', { class: 'aprow__title aprow__title--off' }, title),
        h('div', { class: 'row__sub' }, item.reason || '자동 재생에서 빠져 있어요')),
      free ? h('div', { class: 'aprow__acts' }, free) : null);
  });

  return apSection({
    icon: '🚫', name: '빼 둔 곡',
    count: `${blocked.length}곡`,
    action: apWipe('blocked', '빼 둔 곡', blocked.length,
      '빼 둔 곡을 전부 풀까요?\n다시 추천에 나올 수 있어요.'),
    desc: '`📻 이 곡 말고` 로 뺐거나 재생에 실패한 곡이에요. 한동안 다시 안 뽑아요.',
    children: [
      rows.length ? h('div', { class: 'ap__rows' }, ...rows) : h('p', { class: 'ap__note' }, '빼 둔 곡이 없어요.'),
      blocked.length > 12 ? h('p', { class: 'ap__note' }, `외 ${blocked.length - 12}곡`) : null,
    ],
  });
}

/** 순서 바꾸기는 관리 콘솔에만 있던 `/autoplay/seeds/reorder` 를 그대로 쓴다. 새 API 를 만들지 않는다. */
async function moveSeed(index, delta) {
  const keys = (seedState?.seeds || []).map((seed) => String(seed.cacheKey));
  const to = index + delta;
  if (to < 0 || to >= keys.length) return;
  const [moved] = keys.splice(index, 1);
  keys.splice(to, 0, moved);
  await call(() => api('/autoplay/seeds/reorder', { body: { cacheKeys: keys } }),
    '기준 곡 순서를 바꿨어요.');
  // 성공이든 실패든 서버가 가진 순서로 다시 맞춘다. 실패했는데 화면만 바뀐 채로 두면
  // 다음 ↑↓ 가 엉뚱한 자리를 옮긴다.
  loadSeeds();
}

function renderSeeds() {
  if (!el.seedBox) return;
  // 최종 판정은 서버의 canEdit(권한 키 autoplay)이다. 화면은 seedEditable() 로 덧대 막는다.
  // data-seeds 는 "**서버가 이 기능을 아는가**" 하나만 뜻한다. 권한 판정까지 여기에 섞으면
  // 권한 없는 사람에게 📻 버튼이 CSS로 사라져서 왜 없는지 물어볼 데도 없어진다 (§23.3).
  el.portal?.setAttribute('data-seeds', seedState ? '1' : '0');
  refreshSeedButtons();

  if (!seedState) { el.seedBox.hidden = true; return; }
  el.seedBox.hidden = false;

  // 대기열 탭에 남는 것은 한 줄 요약뿐이다. "지금 무엇을 근거로 고르는지" 를 읽을 수 있으면
  // 설정을 열지 않아도 되고, 이상하면 그때 눌러서 시트를 연다.
  const mode = autoplayState ? autoplayModeMeta(autoplayState.mode) : null;
  const policy = autoplayState ? autoplayPolicyMeta(autoplayState.policy) : null;
  const seeds = seedState.seeds.length;
  el.seedSummary.textContent = mode
    ? `· ${mode[1]}${policy ? ` · ${policy[1]}` : ''}`
    : '· 기준 곡';
  el.seedCount.textContent = mode && mode[0] !== 'seed'
    ? `기준 곡 ${seeds}곡`
    : `${seeds} / ${fmtLimit(seedState.max, '곡')}`;
  el.seedCount.setAttribute('data-tip', seedState.max > 0
    ? `기준 곡은 ${seedState.max}곡까지 넣을 수 있어요`
    : '기준 곡은 몇 곡이든 넣을 수 있어요');

  renderAutoplaySheet();
}

/** 대기열·검색 결과에 붙는 '기준으로 삼기'. 권한이 없으면 CSS가 통째로 숨긴다. */
/** 기준 곡으로 삼을 수 있는가. **서버가 이 기능을 아는지**(seedState)와 **내 권한**은 다른 문제다.
 *  기능이 없으면 버튼을 숨기고(있지도 않은 걸 회색으로 두면 더 헷갈린다),
 *  기능은 있는데 권한이 없으면 숨기지 말고 비활성 + 이유로 남긴다 (§23.3 · §20). */
function seedEditable() {
  const state = store.get();
  if (!seedState) return false;
  if (state.conn === 'down' || state.tier === 'viewer') return false;
  if (state.suspension && (state.suspension.scope === 'all' || state.suspension.scope === 'queue')) return false;
  return !!seedState.canEdit && canAutoplay();
}

/** 시드 기능 상태(`/autoplay`)는 대기열 렌더와 따로 도착한다. 그때 대기열 전체를 다시 그리지 않고
 *  📻 버튼의 잠금만 손본다 (§23.2 "전체 재렌더 금지"). */
function refreshSeedButtons() {
  if (!el.queueList) return;
  const locked = !seedEditable();
  const reason = lockReason('autoplay');
  for (const node of el.queueList.querySelectorAll('.qitem')) {
    if (node.__parts?.seed) setLock(node.__parts.seed, locked, reason);
  }
}

function seedButton(track, wide) {
  if (!track) return null;
  const button = bindAct(h('button', {
    class: wide ? 'vote seedbtn' : 'iconbtn seedbtn', type: 'button',
    tip: '📻 기준으로 삼기 — 자동 재생이 이 곡과 비슷한 노래를 골라 와요',
    'aria-label': '기준으로 삼기',
  }, wide ? '📻 기준' : '📻'), () => addSeed(track));
  return setLock(button, !seedEditable(), lockReason('autoplay'));
}

async function addSeed(track) {
  if (!seedState) return;
  const result = await call(() => api('/autoplay/seeds', { body: { track } }), '자동 재생 기준 곡에 담았어요.');
  // 예전에는 여기서 접이식 상자를 펼쳤다. 지금은 시트가 따로 있으니 목록만 다시 받는다 —
  // 시트가 열려 있으면 renderSeeds() 가 그 안까지 같이 고친다.
  if (result) loadSeeds();
}

async function removeSeed(cacheKey) {
  const result = await call(() => api('/autoplay/seeds/remove', { body: { cacheKey } }), '기준 곡에서 뺐어요.');
  if (result) loadSeeds();
}

/** 최근 재생 한 줄만 빼기. **행 id 로 보낸다** — 같은 곡을 여러 번 틀면 cacheKey 가 겹쳐서,
 *  키로 지우면 "이 한 번"이 아니라 그 곡 이력이 통째로 날아간다. */
async function removeRecent(id) {
  const result = await call(() => api('/autoplay/recent/remove', { body: { id } }), '최근 기록에서 뺐어요.');
  if (result) loadSeeds();
}

/** 빼 둔 곡 하나만 풀기. 곡 하나당 한 줄이라 키가 곧 그 줄이다. */
async function unblockCandidate(cacheKey) {
  const result = await call(() => api('/autoplay/blocked/remove', { body: { cacheKey } }), '다시 추천에 나올 수 있어요.');
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

/* ── 점수 계산식은 서버 설정을 따라야 한다 (§10.1) ──
 * 좋아요를 2점으로 바꿔 뒀는데 화면이 1점으로 계산하면 화면이 거짓말을 하는 것이다. */
function votePointsFrom(settings) {
  const source = settings || {};
  // 서버는 `settings.votePoints` **중첩 객체**로 준다 (remote.rs `/state/cold`·`/state/hot`).
  // 평평한 `likePoints` 만 읽던 시절에는 값이 늘 undefined 라 화면이 항상 기본 배점(1/-1/2/1)으로
  // 계산했다 — 관리자가 좋아요를 3점으로 바꿔도 계산식이 1점을 말하는, §10.1이 금지한 상황이다.
  const nested = source.votePoints || {};
  const pick = (...values) => {
    for (const value of values) {
      if (value === null || value === undefined || value === '') continue;
      const number = Number(value);
      if (Number.isFinite(number)) return number;
    }
    return 0;
  };
  return {
    like: pick(nested.like, source.likePoints, 1),
    dislike: pick(nested.dislike, source.dislikePoints, -1),
    superLike: pick(nested.superLike, source.superLikePoints, 2),
    wait: pick(nested.wait, source.waitPoints, 1),
  };
}

function votePoints() {
  return votePointsFrom(store.get().settings);
}

/** 서버가 준 사용자 ID를 화면에 쓸 이름으로. 모르는 ID는 `누군가`다 (§10.4). */
function nameOf(userId) {
  const id = String(userId);
  const state = store.get();
  if (id === String(state.user?.id || '')) return state.user?.displayName || '나';
  const member = state.members.find((row) => String(row.userId ?? row.id) === id);
  if (member?.displayName) return member.displayName;
  const message = state.chat.find((row) => String(row.userId) === id);
  if (message?.displayName) return message.displayName;
  const queued = state.queue.find((row) => String(row.requestedByUserId) === id);
  if (queued?.requestedByDisplay) return queued.requestedByDisplay;
  return '누군가';
}

function avatarOf(userId) {
  const id = String(userId);
  const state = store.get();
  if (id === String(state.user?.id || '')) return state.user?.avatarUrl || '';
  const member = state.members.find((row) => String(row.userId ?? row.id) === id);
  if (member?.avatarUrl) return member.avatarUrl;
  const message = state.chat.find((row) => String(row.userId) === id);
  return message?.avatarUrl || '';
}

/** 슈퍼 좋아요 남은 횟수·쿨타임 (§10.6). 서버가 안 주면 제한이 없다는 뜻이다. */
function superLikeInfo() {
  const info = store.get().superLike;
  if (!info) return { limited: false, left: null, coolLeft: 0 };
  const daily = Number(info.dailyLimit) || 0;
  const used = Number(info.usedToday) || 0;
  const availableAt = parseUtc(info.availableAtUtc);
  const coolLeft = availableAt ? Math.max(0, Math.ceil((availableAt - serverNow()) / 1000)) : 0;
  return {
    limited: daily > 0 || Number(info.cooldownSec) > 0,
    left: daily > 0 ? Math.max(0, daily - used) : null,
    coolLeft,
    cooldownSec: Number(info.cooldownSec) || 0,
  };
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
  p.superLike = bindAct(h('button', { class: 'vote', type: 'button', tip: '슈퍼 좋아요' }), () => vote(node.dataset.id, 'superLike'));
  p.dislike = bindAct(h('button', { class: 'vote vote--down', type: 'button', tip: '싫어요' }), () => vote(node.dataset.id, 'dislike'));
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
  // 원본 링크 (§34). 버튼이 아니라 링크라서 새 탭·복사 같은 브라우저 기능이 그대로 먹는다.
  p.link = h('a', {
    class: 'vote vote--link', target: '_blank', rel: 'noreferrer noopener',
    tip: '원본에서 열기', 'aria-label': '원본에서 열기',
  }, '↗');
  p.acts.append(p.like, p.superLike, p.dislike, p.save, p.link, p.seed, p.pin, p.remove);

  // 우클릭 / 롱프레스 — 곡 하나에 할 수 있는 걸 전부 한자리에 (§24.1)
  bindContextTarget(node, () => trackMenu(node.__item.track, {
    itemId: node.dataset.id, item: node.__item, source: 'queue',
  }));
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

  // 원본 링크 (§34). 주소를 모르면 숨긴다 — 눌러도 아무 데도 안 가는 버튼이 제일 나쁘다.
  const rowUrl = trackUrl(item.track);
  p.link.hidden = !rowUrl;
  if (rowUrl) p.link.href = rowUrl;

  const titleInner = p.title.firstElementChild;
  const title = trackTitle(item.track);
  if (titleInner.textContent !== title) titleInner.textContent = title;

  const round = rounds.get(item.id) || 1;
  put(clear(p.who),
    personButton(item.requestedByUserId, item.requestedByDisplay || '알 수 없음'),
    `의 ${round}번째 곡`,
    item.isMine ? h('span', { class: 'chip chip--accent', style: { marginLeft: 'var(--sp-2)' } }, '내 곡') : null);
  p.who.setAttribute('data-tip', `${item.requestedByDisplay || '누군가'}님이 이 서버에서 담은 ${round}번째 곡이에요`);

  renderScore(p.score, score, mode);

  const like = score.likeCount || 0;
  const superLike = score.superLikeCount || 0;
  const dislike = score.dislikeCount || 0;
  p.like.setAttribute('aria-pressed', String(item.myVote === 'like'));
  p.superLike.setAttribute('aria-pressed', String(item.myVote === 'superLike'));
  p.dislike.setAttribute('aria-pressed', String(item.myVote === 'dislike'));
  p.like.textContent = `👍 ${like}`;
  p.dislike.textContent = `👎 ${dislike}`;

  // 슈퍼 좋아요는 남은 횟수와 쿨타임을 숫자로 보여준다. 회색으로만 두면 고장인 줄 안다 (§10.6).
  // 다만 **내가 이미 슈퍼를 눌러 둔 곡**에는 쿨타임을 띄우지 않는다 — 그 버튼이 하는 일은
  // 새 슈퍼가 아니라 취소라서, 쿨타임을 보여주면 못 누르는 버튼처럼 보인다.
  const superInfo = superLikeInfo();
  const superMine = item.myVote === 'superLike';
  p.superLike.textContent = superInfo.coolLeft > 0 && !superMine
    ? `⭐ ${fmtTime(superInfo.coolLeft)}`
    : `⭐ ${superLike}`;

  const points = votePoints();
  const canVote = can('vote') && !item.isMine;
  const mineReason = '내가 신청한 곡에는 투표할 수 없어요';
  setLock(p.like, !canVote, item.isMine ? mineReason : lockReason('vote'));
  setLock(p.dislike, !canVote, item.isMine ? mineReason : lockReason('vote'));
  p.like.setAttribute('data-tip', canVote
    ? `좋아요 · ${points.like}점 올라가요`
    : p.like.getAttribute('data-tip'));
  p.dislike.setAttribute('data-tip', canVote
    ? `싫어요 · ${points.dislike}점이에요${boomttaNote(dislike)}`
    : p.dislike.getAttribute('data-tip'));

  // 이미 내가 슈퍼를 눌러 둔 곡이면 **취소는 언제나 열어 둔다.** 서버도 취소는 제한 검사 없이
  // 허용하고 횟수까지 환불한다(§10.6 — "실수로 누른 걸 하루 종일 못 쓰게 하면 가혹해요").
  // 여기서 잠그면 하루 1회 설정에서 잘못 누른 슈퍼를 되돌릴 방법이 사라진다.
  const mySuper = superMine;
  const superBlocked = !canVote || (!mySuper && (superInfo.coolLeft > 0 || superInfo.left === 0));
  setLock(p.superLike, superBlocked,
    item.isMine ? mineReason
      : superInfo.coolLeft > 0 ? `슈퍼 좋아요는 ${fmtTime(superInfo.coolLeft)} 뒤에 다시 쓸 수 있어요`
        : superInfo.left === 0 ? '오늘 슈퍼 좋아요를 다 썼어요 (UTC 자정에 초기화돼요)'
          : lockReason('vote'));
  if (!superBlocked) {
    p.superLike.setAttribute('data-tip', mySuper
      ? '다시 누르면 슈퍼 좋아요를 취소해요 · 오늘 쓴 횟수도 돌려받아요'
      : superInfo.left === null
        ? `슈퍼 좋아요 · ${points.superLike}점 올라가요`
        : `슈퍼 좋아요 · ${points.superLike}점 · 오늘 ${superInfo.left}번 남았어요`);
  }

  setLock(p.save, !can('library'), lockReason('library'));
  // 📻 는 숨기지 않는다 — 기능이 있는 서버라면 비활성 + 이유로 남긴다 (§23.3).
  setLock(p.seed, !seedEditable(), lockReason('autoplay'));

  p.pin.hidden = !can('queueEdit') || tierOf() === 'member';
  p.pin.setAttribute('aria-pressed', String(score.manualPriority !== null && score.manualPriority !== undefined));
  const canRemove = item.isMine ? can('queueEdit') || can('search') : can('queueEdit');
  // 내 곡은 `queueEdit` 이나 `search` 중 하나만 있어도 뺄 수 있다. 둘 다 막혔을 때 빈 문자열을
  // 넘기면 setLock 이 기본 문구로 떨어져 §23.3("왜 막혔는지")을 못 지킨다.
  setLock(p.remove, !canRemove, item.isMine ? lockReason('search') : lockReason('queueEdit'));
}

/** 붐따가 켜져 있으면 몇 개 더 모이면 내려가는지 알려 준다 (§10.3). */
function boomttaNote(dislike) {
  const settings = store.get().settings || {};
  if (!settings.boomttaEnabled) return '';
  const threshold = Number(settings.boomttaThreshold) || 0;
  if (threshold <= 0) return '';
  const left = Math.max(0, threshold - (dislike || 0));
  const action = settings.boomttaAction === 'remove' ? '대기열에서 빠져요' : '맨 뒤로 내려가요';
  return left > 0 ? ` · ${left}개 더 모이면 ${action}` : ` · 곧 ${action}`;
}

/** 서버가 준 계산식에서 `= 합계` 꼬리를 떼고 항 부분만 돌려준다.
 *  아직 점수가 없을 때 서버는 `아직 점수가 없어요 = 0` 을 주는데, 그건 우리 쪽 빈 상태 문구가 이긴다. */
function serverFormula(score) {
  const raw = typeof score?.formula === 'string' ? score.formula.trim() : '';
  if (!raw || raw.startsWith('아직 점수가 없어요')) return '';
  const cut = raw.lastIndexOf(' = ');
  const body = cut > 0 ? raw.slice(0, cut) : raw;
  return body.trim();
}

/** 시그니처 — 점수를 숫자 하나로 숨기지 않고 계산식과 막대로 보여준다.
 *  점수 배점은 서버 설정을 그대로 반영한다 (§10.1). 안 그러면 화면이 거짓말을 한다. */
function renderScore(host, score, mode) {
  const points = votePoints();
  const like = score.likeCount || 0;
  const superLike = score.superLikeCount || 0;
  const dislike = score.dislikeCount || 0;
  const wait = score.waitScore || 0;
  const total = score.totalScore
    ?? (wait * points.wait + like * points.like + superLike * points.superLike + dislike * points.dislike);

  clear(host);
  host.classList.toggle('score--muted', mode === 'fifo');

  const bar = h('div', { class: 'score__bar', 'aria-hidden': 'true' });
  const weights = [
    ['like', like * points.like],
    ['super', superLike * points.superLike],
    ['wait', wait * points.wait],
    ['down', Math.abs(dislike * points.dislike)],
  ];
  const sum = Math.max(1, weights.reduce((acc, [, value]) => acc + Math.max(0, value), 0));
  for (const [kind, value] of weights) {
    if (value <= 0) continue;
    bar.appendChild(h('span', { class: `score__seg score__seg--${kind}`, style: { flex: String(value / sum) } }));
  }
  if (!bar.children.length) bar.appendChild(h('span', { class: 'score__seg score__seg--wait', style: { flex: '1', opacity: '0.3' } }));

  // 배점이 1이면 곱셈을 안 쓴다. 음수 배점은 부호를 항목이 아니라 연산자로 낸다 —
  // `− 👎1×-1` 같은 이중 부정은 읽을 수가 없다.
  const term = (icon, count, per) => (Math.abs(per) === 1 ? `${icon}${count}` : `${icon}${count}×${Math.abs(per)}`);
  const parts = [];
  const push = (value, per, text) => { if (value) parts.push({ minus: per < 0, text }); };
  push(like, points.like, term('👍', like, points.like));
  push(superLike, points.superLike, term('⭐', superLike, points.superLike));
  push(wait, points.wait, term('대기', wait, points.wait));
  push(dislike, points.dislike, term('👎', dislike, points.dislike));

  const localFormula = parts.map((part, index) => (index === 0
    ? (part.minus ? `−${part.text}` : part.text)
    : `${part.minus ? ' − ' : ' + '}${part.text}`)).join('');
  // 계산식은 **서버가 만들어 준 것을 쓴다** (§10.4). 클라이언트가 배수를 다시 곱하면
  // 점수 설정이 바뀐 순간 화면과 실제 합계가 갈린다. 서버 문자열은 `👍3 + ⭐1×2 = 7` 꼴이라
  // `= N` 은 잘라 내고 우리 쪽 `score__total` 로 따로 고정한다.
  const formula = serverFormula(score) || localFormula;

  const text = h('span', { class: 'score__text' });
  // 합계는 절대 잘리면 안 된다. 계산식만 줄어들고 '= 7'은 따로 고정한다.
  if (mode === 'fifo') {
    // 점수가 보이는데 순서와 무관하면 그게 제일 헷갈린다. 흐리게 하고 한 줄 덧붙인다 (§10.4)
    text.textContent = parts.length ? `${formula} · 지금은 신청 순서대로 나가요` : '지금은 신청 순서대로 나가요';
    put(host, bar, text);
  } else if (parts.length) {
    text.textContent = formula;
    put(host, bar, text, h('b', { class: 'score__total' }, `= ${total}`));
  } else {
    text.textContent = '0점 · 방금 담겼어요';
    put(host, bar, text);
  }

  host.setAttribute('data-tip', mode === 'fifo'
    ? '시간제에서는 좋아요가 순서를 바꾸지 않아요'
    : `총 ${total}점 · 마우스를 올리면 누가 눌렀는지 보여요`);
  host.tabIndex = 0;
  host.__score = score;
  bindVoterCard(host);
}

/* ── 누가 눌렀는지 (§10.4) ──
 * 서버는 ID만 준다. 이름은 클라이언트가 이미 갖고 있는 목록으로 붙인다 —
 * 서버가 이름을 조회하면 대기열 길이만큼 쿼리가 는다.
 */
function voterList(score) {
  const rows = [];
  const add = (icon, label, ids, count) => {
    const list_ = (ids || []).map(String);
    if (!list_.length && !count) return;
    rows.push({ icon, label, ids: list_, extra: Math.max(0, (count || list_.length) - list_.length) });
  };
  add('👍', '좋아요', score.likeBy, score.likeCount);
  add('⭐', '슈퍼 좋아요', score.superBy, score.superLikeCount);
  add('👎', '싫어요', score.dislikeBy, score.dislikeCount);
  return rows;
}

function voterPanel(score) {
  const rows = voterList(score);
  if (!rows.length) {
    return h('div', { class: 'voters' }, h('p', { class: 'hint' }, '아직 아무도 안 눌렀어요.'));
  }
  return h('div', { class: 'voters' }, ...rows.map((row) => h('div', { class: 'voters__row' },
    h('span', { class: 'voters__kind' }, row.icon, ' ', row.label),
    h('div', { class: 'voters__who' },
      ...row.ids.map((id) => h('span', { class: 'voters__one' },
        avatar(avatarOf(id), nameOf(id), 'sm'),
        h('span', null, nameOf(id)))),
      row.extra > 0 ? h('span', { class: 'chip' }, `+${row.extra}명`) : null,
      !row.ids.length ? h('span', { class: 'hint' }, `${row.extra}명`) : null))));
}

let voterPop = null;
function bindVoterCard(host) {
  if (host.__voterBound) return;
  host.__voterBound = true;
  const show = () => {
    hideVoterCard();
    const score = host.__score || {};
    if (!voterList(score).length) return;
    voterPop = h('div', { class: 'pop voterpop' }, voterPanel(score));
    document.body.appendChild(voterPop);
    const rect = host.getBoundingClientRect();
    const box = voterPop.getBoundingClientRect();
    voterPop.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - box.width - 8))}px`;
    voterPop.style.top = `${rect.top - box.height - 8 < 8 ? rect.bottom + 8 : rect.top - box.height - 8}px`;
  };
  host.addEventListener('pointerenter', show);
  host.addEventListener('focus', show);
  host.addEventListener('pointerleave', hideVoterCard);
  host.addEventListener('blur', hideVoterCard);
}

function hideVoterCard() {
  if (voterPop) { voterPop.remove(); voterPop = null; }
}

/* 지금 나오는 곡의 좋아요·슈퍼·싫어요 (§10.7).
 *
 * 전에는 곡이 재생을 시작하는 순간 버튼이 사라졌다. 정작 **듣고 나서 판단이 서는** 시점이
 * 그때인데 누를 데가 없었다. 점수는 우리 차트와 개인 통계로 가고, 이미 나가고 있는 곡의
 * 순서는 바뀌지 않는다 — 대기열 정렬 대상이 아니기 때문이다.
 *
 * 권한 판정과 잠금 문구는 대기열 행과 **똑같은 규칙**을 쓴다. 여기만 느슨하면
 * 같은 곡이 대기열에 있을 때와 재생 중일 때 서로 다르게 굴어 버린다.
 */
function nowVoteButtons(current) {
  const points = votePoints();
  const isMine = current.isMine
    ?? (current.requestedByUserId && String(current.requestedByUserId) === String(store.get().user?.id));
  const canVote = can('vote') && !isMine;
  const mineReason = '내가 신청한 곡에는 투표할 수 없어요';
  const reason = isMine ? mineReason : lockReason('vote');

  const make = (kind, label, tip) => {
    const button = bindAct(h('button', {
      class: 'vote', type: 'button',
      'aria-pressed': String(current.myVote === kind),
      tip,
    }, label), () => vote(current.id, kind));
    return setLock(button, !canVote, reason);
  };

  const superInfo = superLikeInfo();
  const superMine = current.myVote === 'superLike';
  const superLabel = superInfo.coolLeft > 0 && !superMine
    ? `⭐ ${fmtTime(superInfo.coolLeft)}`
    : `⭐ ${points.superLike}`;
  const superButton = make('superLike', superLabel, superMine
    ? '슈퍼 좋아요를 취소해요'
    : `슈퍼 좋아요 · ${points.superLike}점 올라가요`);
  // 이미 눌러 둔 슈퍼의 **취소는 언제나 열어 둔다** — 대기열 행과 같은 규칙이다.
  if (!superMine && (superInfo.coolLeft > 0 || superInfo.left === 0)) {
    setLock(superButton, true, superInfo.coolLeft > 0
      ? `슈퍼 좋아요는 ${fmtTime(superInfo.coolLeft)} 뒤에 다시 쓸 수 있어요`
      : '오늘 슈퍼 좋아요를 다 썼어요 (UTC 자정에 초기화돼요)');
  }

  return h('div', { class: 'nowvote', role: 'group', 'aria-label': '지금 곡에 투표' },
    make('like', `👍 ${points.like}`, `좋아요 · ${points.like}점 올라가요`),
    superButton,
    make('dislike', `👎 ${points.dislike}`, `싫어요 · ${points.dislike}점이에요`));
}

async function vote(itemId, kind) {
  const state = store.get();
  // 대기열에도, **지금 나오는 곡에도** 투표할 수 있다 (§10.7).
  const item = state.queue.find((row) => row.id === itemId)
    || (state.current?.id === itemId ? state.current : null);
  const next = item && item.myVote === kind ? null : kind;
  const result = await call(() => api('/vote', { body: { itemId, kind: next } }));
  if (!result) return;

  // 서버 브로드캐스트(`queue.set`·`playback`)는 개인화 필드를 비워 보낸다. 내가 무엇을
  // 눌렀는지는 여기서만 알 수 있으므로 바로 기억하고 화면에도 반영한다 (§10.4).
  notePersonalVote(itemId, next);
  const patch = {
    queue: store.get().queue.map((row) => (row.id === itemId
      ? Object.assign({}, row, { myVote: next })
      : row)),
  };
  if (store.get().current?.id === itemId) {
    patch.current = Object.assign({}, store.get().current, { myVote: next });
  }
  store.patch(patch);

  // 슈퍼 좋아요 남은 횟수·쿨타임은 응답에 실려 온다 (§10.6). 버리면 `⭐ 2:14` 가 절대 안 뜨고,
  // 다시 눌러 429 를 받기 전까지 화면은 아무 일도 없던 것처럼 보인다.
  if (result.superLike) store.patch({ superLike: result.superLike });
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
  // 4 세그먼트 (§12.2). 개인 재생목록과 서버 재생목록은 확실히 갈라 둔다 —
  // 실수로 서버 것을 지우면 곤란하다.
  const segments = [
    ['liked', '👍 좋아요', '내가 좋아요를 누른 곡이에요'],
    ['saved', '🔖 담아둔 곡', '나중에 들으려고 담아 둔 곡이에요'],
    ['mine', '📃 내 재생목록', '나만 쓰는 재생목록이에요. 어느 서버에서든 보여요'],
    ['server', '🌐 서버 재생목록', '이 서버가 같이 쓰는 재생목록이에요'],
  ];
  el.libSeg = h('div', { class: 'lib__seg lib__seg--wrap' },
    ...segments.map(([id, label, tip]) =>
      h('button', {
        class: 'seg', type: 'button', 'aria-pressed': String(id === libraryTab), dataset: { seg: id }, tip,
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

/** 내 것인지 서버 것인지. scope 를 안 주는 서버는 owner 로 판단한다. */
function isMyPlaylist(playlist) {
  const me = String(store.get().user?.id || '');
  const scope = String(playlist.scope || '').toLowerCase();
  if (scope === 'user') return true;
  if (scope === 'guild' || scope === 'global') return false;
  return !!playlist.ownerUserId && String(playlist.ownerUserId) === me;
}

function renderLibrary(state) {
  clear(el.libBody);
  const needle = libraryQuery.toLowerCase();
  const match = (track) => !needle || [track.title, track.artist, track.provider].join(' ').toLowerCase().includes(needle);

  if (libraryTab === 'mine' || libraryTab === 'server') {
    const mine = libraryTab === 'mine';
    const rows = state.playlists.filter((playlist) => isMyPlaylist(playlist) === mine
      && (!needle || String(playlist.name || '').toLowerCase().includes(needle)));

    if (mine) el.libBody.appendChild(newPlaylistRow());
    if (!rows.length) {
      el.libBody.appendChild(emptyState('📁',
        mine ? '내 재생목록이 없어요' : '서버 재생목록이 없어요',
        mine ? '자주 듣는 곡을 묶어 두면 한 번에 담을 수 있어요.' : '서버 관리자가 만들어 둔 목록이 여기 나와요.'));
      return;
    }
    for (const playlist of rows) el.libBody.appendChild(playlistCard(playlist, mine));
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

/** 모달 없이 인라인으로. 이름만 넣으면 끝이다 (§12.2). */
function newPlaylistRow() {
  const input = h('input', {
    class: 'field', placeholder: '새 재생목록 이름', maxlength: '60',
    onKeydown: (event) => { if (event.key === 'Enter') { event.preventDefault(); submit(); } },
  });
  const button = bindAct(h('button', { class: 'btn btn--sm btn--primary', type: 'button', tip: '이름만 넣으면 바로 만들어져요' }, '+ 만들기'),
    () => submit());

  async function submit() {
    const name = input.value.trim();
    if (!name) { input.focus(); return; }
    const created = await call(() => api('/playlists/action', { body: { action: 'create', name, scope: 'user' } }),
      `재생목록 '${name}'을 만들었어요.`);
    if (created) { input.value = ''; refetchCold(); }
  }

  return h('div', { class: 'plnew' }, input, button);
}

function playlistDuration(entries) {
  const seconds = entries.reduce((acc, entry) => acc + trackSeconds(entry.track), 0);
  if (!seconds) return '';
  const minutes = Math.round(seconds / 60);
  return minutes >= 60 ? `${Math.floor(minutes / 60)}시간 ${minutes % 60}분` : `${minutes}분`;
}

function playlistCard(playlist, mine) {
  const entries = (playlist.entries || []).filter((entry) => entry.track);
  const count = playlist.entryCount ?? entries.length;
  const length = playlistDuration(entries);

  const enqueueAll = bindAct(h('button', {
    class: 'btn btn--sm', type: 'button', tip: '이 목록을 통째로 대기열에 담아요',
  }, '▶ 전부 담기'), () => enqueuePlaylist(playlist));
  setLock(enqueueAll, !canBulk(), lockReason('bulkEnqueue'));

  const more = h('button', {
    class: 'iconbtn', type: 'button', tip: '이름 바꾸기 · 삭제', 'aria-label': '재생목록 메뉴',
    onClick: (event) => openContextMenu(playlistMenu(playlist, mine), { anchor: event.currentTarget }),
  }, '⋯');
  setLock(more, !mine && !can('console'), mine ? '' : '서버 재생목록은 서버 관리자만 고칠 수 있어요');

  const card = h('div', {
    class: `card plcard plcard--${mine ? 'mine' : 'server'}`,
    'data-testid': 'playlist-card', dataset: { mqRow: '1' },
  },
    h('div', { class: 'plcard__head' },
      h('span', { class: 'plcard__icon', 'aria-hidden': 'true' }, mine ? '📃' : '🌐'),
      h('strong', null, playlist.name),
      h('span', { class: 'chip', tip: length ? `${count}곡 · 다 들으면 ${length}쯤 걸려요` : `${count}곡이에요` },
        length ? `${count}곡 · ${length}` : `${count}곡`),
      enqueueAll, more),
    h('div', { class: 'plcard__entries' },
      entries.slice(0, 5).map((entry) => h('div', { class: 'plcard__entry' },
        h('span', { class: 'row__sub' }, trackTitle(entry.track)),
        setLock(bindAct(h('button', {
          class: 'iconbtn', type: 'button', tip: '이 곡만 대기열에 담기', 'aria-label': '대기열에 담기',
        }, '＋'), () => enqueue(entry.track)), !can('search'), lockReason('search')),
        mine ? bindAct(h('button', {
          class: 'iconbtn iconbtn--danger', type: 'button', tip: '재생목록에서 빼기', 'aria-label': '재생목록에서 빼기',
        }, '✕'), () => removeFromPlaylist(playlist, entry)) : null)),
      count > 5 ? h('div', { class: 'row__sub' }, `· 외 ${count - 5}곡`) : null));

  bindContextTarget(card, () => playlistMenu(playlist, mine));
  return card;
}

/** 전부 담기는 별도 권한이다 (§15.4). 서버가 아직 이 키를 모르면 검색 권한으로 판단한다. */
function canBulk() {
  const permissions = store.get().permissions?.can || {};
  if (permissions.bulkEnqueue !== undefined) return can('bulkEnqueue');
  return can('search');
}

async function enqueuePlaylist(playlist) {
  const result = await call(() => api('/playlists/action', { body: { action: 'enqueue', playlistId: playlist.id } }));
  if (!result) return;
  // 조용히 자르면 안 된다. 몇 곡만 담겼는지 반드시 말한다 (§12.3)
  if (result.limited) toast(`대기열 한도까지 ${result.added ?? 0}곡만 담았어요.`, 'warn');
  else toast(`${result.added ?? playlist.entryCount ?? ''}곡을 담았어요.`.replace('  ', ' '), 'ok');
}

async function removeFromPlaylist(playlist, entry) {
  const ok = await confirmSheet({
    title: '이 곡을 뺄까요', desc: `${playlist.name} · ${trackTitle(entry.track)}`,
    danger: true, confirmText: '빼기',
  });
  if (!ok) return;
  await call(() => api('/playlists/action', {
    body: { action: 'removeTrack', playlistId: playlist.id, entryId: entry.id, cacheKey: trackKey(entry.track) },
  }), '재생목록에서 뺐어요.');
  refetchCold();
}

function playlistMenu(playlist, mine) {
  const editable = mine || can('console');
  const reason = mine ? '' : '서버 재생목록은 서버 관리자만 고칠 수 있어요';
  return [
    { icon: '▶', label: '전부 대기열에 담기', disabled: !canBulk(), reason: lockReason('bulkEnqueue'), onPick: () => enqueuePlaylist(playlist) },
    { icon: '✏', label: '이름 바꾸기', disabled: !editable, reason, onPick: () => renamePlaylist(playlist) },
    { icon: '🗑', label: '재생목록 삭제', danger: true, disabled: !editable, reason, onPick: () => deletePlaylist(playlist) },
  ];
}

async function renamePlaylist(playlist) {
  const input = h('input', { class: 'field', value: playlist.name || '', maxlength: '60' });
  const ok = await sheet({
    title: '이름 바꾸기', desc: '이 재생목록을 뭐라고 부를까요', body: input,
    dismissValue: false,
    actions: [{ label: '취소', kind: 'ghost', value: false }, { label: '저장', kind: 'primary', value: true, autofocus: true }],
  }).result;
  if (!ok) return;
  const name = input.value.trim();
  if (!name) return;
  await call(() => api('/playlists/action', { body: { action: 'rename', playlistId: playlist.id, name } }), '이름을 바꿨어요.');
  refetchCold();
}

async function deletePlaylist(playlist) {
  const ok = await confirmSheet({
    title: `'${playlist.name}'을 지울까요`,
    desc: `${playlist.entryCount ?? (playlist.entries || []).length}곡이 같이 사라져요. 되돌릴 수 없어요.`,
    danger: true, confirmText: '삭제',
  });
  if (!ok) return;
  await call(() => api('/playlists/action', { body: { action: 'delete', playlistId: playlist.id } }), '재생목록을 지웠어요.');
  refetchCold();
}

/* ── 재생목록 선택 팝오버 (§12.2) ──
 * 어디서든 ＋를 누르면 뜬다. 자주 쓴 순으로 놓고, 맨 위에 "새로 만들기"를 둔다.
 */
function openPlaylistPicker(track, anchor) {
  const state = store.get();
  const mine = state.playlists.filter(isMyPlaylist)
    .slice()
    .sort((a, b) => (b.usedCount || 0) - (a.usedCount || 0) || parseUtc(b.updatedUtc) - parseUtc(a.updatedUtc));

  const items = [{
    icon: '＋', label: '새로 만들어서 담기',
    onPick: async () => {
      const input = h('input', { class: 'field', placeholder: '재생목록 이름', maxlength: '60' });
      const ok = await sheet({
        title: '새 재생목록', desc: trackTitle(track), body: input, dismissValue: false,
        actions: [{ label: '취소', kind: 'ghost', value: false }, { label: '만들고 담기', kind: 'primary', value: true, autofocus: true }],
      }).result;
      if (!ok || !input.value.trim()) return;
      const created = await call(() => api('/playlists/action', {
        body: { action: 'create', name: input.value.trim(), scope: 'user', track },
      }), '새 재생목록에 담았어요.');
      if (created) refetchCold();
    },
  }];

  for (const playlist of mine.slice(0, 8)) {
    items.push({
      icon: '📃', label: playlist.name,
      hint: `${playlist.entryCount ?? (playlist.entries || []).length}곡`,
      onPick: async () => {
        await call(() => api('/playlists/action', { body: { action: 'addTrack', playlistId: playlist.id, track } }),
          `'${playlist.name}'에 담았어요.`);
        refetchCold();
      },
    });
  }
  if (!mine.length) items.push({ icon: '·', label: '아직 내 재생목록이 없어요', disabled: true, reason: '위에서 하나 만들면 여기 나와요' });

  openContextMenu(items, { anchor });
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
  // 원본 열기 (§34). 우클릭 메뉴에만 있으면 모바일에서는 사실상 없는 기능이다.
  // 제목 옆에 조용히 붙여 두고, 주소를 모르는 곡에서는 아예 숨긴다.
  el.nowLink = h('a', {
    class: 'now__link', target: '_blank', rel: 'noreferrer noopener', hidden: true,
    tip: '원본에서 열기', 'aria-label': '원본에서 열기',
  }, '↗');
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
  }, '⏭'), doSkip);

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

  // 자동 재생 켜고 끄기 — 관리자 전용이 아니라 autoplay 권한을 본다 (§24.3)
  el.autoplayBtn = bindAct(h('button', {
    class: 'pbtn', type: 'button', tip: '자동 재생 켜기/끄기', 'aria-label': '자동 재생',
  }, '📻'), async () => {
    const on = autoplayIsOn(store.get().player);
    // **서버 응답을 기다렸다 알린다.** 먼저 띄우면 권한이 없어 거절당해도
    // "켰어요" 가 뜨고 버튼은 그대로라, 눌렀는데 아무 일도 안 난 것처럼 보인다.
    await control('autoplay', on ? 0 : 1);
    toast(on ? '자동 재생을 껐어요. 대기열이 비면 조용해져요.' : '자동 재생을 켰어요. 대기열이 비면 알아서 골라 와요.', 'ok');
  });

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

  // 지금 재생 중인 곡에 누가 좋아요를 눌렀는지는 카드에 바로 보여준다 (§10.4)
  el.nowVoters = h('div', { class: 'nowvoters', hidden: true });

  // 다음 곡 한 줄 (§14.3). 카드를 하나 더 만들면 화면만 무거워진다.
  el.nextRow = h('div', { class: 'nextrow', hidden: true, dataset: { mqRow: '1' } });

  el.nowCard = h('section', { class: 'now', 'data-testid': 'now-playing', dataset: { mqRow: '1' } },
    h('div', { class: 'now__artwrap' }, el.nowArt),
    h('div', { class: 'now__side' },
      el.nowEyebrow,
      h('div', { class: 'now__titlerow' }, el.nowTitle, el.nowLink),
      el.nowBy,
      el.nowVoters,
      el.viz,
      h('div', { class: 'seek' }, el.seekTrack, h('div', { class: 'seek__times' }, el.timeNow, el.timeEnd)),
      h('div', { class: 'ctrl' },
        el.restartBtn, el.playBtn, el.skipBtn,
        el.repeatBtn, el.shuffleBtn, el.autoplayBtn,
        h('span', { class: 'ctrl__spacer' }),
        el.lyricsToggle,
        el.webBtn),
      h('div', { class: 'vols' }, el.volumeWrap, el.webVolWrap, el.webSync, el.webNote)),
    el.nextRow);

  bindSeek();
  bindContextTarget(el.nowCard, () => {
    const current = store.get().current;
    return current ? trackMenu(current.track, { itemId: current.id, item: current, source: 'now' }) : null;
  });
  el.stageScroll = h('div', { class: 'stage__scroll scroll' }, el.nowCard, el.lyricsBox);
  return el.stageScroll;
}

/* ── 스킵 (§10.5) ──
 * 투표 모드면 버튼이 `⏭ 스킵 2/3`이 되고, 다시 누르면 내 표를 뺀다.
 */
async function doSkip() {
  const vote = store.get().skipVote;
  if (vote && vote.mine) {
    await call(() => api('/control', { body: { action: 'skipVoteCancel' } }), '스킵 투표를 취소했어요.');
    return;
  }
  const result = await call(() => api('/control', {
    body: { action: 'skip', value: null, expectedItemId: store.get().current?.id || null },
  }));
  if (!result) return;
  if (result.skipped === false && result.vote) {
    store.patch({ skipVote: result.vote });
    const left = Math.max(0, (result.vote.need || 0) - (result.vote.have || 0));
    toast(left > 0 ? `스킵에 한 표 넣었어요. ${left}표 더 모이면 넘어가요.` : '스킵 투표를 넣었어요.', 'ok');
  }
}

const SKIP_BASIS = {
  listeners: '듣는 사람',
  viewers: '리모컨을 보는 사람',
  either: '듣는 사람이나 보는 사람',
  both: '듣는 사람과 보는 사람',
};

/** 스킵 권한은 §10.5에서 `playback` 에서 떨어져 나왔다. 서버는 오직 `skip_rule` 만 본다.
 *  그런데도 `can('skip') || can('playback')` 로 두면, `곡 넘기기 = 관리자`인 서버에서 일반 멤버에게
 *  ⏭ 가 **활성으로 보이고 누르면 403** 이 난다(§23.3 정면 위반). 그래서 구버전 폴백은
 *  서버가 `skip` 키를 아예 모를 때만 쓴다 — `canAutoplay()` 가 쓰는 것과 같은 가드다. */
function canSkipNow() {
  const permissions = store.get().permissions?.can || {};
  if (permissions.skip !== undefined) return can('skip');
  return can('skip') || can('playback');
}

function renderSkipButton(offline, offlineReason) {
  const state = store.get();
  const vote = state.skipVote;
  const canSkip = canSkipNow();
  const instant = tierOf() !== 'member' || !!state.current?.isMine;

  let tip;
  if (vote && vote.need) {
    el.skipBtn.classList.add('pbtn--wide');
    el.skipBtn.textContent = `⏭ 스킵 ${vote.have || 0}/${vote.need}`;
    el.skipBtn.setAttribute('aria-pressed', String(!!vote.mine));
    // 한 표 남았다는 걸 알면 누를 마음이 생긴다
    el.skipBtn.classList.toggle('pbtn--almost', (vote.need - (vote.have || 0)) === 1);
    el.skipBtn.setAttribute('aria-label', `스킵 투표 ${vote.have || 0} / ${vote.need}`);
    // 모수(`pool`)는 서버가 줄 때만 말한다. `pool ?? need` 로 두면 듣는 사람 5명 중 3명 필요일 때
    // `3명 중 3명` 이라는 거짓 문장이 뜬다.
    const basis = SKIP_BASIS[vote.basis] || '듣는 사람';
    tip = vote.mine
      ? '내 표가 들어가 있어요 · 다시 누르면 빼요'
      : Number.isFinite(Number(vote.pool))
        ? `${basis} ${Number(vote.pool)}명 중 ${vote.need}명이 동의하면 넘어가요`
        : `${basis} ${vote.need}명이 동의하면 넘어가요`;
  } else {
    el.skipBtn.classList.remove('pbtn--wide', 'pbtn--almost');
    el.skipBtn.removeAttribute('aria-pressed');
    const voteMode = !!(state.settings && state.settings.voteSkipEnabled);
    el.skipBtn.textContent = '⏭';
    el.skipBtn.setAttribute('aria-label', '다음 곡');
    tip = voteMode
      ? (instant ? '바로 넘기기 — 관리자라서 투표 없이 넘어가요' : '스킵 투표를 열어요')
      : '다음 곡으로 넘겨요';
  }
  // setLock 은 잠금이 풀릴 때 처음 붙어 있던 툴팁으로 되돌린다. 그래서 순서가 중요하다 — 잠금 먼저.
  setLock(el.skipBtn, offline || !canSkip, offline ? offlineReason : lockReason('skip'));
  if (!offline && canSkip) el.skipBtn.setAttribute('data-tip', tip);
}

/* ── 다음 곡 (§14.3) ── */
function renderNextRow(state) {
  const next = state.next;
  if (!next || !next.item || !next.item.track) { el.nextRow.hidden = true; return; }
  const fromAutoplay = next.source === 'autoplay';
  const track = next.item.track;

  el.nextRow.hidden = false;
  el.nextRow.dataset.kind = fromAutoplay ? 'autoplay' : 'queue';

  const key = `${next.source}:${trackKey(track)}`;
  if (el.nextRow.__key === key) return;      // 안 바뀌었으면 다시 그리지 않는다
  const changed = !!el.nextRow.__key && el.nextRow.__key !== key;
  el.nextRow.__key = key;

  clear(el.nextRow);
  put(el.nextRow,
    h('span', { class: 'nextrow__tag' }, fromAutoplay ? '📻 다음 (자동)' : '다음'),
    mqText(trackTitle(track), 'nextrow__title'),
    fromAutoplay
      ? h('span', { class: 'nextrow__sub' }, '대기열이 비면 이 곡이 나와요')
      : h('span', { class: 'nextrow__sub' }, `· ${next.item.requestedByDisplay || '알 수 없음'}`),
    h('span', { class: 'ctrl__spacer' }),
    fromAutoplay ? nextRowActions(track) : null);

  el.nextRow.setAttribute('data-tip', fromAutoplay
    ? '자동 재생이 골라 둔 후보예요. 누가 곡을 담으면 밀려요'
    : `다음에 나갈 곡이에요 · ${next.item.requestedByDisplay || '알 수 없음'}님이 담았어요`);

  bindContextTarget(el.nextRow, () => trackMenu(track, { source: 'next' }));
  if (changed) flashNode(el.nextRow);
  marquee.scan(el.nextRow);
}

function nextRowActions(track) {
  const reroll = bindAct(h('button', {
    class: 'btn btn--sm btn--ghost', type: 'button',
    tip: '이 후보를 빼고 다시 골라요',
  }, '📻 이 곡 말고'), rerollAutoplay);
  setLock(reroll, !canAutoplay(), lockReason('autoplay'));

  const add = bindAct(h('button', {
    class: 'btn btn--sm', type: 'button', tip: '이 곡을 대기열에 확정으로 담아요',
  }, '＋ 담기'), () => enqueue(track));
  setLock(add, !can('search'), lockReason('search'));

  return h('span', { class: 'nextrow__acts' }, reroll, add);
}

function canAutoplay() {
  const permissions = store.get().permissions?.can || {};
  // autoplay 권한이 아직 없는 서버는 예전 키(autoplaySeed)로 판단한다
  if (permissions.autoplay !== undefined) return can('autoplay');
  return can('autoplaySeed');
}

async function rerollAutoplay() {
  const key = trackKey(store.get().next?.item?.track);
  const result = await call(() => api('/autoplay/reroll', { body: { cacheKey: key } }),
    '다른 곡으로 다시 골랐어요.');
  if (result) refetchHot();
}

async function control(action, value, extra) {
  const state = store.get();
  await call(() => api('/control', {
    body: Object.assign({ action, value: value ?? null, expectedItemId: state.current?.id || null }, extra),
  }));
}

/* ═══════════════════════ 웹에서 듣기 (§9) ═══════════════════════
 * 서버는 오디오를 한 바이트도 나르지 않는다. 브라우저가 YouTube·SoundCloud 에서 직접 받아
 * 재생하고, 위치·곡 정보만 서버에서 받아 봇을 따라간다. 그래서 서버 추가 부담이 0이다.
 * 듣기 전용이라 플레이어 UI는 안 보여준다 — 여기서 조작해도 봇은 꿈쩍하지 않는다.
 *
 * 제공자마다 플레이어가 다르다. **둘을 동시에 켜면 소리가 겹친다.** 그래서 곡이 바뀔 때
 * 쓰지 않는 쪽을 반드시 먼저 멈춘다(`stopVideoQuietly`).
 */

const WEB_SYNC_GAP = 2;        // 봇과 이만큼 벌어지면 조용히 맞춘다 (초)
const WEB_OFFSET_LIMIT = 10;   // 싱크 보정 한계 (초). 서버 검증값과 같아야 한다.
/* 이 안쪽이면 "방금 시작한 곡"으로 보고 0초부터 튼다 (§31).
 * 곡이 바뀌는 순간 계산된 위치가 0.3초쯤 나오는데, 거기서 시작하면 도입부가 잘려
 * 사람에게는 곡이 끊긴 것처럼 들린다. 앞부분은 잘라내지 않는 쪽이 낫다. */
const WEB_START_SNAP = 1.5;
/* 0.1초 단위. 0.5초는 "조금만 더" 를 못 맞춘다 — 사람 귀는 100ms 어긋남을 잡아낸다.
 * 길게 누르면 빨라지므로 ±10초까지 가는 데도 오래 안 걸린다. */
const WEB_OFFSET_STEP = 0.1;

/* 브라우저 자동재생 정책 때문에 새로고침 뒤에는 반드시 사용자가 한 번 눌러야 한다.
 * 그래서 "켜 두겠다는 뜻"(webWanted)과 "지금 켜져 있다"(webOn)를 나눠 둔다. */
const webWanted = prefGet('webPlayback') === '1';
let webOn = false;
let webVolume = clampVolume(Number(prefGet('webVolume')));
let ytApiPromise = null;
let ytPlayer = null;
let ytReady = false;
let scApiPromise = null;
let scWidget = null;
let scReady = false;
/** 지금 물린 소스. `{ kind: 'yt'|'sc', key }` — 같은 곡이면 다시 로드하지 않는다. */
let webSource = null;
let webTimer = 0;
let webBlocked = '';           // 외부 스크립트를 못 불러왔을 때의 이유
let webOffset = clampOffset(Number(prefGet('webOffset')));

function clampVolume(value) {
  const n = Number.isFinite(value) ? value : 60;
  return Math.round(Math.min(100, Math.max(0, n)));
}

/** 싱크 보정은 음수가 정상이다. 0.1초 단위로 반올림한다.
 *
 * 부동소수 누적을 막으려고 정수(밀리초)로 반올림해서 되돌린다. 0.1 을 그냥 더하면
 * `0.30000000000000004` 같은 값이 쌓여서 표시가 흔들리고 서버 검증에도 걸린다. */
function clampOffset(value) {
  const n = Number.isFinite(value) ? value : 0;
  const ms = Math.round(n * 1000 / (WEB_OFFSET_STEP * 1000)) * (WEB_OFFSET_STEP * 1000);
  const clamped = Math.min(WEB_OFFSET_LIMIT * 1000, Math.max(-WEB_OFFSET_LIMIT * 1000, ms));
  return Math.round(clamped) / 1000;
}

/* 부호 뜻을 글자로 못 박는다. `+0.3초` 만 보여 주면 그게 늦추는 건지 당기는 건지
 * 아무도 모르고, 실제로 툴팁은 "뒤로 미뤄요" 인데 계산은 앞당기고 있었다. */
function offsetLabel(value) {
  if (!value) return '딱 맞음';
  return value > 0
    ? `${value.toFixed(1)}초 늦춤`
    : `${Math.abs(value).toFixed(1)}초 당김`;
}

/** 봇 위치에 보정을 얹은 값. 웹 재생이 참고하는 유일한 시각이다.
 *
 * **양수 = 늦춘다** → 웹이 봇보다 앞서 들릴 때 쓰는 값이라, 더 **이른** 지점을 튼다.
 * 자막 싱크와 같은 규약이다.
 *
 * 서버가 준 0초 시각(`startedUtc`)이 있으면 그것으로 계산한다 (§31). 폴링으로 받은
 * "지금 몇 초"는 표본마다 흔들려서 곡이 바뀔 때마다 조금씩 다르게 맞았다.
 * 절대 시각은 곡이 바뀌어도 같은 식이라 **곡별 편차가 안 생긴다.**
 *
 * 보정은 두 겹이다. 서버 전역(`webSyncOffsetMs`)으로 큰 차이를 한 번에 잡고,
 * 개인(`webOffset`)이 사람마다 남는 차이를 다듬는다.
 */
function syncOffsetSeconds() {
  const server = (store.get().schedule?.webSyncOffsetMs || 0) / 1000;
  return webOffset + server;
}

function webTargetPosition() {
  const startedAt = clock.startedAt();
  const raw = startedAt && !clock.paused && !clock.stopped
    ? (Date.now() - startedAt) / 1000
    : clock.position();
  return Math.max(0, raw - syncOffsetSeconds());
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

  // 싱크 보정. 회선과 버퍼가 사람마다 달라 봇과 몇 초씩 어긋나는 걸 직접 맞춘다.
  el.webOffLabel = h('span', { class: 'sync__value' }, offsetLabel(webOffset));
  const bump = (delta) => bindAct(h('button', {
    class: 'btn btn--sm btn--ghost sync__btn', type: 'button',
    'aria-label': delta > 0 ? '웹 소리 늦추기' : '웹 소리 당기기',
    tip: delta > 0
      ? '웹이 봇보다 빨리 들릴 때. 0.1초씩 늦춰요.'
      : '웹이 봇보다 늦게 들릴 때. 0.1초씩 당겨요.',
  }, delta > 0 ? '＋' : '－'), () => setWebOffset(webOffset + delta));

  el.webSync = h('div', {
    class: 'sync', hidden: true,
    tip: '봇과 웹 소리가 어긋날 때 나만 쓰는 보정이에요. 계정에 저장돼서 다른 기기에서도 그대로예요.',
  },
    h('span', { class: 'sync__tag' }, '⏱ 싱크(나만)'),
    bump(-WEB_OFFSET_STEP), el.webOffLabel, bump(WEB_OFFSET_STEP),
    bindAct(h('button', {
      class: 'btn btn--sm btn--ghost sync__btn', type: 'button', tip: '보정을 0으로 되돌려요.',
    }, '↺'), () => setWebOffset(0)));

  el.webNote = h('div', { class: 'webnote', hidden: true, role: 'status' });

  // 숨긴 1×1 플레이어 둘. 화면에 안 보이지만 소리는 난다.
  el.webHost = h('div', { class: 'webhost', 'aria-hidden': 'true' },
    h('div', { id: 'macham-yt' }),
    h('div', { id: 'macham-sc' }));
  document.body.appendChild(el.webHost);

  window.addEventListener('pagehide', stopWebPlayback);
}

function setWebOffset(next) {
  webOffset = clampOffset(next);
  el.webOffLabel.textContent = offsetLabel(webOffset);
  prefSet('webOffset', String(webOffset));
  // 바로 체감되게 지금 위치를 다시 맞춘다. 다음 틱을 기다리면 조정한 티가 안 난다.
  seekWebTo(webTargetPosition());
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

/** SoundCloud Widget API. 유튜브와 달리 트랙 주소로 iframe 을 만들어 붙인다. */
function loadSoundCloudApi() {
  if (scApiPromise) return scApiPromise;
  scApiPromise = new Promise((resolve, reject) => {
    if (window.SC && window.SC.Widget) { resolve(window.SC); return; }
    const timer = setTimeout(() => reject(new Error('사운드클라우드 플레이어가 12초 안에 응답하지 않았어요.')), 12000);
    const script = document.createElement('script');
    script.src = 'https://w.soundcloud.com/player/api.js';
    script.async = true;
    script.onload = () => {
      clearTimeout(timer);
      if (window.SC && window.SC.Widget) resolve(window.SC);
      else reject(new Error('사운드클라우드 플레이어를 준비하지 못했어요.'));
    };
    script.onerror = () => {
      clearTimeout(timer);
      reject(new Error('사운드클라우드 스크립트를 불러오지 못했어요.'));
    };
    document.head.appendChild(script);
  });
  return scApiPromise;
}

/* SoundCloud 위젯은 곡을 바꿀 때 iframe src 를 갈아 끼운다. 유튜브의 loadVideoById 처럼
 * 한 플레이어를 재사용하는 API 가 없다. 그래서 곡마다 위젯을 새로 묶는다. */
function mountSoundCloud(sourceUrl, startSeconds, paused) {
  const host = document.getElementById('macham-sc');
  if (!host) return;
  host.textContent = '';
  const frame = document.createElement('iframe');
  frame.width = '1';
  frame.height = '1';
  frame.allow = 'autoplay';
  frame.setAttribute('frameborder', 'no');
  const params = new URLSearchParams({
    url: sourceUrl,
    auto_play: paused ? 'false' : 'true',
    show_artwork: 'false',
    visual: 'false',
  });
  frame.src = `https://w.soundcloud.com/player/?${params.toString()}`;
  host.appendChild(frame);

  scReady = false;
  scWidget = window.SC.Widget(frame);
  scWidget.bind(window.SC.Widget.Events.READY, () => {
    scReady = true;
    try {
      scWidget.setVolume(webVolume);
      if (startSeconds > 0) scWidget.seekTo(startSeconds * 1000);
      if (paused) scWidget.pause();
    } catch { /* 다음 틱에서 다시 맞춘다 */ }
  });
  // 임베드가 막힌 트랙이 꽤 있다. 조용히 무음이 되면 고장으로 보이니 이유를 말한다.
  scWidget.bind(window.SC.Widget.Events.ERROR, () => {
    webSource = null;
    setWebNote('이 사운드클라우드 곡은 다른 사이트에서의 재생이 막혀 있어요. Discord로 들어 주세요.');
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

  // **지금 곡에 필요한 쪽만 준비한다.** 유튜브가 막혀 있어도 사운드클라우드 곡은 들려야 하고,
  // 그 반대도 마찬가지다. 예전엔 유튜브가 실패하면 토글을 통째로 잠갔다.
  const need = webSourceOf(store.get().current?.track);
  const kind = need?.kind || 'yt';
  try {
    if (kind === 'yt') {
      await loadYouTubeApi();
      await createYtPlayer();
    } else {
      await loadSoundCloudApi();
    }
  } catch (error) {
    const what = kind === 'yt' ? '유튜브' : '사운드클라우드';
    // 토글은 살려 둔다 — 다음 곡이 다른 제공자면 멀쩡히 들린다.
    const reason = `${error.message || `${what} 플레이어를 불러오지 못했어요.`} 이 곡은 Discord로 들어 주세요.`;
    setWebNote(reason);
    toast(reason, 'warn');
    startWebLoop();
    return;
  }
  startWebLoop();
  syncWebNow(true);
  toast('웹에서 듣기를 켰어요. 봇이 있는 위치에 맞춰 재생할게요.', 'ok');
}

function stopWebPlayback() {
  clearInterval(webTimer);
  webTimer = 0;
  stopVideoQuietly();
}

function startWebLoop() {
  clearInterval(webTimer);
  webTimer = setInterval(webTick, 1500);
}

/** 이 곡을 어느 플레이어로 틀지. 못 트는 곡이면 `null`. */
function webSourceOf(track) {
  if (!track) return null;
  const provider = String(track.provider || '');
  if (provider.startsWith('YouTube')) {
    return track.contentId ? { kind: 'yt', key: track.contentId } : null;
  }
  if (provider === 'SoundCloud') {
    // 위젯은 영상 ID 가 아니라 **트랙 주소**를 받는다. 주소가 없으면 못 튼다.
    const url = track.sourceUrl || track.url || '';
    return url.startsWith('http') ? { kind: 'sc', key: url } : null;
  }
  return null;
}

/** 곡이 바뀌거나 일시정지가 바뀌면 부른다. force면 위치까지 다시 맞춘다. */
function syncWebNow(force) {
  if (!webOn) return;
  const state = store.get();
  const current = state.current;
  const next = webSourceOf(current?.track);

  if (!current) {
    stopVideoQuietly();
    setWebNote('재생 중인 곡이 없어요. 봇이 곡을 틀면 바로 따라갈게요.');
    return;
  }
  if (!next) {
    stopVideoQuietly();
    setWebNote('이 곡은 웹에서 재생할 수 없어요. Discord로 들어 주세요.');
    return;
  }
  // 필요한 플레이어가 아직 안 붙었으면 붙이고 나서 다시 부른다.
  if (next.kind === 'yt' && !ytReady) { ensureYouTube(); return; }
  if (next.kind === 'sc' && !window.SC?.Widget) { ensureSoundCloud(); return; }

  setWebNote('');
  const paused = !!state.player?.isPaused;
  const position = webTargetPosition();
  const changed = force || !webSource || webSource.kind !== next.kind || webSource.key !== next.key;

  if (changed) {
    // **다른 제공자로 넘어갈 때 이전 플레이어를 반드시 멈춘다.** 안 그러면 두 곡이 겹쳐 들린다.
    if (webSource && webSource.kind !== next.kind) stopVideoQuietly();
    webSource = next;
    if (next.kind === 'yt') {
      try {
        // 곡이 방금 시작했으면 **0초부터** 튼다. 절대 시각 기준이라 몇 ms 뒤처져 있어도
        // `startSeconds` 에 그 값을 넣으면 도입부가 잘려서 "끊긴 것처럼" 들린다 (§31).
        const from = position < WEB_START_SNAP ? 0 : position;
        ytPlayer.loadVideoById({ videoId: next.key, startSeconds: from });
        ytPlayer.setVolume(webVolume);
        if (paused) setTimeout(() => { try { ytPlayer.pauseVideo(); } catch { /* 무시 */ } }, 500);
        webPreloaded = null;
      } catch { /* 다음 틱에서 다시 시도한다 */ }
    } else {
      mountSoundCloud(next.key, position, paused);
    }
    return;
  }
  try {
    if (next.kind === 'yt') {
      if (paused) ytPlayer.pauseVideo();
      else ytPlayer.playVideo();
    } else if (scReady) {
      if (paused) scWidget.pause();
      else scWidget.play();
    }
  } catch { /* 무시 */ }
}

/** 켠 김에 준비만 해 둔다. 실패해도 토글은 살려 둔다 — 다른 제공자는 멀쩡할 수 있다. */
function ensureYouTube() {
  loadYouTubeApi().then(createYtPlayer).then(() => syncWebNow(true)).catch((error) => {
    setWebNote(`${error.message || '유튜브 플레이어를 불러오지 못했어요.'} 이 곡은 Discord로 들어 주세요.`);
  });
}

function ensureSoundCloud() {
  loadSoundCloudApi().then(() => syncWebNow(true)).catch((error) => {
    setWebNote(`${error.message || '사운드클라우드 플레이어를 불러오지 못했어요.'} 이 곡은 Discord로 들어 주세요.`);
  });
}

function stopVideoQuietly() {
  webSource = null;
  try { ytPlayer?.stopVideo?.(); } catch { /* 무시 */ }
  try { scWidget?.pause?.(); } catch { /* 무시 */ }
  const host = document.getElementById('macham-sc');
  if (host) host.textContent = '';   // iframe 을 떼야 소리가 완전히 멎는다
  scWidget = null;
  scReady = false;
}

/** 지금 위치를 강제로 맞춘다. 싱크 보정을 바꿨을 때 즉시 반영하려고 쓴다. */
function seekWebTo(seconds) {
  if (!webOn || !webSource) return;
  try {
    if (webSource.kind === 'yt' && ytReady) ytPlayer.seekTo(seconds, true);
    else if (webSource.kind === 'sc' && scReady) scWidget.seekTo(seconds * 1000);
  } catch { /* 무시 */ }
}

/* 곡이 바뀔 때 소리가 끊기는 걸 막는다 (§31 최우선).
 *
 * 유튜브 플레이어는 `loadVideoById` 로 갈아 끼우는데, 그 사이 몇 초가 무음이다.
 * 곡이 실제로 끝난 **뒤에** 서버 이벤트를 받고 나서야 로드하면 그 공백이 그대로 들린다.
 *
 * 그래서 다음 곡을 미리 **cue** 해 둔다. `cueVideoById` 는 버퍼만 채우고 소리는 안 낸다.
 * 서버가 알려 준 다음 곡 시작 시각이 되면 이미 준비된 것을 재생만 시작하면 된다.
 */
let webPreloaded = null;   // 미리 준비해 둔 다음 곡 키

function preloadNext() {
  if (!webOn || !ytReady || !ytPlayer) return;
  const state = store.get();
  const next = webSourceOf(state.next?.track);
  // 유튜브끼리일 때만 의미가 있다. 제공자가 바뀌면 어차피 플레이어를 갈아야 한다.
  if (!next || next.kind !== 'yt' || next.key === webPreloaded) return;
  const startAt = parseUtc(state.schedule?.nextStartUtc);
  if (!startAt) return;
  const left = startAt + clock.skew - Date.now();
  // 너무 일찍 cue 하면 지금 곡이 끊긴다. 끝나기 직전에만 준비한다.
  if (left > WEB_PRELOAD_LEAD || left < 0) return;
  webPreloaded = next.key;
}

const WEB_PRELOAD_LEAD = 8000;  // 다음 곡을 준비하기 시작하는 시점 (ms)

/** 매 프레임 맞추면 소리가 튄다. 2초 이상 벌어졌을 때만 조용히 옮긴다. */
function webTick() {
  if (!webOn || !webSource) return;
  // 봇이 음성에서 빠졌는데 웹만 계속 트는 상태를 막는다 (§36).
  // 리모컨은 멈춰 있는데 내 브라우저에서만 노래가 나오면 상황이 전혀 안 읽힌다.
  if (clock.stopped) {
    try { ytPlayer?.pauseVideo?.(); } catch { /* 무시 */ }
    try { scWidget?.pause?.(); } catch { /* 무시 */ }
    return;
  }
  preloadNext();
  if (store.get().player?.isPaused) return;
  const there = webTargetPosition();

  if (webSource.kind === 'yt') {
    if (!ytReady || !ytPlayer) return;
    let here = 0;
    try { here = Number(ytPlayer.getCurrentTime()) || 0; } catch { return; }
    if (Math.abs(there - here) > WEB_SYNC_GAP) {
      try { ytPlayer.seekTo(there, true); ytPlayer.playVideo(); } catch { /* 무시 */ }
    }
    return;
  }
  if (!scReady || !scWidget) return;
  // 위젯은 위치를 콜백으로만 준다. 밀리초 단위라 초로 바꿔서 본다.
  try {
    scWidget.getPosition((ms) => {
      const here = Number(ms) / 1000;
      if (!Number.isFinite(here)) return;
      if (Math.abs(there - here) > WEB_SYNC_GAP) {
        try { scWidget.seekTo(there * 1000); scWidget.play(); } catch { /* 무시 */ }
      }
    });
  } catch { /* 무시 */ }
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
  if (el.webSync) el.webSync.hidden = !webOn;
  if (!webOn) setWebNote(webBlocked || '');
}

/** 자동 재생이 켜져 있는가.
 *
 * **판정이 한 군데여야 한다.** 예전엔 그리는 쪽은 `!== false`(모르면 켜짐), 누르는 쪽은
 * `!!`(모르면 꺼짐)를 써서 서로 반대였다. 값이 아직 안 온 상태에서 누르면 켜져 보이는
 * 버튼이 "켰어요" 토스트를 띄우고 아무것도 안 바뀌는, 딱 어색한 동작이 나왔다.
 */
function autoplayIsOn(player) {
  return (player || {}).autoplayEnabled !== false;
}

/* ── 진행바 드래그 ── */

let seeking = false;

/** 곡이 끝나기 직전인가 (§31). 서버가 정한 값(`seekLockoutMs`)을 그대로 쓴다. */
function seekLocked() {
  const lockMs = store.get().schedule?.seekLockoutMs ?? 0;
  if (!lockMs || !clock.duration) return false;
  return (clock.duration - clock.position()) * 1000 <= lockMs;
}

function seekLockNote() {
  const seconds = Math.round((store.get().schedule?.seekLockoutMs || 0) / 1000);
  return `곡이 끝나기 ${seconds}초 전부터는 위치를 못 옮겨요. 다음 곡으로 넘어가는 중이라서요.`;
}

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
    // 끝나기 직전에는 못 움직인다 (§31). 그 이동이 반영되기 전에 다음 곡으로 넘어가서
    // 웹만 엉뚱한 지점에 남고 봇은 다음 곡을 트는 상태가 된다.
    if (seekLocked()) { toast(seekLockNote(), 'warn'); return; }
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
      h('span', { class: clock.stopped ? 'dot dot--offline' : player.isPaused ? 'dot dot--idle' : 'dot dot--listening' }),
      clock.stopped ? '멈춤 · 봇이 음성 채널에 없어요' : player.isPaused ? '일시정지' : '재생 중');
    const title = trackTitle(current.track);
    if (el.nowTitle.firstElementChild.textContent !== title) el.nowTitle.firstElementChild.textContent = title;
    // 원본 링크 (§34). 주소를 모르는 곡에서는 숨긴다 — 눌러도 아무 데도 안 가는 버튼이 제일 나쁘다.
    const nowUrl = trackUrl(current.track);
    el.nowLink.hidden = !nowUrl;
    if (nowUrl) el.nowLink.href = nowUrl;
    put(clear(el.nowBy),
      current.track?.artist ? h('span', null, current.track.artist) : null,
      current.track?.artist ? h('span', { class: 'row__sub' }, '·') : null,
      h('span', null, '신청 '),
      personButton(current.requestedByUserId, current.requestedByDisplay || '알 수 없음'),
      current.requestedByUserId && String(current.requestedByUserId) === String(state.user?.id)
        ? h('span', { class: 'chip chip--accent' }, '내 곡') : null);
    el.timeEnd.textContent = fmtTime(current.durationSeconds || trackSeconds(current.track));
  }

  // 지금 재생 중인 곡은 제일 궁금한 곡이라 툴팁이 아니라 카드에 바로 보여준다 (§10.4).
  // 서버가 `current.score` 를 안 실어 주는 동안에는 대기열에 있던 시절의 점수를 쓴다.
  const nowScore = scoreForCurrent(current);
  clear(el.nowVoters);
  if (current) el.nowVoters.appendChild(nowVoteButtons(current));
  if (nowScore && voterList(nowScore).length) {
    el.nowVoters.appendChild(voterPanel(nowScore));
  }
  el.nowVoters.hidden = !current;

  el.playBtn.textContent = player.isPaused ? '▶' : '⏸';
  el.playBtn.setAttribute('aria-label', player.isPaused ? '재생' : '일시정지');
  // 봇이 음성에 없으면 **멈춤으로 확실히 보여준다** (§36). 진행바가 혼자 가면
  // 재생 중인 줄 알고 왜 소리가 안 나는지 찾게 된다.
  el.nowCard.classList.toggle('now--stopped', !!clock.stopped);

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

  // 자동 재생 토글 (§24.3)
  const autoplayOn = autoplayIsOn(player);
  el.autoplayBtn.setAttribute('aria-pressed', String(autoplayOn));
  // 색만으로 구분하면 흑백·고대비 화면에서 켜짐/꺼짐이 같아 보인다. 아이콘도 바꾼다.
  el.autoplayBtn.textContent = autoplayOn ? '📻' : '🚫';

  /* 봇이 음성에 없다고 무조건 잠그지 않는다.
   *
   * 서버가 `봇이 음성 채널에 있어야만 조작` 을 끄면 "봇 없이도 리모컨을 쓰겠다"는 뜻이다.
   * 그런데 화면이 그 설정을 몰라서 `!connected` 만 보고 잠가 버렸고, 그래서 설정을 꺼도
   * 아무것도 안 풀렸다. 서버 판정(`same_voice_satisfied`)과 같은 기준을 화면도 쓴다.
   *
   * 봇 프로세스가 꺼져 있으면(`!online`) 그때는 어느 설정이든 조작이 닿을 곳이 없다. */
  const needsVoice = store.get().settings?.requireVoiceForPlayback !== false;
  const offline = !online || (needsVoice && !connected);
  const offlineReason = !online ? '봇이 꺼져 있어요' : '봇이 음성 채널에 없어요';
  for (const [node, key] of [[el.playBtn, 'playback'], [el.restartBtn, 'seek'],
    [el.repeatBtn, 'playback'], [el.shuffleBtn, 'queueEdit']]) {
    setLock(node, offline || !can(key), offline ? offlineReason : lockReason(key));
  }
  /* 자동 재생만 음성 연결을 안 따진다. 저장되는 설정이라 봇이 음성에 없을 때야말로
   * 켜 두려는 값이다 — 여기서 `offline` 을 그대로 쓰면 "재생 중인 곡이 없을 때
   * 아무리 눌러도 안 켜짐"이 된다. 봇 프로세스가 꺼져 있으면 저장 자체가 안 되니 그때만 잠근다. */
  const autoplayOffline = !online;
  setLock(el.autoplayBtn, autoplayOffline || !canAutoplay(),
    autoplayOffline ? offlineReason : lockReason('autoplay'));
  if (!autoplayOffline && canAutoplay()) {
    el.autoplayBtn.setAttribute('data-tip', autoplayOn
      ? '자동 재생이 켜져 있어요 · 대기열이 비면 알아서 골라 와요'
      : '자동 재생이 꺼져 있어요 · 대기열이 비면 조용해져요');
  }
  renderSkipButton(offline, offlineReason);
  renderNextRow(state);
  /* 잠기는 이유가 셋인데 메시지를 하나로 뭉치면 안 된다. 권한은 멀쩡한데 길이를 모르는 곡이면
   * `lockReason` 이 마지막 분기까지 내려가 "이 서버의 멤버여야 눌러요" 라는 엉뚱한 말을 한다
   * (일시정지는 되는데 위치 이동만 안 되는 것처럼 보여서 두 배로 헷갈린다). 사유별로 갈라 준다. */
  const seekLock = offline ? offlineReason
    : !can('seek') ? lockReason('seek')
    : !clock.duration ? '길이를 알 수 없는 곡이라 위치를 옮길 수 없어요'
    : '';
  setLock(el.seekTrack, !!seekLock, seekLock);
  el.volume.disabled = offline || !can('volume');
  el.volumeWrap.setAttribute('data-tip', el.volume.disabled
    ? (offline ? offlineReason : lockReason('volume'))
    : '서버 볼륨이에요. 바꾸면 Discord로 듣는 모든 사람에게 같이 적용돼요');
  el.volumeWrap.classList.toggle('is-locked', el.volume.disabled);

  // 곡이 바뀌면 스크린리더에 알리고, 내 신청곡이면 알림도 띄운다
  const id = current?.id || null;
  const changed = id !== lastCurrentId;
  if (changed) {
    lastCurrentId = id;
    if (current) {
      el.live.textContent = `지금 재생: ${trackTitle(current.track)} · 신청 ${current.requestedByDisplay || ''}`;
      if (String(current.requestedByUserId || '') === String(state.user?.id || '')) {
        pushNotify('song', { title: '내 신청곡이 시작됐어요', body: trackTitle(current.track), icon: artUrl(current.track) });
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

  // 제안은 탭이 아니라 헤더 버튼 + 모달이다 (§11). 알맹이는 여기서 한 번만 만들어 두고 모달이 빌려 쓴다.
  el.suggestPane = buildSuggestPane();

  el.sidePanes = {
    chat: buildChatPane(),
    members: buildMembersPane(),
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
  if (id === 'audit') loadAudit();
  syncMobileTabs();
  marquee.scan(el.side);
}

function openDrawer(open) {
  el.side.dataset.open = open ? '1' : '0';
  el.drawerBtn?.setAttribute('aria-expanded', String(!!open));
  syncScrim();
}

/** 좌우 어느 쪽이든 열려 있으면 뒷막이 하나만 깔린다. */
function syncScrim() {
  const open = el.side?.dataset.open === '1' || el.rail?.dataset.open === '1';
  if (open && !el.scrim) {
    el.scrim = h('div', {
      class: 'side-scrim',
      onClick: () => { openDrawer(false); openRailDrawer(false); },
    });
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
    ava: personAvatar(message.userId, message.displayName, message.avatarUrl),
    main: h('div', { class: 'msg__col' }),
    tools: h('div', { class: 'msg__tools' }),
  };
  node.append(node.__parts.ava, node.__parts.main, node.__parts.tools);
  bindContextTarget(node, () => (node.__message ? messageMenu(node.__message) : null));
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
      : personAvatar(message.userId, message.displayName, message.avatarUrl);
    p.ava.replaceWith(next);
    p.ava = next;
  } else if (wantGutter) {
    p.ava.textContent = fmtClock(message.createdUtc);
  }

  clear(p.main);
  if (message.replyTo) {
    p.main.appendChild(h('button', {
      class: 'quote', type: 'button', tip: '답장한 원문으로 이동해요',
      onClick: () => jumpToMessage(message.replyTo.id),
    }, h('b', null, message.replyTo.displayName || '알 수 없음'), h('span', null, message.replyTo.preview || '삭제된 메시지')));
  }
  if (!grouped) {
    p.main.appendChild(h('div', { class: 'msg__head' },
      personButton(message.userId, message.displayName || '알 수 없음'),
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
  // 알림을 껐으면 탭 제목 숫자도 안 띄운다 (§16 B3)
  notify.badge(titleBadgeOn() ? unread : 0);
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
    pushNotify('mention', { title: `${message.displayName}님이 불렀어요`, body: message.content, icon: message.avatarUrl });
    if (!active) toast(`${message.displayName}님이 나를 불렀어요`, 'info');
  }
  // 내 메시지에 답장이 달렸을 때도 알려 준다 (§16 B3의 종류 3번)
  const repliedToMe = message.replyTo && state.chat.some((row) => row.id === message.replyTo.id
    && String(row.userId) === String(state.user?.id));
  if (repliedToMe && !mine && !mentioned) {
    pushNotify('reply', { title: `${message.displayName}님이 답장했어요`, body: message.content, icon: message.avatarUrl });
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

const STATUS_TIP = {
  listening: '봇과 같은 음성 채널에 있어요',
  othervoice: '다른 음성 채널에 있어요',
  viewing: '리모컨을 보고 있어요',
  online: '디스코드에 접속해 있어요',
  idle: '자리를 비웠어요',
  dnd: '다른 용무 중이에요',
  offline: '지금은 접속해 있지 않아요',
};

function memberRow(member, status) {
  const id = String(member.userId ?? member.id);
  const name = member.displayName || '알 수 없음';
  const row = h('div', { class: `member member--${status}` },
    personAvatar(id, name, member.avatarUrl, 'sm'),
    h('span', { class: `dot dot--${status}`, tip: STATUS_TIP[status] || '접속 상태예요' }),
    h('span', { class: 'member__name' }, personButton(id, name)),
    member.tier && member.tier !== 'member'
      ? h('span', { class: `tier tier--${member.tier}`, tip: TIERS[member.tier]?.desc || '' }, TIERS[member.tier]?.icon || '') : null);

  if (can('suspend') && id !== String(store.get().user?.id)) {
    row.appendChild(h('div', { class: 'member__acts' },
      bindAct(h('button', { class: 'iconbtn iconbtn--danger', type: 'button', tip: '이 사람의 조작을 잠시 막아요', 'aria-label': '정지' }, '⛔'),
        () => openSuspendSheet(member))));
  }
  bindContextTarget(row, () => personMenu(id, name));
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

  return h('div', { class: 'tabpane sugpane' }, el.suggestForm, el.suggestBody);
}

/** 자주 쓰는 기능이 아니라 탭 자리가 아깝다. 헤더 버튼 → 모달로 연다 (§11). */
let suggestModalOpen = false;

function openSuggestModal() {
  suggestUnread = false;
  suggestModalOpen = true;
  el.suggestDot.hidden = true;
  loadSuggestions();
  const handle = sheet({
    title: '💡 제안',
    desc: '불편한 걸 적어 두면 반영될 수도 있어요. 다른 사람 제안에 공감할 수도 있고요.',
    wide: true,
    body: el.suggestPane,
    dismissValue: false,
    actions: [{ label: '닫기', kind: 'primary', value: false }],
  });
  handle.result.then(() => { suggestModalOpen = false; });
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
        personAvatar(item.userId, item.displayName, item.avatarUrl, 'sm'),
        personButton(item.userId, item.displayName || '알 수 없음'),
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
  setLock(el.recentAll, !canBulk(), lockReason('bulkEnqueue'));
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

/* ── 활동 로그 (§13) ──
 * 사람이 읽는 피드다. `settings.update` 같은 기계용 액션명과 전후값 JSON은 관리 콘솔 몫이다.
 * 문장(text)은 서버가 완성해서 내려준다 — 클라이언트가 액션명을 문장으로 바꾸는 로직을 갖지 않는다.
 */

function auditKinds() {
  const raw = prefGet('auditFilter');
  let parsed = null;
  try { parsed = raw ? JSON.parse(raw) : null; } catch { parsed = null; }
  const list_ = Array.isArray(parsed) ? parsed.filter((key) => AUDIT_KINDS[key]) : null;
  return list_ && list_.length ? list_ : AUDIT_DEFAULT.slice();
}

function toggleAuditKind(key) {
  const current = new Set(auditKinds());
  if (current.has(key)) current.delete(key); else current.add(key);
  const next = [...current];
  prefSet('auditFilter', JSON.stringify(next.length ? next : AUDIT_DEFAULT));
  store.patch({ audit: [] });
  syncAuditChips();
  loadAudit(true);
}

function buildAuditPane() {
  el.auditChips = h('div', { class: 'chiprow', role: 'group', 'aria-label': '로그 분류' },
    ...Object.entries(AUDIT_KINDS).map(([key, meta]) => h('button', {
      class: 'chipbtn', type: 'button', dataset: { kind: key },
      'aria-pressed': 'false',
      tip: `${meta.label} 기록을 보여줄지 정해요`,
      onClick: () => toggleAuditKind(key),
    }, `${meta.icon} ${meta.label}`)));

  el.auditFilter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'audit-filter', placeholder: '사람 · 곡 제목으로 거르기',
    onInput: debounce(() => { auditQuery = el.auditFilter.value.trim().toLowerCase(); renderAudit(store.get()); }, 140),
  });
  el.auditHidden = h('button', {
    class: 'audit__more', type: 'button', hidden: true,
    tip: '켜지 않은 분류에 몇 줄이 숨어 있는지 알려드려요',
    onClick: () => {
      prefSet('auditFilter', JSON.stringify(Object.keys(AUDIT_KINDS)));
      store.patch({ audit: [] });
      syncAuditChips();
      loadAudit(true);
    },
  });
  el.auditBody = h('div', { class: 'scroll', style: { flex: '1', minHeight: '0' } });
  syncAuditChips();

  return h('div', { class: 'tabpane', role: 'tabpanel', 'aria-labelledby': 'sidetab-audit' },
    h('div', { class: 'filterbar' }, el.auditFilter, el.auditChips, el.auditHidden),
    el.auditBody);
}

function syncAuditChips() {
  if (!el.auditChips) return;
  const on = new Set(auditKinds());
  for (const chip of el.auditChips.children) {
    chip.setAttribute('aria-pressed', String(on.has(chip.dataset.kind)));
  }
}

async function loadAudit(force) {
  if (!force && store.get().audit.length) return;
  clear(el.auditBody).appendChild(skeletonRows(4));
  try {
    const data = await api(`/audit?kinds=${encodeURIComponent(auditKinds().join(','))}`);
    store.patch({ audit: data?.entries || data || [] });
    if (Number.isFinite(data?.hiddenCount) && data.hiddenCount > 0) {
      el.auditHidden.textContent = `+ ${data.hiddenCount}개 더 (안 켠 분류에 있어요)`;
      el.auditHidden.hidden = false;
    } else {
      el.auditHidden.hidden = true;
    }
  } catch (error) {
    clear(el.auditBody).appendChild(emptyState('📜', '활동 로그를 못 불러왔어요', error.message));
  }
}

/** 서버가 아직 text 를 안 주면 예전 모양이라도 읽히게 최소한만 만든다. */
function auditText(entry) {
  if (entry.text) return entry.text;
  const who = entry.actorName || entry.displayName || '시스템';
  return `${who}님 · ${entry.action || '알 수 없는 동작'}`;
}

function renderAudit(state) {
  clear(el.auditBody);
  const on = new Set(auditKinds());
  const rows = state.audit.filter((entry) => {
    if (entry.kind && !on.has(entry.kind)) return false;
    if (!auditQuery) return true;
    return [auditText(entry), entry.actorName, entry.displayName, entry.trackTitle]
      .join(' ').toLowerCase().includes(auditQuery);
  });

  if (!rows.length) {
    el.auditBody.appendChild(emptyState('📜',
      auditQuery ? '조건에 맞는 기록이 없어요' : '아직 기록이 없어요',
      auditQuery ? '다른 단어로 찾아 보세요.' : '위 칩을 눌러 보고 싶은 분류를 골라 보세요.'));
    return;
  }

  for (const entry of rows) {
    const kind = AUDIT_KINDS[entry.kind] || { icon: '·', label: '기타' };
    const merged = Number(entry.mergedCount) || 0;
    const row = h('div', { class: `logrow${entry.success === false ? ' logrow--fail' : ''}` },
      h('time', { datetime: entry.createdUtc || '', tip: fmtAgo(entry.createdUtc) }, fmtClock(entry.createdUtc)),
      h('div', { class: 'logrow__main' },
        h('span', { class: 'logrow__kind', 'aria-hidden': 'true', tip: kind.label }, kind.icon),
        auditLine(entry, merged)));
    el.auditBody.appendChild(row);
  }
}

/** 서버 문장은 곡 제목을 `**아이브 - I AM**` 처럼 굵게 표시하라고 표시해 온다(models.rs `audit_text`).
 *  그걸 텍스트 노드로 그대로 붙이면 화면에 별표가 나온다. innerHTML은 안 쓰므로 여기서 직접 자른다.
 *  짝이 안 맞는 `**` 는 원문 그대로 둔다 — 제목에 별표가 들어 있을 수도 있다. */
function markdownBold(text) {
  const source = String(text || '');
  const nodes = [];
  let index = 0;
  for (;;) {
    const open = source.indexOf('**', index);
    if (open < 0) break;
    const close = source.indexOf('**', open + 2);
    if (close < 0) break;
    if (open > index) nodes.push(document.createTextNode(source.slice(index, open)));
    nodes.push(h('b', null, source.slice(open + 2, close)));
    index = close + 2;
  }
  if (index < source.length) nodes.push(document.createTextNode(source.slice(index)));
  return nodes.length ? nodes : [document.createTextNode(source)];
}

/** 합쳐진 줄은 펼칠 수 있어야 한다. 숫자만 보여주면 "뭘 넣은 거지?"가 남는다 (§13.3). */
function auditLine(entry, merged) {
  const text = auditText(entry);
  const actor = entry.actorId
    ? frag(personButton(entry.actorId, entry.actorName || '알 수 없음'), document.createTextNode(' '))
    : null;
  const bodyText = entry.actorId && entry.actorName && text.startsWith(entry.actorName)
    ? text.slice(entry.actorName.length)
    : text;

  if (merged <= 1 || !(entry.items || []).length) {
    return h('div', { class: 'logrow__text' }, actor, h('span', null, ...markdownBold(bodyText)));
  }

  const items = h('div', { class: 'logrow__items', hidden: true },
    ...entry.items.map((row) => h('div', { class: 'row__sub' }, `· ${row.title || trackTitle(row.track)}`)));
  const toggle = h('button', {
    class: 'logrow__toggle', type: 'button', 'aria-expanded': 'false',
    tip: '무엇이 담겼는지 펼쳐 봐요',
    onClick: () => {
      items.hidden = !items.hidden;
      toggle.setAttribute('aria-expanded', String(!items.hidden));
      toggle.textContent = items.hidden ? `▸ ${merged}곡 보기` : '▾ 접기';
    },
  }, `▸ ${merged}곡 보기`);

  return h('div', { class: 'logrow__text' }, actor, h('span', null, ...markdownBold(bodyText)), toggle, items);
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
  const railExtra = [
    { id: 'rail:charts', icon: '📈', label: '차트' },
    { id: 'rail:library', icon: '📚', label: '보관함' },
  ];
  const body = h('div', { style: { display: 'grid', gap: 'var(--sp-1)' } },
    ...railExtra.map((tab) => h('button', {
      class: 'dd__item', type: 'button', tip: `${tab.label} 화면으로 가요`,
      onClick: () => handle?.close(tab.id),
    }, h('span', null, tab.icon), h('span', null, tab.label))),
    ...SIDE_TABS.filter((tab) => tab.id !== 'chat').map((tab) => h('button', {
      class: 'dd__item', type: 'button', tip: `${tab.label} 화면으로 가요`,
      onClick: () => handle?.close(tab.id),
    }, h('span', null, tab.icon), h('span', null, tab.label))),
    h('button', {
      class: 'dd__item', type: 'button', tip: '제안 게시판을 열어요',
      onClick: () => handle?.close('modal:suggest'),
    }, h('span', null, '💡'), h('span', null, '제안')),
    h('button', {
      class: 'dd__item', type: 'button', tip: '내 기록을 봐요',
      onClick: () => handle?.close('modal:stats'),
    }, h('span', null, '📊'), h('span', null, '내 기록')),
    // 자동 재생 설정은 대기열 위 막대에도 있지만, 좁은 화면에서는 대기열 탭까지 가야 보인다.
    // 여기 한 줄을 더 두는 값이 그 왕복보다 싸다.
    h('button', {
      class: 'dd__item', type: 'button', tip: '자동 재생이 무엇을 근거로 고르는지 보고 바꿔요',
      onClick: () => handle?.close('modal:autoplay'),
    }, h('span', null, '📻'), h('span', null, '자동 재생')));

  handle = sheet({ title: '더 보기', body, dismissValue: null, actions: [] });
  const id = await handle.result;
  if (!id) return;
  if (id === 'modal:suggest') { openSuggestModal(); return; }
  if (id === 'modal:stats') { openStatsModal(null); return; }
  if (id === 'modal:autoplay') { openAutoplaySheet(); return; }
  if (id.startsWith('rail:')) {
    document.body.dataset.pane = 'rail';
    setRailTab(id.slice(5));
    syncMobileTabs();
    return;
  }
  document.body.dataset.pane = 'side';
  openSide(id);
  syncMobileTabs();
}

/* ═══════════════════════ 시트들 ═══════════════════════ */

function openModeSheet() {
  const current = store.get().queueMode;
  const points = votePoints();
  // 배점이 설정으로 바뀌었으면 설명도 같이 바뀌어야 한다 (§10.1)
  const formulaOf = (id) => (id === 'score'
    ? `관리자 우선 → (대기×${points.wait} + 👍×${points.like} + ⭐×${points.superLike} + 👎×${points.dislike}) 높은 순 → 신청 순`
    : MODES[id].formula);

  sheet({
    title: '왜 이 순서인가요',
    desc: '지금 이 서버는 아래 방식으로 순서를 정해요. 바꾸는 건 서버 관리자만 할 수 있어요.',
    wide: true,
    body: h('div', null,
      h('div', { class: 'modecmp' }, ...Object.entries(MODES).map(([id, mode]) => h('div', {
        class: 'modecmp__card', dataset: { active: id === current ? '1' : '0' },
      },
        h('h3', null, h('span', { 'aria-hidden': 'true' }, mode.icon), mode.label,
          id === current ? h('span', { class: 'chip chip--accent' }, '지금 이 방식') : null),
        h('p', null, mode.desc),
        h('code', null, formulaOf(id))))),
      h('div', { class: 'modecmp__note' },
        h('h3', null, '동점이면 어떻게 되나요'),
        h('p', null, '관리자가 맨 앞으로 올린 곡이 언제나 먼저 나가요. 그다음은 점수가 높은 순, 점수까지 같으면 먼저 신청한 곡이 앞이에요.'),
        h('h3', null, '순서가 저절로 움직여요'),
        h('p', null, `지금 이 서버는 ${sortPeriodSeconds()}초마다 순서를 다시 정해요. 대기열이 길어지면 주기가 더 늘어나요. 대기열 헤더의 숫자가 다음 재정렬까지 남은 초예요.`))),
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

  // §23.3 마지막 문단: "'내 권한' 화면의 '할 수 없는 것' 목록에도 같은 이유와 대상을 붙여요.
  // 거기가 이 정보를 가장 차분하게 읽을 수 있는 자리예요." 서버는 이미 allowedCount /
  // allowedRoleNames 를 주고 있으니 클라가 붙이기만 하면 된다.
  const rowOf = (entry) => h('div', { class: `perm__row${entry.allowed ? '' : ' perm__row--no'}` },
    h('span', { 'aria-hidden': 'true' }, entry.allowed ? '✅' : '❌'),
    h('span', { class: 'label' }, entry.label || PERM_LABELS[entry.key] || entry.key),
    h('span', { class: 'why' },
      entry.ruleLabel || RULE_LABELS[entry.rule] || '',
      Array.isArray(entry.roleNames) && entry.roleNames.length ? ` (${entry.roleNames.join(', ')})` : '',
      entry.viaAdmin ? h('em', null, ' ← 관리자라 통과') : null,
      entry.reason && !entry.allowed ? ` · ${entry.reason}` : '',
      !entry.allowed && whoCan(entry) ? h('span', { class: 'perm__who' }, ` · ${whoCan(entry)}`) : null));

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

  // 업데이트로 끊기는 건 사고가 아니다. 빨간 배너 대신 "곧 돌아와요" 를 띄운다.
  if (state.conn === 'restarting') {
    el.banners.appendChild(h('div', { class: 'banner banner--info', role: 'status' },
      h('span', { class: 'banner__icon' }, '🛠'),
      h('div', { class: 'banner__text' },
        state.restartNote || '업데이트 중이에요. 몇 초 뒤에 자동으로 다시 연결돼요.'),
      h('span', { class: 'spinner', 'aria-hidden': 'true' })));
  } else if (state.conn === 'reconnecting' || state.conn === 'down') {
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

/* 패치노트 (§30).
 *
 * 원본은 `docs/CHANGELOG.md` 하나고 exe 에 같이 들어 있다. 화면용으로 옮겨 적지 않는다 —
 * 두 벌이 되면 결국 화면 쪽이 낡는다.
 *
 * 마크다운은 여기서 아주 좁게만 해석한다. 범용 파서를 넣으면 그만큼 XSS 면이 넓어지는데,
 * 우리가 쓰는 문법은 제목·목록·표·코드·굵게가 전부다. **HTML 은 통째로 이스케이프**하고
 * 그 위에 우리가 아는 문법만 되살린다.
 */
function renderMarkdown(text) {
  const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const inline = (s) => esc(s)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');

  const out = [];
  let inCode = false;
  let listOpen = false;
  const closeList = () => { if (listOpen) { out.push('</ul>'); listOpen = false; } };

  for (const raw of String(text).split('\n')) {
    const line = raw.replace(/\r$/, '');
    if (line.startsWith('```')) {
      closeList();
      out.push(inCode ? '</code></pre>' : '<pre><code>');
      inCode = !inCode;
      continue;
    }
    if (inCode) { out.push(esc(line)); continue; }

    const heading = line.match(/^(#{1,4})\s+(.*)$/);
    if (heading) { closeList(); out.push(`<h${heading[1].length}>${inline(heading[2])}</h${heading[1].length}>`); continue; }
    // 수평선. **목록보다 먼저 본다** — `* * *` 는 목록 규칙에도 걸리기 때문이다.
    // 이게 없으면 `---` 가 어느 분기에도 안 걸려 맨 아래 `<p>` 로 떨어지고, 화면에 글자로 나온다.
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { closeList(); out.push('<hr>'); continue; }
    if (/^\s*[-*]\s+/.test(line)) {
      if (!listOpen) { out.push('<ul>'); listOpen = true; }
      out.push(`<li>${inline(line.replace(/^\s*[-*]\s+/, ''))}</li>`);
      continue;
    }
    // 표는 셀만 살린다. 정렬 줄(---)은 버린다.
    if (line.includes('|') && line.trim().startsWith('|')) {
      if (/^\s*\|[\s:|-]+\|\s*$/.test(line)) continue;
      closeList();
      const cells = line.split('|').slice(1, -1).map((cell) => `<td>${inline(cell.trim())}</td>`);
      out.push(`<table class="md__t"><tr>${cells.join('')}</tr></table>`);
      continue;
    }
    if (!line.trim()) { closeList(); continue; }
    closeList();
    out.push(`<p>${inline(line)}</p>`);
  }
  closeList();
  if (inCode) out.push('</code></pre>');
  return out.join('\n');
}

let changelogCache = null;

async function openChangelog() {
  if (!changelogCache) {
    try {
      changelogCache = await api('/music/api/changelog');
    } catch {
      toast('패치노트를 불러오지 못했어요.', 'warn');
      return;
    }
  }
  const body = h('div', { class: 'md' });
  body.innerHTML = renderMarkdown(changelogCache.markdown || '');
  // 읽었다고 표시한다. 다음 접속 때 같은 버전이면 안 띄운다.
  try { localStorage.setItem(LS.seenChangelog, ctx.buildId || ''); } catch { /* 시크릿 모드 */ }
  sheet({
    title: '패치노트',
    desc: changelogCache.latest ? `최신: ${changelogCache.latest}` : null,
    wide: true,
    body,
    actions: [{ label: '닫기', kind: 'ghost' }],
  });
}

/** 새 버전으로 처음 들어왔으면 무엇이 바뀌었는지 한 번 보여준다 (§30). */
function maybeShowChangelog() {
  if (!ctx.buildId) return;
  let seen = null;
  try { seen = localStorage.getItem(LS.seenChangelog); } catch { /* 시크릿 모드 */ }
  // 처음 쓰는 사람에게는 안 띄운다 — 첫인상이 변경 목록이면 곤란하다.
  if (!seen) {
    try { localStorage.setItem(LS.seenChangelog, ctx.buildId); } catch { /* 무시 */ }
    return;
  }
  if (seen === ctx.buildId) return;
  openChangelog();
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

  setLock(el.consoleBtn, !can('console'), lockReason('console'));
  el.opsLink.hidden = !can('ops');
  renderNotifyBox();
  syncLayoutOptions();

  // 마참 점수는 프로필에서도 조용히 보여준다 (§22.4). 등수는 매기지 않는다.
  if (Number.isFinite(myScore)) {
    el.meHead.appendChild(h('span', {
      class: 'chip chip--accent machamscore',
      tip: '받은 좋아요·슈퍼 좋아요와 재생 기록이 쌓인 점수예요. 대기열 순서에는 영향이 없어요',
    }, `마참 점수 ${myScore}`));
  }
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

/* 서버 승인 (§26.2) — 봇 주인 전용.
 *
 * 봇 초대는 Discord 쪽이라 막을 수 없다. 그래서 **쓰는 것**을 승인제로 돌린다.
 * 이 화면이 그 결정을 내리는 유일한 자리다.
 */
async function loadApprovals() {
  // 봇 주인이 아니면 API 가 403 을 준다. 조용히 접는다 — 있지도 않은 메뉴를 깜빡이면 안 된다.
  if (ctx.tier !== 'owner') return null;
  try {
    const data = await api('/music/api/owner/guilds');
    const guilds = Array.isArray(data?.guilds) ? data.guilds : [];
    const pending = guilds.filter((row) => row.status === 'pending').length;
    if (el.approvalBtn) {
      el.approvalBtn.hidden = false;
      el.approvalCount.hidden = pending === 0;
      el.approvalCount.textContent = String(pending);
    }
    return guilds;
  } catch {
    return null;
  }
}

async function openApprovalSheet() {
  const guilds = await loadApprovals();
  if (!guilds) { toast('서버 목록을 불러오지 못했어요.', 'warn'); return; }

  const decide = async (guildId, status, label) => {
    const result = await call(() => api('/music/api/owner/guilds/decide', {
      body: { guildId, status },
    }));
    if (!result) return;
    toast(`${label} 처리했어요.`, 'ok');
    document.querySelectorAll('.sheet-back').forEach((back) => back.remove());
    openApprovalSheet();
  };

  const row = (guild) => {
    const pending = guild.status === 'pending';
    const blocked = guild.status === 'blocked';
    // 이미 나간 서버는 승인해도 소용없다. 그 사실을 먼저 말한다.
    const gone = guild.botInGuild === false;
    return h('div', { class: `apr apr--${guild.status}` },
      h('div', { class: 'apr__main' },
        h('div', { class: 'apr__name' }, guild.name || `서버 ${guild.guildId}`),
        h('div', { class: 'apr__sub' },
          guild.statusLabel,
          ' · ',
          fmtAgo(guild.requestedUtc),
          gone ? ' · 봇이 이미 나간 서버예요' : '')),
      h('div', { class: 'apr__acts' },
        pending || blocked
          ? bindAct(h('button', { class: 'btn btn--sm btn--primary', type: 'button', tip: '이 서버가 봇을 쓸 수 있게 해요' }, '승인'),
            () => decide(guild.guildId, 'approved', '승인'))
          : null,
        !blocked
          ? bindAct(h('button', { class: 'btn btn--sm btn--danger', type: 'button', tip: '이 서버에서 봇을 못 쓰게 막아요' }, '차단'),
            () => decide(guild.guildId, 'blocked', '차단'))
          : null));
  };

  const pending = guilds.filter((g) => g.status === 'pending');
  const rest = guilds.filter((g) => g.status !== 'pending');

  sheet({
    title: '서버 승인',
    desc: '봇을 초대한 서버예요. 승인해야 명령어와 리모컨이 열려요.',
    wide: true,
    body: h('div', { class: 'aprlist' },
      pending.length
        ? frag(h('div', { class: 'who__title' }, '⏳ 승인 대기', h('span', { class: 'count' }, String(pending.length))),
          ...pending.map(row))
        : h('p', { class: 'hint' }, '기다리는 서버가 없어요.'),
      rest.length
        ? frag(h('div', { class: 'who__title' }, '📋 나머지', h('span', { class: 'count' }, String(rest.length))),
          ...rest.map(row))
        : null),
    actions: [{ label: '닫기', kind: 'ghost' }],
  });
}

/* 듣는중·보는중을 누르면 뜨는 창 (§28).
 *
 * 전에는 옆 패널을 여는 게 전부였다. **집중·패널 배치에서는 그 패널이 아예 없어서**
 * 눌러도 아무 일이 안 일어났다. 어느 배치에서든 뜨도록 창으로 바꾼다.
 *
 * 옆 패널의 멤버 목록과 같은 데이터를 쓴다 — 두 벌로 만들면 한쪽만 고치게 된다.
 */
function whoBuckets(state) {
  const presence = state.presence || {};
  const bot = presence.bot || null;
  const listening = new Set((presence.listening || []).map(String));
  const otherVoice = new Set((presence.inOtherVoice || []).map(String));
  const viewing = new Set((presence.viewing || []).map(String));
  // 봇이 음성에 없으면 '이 채널에서 듣는 중'은 있을 수 없다.
  if (bot && bot.inVoice === false) listening.clear();

  const members = state.members.length
    ? state.members
    : synthesizeMembers(state, listening, otherVoice, viewing);
  const pick = (ids) => members.filter((member) => ids.has(String(member.userId ?? member.id)));
  return { bot, listening: pick(listening), otherVoice: pick(otherVoice), viewing: pick(viewing) };
}

function openWhoSheet() {
  const state = store.get();
  const { bot, listening, otherVoice, viewing } = whoBuckets(state);

  const group = (icon, title, rows, empty) => h('div', { class: 'who__group' },
    h('div', { class: 'who__title' },
      h('span', { 'aria-hidden': 'true' }, icon), title,
      h('span', { class: 'count' }, String(rows.length))),
    rows.length
      ? h('div', { class: 'who__rows' }, ...rows.map((member) => memberRow(member, 'listening')))
      : h('p', { class: 'hint' }, empty));

  const note = !bot || bot.inGuild === false
    ? '봇이 이 서버에 없어요. 다시 초대해야 재생할 수 있어요.'
    : bot.inVoice === false
      ? '봇이 음성 채널에 없어서 같이 듣는 사람도 없어요. 봇을 부르면 바로 채워져요.'
      : bot.voiceChannelName
        ? `봇은 지금 '${bot.voiceChannelName}' 채널에 있어요.`
        : '봇은 지금 음성 채널에 있어요.';

  sheet({
    title: '지금 누가 있나요',
    desc: note,
    wide: true,
    body: h('div', { class: 'who' },
      group('🎧', '이 채널에서 듣는 중', listening, '아직 아무도 없어요.'),
      // 다른 채널에 있는 사람은 "부르면 올 수 있는 사람"이라 같이 보여준다.
      otherVoice.length ? group('🔈', '다른 음성 채널에 있어요', otherVoice, '') : null,
      group('🖥', '리모컨 보는 중', viewing, '지금은 아무도 안 보고 있어요.')),
    actions: [{ label: '닫기', kind: 'ghost', autofocus: true }],
  });
}

/* ═══════════════════════ 우클릭 메뉴 (§24.1) ═══════════════════════
 * 대상마다 따로 구현하면 동작이 미묘하게 달라지고 툴팁·키보드 처리가 빠진다.
 * 그래서 **컴포넌트 하나**로 만들고 대상마다 항목 배열만 넘긴다.
 *
 * 규칙
 *  - Ctrl / Alt / Shift + 우클릭이면 브라우저 기본 메뉴를 그대로 띄운다(preventDefault 안 한다).
 *  - 모바일은 롱프레스 500ms.
 *  - 항목은 6개 이하. 넘으면 하위 메뉴(▸)로 접는다.
 *  - 권한 없는 항목은 숨기지 않고 비활성 + 이유. 뭐가 있는지는 알아야 한다.
 */

const MENU_MAX = 6;
let openMenu = null;

function closeContextMenu() {
  if (!openMenu) return;
  openMenu.node.remove();
  document.removeEventListener('pointerdown', openMenu.onOutside, true);
  document.removeEventListener('keydown', openMenu.onKey, true);
  window.removeEventListener('scroll', closeContextMenu, true);
  openMenu.restore?.();
  openMenu = null;
}

/**
 * items: [{ icon, label, hint, danger, disabled, reason, onPick, children }]
 * where: { x, y } 또는 { anchor }
 */
function openContextMenu(items, where) {
  closeContextMenu();
  if (!items || !items.length) return;

  const rows = items.length > MENU_MAX
    ? items.slice(0, MENU_MAX - 1).concat([{ icon: '⋯', label: '더 보기', children: items.slice(MENU_MAX - 1) }])
    : items;

  const node = h('div', { class: 'pop pop--menu ctxmenu', role: 'menu' });
  for (const item of rows) node.appendChild(menuRow(item, node));
  document.body.appendChild(node);
  placePopover(node, where);

  const previous = document.activeElement;
  const onOutside = (event) => { if (!node.contains(event.target)) closeContextMenu(); };
  const onKey = (event) => {
    if (event.key === 'Escape') { event.stopPropagation(); closeContextMenu(); return; }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const buttons = [...node.querySelectorAll('button:not([aria-disabled="true"])')];
    if (!buttons.length) return;
    const index = buttons.indexOf(document.activeElement);
    const next = event.key === 'ArrowDown' ? index + 1 : index - 1;
    buttons[(next + buttons.length) % buttons.length].focus();
  };

  openMenu = {
    node, onOutside, onKey,
    restore: () => { if (previous && previous.isConnected) previous.focus?.(); },
  };
  setTimeout(() => {
    document.addEventListener('pointerdown', onOutside, true);
    document.addEventListener('keydown', onKey, true);
    window.addEventListener('scroll', closeContextMenu, true);
  }, 0);
  node.querySelector('button:not([aria-disabled="true"])')?.focus();
}

function menuRow(item, menu) {
  if (item.children) {
    const sub = h('div', { class: 'ctxmenu__sub', hidden: true },
      ...item.children.map((child) => menuRow(child, menu)));
    const button = h('button', {
      class: 'dd__item', type: 'button', role: 'menuitem', 'aria-expanded': 'false',
      tip: '하위 메뉴를 펼쳐요',
      onClick: () => {
        sub.hidden = !sub.hidden;
        button.setAttribute('aria-expanded', String(!sub.hidden));
      },
    }, h('span', null, item.icon || '▸'), h('span', { class: 'dd__grow' }, item.label), h('span', null, '▸'));
    return h('div', null, button, sub);
  }

  const button = h('button', {
    class: ['dd__item', item.danger && 'dd__item--danger'],
    type: 'button', role: 'menuitem',
    tip: item.tip || (item.disabled ? item.reason : ''),
    onClick: () => {
      if (button.getAttribute('aria-disabled') === 'true') {
        toast(item.reason || '지금은 할 수 없어요.', 'warn');
        return;
      }
      closeContextMenu();
      item.onPick?.();
    },
  },
    h('span', null, item.icon || '·'),
    h('span', { class: 'dd__grow' }, item.label),
    item.hint ? h('span', { class: 'dd__hint' }, item.hint) : null);
  if (item.disabled) setLock(button, true, item.reason || '지금은 할 수 없어요');
  return button;
}

function placePopover(node, where) {
  const box = node.getBoundingClientRect();
  let left;
  let top;
  if (where && where.anchor) {
    const rect = where.anchor.getBoundingClientRect();
    left = rect.left;
    top = rect.bottom + 4;
  } else {
    left = (where && where.x) || 0;
    top = (where && where.y) || 0;
  }
  node.style.left = `${Math.max(8, Math.min(left, window.innerWidth - box.width - 8))}px`;
  node.style.top = `${Math.max(8, Math.min(top, window.innerHeight - box.height - 8))}px`;
}

/** 우클릭 · 롱프레스 · Shift+F10 을 한 번에 붙인다.
 *
 *  **메뉴를 열기로 했으면 전파를 반드시 끊는다.** `.person` 버튼은 `.qitem`·`.msg` 안에 들어 있어서,
 *  안 끊으면 이벤트가 조상까지 올라가 조상 핸들러의 `openContextMenu` 가 맨 앞에서
 *  `closeContextMenu()` 를 부르고 방금 뜬 사람 메뉴를 곡/메시지 메뉴로 갈아치운다.
 *  그 결과 §24.1의 "사람" 메뉴가 멤버 탭 말고는 어디서도 안 열렸다. */
function bindContextTarget(node, build) {
  node.addEventListener('contextmenu', (event) => {
    // 브라우저 기본 메뉴(링크 복사·이미지 저장)를 뺏으면 안 된다
    if (event.ctrlKey || event.altKey || event.shiftKey || event.metaKey) return;
    const items = build(event);
    if (!items || !items.length) return;
    event.preventDefault();
    event.stopPropagation();
    openContextMenu(items, { x: event.clientX, y: event.clientY });
  });

  let timer = 0;
  let startX = 0;
  let startY = 0;
  // iOS 사파리는 길게 누르면 **글자 선택 말풍선**을 먼저 띄워서 메뉴를 덮는다.
  // 여기 붙는 요소는 어차피 조작 대상이라 글자를 끌어 선택할 일이 없다.
  node.style.webkitUserSelect = 'none';
  node.style.userSelect = 'none';
  node.style.webkitTouchCallout = 'none';

  node.addEventListener('pointerdown', (event) => {
    if (event.pointerType !== 'touch') return;
    startX = event.clientX;
    startY = event.clientY;
    clearTimeout(timer);
    timer = setTimeout(() => {
      const items = build(event);
      if (!items || !items.length) return;
      // 롱프레스는 조상 타이머도 같이 돌고 있다. 안쪽이 이겼다는 표시를 남겨 조상이 덮어쓰지 않게 한다.
      if (event.__mmLongPressTaken) return;
      event.__mmLongPressTaken = true;
      // 눌린 게 먹혔다는 신호. 손가락은 커서가 없어서 이게 없으면 메뉴가 갑자기 튀어나온 것처럼 느껴진다.
      try { navigator.vibrate?.(12); } catch { /* 지원 안 하면 그만 */ }
      openContextMenu(items, { x: startX, y: startY });
    }, 500);
  }, { passive: true });
  const cancel = (event) => {
    if (event && event.clientX !== undefined
      && Math.abs(event.clientX - startX) + Math.abs(event.clientY - startY) < 8
      && event.type === 'pointermove') return;
    clearTimeout(timer);
  };
  node.addEventListener('pointerup', cancel, { passive: true });
  node.addEventListener('pointermove', cancel, { passive: true });
  node.addEventListener('pointercancel', cancel, { passive: true });

  node.addEventListener('keydown', (event) => {
    if (event.key !== 'ContextMenu' && !(event.key === 'F10' && event.shiftKey)) return;
    const items = build(event);
    if (!items || !items.length) return;
    event.preventDefault();
    event.stopPropagation();
    openContextMenu(items, { anchor: event.currentTarget });
  });
}

/* ── 대상별 메뉴 ── */

function trackMenu(track, opts = {}) {
  if (!track) return null;
  const state = store.get();
  const itemId = opts.itemId;
  const item = opts.item;
  const mine = !!item?.isMine;
  const voteReason = mine ? '내가 신청한 곡에는 투표할 수 없어요' : lockReason('vote');
  const url = trackUrl(track);

  const items = [
    { icon: '＋', label: '대기열에 담기', disabled: !can('search'), reason: lockReason('search'), onPick: () => enqueue(track) },
    { icon: '🔖', label: '보관함에 담기', disabled: !can('library'), reason: lockReason('library'), onPick: () => toggleSaved(track, true) },
    { icon: '📃', label: '재생목록에 추가', onPick: () => openPlaylistPicker(track, null) },
    {
      icon: '👍',
      label: '투표',
      children: [
        { icon: '👍', label: '좋아요', disabled: !itemId || !can('vote') || mine, reason: voteReason, onPick: () => vote(itemId, 'like') },
        { icon: '⭐', label: '슈퍼 좋아요', disabled: !itemId || !can('vote') || mine, reason: voteReason, onPick: () => vote(itemId, 'superLike') },
        { icon: '👎', label: '싫어요', disabled: !itemId || !can('vote') || mine, reason: voteReason, onPick: () => vote(itemId, 'dislike') },
      ],
    },
  ];

  if (opts.source === 'now') {
    items.push({ icon: '⏭', label: '스킵', disabled: !canSkipNow(), reason: lockReason('skip'), onPick: doSkip });
    items.push({ icon: '🎤', label: '가사 보기', onPick: () => { if (!lyricsOpen) toggleLyrics(); } });
  } else if (itemId) {
    items.push({
      icon: '📌', label: '맨 앞으로 올리기',
      disabled: !can('queueEdit') || tierOf() === 'member', reason: lockReason('queueEdit'),
      onPick: () => call(() => api('/queue/action', { body: { action: 'togglePin', itemId } })),
    });
    items.push({
      icon: '✕', label: '대기열에서 빼기', danger: true,
      disabled: !(mine ? can('queueEdit') || can('search') : can('queueEdit')), reason: lockReason('queueEdit'),
      onPick: () => call(() => api('/queue/action', { body: { action: 'remove', itemId } }), '대기열에서 뺐어요.'),
    });
  }

  items.push({
    icon: '📻', label: '자동 재생 기준으로 삼기',
    disabled: !seedState || !canAutoplay(), reason: lockReason('autoplay'),
    onPick: () => addSeed(track),
  });
  items.push({
    icon: '🚫', label: '차단 목록에 넣기',
    disabled: tierOf() === 'member' || tierOf() === 'viewer', reason: '서버 관리자만 차단할 수 있어요',
    onPick: () => openBlacklistSheet(track),
  });
  if (url) {
    items.push({ icon: '🔗', label: '링크 복사', onPick: () => copyText(url, '링크를 복사했어요.') });
    items.push({ icon: '↗', label: '원본에서 열기', onPick: () => window.open(url, '_blank', 'noreferrer') });
  }
  if (state.tier === 'viewer') return items;
  return items;
}

function trackUrl(track) {
  if (!track) return '';
  if (track.url) return track.url;
  // 서버가 주는 정식 주소. **이걸 안 보면 사운드클라우드는 영영 빈 값**이다 —
  // 그쪽은 ID 로 주소를 만들 수 없어서 sourceUrl 말고는 알 방법이 없다.
  if (track.sourceUrl && /^https?:\/\//.test(track.sourceUrl)) return track.sourceUrl;
  const provider = String(track.provider || '');
  if (provider.startsWith('YouTube') && track.contentId) return `https://www.youtube.com/watch?v=${encodeURIComponent(track.contentId)}`;
  return '';
}

async function copyText(text, okMessage) {
  try {
    await navigator.clipboard.writeText(text);
    toast(okMessage || '복사했어요.', 'ok');
  } catch {
    toast('클립보드에 못 넣었어요. 주소를 직접 복사해 주세요.', 'warn');
  }
}

function personMenu(userId, displayName) {
  const me = String(store.get().user?.id || '');
  const id = String(userId || '');
  return [
    { icon: '📊', label: '기록 보기', onPick: () => openStatsModal(id) },
    {
      icon: '@', label: '멘션하기',
      disabled: !can('chat'), reason: lockReason('chat'),
      onPick: () => {
        openSide('chat');
        el.chatInput.value = `${el.chatInput.value}@${displayName} `.trimStart();
        el.chatInput.focus();
        autoGrow(el.chatInput);
      },
    },
    {
      icon: '🔍', label: '이 사람이 담은 곡만 보기',
      onPick: () => { setRailTab('queue'); highlightRequester(id, displayName); },
    },
    {
      icon: '⛔', label: '정지', danger: true,
      disabled: !can('suspend') || id === me,
      reason: id === me ? '자기를 정지할 수는 없어요' : lockReason('suspend'),
      onPick: () => openSuspendSheet({ userId: id, displayName }),
    },
  ];
}

/** 대기열에서 그 사람 곡만 잠깐 강조한다. 새 화면을 만들 만큼 무거운 기능이 아니다. */
function highlightRequester(userId, displayName) {
  const nodes = [...el.queueList.querySelectorAll('.qitem')]
    .filter((node) => String(node.__item?.requestedByUserId) === String(userId));
  if (!nodes.length) { toast(`${displayName}님이 담은 곡이 지금 대기열에 없어요.`, 'info'); return; }
  for (const node of nodes) { node.classList.remove('flash'); void node.offsetWidth; node.classList.add('flash'); }
  nodes[0].scrollIntoView({ block: 'center', behavior: prefersReduced() ? 'auto' : 'smooth' });
  setTimeout(() => { for (const node of nodes) node.classList.remove('flash'); }, 1600);
  toast(`${displayName}님이 담은 곡 ${nodes.length}개를 표시했어요.`, 'ok');
}

function messageMenu(message) {
  const mine = String(message.userId) === String(store.get().user?.id);
  return [
    { icon: '↩', label: '답장', disabled: !can('chat'), reason: lockReason('chat'), onPick: () => setReply(message) },
    { icon: '🙂', label: '반응 남기기', disabled: !can('chat'), reason: lockReason('chat'), onPick: () => openEmojiPicker(message.id, el.chatLog.querySelector(`[data-id="${CSS.escape(String(message.id))}"]`) || el.chatLog) },
    { icon: '📋', label: '내용 복사', onPick: () => copyText(message.content || '', '메시지를 복사했어요.') },
    {
      icon: '🔗', label: '이 메시지 링크',
      onPick: () => copyText(messageLink(message.id), '메시지 링크를 복사했어요. 채팅을 열어 둔 사람에게 보내면 그 줄로 바로 가요.'),
    },
    {
      icon: '🗑', label: '삭제', danger: true,
      disabled: !(mine || can('chatDelete')), reason: '내 메시지이거나 관리 권한이 있어야 지울 수 있어요',
      onPick: async () => {
        if (await confirmSheet({ title: '메시지를 지울까요', desc: message.content?.slice(0, 80), danger: true, confirmText: '삭제' })) {
          call(() => api('/chat/delete', { body: { messageId: message.id } }));
        }
      },
    },
    {
      icon: '🚩', label: '신고',
      disabled: mine, reason: '내 메시지는 신고할 수 없어요',
      onPick: () => call(() => api('/chat/report', { body: { messageId: message.id } }), '신고했어요. 관리자가 확인할 거예요.'),
    },
  ];
}

/** 메시지 한 줄을 가리키는 링크 (§24.1). 같은 서버 화면을 열면 그 줄로 데려간다. */
function messageLink(messageId) {
  return `${location.origin}${location.pathname}#msg-${encodeURIComponent(String(messageId))}`;
}

/** `#msg-…` 로 들어왔으면 채팅을 열고 그 줄을 잠깐 강조한다. 아직 안 불러온 메시지면 조용히 넘어간다. */
function focusLinkedMessage() {
  const match = /^#msg-(.+)$/.exec(location.hash || '');
  if (!match) return;
  const id = decodeURIComponent(match[1]);
  openSide('chat');
  requestAnimationFrame(() => {
    const node = el.chatLog.querySelector(`[data-id="${CSS.escape(id)}"]`);
    if (node) flashNode(node);
    else toast('그 메시지는 아직 안 불러왔어요. 위로 올려서 더 불러와 주세요.', 'info');
  });
}

function queueHeadMenu() {
  const manager = tierOf() !== 'member' && tierOf() !== 'viewer';
  return [
    { icon: '🔀', label: '정렬 방식 보기', onPick: openModeSheet },
    // 대기열이 비면 여기서 나가는 곡을 자동 재생이 고른다. 그 설정으로 가는 길을 대기열 머리에도 둔다.
    { icon: '📻', label: '자동 재생 설정', disabled: !seedState, reason: '이 서버는 아직 자동 재생 설정을 몰라요', onPick: openAutoplaySheet },
    { icon: '🧹', label: '대기열 비우기', danger: true, disabled: !manager || !can('queueEdit'), reason: lockReason('queueEdit'), onPick: clearQueue },
    { icon: '↻', label: '새로고침', onPick: () => { loadHot().catch(() => {}); toast('대기열을 다시 받아 왔어요.', 'ok'); } },
  ];
}

function backgroundMenu() {
  return [
    { icon: '🎨', label: '테마', children: Object.entries(THEMES).map(([id, meta]) => ({
      icon: '·', label: meta.label, onPick: () => commitTheme(id),
    })) },
    { icon: '▦', label: '화면 배치', children: Object.entries(LAYOUTS).map(([id, def]) => ({
      icon: '·', label: def.label, onPick: () => setLayout(id),
    })) },
    { icon: '📊', label: '내 기록', onPick: () => openStatsModal(null) },
    // 개발자 콘솔 (§33). 기본은 숨김이라 여기서만 켠다.
    {
      icon: '⌨',
      label: consoleOpen ? '개발자 콘솔 닫기' : '개발자 콘솔 열기',
      onPick: () => toggleDevConsole(),
    },
  ];
}

/* ═══════════════════════ 개발자 콘솔 (§33) ═══════════════════════
 *
 * 화면을 다 만들어 두고도 "이 값이 지금 뭐지"를 확인하려면 개발자 도구를 열어야 했다.
 * 여기서 상태를 바로 찍어 보고 명령도 날린다. **기본은 숨김**이고 빈 배경 우클릭으로만 연다.
 *
 * 명령은 한 곳(`DEV_COMMANDS`)에만 적는다. 자동완성·도움말·실행이 전부 이 표를 읽으므로
 * 명령을 추가하면 Tab 완성과 `help` 가 저절로 따라온다 — 세 군데에 나눠 적으면 어긋난다.
 */
let consoleOpen = false;
let devHistory = [];
let devHistoryAt = -1;

/** `get` 이 완성해 주는 경로. 실제로 열려 있는 GET 라우트만 적는다 (remote.rs 기준).
 *  쓰기 경로(`/control`, `/queue/action`, `/autoplay/seeds/remove` 같은 것)는 넣지 않는다 —
 *  Tab 한 번에 서버 상태가 바뀌면 안 된다. 여기 것들은 전부 읽기 전용이다. */
const DEV_GET_PATHS = [
  '/state', '/state/hot', '/state/cold', '/queue', '/settings', '/public',
  '/charts', '/autoplay', '/autoplay/seeds', '/library', '/lyrics', '/search',
  '/audit', '/stats/me', '/chat', '/mention-candidates',
  '/admin/settings', '/admin/roles', '/admin/roleview', '/admin/audit',
  '/admin/diagnostics', '/admin/participants', '/admin/reports',
  '/admin/suggestions', '/admin/suspensions', '/admin/blacklist',
  '/admin/queue-preview', '/admin/preview', '/admin/permission-preview',
];

/** 점으로 이어진 상태 경로를 한 칸씩 완성한다. `player.` 까지 쳤으면 그 아래 키를 준다. */
function completeStatePath(prefix) {
  const state = store.get();
  const cut = String(prefix || '').lastIndexOf('.');
  const base = cut < 0 ? '' : prefix.slice(0, cut);
  const node = base
    ? base.split('.').reduce((acc, key) => (acc == null ? acc : acc[key]), state)
    : state;
  if (node == null || typeof node !== 'object') return [];
  return Object.keys(node).map((key) => (base ? `${base}.${key}` : key));
}

const DEV_COMMANDS = {
  help: { args: '[명령]', desc: '명령 목록을 보여줘요',
    run: (arg) => {
      const spec = arg && DEV_COMMANDS[arg];
      if (spec) return `${arg}${spec.args ? ' ' + spec.args : ''}\n    ${spec.desc}`;
      if (arg) return `모르는 명령이에요. ${Object.keys(DEV_COMMANDS).join(', ')}`;
      return Object.entries(DEV_COMMANDS)
        .map(([name, s]) => `${name}${s.args ? ' ' + s.args : ''}\n    ${s.desc}`).join('\n');
    },
    complete: () => Object.keys(DEV_COMMANDS) },
  state: { args: '[키]', desc: '지금 상태를 찍어요. 키를 주면 그 부분만',
    run: (arg) => {
      const state = store.get();
      const value = arg ? arg.split('.').reduce((acc, k) => (acc == null ? acc : acc[k]), state) : Object.keys(state);
      return JSON.stringify(value, null, 2);
    },
    complete: completeStatePath },
  now: { args: '', desc: '재생 중인 곡과 위치',
    run: () => {
      const s = store.get();
      if (!s.current) return '재생 중인 곡이 없어요.';
      return [
        `곡      ${trackTitle(s.current.track)}`,
        `위치    ${fmtTime(clock.position())} / ${fmtTime(clock.duration)}`,
        `일시정지 ${s.player?.isPaused ? '예' : '아니오'}`,
        `시작시각 ${s.schedule?.startedUtc || '(없음)'}`,
        `보정    개인 ${webOffset.toFixed(1)}초 + 서버 ${(s.schedule?.webSyncOffsetMs || 0) / 1000}초`,
      ].join('\n');
    } },
  sync: { args: '[±초]', desc: '내 싱크 보정을 보거나 바꿔요',
    run: (arg) => {
      if (!arg) return `지금 ${offsetLabel(webOffset)}`;
      const next = Number(arg);
      if (!Number.isFinite(next)) return '숫자를 넣어 주세요. 예: sync -0.3';
      setWebOffset(next);
      return `${offsetLabel(webOffset)} 로 바꿨어요.`;
    } },
  theme: { args: '[이름]', desc: '테마를 바꿔요',
    run: (arg) => {
      if (!arg) return Object.keys(THEMES).join(', ');
      if (!THEMES[arg]) return `모르는 테마예요. ${Object.keys(THEMES).join(', ')}`;
      commitTheme(arg);
      return `${arg} 로 바꿨어요.`;
    },
    complete: () => Object.keys(THEMES) },
  layout: { args: '[이름]', desc: '화면 배치를 바꿔요',
    run: (arg) => {
      if (!arg) return Object.keys(LAYOUTS).join(', ');
      if (!LAYOUTS[arg]) return `모르는 배치예요. ${Object.keys(LAYOUTS).join(', ')}`;
      setLayout(arg);
      return `${arg} 로 바꿨어요.`;
    },
    complete: () => Object.keys(LAYOUTS) },
  perms: { args: '', desc: '내 권한을 전부 찍어요',
    run: () => JSON.stringify(store.get().permissions, null, 2) },
  get: { args: '<경로>', desc: 'API 를 GET 해요. 예: get /state/hot',
    run: async (arg) => {
      if (!arg) return '경로를 주세요. 예: get /state/hot';
      return JSON.stringify(await api(arg), null, 2);
    },
    complete: () => DEV_GET_PATHS },
  clearlog: { args: '', desc: '콘솔 화면을 비워요', run: () => ' clear' },
};

function devComplete(line) {
  const parts = line.split(/\s+/);
  if (parts.length <= 1) {
    return Object.keys(DEV_COMMANDS).filter((name) => name.startsWith(parts[0] || ''));
  }
  const spec = DEV_COMMANDS[parts[0]];
  if (!spec || !spec.complete) return [];
  const prefix = parts[parts.length - 1];
  // **후보를 만들 때 지금 입력값을 넘긴다.** `state player.` 처럼 앞부분이 정해져야
  // 다음 후보를 알 수 있는 명령이 있다. 인자를 안 받는 옛 `complete` 는 그냥 무시한다.
  return spec.complete(prefix).filter((value) => value.startsWith(prefix))
    .map((value) => [...parts.slice(0, -1), value].join(' '));
}

function buildDevConsole() {
  el.devOut = h('pre', { class: 'dev__out', tabindex: '0' });
  el.devHint = h('div', { class: 'dev__hint' });
  el.devIn = h('input', {
    class: 'dev__in', type: 'text', spellcheck: 'false', autocomplete: 'off',
    'aria-label': '개발자 명령', placeholder: 'help 를 치면 목록이 나와요 · Tab 으로 자동완성',
  });

  const print = (text) => {
    if (text === ' clear') { el.devOut.textContent = ''; return; }
    el.devOut.textContent += (el.devOut.textContent ? '\n' : '') + text;
    el.devOut.scrollTop = el.devOut.scrollHeight;
  };

  const repaintHint = () => {
    const matches = devComplete(el.devIn.value);
    el.devHint.textContent = matches.length && el.devIn.value ? matches.slice(0, 8).join('  ') : '';
  };

  el.devIn.addEventListener('input', repaintHint);
  el.devIn.addEventListener('keydown', async (event) => {
    if (event.key === 'Tab') {
      // Tab 은 브라우저 기본이 포커스 이동이라 반드시 막는다.
      event.preventDefault();
      const matches = devComplete(el.devIn.value);
      if (matches.length === 1) { el.devIn.value = matches[0] + ' '; repaintHint(); }
      else if (matches.length > 1) print(matches.join('  '));
      return;
    }
    // 위/아래로 이전 명령을 꺼낸다. 같은 걸 다시 치게 만들면 콘솔로 쓸 수가 없다.
    if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
      if (!devHistory.length) return;
      event.preventDefault();
      devHistoryAt = event.key === 'ArrowUp'
        ? Math.min(devHistory.length - 1, devHistoryAt + 1)
        : Math.max(-1, devHistoryAt - 1);
      el.devIn.value = devHistoryAt < 0 ? '' : devHistory[devHistory.length - 1 - devHistoryAt];
      return;
    }
    if (event.key !== 'Enter') return;
    const line = el.devIn.value.trim();
    if (!line) return;
    el.devIn.value = '';
    el.devHint.textContent = '';
    devHistory.push(line);
    devHistoryAt = -1;
    print(`› ${line}`);
    const [name, ...rest] = line.split(/\s+/);
    const spec = DEV_COMMANDS[name];
    if (!spec) { print(`모르는 명령이에요: ${name} — help 를 쳐 보세요.`); return; }
    try {
      print(String(await spec.run(rest.join(' '))));
    } catch (error) {
      print(`오류: ${error?.message || error}`);
    }
  });

  el.devBox = h('section', { class: 'dev', hidden: true, role: 'region', 'aria-label': '개발자 콘솔' },
    h('div', { class: 'dev__bar' },
      h('span', null, '⌨ 개발자 콘솔'),
      h('span', { class: 'queue__spacer' }),
      bindAct(h('button', { class: 'btn btn--sm btn--ghost', type: 'button', tip: '닫아요' }, '✕'),
        () => toggleDevConsole(false))),
    el.devOut, el.devHint, el.devIn);
  document.body.appendChild(el.devBox);
}

function toggleDevConsole(force) {
  if (!el.devBox) buildDevConsole();
  consoleOpen = force === undefined ? !consoleOpen : !!force;
  el.devBox.hidden = !consoleOpen;
  if (consoleOpen) setTimeout(() => el.devIn.focus(), 20);
}

/** 관리자가 곡을 보고 있을 때가 실제로 차단하고 싶어지는 순간이다 (§19.3). */
async function openBlacklistSheet(track) {
  let kind = 'title';
  const pattern = h('input', { class: 'field', value: trackTitle(track), maxlength: '200' });
  const row = h('div', { class: 'lib__seg', style: { padding: '0' } },
    ...[['title', '제목 그대로'], ['channel', '채널·아티스트 단위'], ['link', '링크']].map(([id, label]) =>
      h('button', {
        class: 'seg', type: 'button', 'aria-pressed': String(id === kind), dataset: { seg: id },
        onClick: () => {
          kind = id;
          for (const node of row.children) node.setAttribute('aria-pressed', String(node.dataset.seg === id));
          pattern.value = id === 'channel' ? (track.artist || '') : id === 'link' ? trackUrl(track) : trackTitle(track);
        },
      }, label)));

  const ok = await sheet({
    title: '차단 목록에 넣을까요',
    desc: '여기 걸리는 곡은 이 서버에서 담을 수 없게 돼요.',
    body: h('div', { style: { display: 'grid', gap: 'var(--sp-3)' } }, row, pattern),
    danger: true, dismissValue: false,
    actions: [{ label: '취소', kind: 'ghost', value: false }, { label: '차단하기', kind: 'danger', value: true }],
  }).result;
  if (!ok || !pattern.value.trim()) return;
  await call(() => api('/admin/blacklist', { body: { kind, pattern: pattern.value.trim(), note: null } }),
    '차단 목록에 넣었어요.');
}

/* ═══════════════════════ 사람 카드 (§24.2) ═══════════════════════
 * 닉네임·아바타를 **좌클릭**하면 어디서든 열린다.
 * 데이터는 §22의 stats API 하나로 끝난다 — 새 API를 늘리지 않는다.
 */

/* ── 통계 응답 정규화 (§22.5 · §24.2) ──
 * 서버(`/stats/me`·`/stats/user/{id}`)는 요약을 **`summary` 안에 중첩**해서 주고, 이름도 다르다.
 *   summary.queuedTotal ↔ queued · summary.karma ↔ machamScore
 *   topRequested[].requested ↔ count · topLiked[].liked ↔ count · topLoved[].likesRecv ↔ likes
 * 화면이 평평한 이름만 읽던 동안 타일 4장·비율 막대·목록 숫자가 전부 0 이었다.
 * 여기 한 곳에서 한 모양으로 편다 — 화면 코드가 두 모양을 다 알 필요는 없다.
 */
function normalizeStats(raw) {
  const source = raw || {};
  const summary = source.summary && typeof source.summary === 'object' ? source.summary : source;
  const num = (...values) => {
    for (const value of values) {
      const number = Number(value);
      if (Number.isFinite(number)) return number;
    }
    return null;
  };
  const rows = (list_, countKeys) => (Array.isArray(list_) ? list_ : []).map((row) => {
    const count = num(...countKeys.map((key) => row?.[key]));
    return Object.assign({}, row, {
      count: count ?? 0,
      likes: num(row?.likes, row?.likesRecv) ?? 0,
    });
  });

  return {
    available: source.available !== false,
    message: source.message || '',
    userId: source.userId ?? null,
    queued: num(source.queued, summary.queuedTotal, summary.queued) ?? 0,
    queuedSingle: num(source.queuedSingle, summary.queuedSingle),
    queuedBulk: num(source.queuedBulk, summary.queuedBulk),
    bulkTimes: num(source.bulkTimes, summary.bulkTimes),
    played: num(source.played, summary.played) ?? 0,
    skipped: num(source.skipped, summary.skipped) ?? 0,
    boomtta: num(source.boomtta, summary.boomtta) ?? 0,
    likesRecv: num(source.likesRecv, summary.likesRecv) ?? 0,
    supersRecv: num(source.supersRecv, summary.supersRecv) ?? 0,
    dislikesRecv: num(source.dislikesRecv, summary.dislikesRecv) ?? 0,
    chats: num(source.chats, summary.chats),
    // 서버에서는 `karma` 한 이름으로만 나온다 (remote.rs user_stats_json). 화면 이름은 마참 점수다.
    machamScore: num(source.machamScore, summary.machamScore, summary.karma),
    topRequested: rows(source.topRequested, ['count', 'requested']),
    topLiked: rows(source.topLiked, ['count', 'liked']),
    topLoved: rows(source.topLoved, ['likes', 'likesRecv']),
    // `recent` 는 아직 서버 응답에 없다. 없으면 "최근 담은 곡" 섹션은 통째로 빠진다(0으로 꾸미지 않는다).
    recent: Array.isArray(source.recent) ? source.recent : [],
    daily: Array.isArray(source.daily) ? source.daily : [],
  };
}

/** 통계가 꺼져 있으면 서버는 **HTTP 200 + `{available:false}`** 로 답한다 (§22.6).
 *  404 만 보고 있으면 그 안내가 절대 안 뜨고, 대신 0으로 꾸민 화면이 뜬다 —
 *  "0회 재생"과 "기록을 안 받고 있음"은 완전히 다른 이야기라 서버 주석이 스스로 금지한 동작이다. */
function statsOffNode(stats) {
  return emptyState('📊', '아직 기록을 모으지 않아요',
    (stats && stats.message) || '서버에서 기록 기능이 꺼져 있어요. 관리자가 켜면 여기에 쌓여요.');
}

/** 프로필 드롭다운에 조용히 띄우는 마참 점수 (§22.4). 진입 때 한 번만 물어본다 —
 *  `/state/cold` 에는 이 값이 없고, 서버는 60초 캐시라 이 한 번이 비싸지 않다. 폴링은 안 한다. */
async function loadMyScore() {
  if (Number.isFinite(myScore)) return;
  try {
    const stats = normalizeStats(await api('/stats/me'));
    if (!stats.available || !Number.isFinite(stats.machamScore)) return;
    myScore = stats.machamScore;
    renderProfile();
  } catch { /* 통계를 모르는 서버면 점수 줄을 그냥 안 그린다 */ }
}

function personButton(userId, displayName) {
  const name = displayName || '알 수 없음';
  if (!userId) return h('b', null, name);
  const button = h('button', {
    class: 'person', type: 'button', tip: `${name}님이 어떤 사람인지 봐요`,
    onClick: (event) => { event.stopPropagation(); openPersonCard(userId, name); },
  }, name);
  bindContextTarget(button, () => personMenu(userId, name));
  return button;
}

function personAvatar(userId, displayName, url, size) {
  const node = avatar(url, displayName, size);
  node.classList.add('ava--click');
  node.setAttribute('data-tip', `${displayName || '알 수 없음'}님이 어떤 사람인지 봐요`);
  node.addEventListener('click', (event) => { event.stopPropagation(); openPersonCard(userId, displayName); });
  bindContextTarget(node, () => personMenu(userId, displayName));
  return node;
}

async function openPersonCard(userId, displayName) {
  const me = String(store.get().user?.id || '');
  const isMe = String(userId) === me;
  const body = h('div', { class: 'pcard' }, skeletonRows(3));      // 빈 카드가 떴다 채워지면 깜빡여 보인다

  const handle = sheet({
    title: displayName || '알 수 없음',
    body,
    dismissValue: false,
    actions: [
      { label: '닫기', kind: 'ghost', value: false },
      { label: isMe ? '⚙ 내 설정' : '📊 전체 기록', kind: 'primary', value: 'stats' },
    ],
  });
  handle.result.then((value) => {
    if (value === 'stats' && !isMe) openStatsModal(String(userId));
    else if (value === 'stats') toggleMenu(el.meMenu, el.meBtn);
  });

  let stats = null;
  try {
    stats = normalizeStats(await api(`/stats/user/${encodeURIComponent(userId)}`));
  } catch (error) {
    clear(body).appendChild(emptyState('📊', '기록을 못 불러왔어요',
      error && error.status === 404 ? '이 서버는 아직 기록을 모으지 않아요.' : error.message));
    return;
  }
  if (!body.isConnected) return;
  clear(body);
  if (!stats.available) { body.appendChild(statsOffNode(stats)); return; }
  renderPersonCard(body, userId, displayName, stats, isMe);
}

function renderPersonCard(host, userId, displayName, stats, isMe) {
  const presence = store.get().presence || {};
  const listening = (presence.listening || []).map(String).includes(String(userId));
  const member = store.get().members.find((row) => String(row.userId ?? row.id) === String(userId));
  const tier = TIERS[member?.tier] || null;

  put(host,
    h('div', { class: 'pcard__head' },
      avatar(avatarOf(userId), displayName, 'lg'),
      h('div', null,
        h('strong', null, displayName || '알 수 없음'),
        h('div', { class: 'pcard__tags' },
          tier ? h('span', { class: `tier tier--${member.tier}`, tip: tier.desc }, `${tier.icon} ${tier.label}`) : null,
          listening ? h('span', { class: 'chip chip--ok', tip: '봇과 같은 음성 채널에 있어요' }, '🎧 듣는 중') : null),
        Number.isFinite(stats.machamScore)
          ? h('div', { class: 'pcard__score', tip: '받은 좋아요·슈퍼 좋아요와 재생 기록이 쌓인 점수예요. 순서에는 영향이 없어요' },
            `마참 점수 ${stats.machamScore}`)
          : null)),
    h('div', { class: 'pcard__nums' },
      statTile('담은 곡', stats.queued, '이 사람이 담은 곡 수예요'),
      statTile('재생', stats.played, '끝까지 재생된 곡 수예요'),
      statTile('받은 👍', stats.likesRecv, '이 사람 곡이 받은 좋아요예요')),
    ratioBar(stats),
    recentList('최근 담은 곡', stats.recent.slice(0, 5), (row) => fmtAgo(row.playedUtc || row.addedUtc || row.lastUtc)),
    recentList('자주 담는 곡', stats.topRequested.slice(0, 3), (row) => `${row.count}회`));

  if (isMe) return;
  host.appendChild(h('div', { class: 'pcard__acts' },
    setLock(bindAct(h('button', { class: 'btn btn--sm', type: 'button', tip: '채팅에 이 사람을 불러요' }, '@ 멘션'),
      () => {
        openSide('chat');
        el.chatInput.value = `${el.chatInput.value}@${displayName} `.trimStart();
        el.chatInput.focus();
      }), !can('chat'), lockReason('chat')),
    setLock(bindAct(h('button', { class: 'btn btn--sm btn--danger', type: 'button', tip: '이 사람의 조작을 잠시 막아요' }, '⛔ 정지'),
      () => openSuspendSheet({ userId, displayName })), !can('suspend'), lockReason('suspend'))));
}

function statTile(label, value, tip) {
  return h('div', { class: 'stile', tip }, h('b', null, String(value)), h('span', null, label));
}

/** 끝까지 / 스킵 / 붐따 비율을 막대 하나로. */
function ratioBar(stats) {
  const played = Number(stats.played) || 0;
  const skipped = Number(stats.skipped) || 0;
  const boomtta = Number(stats.boomtta) || 0;
  const total = played + skipped + boomtta;
  if (!total) return null;
  const pct = (value) => Math.round((value / total) * 100);
  return h('div', { class: 'ratio', tip: `끝까지 ${pct(played)}% · 스킵 ${pct(skipped)}% · 붐따 ${pct(boomtta)}%` },
    h('div', { class: 'ratio__bar' },
      played ? h('span', { class: 'ratio__seg ratio__seg--ok', style: { flex: String(played) } }) : null,
      skipped ? h('span', { class: 'ratio__seg ratio__seg--warn', style: { flex: String(skipped) } }) : null,
      boomtta ? h('span', { class: 'ratio__seg ratio__seg--down', style: { flex: String(boomtta) } }) : null),
    h('div', { class: 'hint' }, `끝까지 ${pct(played)}% · 스킵 ${pct(skipped)}%${boomtta ? ` · 붐따 ${pct(boomtta)}%` : ''}`));
}

function recentList(title, rows, sub) {
  if (!rows.length) return null;
  return h('div', { class: 'pcard__list' },
    h('h3', null, title),
    ...rows.map((row) => {
      const track = row.track || row;
      const node = h('div', { class: 'row row--tight', dataset: { mqRow: '1' } },
        h('img', { class: 'row__art', src: artUrl(track) || '', alt: '', loading: 'lazy' }),
        h('div', { class: 'row__main' }, mqText(trackTitle(track), 'row__title')),
        h('span', { class: 'row__sub' }, sub(row)));
      bindContextTarget(node, () => trackMenu(track, { source: 'person' }));
      return node;
    }));
}

/* ═══════════════════════ 내 기록 모달 (§22.5) ═══════════════════════ */

async function openStatsModal(userId) {
  const body = h('div', { class: 'stats' }, skeletonRows(4));
  const mine = !userId;
  sheet({
    title: mine ? '📊 내 기록' : '📊 기록',
    desc: mine ? '담은 곡·재생·받은 반응이 여기 쌓여요.' : '받은 것만 보여드려요.',
    wide: true, body, dismissValue: false,
    actions: [{ label: '닫기', kind: 'primary', value: false }],
  });

  let stats = null;
  try {
    stats = normalizeStats(await api(mine ? '/stats/me' : `/stats/user/${encodeURIComponent(userId)}`));
  } catch (error) {
    clear(body).appendChild(emptyState('📊', '기록을 못 불러왔어요',
      error && error.status === 404 ? '이 서버는 아직 기록을 모으지 않아요.' : error.message));
    return;
  }
  if (!body.isConnected) return;
  clear(body);
  if (!stats.available) { body.appendChild(statsOffNode(stats)); return; }
  if (mine && Number.isFinite(stats.machamScore)) { myScore = stats.machamScore; renderProfile(); }
  renderStats(body, stats, mine);
}

function renderStats(host, stats, mine) {
  put(host,
    h('div', { class: 'stats__tiles' },
      statTile('담은 곡', stats.queued, '한 곡씩 담은 것과 한 번에 담은 것을 모두 세요'),
      statTile('재생된 곡', stats.played, '내 곡이 끝까지 재생된 수예요'),
      statTile('받은 좋아요', stats.likesRecv, '내 곡이 받은 좋아요예요'),
      // 마참 점수는 아직 못 받았으면 0으로 꾸미지 않는다 — 0점과 "모름"은 다른 이야기다.
      Number.isFinite(stats.machamScore)
        ? statTile('마참 점수', stats.machamScore, '받은 반응이 쌓인 점수예요. 대기열 순서에는 영향이 없어요')
        : null),
    ratioBar(stats),
    mine && Number.isFinite(stats.queuedSingle) ? h('p', { class: 'hint' },
      `한 곡씩 ${stats.queuedSingle}곡 · 한 번에 ${stats.queuedBulk ?? 0}곡 (${stats.bulkTimes ?? 0}번)`) : null,
    h('p', { class: 'hint' },
      `받은 반응: 👍${stats.likesRecv} ⭐${stats.supersRecv} 👎${stats.dislikesRecv}`),
    recentList('가장 많이 신청한 곡', stats.topRequested.slice(0, 5), (row) => `${row.count}회`),
    mine ? recentList('내가 가장 많이 좋아요한 곡', stats.topLiked.slice(0, 5), (row) => `${row.count}회`) : null,
    recentList('가장 많이 사랑받은 내 곡', stats.topLoved.slice(0, 5), (row) => `👍${row.likes}`),
    dailyChart(stats.daily));
}

/** 30일 꺾은선. 라이브러리 없이 SVG 하나로 그린다 — 서버도 브라우저도 가볍게. */
function dailyChart(daily) {
  if (daily.length < 3) {
    return h('p', { class: 'hint' }, '기록이 쌓이면 여기에 그래프가 나와요.');
  }
  const width = 520;
  const height = 120;
  const max = Math.max(1, ...daily.map((row) => Math.max(row.queued || 0, row.played || 0)));
  const path = (key) => daily.map((row, index) => {
    const x = (index / Math.max(1, daily.length - 1)) * width;
    const y = height - ((row[key] || 0) / max) * (height - 8) - 4;
    return `${index === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`;
  }).join(' ');

  const svg = h('svg', {
    class: 'sparkline', viewBox: `0 0 ${width} ${height}`, preserveAspectRatio: 'none',
    role: 'img', 'aria-label': `최근 ${daily.length}일 동안 담은 곡과 재생된 곡`,
  },
    h('path', { d: path('queued'), fill: 'none', stroke: 'var(--accent)', 'stroke-width': '2' }),
    h('path', { d: path('played'), fill: 'none', stroke: 'var(--ok)', 'stroke-width': '2' }));

  return h('div', { class: 'stats__chart' },
    h('h3', null, `최근 ${daily.length}일`),
    svg,
    h('div', { class: 'hint' }, '보라 = 담은 곡 · 초록 = 재생된 곡'));
}

/* ═══════════════════════ 차트 (§15) ═══════════════════════
 * "너무 다다다닥"을 피하는 게 이 화면의 요구사항이다. 그래서 2단계로 들어간다.
 * 1단계 분류 카드 6장 → 2단계 그 분류의 차트 목록 → 곡 목록.
 * 뒤로 가기는 ← 버튼과 브라우저 뒤로가기 둘 다 먹는다.
 */

let chartView = { level: 'categories', category: null, chart: null, period: 'month' };
/* 차트 한 장을 받는 데 yt-dlp 가 도느라 몇 초씩 걸린다. 그동안 사람은 뒤로 가거나 다른 차트를
 * 누른다. 응답이 그때 도착해서 그대로 그리면 **보고 있던 화면을 남의 결과가 덮는다.**
 * 요청마다 번호를 매기고, 돌아왔을 때 내가 아직 최신 요청인지 확인한다. */
let chartLoadSeq = 0;

function buildChartsPane() {
  el.chartBack = h('button', {
    class: 'iconbtn', type: 'button', hidden: true, tip: '한 단계 뒤로', 'aria-label': '뒤로',
    onClick: () => chartBack(),
  }, '←');
  el.chartTitle = h('h2', null, '차트');
  el.chartRefresh = bindAct(h('button', {
    class: 'iconbtn', type: 'button', hidden: true,
    tip: '캐시를 무시하고 지금 다시 가져와요', 'aria-label': '차트 새로고침',
  }, '↻'), () => refreshChart());
  el.chartBulk = bindAct(h('button', {
    class: 'btn btn--sm', type: 'button', hidden: true, tip: '이 차트를 통째로 대기열에 담아요',
  }, '전부 담기'), () => enqueueChart());
  el.chartPeriod = h('div', { class: 'lib__seg', hidden: true },
    ...CHART_PERIODS.map(([id, label]) => h('button', {
      class: 'seg', type: 'button', 'aria-pressed': String(id === chartView.period), dataset: { seg: id },
      tip: `${label} 기준으로 순위를 매겨요`,
      onClick: () => {
        chartView.period = id;
        for (const node of el.chartPeriod.children) node.setAttribute('aria-pressed', String(node.dataset.seg === id));
        if (chartView.level === 'tracks') openChart(chartView.chart, true);
      },
    }, label)));

  el.chartBody = h('div', { class: 'scroll charts__body' });
  el.chartsPane = h('div', { class: 'tabpane charts', role: 'tabpanel', 'aria-labelledby': 'railtab-charts' },
    h('div', { class: 'charts__head' }, el.chartBack, el.chartTitle, h('span', { class: 'queue__spacer' }), el.chartBulk, el.chartRefresh),
    el.chartPeriod,
    el.chartBody);
  return el.chartsPane;
}

async function loadCharts() {
  if (chartState) { renderCharts(); return; }
  clear(el.chartBody).appendChild(skeletonRows(4));
  try {
    const data = await api('/charts');
    chartState = { categories: data?.categories || [] };
  } catch (error) {
    // 서버가 아직 차트를 모르면 탭을 통째로 숨긴다. 빈 탭이 남아 있는 게 제일 나쁘다.
    if (error && (error.status === 404 || error.status === 501)) {
      chartState = null;
      hideChartsTab();
      return;
    }
    clear(el.chartBody).appendChild(emptyState('📈', '차트를 못 불러왔어요', error.message));
    return;
  }
  renderCharts();
}

function hideChartsTab() {
  const tab = (el.railTabs || []).find((node) => node.dataset.rail === 'charts');
  if (tab) tab.hidden = true;
  if (el.railPanes?.charts) el.railPanes.charts.hidden = true;
  if (activeRailTab === 'charts') setRailTab('search');
}

function chartBack() {
  // 진행 중인 차트 로드를 무효로 만든다. 이게 없으면 뒤로 간 뒤에 도착한 응답이
  // 목록 화면 위에 곡을 그려 버린다 (번호를 안 올리면 `token === chartLoadSeq` 라 통과한다).
  chartLoadSeq += 1;
  if (chartView.level === 'tracks') chartView = { ...chartView, level: 'charts', chart: null };
  else chartView = { ...chartView, level: 'categories', category: null };
  renderCharts();
}

function renderCharts() {
  if (!chartState) return;
  clear(el.chartBody);
  const inCategories = chartView.level === 'categories';
  el.chartBack.hidden = inCategories;
  el.chartRefresh.hidden = chartView.level !== 'tracks' || tierOf() === 'member' || tierOf() === 'viewer';
  el.chartBulk.hidden = chartView.level !== 'tracks';
  el.chartPeriod.hidden = !(chartView.level === 'tracks' && chartView.category === 'ours');

  if (inCategories) {
    el.chartTitle.textContent = '차트';
    const cards = chartState.categories.map((category) => {
      const meta = CHART_CATEGORIES[category.key] || { icon: category.icon || '📈', label: category.label, desc: '' };
      return h('button', {
        class: 'chartcat', type: 'button', tip: `${meta.label} 차트를 열어요`,
        onClick: () => { chartView = { ...chartView, level: 'charts', category: category.key }; renderCharts(); },
      },
        h('span', { class: 'chartcat__icon', 'aria-hidden': 'true' }, meta.icon),
        h('strong', null, meta.label),
        h('small', null, meta.desc || `${(category.charts || []).length}개 차트`));
    });
    el.chartBody.appendChild(cards.length
      ? h('div', { class: 'chartcats' }, ...cards)
      : emptyState('📈', '쓸 수 있는 차트가 없어요', '서버 관리자가 관리 콘솔에서 차트를 켤 수 있어요.'));
    return;
  }

  if (chartView.level === 'charts') {
    const category = chartState.categories.find((row) => row.key === chartView.category);
    const meta = CHART_CATEGORIES[chartView.category] || { label: category?.label || '차트' };
    el.chartTitle.textContent = meta.label;
    // 작동하지 않는 차트는 유저 화면에서 뺀다. 눌렀는데 아무 일도 안 일어나는 게 제일 나쁘다 (§15.2)
    const charts = (category?.charts || []).filter((chart) => chart.ok !== false);
    if (!charts.length) {
      el.chartBody.appendChild(emptyState('📈', '이 분류에 쓸 수 있는 차트가 없어요', null));
      return;
    }
    for (const chart of charts) {
      el.chartBody.appendChild(h('button', {
        class: 'row row--btn', type: 'button', tip: `${chart.name} 곡 목록을 열어요`,
        onClick: () => openChart(chart),
      },
        h('span', { class: 'chartcat__icon', 'aria-hidden': 'true' }, meta.icon || '📈'),
        h('div', { class: 'row__main' },
          h('div', { class: 'row__title' }, chart.name),
          h('div', { class: 'row__sub' }, [chart.provider, chart.lastFetchedUtc ? `${fmtAgo(chart.lastFetchedUtc)} 갱신` : null]
            .filter(Boolean).join(' · ')))));
    }
    return;
  }

  renderChartTracks();
}

async function openChart(chart, keepLevel) {
  // 이전 차트의 줄이 남아 있으면 로드에 실패했을 때 남의 숫자가 그려진다.
  chartView = { ...chartView, level: 'tracks', chart, rows: null, tracks: [] };
  const token = ++chartLoadSeq;
  if (!keepLevel) pushChartHistory();
  renderCharts();
  clear(el.chartBody).appendChild(skeletonRows(6));      // yt-dlp가 도는 몇 초 동안 빈 화면이면 고장으로 보인다
  try {
    // 서버가 읽는 쿼리 키는 `window` 다 (remote.rs `ChartWindowQuery`). `period` 만 보내면
    // serde 가 조용히 버려서 어느 기간을 눌러도 늘 `month` 가 나온다. 예전 빌드 호환으로 둘 다 보낸다.
    const value = encodeURIComponent(chartView.period);
    const query = chartView.category === 'ours' ? `?window=${value}&period=${value}` : '';
    const data = await api(`/charts/${encodeURIComponent(chart.id)}${query}`);
    // 기다리는 사이에 뒤로 갔거나 다른 차트를 눌렀으면 여기서 끝낸다.
    if (token !== chartLoadSeq) return;
    // 우리 차트는 통계가 붙은 `rows` 와 맨 트랙 배열 `tracks` 를 둘 다 준다.
    // `tracks` 만 읽으면 §15.2b 가 요구한 숫자·계산식이 하나도 안 나온다 (그냥 곡 목록이 된다).
    chartView.rows = Array.isArray(data?.rows) ? data.rows : null;
    chartView.tracks = data?.tracks || [];
    chartView.fetchedUtc = data?.fetchedUtc || null;
  } catch (error) {
    // 실패 화면도 마찬가지다 — 늦게 온 실패가 지금 보고 있는 차트를 지우면 안 된다.
    if (token !== chartLoadSeq) return;
    clear(el.chartBody).appendChild(emptyState('📈', '이 차트를 못 가져왔어요', error.message));
    return;
  }
  renderChartTracks();
}

function pushChartHistory() {
  try {
    history.pushState({ chart: chartView.chart?.id, category: chartView.category }, '', location.href);
  } catch { /* 히스토리를 못 쓰면 ← 버튼만 쓴다 */ }
}

window.addEventListener('popstate', () => {
  if (chartView.level !== 'categories') chartBack();
});

/** 우리 차트 한 줄의 부제 — "42회 재생 · 7명이 신청", "👍284 + ⭐52×2 = 388" (§15.2b).
 *  순위가 왜 그런지 숫자로 보여야 차트가 차트다. 서버가 `loveFormula` 문자열까지 만들어 주므로
 *  계산식은 그걸 그대로 쓴다 — 클라가 가중치를 다시 곱하면 설정을 바꿨을 때 화면이 갈린다. */
function chartRowExtra(row) {
  const extra = [];
  const plays = Number.isFinite(Number(row.playsUser)) ? Number(row.playsUser)
    : (Number.isFinite(Number(row.plays)) ? Number(row.plays) : null);
  if (plays !== null) extra.push(`${plays}회 재생`);
  if (Number.isFinite(Number(row.requesters))) extra.push(`${Number(row.requesters)}명이 신청`);
  if (row.loveFormula) extra.push(String(row.loveFormula));
  else if (Number.isFinite(Number(row.loveScore))) {
    extra.push(`👍${row.likes || 0} + ⭐${row.supers || 0}×${row.superWeight ?? 2} = ${row.loveScore}`);
  }
  return extra;
}

function renderChartTracks() {
  const chart = chartView.chart;
  el.chartTitle.textContent = chart?.name || '차트';
  clear(el.chartBody);
  // 우리 차트면 통계가 붙은 `rows` 를, 바깥 차트면 맨 `tracks` 를 그린다.
  const rows = chartView.rows && chartView.rows.length ? chartView.rows : (chartView.tracks || []);
  if (!rows.length) {
    el.chartBody.appendChild(emptyState('📈', '이 차트에 곡이 없어요', '잠시 뒤에 다시 열어 보세요.'));
    return;
  }

  el.chartBody.appendChild(h('div', { class: 'charts__meta' },
    chartView.fetchedUtc ? h('span', null, `마지막 갱신 ${fmtAgo(chartView.fetchedUtc)}`) : null,
    h('span', { class: 'queue__spacer' }),
    h('span', null, `${rows.length}곡`)));

  el.chartBulk.textContent = `전부 담기 (${rows.length}곡)`;
  setLock(el.chartBulk, !canBulk(), lockReason('bulkEnqueue'));

  rows.forEach((row, index) => {
    const track = row.track || row;
    const extra = chartRowExtra(row);
    const node = trackRow(track, 'chart', extra.length ? extra.join(' · ') : trackSub(track), { rank: index + 1 });
    if (Number.isFinite(Number(row.playsAutoplay)) && Number(row.playsAutoplay) > 0) {
      node.setAttribute('data-tip', `자동재생으로 ${row.playsAutoplay}회 더 나갔어요 (순위에는 안 세요)`);
    }
    el.chartBody.appendChild(node);
  });
  marquee.scan(el.chartBody);
}

async function enqueueChart() {
  const chart = chartView.chart;
  if (!chart) return;
  const shown = chartView.rows && chartView.rows.length ? chartView.rows : (chartView.tracks || []);
  const count = shown.length;
  const ok = await confirmSheet({
    title: `${count}곡을 담을까요`,
    desc: `'${chart.name}'의 곡을 순서대로 대기열에 넣어요. 한 번에 담는 양에는 상한이 있어요.`,
    confirmText: '담기',
  });
  if (!ok) return;
  // 화면에 보이는 기간을 같이 보낸다. 안 보내면 서버가 우리 차트를 늘 `month` 로 담아서
  // 사용자가 본 목록과 담긴 목록이 달라진다 (§15.4).
  // 이 값은 **쿼리 문자열**로 간다 — 서버가 `Query<ChartWindowQuery>` 로 읽고 body 는 안 본다.
  const value = encodeURIComponent(chartView.period);
  const query = chartView.category === 'ours' ? `?window=${value}&period=${value}` : '';
  const result = await call(() => api(`/charts/${encodeURIComponent(chart.id)}/enqueue${query}`, { body: {} }));
  if (!result) return;
  if (result.limited) toast(`대기열 한도까지 ${result.added ?? 0}곡만 담았어요.`, 'warn');
  else toast(`${result.added ?? count}곡을 담았어요.`, 'ok');
}

async function refreshChart() {
  const chart = chartView.chart;
  if (!chart) return;
  const result = await call(() => api(`/charts/${encodeURIComponent(chart.id)}/refresh`, { body: {} }), '차트를 다시 가져왔어요.');
  if (result) openChart(chart, true);
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
    superLike: data.superLike || null,
    coldAt: Date.now(),
  });
  if (Number.isFinite(data.machamScore)) myScore = data.machamScore;
}

async function loadHot() {
  const data = await api('/state/hot');
  // 카운트다운은 서버 시각 기준으로 센다. 표본 시각으로 시계 차이를 맞춰 둔다.
  noteServerTime(data.sampledAtUtc || data.sortedAt);
  if (data.presence && data.presence.bot !== undefined) lastBotState = data.presence.bot;
  // 대기열이 길면 서버가 앞 200곡만 보낸다 (§18.2)
  queueTotal = Number.isFinite(data.queueTotal) ? data.queueTotal : (data.queue || []).length;
  queueTruncated = !!data.queueTruncated;
  noteSortPeriod(data.sortPeriodSeconds);
  // 여기가 개인화 필드의 원본이다. 이후 브로드캐스트 프레임에 되붙이려고 기억해 둔다 (§10.4).
  notePersonalFields(data.queue || []);
  // 뒤쪽 페이지는 서버가 정한 순서를 모르면 못 이어 붙인다. 전체 로드 때는 깨끗이 버린다.
  queueTail = [];
  store.patch({
    player: data.player || null,
    current: data.current || null,
    queue: data.queue || [],
    queueMode: data.queueMode || data.mode || 'score',
    sortedAt: data.sortedAt || null,
    nextSortAt: data.nextSortAt || null,
    next: data.next || null,
    skipVote: data.skipVote || null,
    presence: data.presence || store.get().presence,
    hotAt: Date.now(),
  });
  // 서버가 정한 일정 (§31). 진입·재연결 직후부터 정확히 맞아야 한다.
  store.patch({
    schedule: {
      startedUtc: data.startedUtc ?? null,
      nextStartUtc: data.nextStartUtc ?? null,
      skipLeadMs: Number(data.skipLeadMs) || 0,
      seekLockoutMs: Number(data.seekLockoutMs) || 0,
      webSyncOffsetMs: Number(data.webSyncOffsetMs) || 0,
    },
  });
  clock.sync({
    positionSeconds: data.positionSeconds,
    sampledAtUtc: data.sampledAtUtc,
    startedUtc: data.startedUtc,
    isPaused: data.player?.isPaused,
    // **진입 로드에서도 멈춤 여부를 넘긴다** (§36). 이게 없으면 새로고침 직후에는
    // 봇이 음성에 없어도 진행바가 다시 흐르기 시작한다.
    stopped: data.player?.voiceConnected === false || !data.current,
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
  paintTheme(themeChoice());
  buildShell();
  tooltip();
  marqueeRows();
  el.railDrawerBtn.hidden = !railDrawerActive();
  // 빈 배경 우클릭 — 테마·배치·내 기록 (§24.1)
  bindContextTarget(el.portal, (event) => (event.target.closest?.(
    '.qitem, .row, .msg, .member, .plcard, .now, .nextrow, .queue__head, button, a, input, textarea, select',
  ) ? null : backgroundMenu()));
  if (panelMode()) mountDock();

  // 초기 그리기 — 데이터가 오기 전에도 뼈대는 보인다
  store.subscribe(['conn', 'tier', 'suspension', 'intentStatus', 'presence'], renderBanners);
  store.subscribe(['user', 'tier', 'permissions'], renderProfile);
  store.subscribe(['guild', 'guilds'], renderGuild);
  store.subscribe(['presence'], renderPresenceSummary);
  store.subscribe(['presence', 'members', 'intentStatus'], renderMembers);
  store.subscribe(['queue', 'queueMode', 'permissions', 'suspension', 'tier', 'conn', 'hotAt', 'settings', 'superLike'], renderQueue);
  store.subscribe(['current', 'player', 'permissions', 'suspension', 'tier', 'settings', 'conn', 'next', 'skipVote'], renderNow);
  store.subscribe(['chat', 'chatDelta', 'permissions', 'suspension', 'tier', 'conn', 'settings', 'coldAt'], renderChat);
  store.subscribe(['liked', 'saved', 'playlists', 'permissions', 'suspension', 'tier'], renderLibrary);
  store.subscribe(['recent', 'permissions', 'suspension', 'tier'], renderRecent);
  store.subscribe(['suggestions', 'permissions', 'suspension', 'tier'], renderSuggestions);
  store.subscribe(['audit'], renderAudit);
  store.subscribe(['lyrics'], () => { if (lyricsOpen) renderLyrics(); });
  store.subscribe(['buildId'], checkVersion);
  store.subscribe(['permissions', 'suspension', 'tier'], renderSeeds);
  store.subscribe(['queueMode', 'sortedAt', 'nextSortAt', 'queue'], renderSortTick);
  store.subscribe(['suggestions'], (state) => {
    // 새 제안이 올라오면 헤더 버튼에 점만 찍는다. 숫자까지는 필요 없다 (§11).
    // 모달을 보고 있는 동안에는 점을 켜지 않는다 — 눈앞에 있는 걸 또 알릴 필요가 없다.
    if (suggestModalOpen) { el.suggestDot.hidden = true; return; }
    if (suggestUnread) el.suggestDot.hidden = !state.suggestions.length;
  });

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
  loadMyScore();
  // 봇 주인이면 승인 대기 개수를 메뉴에 배지로 붙인다 (§26.2).
  // 대기 중인 서버가 있는데 아무 데도 안 보이면 영영 방치된다.
  loadApprovals();
  // 새 버전으로 처음 들어왔으면 무엇이 바뀌었는지 한 번 알려준다 (§30).
  maybeShowChangelog();
  focusLinkedMessage();
  // 지난번에 보던 탭이 그대로 열려 있으면 그 탭 데이터도 챙긴다.
  // (탭을 다시 누르지 않으면 영영 안 불러오는 구멍이 있었다)
  if (activeRailTab === 'charts') loadCharts();
  if (activeSideTab === 'audit') loadAudit();

  // 한 번도 배치를 고른 적이 없으면 여기서 물어본다. 두 번째 진입부터는 안 뜬다.
  if (!layoutChosen) openLayoutSheet();

  connect(ctx.guildId, {
    onResync: () => { loadHot().catch(() => {}); loadCold().catch(() => {}); loadSeeds(); },
    onRefetch: (what) => {
      if (what === 'library' || what === 'settings' || what === 'permissions') refetchCold();
      // 모달을 열어 둔 채라면 이미 보고 있는 것이다. 점을 다시 켜지 않고 목록만 갱신한다 (§11).
      if (what === 'suggestions') {
        if (suggestModalOpen) loadSuggestions();
        else { suggestUnread = true; el.suggestDot.hidden = false; }
      }
      if (what === 'audit') { store.patch({ audit: [] }); if (activeSideTab === 'audit') loadAudit(true); }
    },
    onChat: onChatArrived,
    onEvent: (type, data) => {
      if (type === 'autoplay') loadSeeds();
      if (type === 'skipvote') store.patch({ skipVote: data && data.need ? data : null });
      if (type === 'charts') { chartState = null; if (activeRailTab === 'charts') loadCharts(); }
      // 서버가 새 로그 한 줄을 실어 보낸다(§13.5). 전체를 다시 부르지 않고 앞에 붙인다.
      // 투표·담기까지 기록 대상이 된 지금 재조회로 돌리면 로그 탭이 열려 있는 내내 요청이 쏟아진다.
      // 탭이 닫혀 있으면 어차피 열 때 불러오므로 그냥 버린다.
      if (type === 'audit' && data && data.entry) {
        if (activeSideTab !== 'audit') return;
        const list = Array.isArray(store.get().audit) ? store.get().audit : [];
        // 합쳐진 줄은 서버가 같은 id 로 다시 보낸다. 있으면 갈아 끼우고 없으면 앞에 붙인다.
        const at = list.findIndex((row) => String(row.id) === String(data.entry.id));
        const next = at >= 0
          ? list.map((row, index) => (index === at ? data.entry : row))
          : [data.entry].concat(list).slice(0, 200);
        store.patch({ audit: next });
      }
    },
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
        queueTotal = Number.isFinite(data.queueTotal) ? data.queueTotal : (data.items || []).length;
        queueTruncated = !!data.queueTruncated;
        // 재정렬 주기는 서버가 대기열 길이를 보고 정한다 (§18.2 (3)). 카운트다운·헤더가 이 값을 센다.
        noteSortPeriod(data.sortPeriodSeconds);
        // core.js가 방금 대기열을 통째로 갈아 끼웠다. 개인화 필드(§10.4)와 이미 받아 둔
        // 뒤쪽 페이지(§18.2 (1))를 여기서 되붙인다 — 안 그러면 재정렬마다 둘 다 날아간다.
        store.patch({
          queue: mergeQueueFrame(store.get().queue),
          nextSortAt: data.nextSortAt || null,
          next: data.next !== undefined ? data.next : store.get().next,
        });
      }
      if (type === 'playback' && data) {
        noteServerTime(data.sampledAtUtc);
        // 곡이 바뀌면 다음 곡도 같이 바뀌고, 스킵 투표는 리셋된다 (§10.5·§14.2)
        if (data.next !== undefined) store.patch({ next: data.next });
        if (data.currentId !== undefined && data.currentId !== store.get().current?.id) store.patch({ skipVote: null });
      }
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

/* ── 툴팁 전수 검사 (§20.3) ──
 * 눈으로 세지 말고 자동으로. 콘솔에서 __machamTipAudit() 를 부르면 0이 나와야 한다.
 * 아이콘만 있는 버튼에 설명이 없으면 그건 수수께끼다.
 */
function tipAudit() {
  const all = [...document.querySelectorAll('button,[role=button],a.iconbtn')];
  const visible = (node) => !node.closest('[hidden]') && node.offsetParent !== null;
  const iconOnly = (node) => !node.textContent.trim().replace(/[\p{Emoji}\p{Emoji_Presentation}\s＋✕←↻▸▾▪·]/gu, '');

  const missing = all.filter((node) => visible(node) && iconOnly(node)
    && !node.dataset.tip && !node.getAttribute('aria-label'));
  const ariaOnly = all.filter((node) => visible(node) && !node.dataset.tip && node.getAttribute('aria-label'));
  const nativeTitle = [...document.querySelectorAll('[title]')];

  const report = {
    missing: missing.map((node) => node.className || node.tagName),
    ariaOnly: ariaOnly.map((node) => node.className || node.tagName),
    nativeTitle: nativeTitle.map((node) => node.className || node.tagName),
  };
  console.info('[툴팁 감사] 설명 없는 아이콘 버튼', report.missing.length,
    '· aria-label만 있는 것', report.ariaOnly.length, '· title= 남은 것', report.nativeTitle.length, report);
  return report;
}
window.__machamTipAudit = tipAudit;

/* ── 회귀 테스트 (§0) ──
 * 여기서 고친 것들은 전부 "화면이 조용히 거짓말을 하는" 종류라 눈으로는 다시 놓치기 쉽다.
 * 순수 함수로 뽑아 두고 콘솔에서 `__machamSelfTest()` 로 돌린다 — 0 fail 이 나와야 한다.
 * (Node 하니스에서도 같은 함수를 부른다.)
 */
function selfTest() {
  const fails = [];
  const eq = (name, actual, expected) => {
    const a = JSON.stringify(actual);
    const b = JSON.stringify(expected);
    if (a !== b) fails.push(`${name}: ${a} ≠ ${b}`);
  };

  /* §5 — 카운트다운이 `갱신 0` 에 멈추면 안 된다.
   * nextSortAt 은 순서가 실제로 바뀐 길드에만 새로 나가고, 곡이 1~2곡이면 아예 안 나간다.
   * 그러면 기준 시각이 과거에 박히는데, 롤오버가 없으면 화면이 영원히 0을 붙들고 있었다. */
  const now = Date.UTC(2026, 0, 1, 0, 0, 30);
  const at = (ms) => new Date(ms).toISOString();
  const queued = [{ id: 'a' }];
  eq('sortRemain: 지난 기준 시각은 주기만큼 굴러간다',
    sortRemainFrom({ queueMode: 'score', queue: queued, nextSortAt: at(now - 12000) }, now, 5), 3);
  eq('sortRemain: 한 주기를 갓 지난 경계도 0이 아니다',
    sortRemainFrom({ queueMode: 'score', queue: queued, nextSortAt: at(now - 5000) }, now, 5), 5);
  eq('sortRemain: 기준 시각이 바로 지금이면 다음 주기를 센다',
    sortRemainFrom({ queueMode: 'score', queue: queued, nextSortAt: at(now) }, now, 5), 5);
  eq('sortRemain: 미래 기준 시각은 그대로 센다',
    sortRemainFrom({ queueMode: 'score', queue: queued, nextSortAt: at(now + 3200) }, now, 5), 4);
  /* §18.2 (3) — 서버가 15초로 늦췄으면 화면도 15를 세야 한다. 예전에는 9로 잘라서
   * 카운트다운이 9 근처에서 계속 리셋되며 0에 도달하지 못했다. */
  eq('sortRemain: 주기는 서버 값을 따른다 (9 하드클램프 없음)',
    sortRemainFrom({ queueMode: 'score', queue: queued, nextSortAt: at(now + 14500) }, now, 15), 15);
  eq('sortRemain: sortedAt만 있어도 주기로 다음 경계를 만든다',
    sortRemainFrom({ queueMode: 'score', queue: queued, sortedAt: at(now - 11000) }, now, 5), 4);
  eq('sortRemain: fifo면 숨긴다',
    sortRemainFrom({ queueMode: 'fifo', queue: queued, nextSortAt: at(now - 1) }, now, 5), null);
  eq('sortRemain: 대기열이 비면 숨긴다',
    sortRemainFrom({ queueMode: 'score', queue: [], nextSortAt: at(now + 1000) }, now, 5), null);
  eq('sortRemain: 기준이 아예 없으면 숨긴다',
    sortRemainFrom({ queueMode: 'score', queue: queued }, now, 5), null);

  /* §10.1 — 점수표는 `settings.votePoints` 중첩 객체로 온다. 평평한 이름만 읽던 동안
   * 화면은 관리자가 뭘 바꾸든 늘 기본 배점으로 계산식을 그렸다. */
  eq('votePoints: 중첩 votePoints를 읽는다',
    votePointsFrom({ votePoints: { like: 3, dislike: -2, superLike: 5, wait: 2 } }),
    { like: 3, dislike: -2, superLike: 5, wait: 2 });
  eq('votePoints: 0도 값이다 (기본값으로 안 떨어진다)',
    votePointsFrom({ votePoints: { like: 0, dislike: 0, superLike: 0, wait: 0 } }),
    { like: 0, dislike: 0, superLike: 0, wait: 0 });
  eq('votePoints: 평평한 옛 이름도 받아 준다',
    votePointsFrom({ likePoints: 4 }), { like: 4, dislike: -1, superLike: 2, wait: 1 });
  eq('votePoints: 아무것도 없으면 기본 배점',
    votePointsFrom(null), { like: 1, dislike: -1, superLike: 2, wait: 1 });

  /* §10.4 — 계산식은 서버가 만든 것을 쓴다. `= 합계` 꼬리는 우리가 따로 고정한다. */
  eq('serverFormula: 합계 꼬리를 뗀다', serverFormula({ formula: '👍3 + ⭐1×2 = 7' }), '👍3 + ⭐1×2');
  eq('serverFormula: 빈 점수 문장은 안 쓴다', serverFormula({ formula: '아직 점수가 없어요 = 0' }), '');
  eq('serverFormula: 없으면 빈 문자열', serverFormula({}), '');

  /* §10.4 · §18.2 (1) — 브로드캐스트가 비워 보낸 개인화 필드를 되붙인다. */
  queuePersonal.clear();
  notePersonalFields([{ id: 'x', isMine: true, myVote: 'like' }, { id: 'y', isMine: false, myVote: null }]);
  eq('personal: null로 온 필드를 되붙인다',
    applyPersonalFields([{ id: 'x', isMine: null, myVote: null }]),
    [{ id: 'x', isMine: true, myVote: 'like' }]);
  eq('personal: 서버가 채운 값은 안 덮는다',
    applyPersonalFields([{ id: 'x', isMine: false, myVote: null }]),
    [{ id: 'x', isMine: false, myVote: null }]);
  eq('personal: 모르는 항목은 그대로 둔다',
    applyPersonalFields([{ id: 'z', isMine: null, myVote: null }]),
    [{ id: 'z', isMine: null, myVote: null }]);
  notePersonalVote('x', null);
  eq('personal: 투표 취소도 기억한다',
    applyPersonalFields([{ id: 'x', isMine: null, myVote: null }]),
    [{ id: 'x', isMine: true, myVote: null }]);
  // 브로드캐스트 프레임(isMine이 null)은 기억 대상이 아니다 — 그걸 삼키면 내 곡 표시가 지워진다.
  notePersonalFields([{ id: 'x', isMine: null, myVote: null }]);
  eq('personal: 브로드캐스트 프레임은 기억을 덮어쓰지 않는다',
    applyPersonalFields([{ id: 'x', isMine: null, myVote: null }])[0].isMine, true);

  /* §18.2 (1) — 이미 받아 둔 뒤쪽 페이지를 재정렬마다 버리면 스크롤이 튀고 재요청이 폴링이 된다. */
  const savedTotal = queueTotal;
  const savedTail = queueTail;
  queueTotal = 4;
  queueTail = [{ id: 'c' }, { id: 'd' }];
  eq('tail: 앞 200곡 뒤로 이어 붙인다',
    keepQueueTail([{ id: 'a' }, { id: 'b' }]).map((item) => item.id), ['a', 'b', 'c', 'd']);
  queueTail = [{ id: 'b' }, { id: 'c' }];
  eq('tail: 앞쪽으로 올라온 항목은 중복으로 안 남긴다',
    keepQueueTail([{ id: 'a' }, { id: 'b' }]).map((item) => item.id), ['a', 'b', 'c']);
  queueTotal = 2;
  queueTail = [{ id: 'c' }, { id: 'd' }];
  eq('tail: 전체 곡 수보다 많이 들고 있지 않는다',
    keepQueueTail([{ id: 'a' }, { id: 'b' }]).map((item) => item.id), ['a', 'b']);
  queueTotal = savedTotal;
  queueTail = savedTail;

  /* §15.2b — 우리 차트는 숫자가 나와야 차트다. `tracks` 만 읽으면 그냥 곡 목록이다. */
  eq('chartRow: playsUser·requesters·loveFormula를 그대로 쓴다',
    chartRowExtra({ playsUser: 42, requesters: 7, loveFormula: '👍284 + ⭐52×2 = 388' }),
    ['42회 재생', '7명이 신청', '👍284 + ⭐52×2 = 388']);
  eq('chartRow: 통계가 없는 바깥 차트는 빈 배열', chartRowExtra({ title: '아무거나' }), []);

  /* §13.3 — 서버 문장의 `**굵게**` 표시가 화면에 별표로 나오면 안 된다. */
  eq('markdown: 굵게 표시를 노드로 만든다',
    markdownBold('민수님이 **아이브 - I AM** 을 담았어요').map((node) => `${node.nodeName}|${node.textContent}`),
    ['#text|민수님이 ', 'B|아이브 - I AM', '#text| 을 담았어요']);
  eq('markdown: 짝이 안 맞으면 원문 그대로',
    markdownBold('별표 ** 하나').map((node) => node.textContent), ['별표 ** 하나']);

  /* §22.5 · §24.2 — 통계 응답 모양이 어느 쪽이든 화면은 같은 숫자를 읽어야 한다. */
  const nested = normalizeStats({
    available: true,
    summary: { queuedTotal: 128, played: 96, skipped: 4, boomtta: 1, likesRecv: 340, supersRecv: 12, dislikesRecv: 2, karma: 512 },
    topRequested: [{ requested: 12 }],
    topLiked: [{ liked: 9 }],
    topLoved: [{ likesRecv: 30 }],
  });
  eq('stats: summary.queuedTotal → queued', nested.queued, 128);
  eq('stats: summary.karma → machamScore', nested.machamScore, 512);
  eq('stats: 비율 막대의 세 값', [nested.played, nested.skipped, nested.boomtta], [96, 4, 1]);
  eq('stats: topRequested.requested → count', nested.topRequested[0].count, 12);
  eq('stats: topLiked.liked → count', nested.topLiked[0].count, 9);
  eq('stats: topLoved.likesRecv → likes', nested.topLoved[0].likes, 30);
  eq('stats: 평평한 새 응답도 그대로 읽는다',
    normalizeStats({ available: true, queued: 7, machamScore: 3 }).queued, 7);
  eq('stats: available:false 를 0으로 꾸미지 않는다',
    normalizeStats({ available: false, message: '꺼져 있어요.' }).available, false);
  eq('stats: 점수를 모르면 null (0이 아니다)', normalizeStats({ available: true }).machamScore, null);

  console.info(fails.length ? `[자가검사] ${fails.length}건 실패` : '[자가검사] 전부 통과', fails);
  return { fail: fails.length, fails };
}
window.__machamSelfTest = selfTest;

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
