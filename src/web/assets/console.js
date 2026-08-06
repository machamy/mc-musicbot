/* 마참뮤직 리모컨 v3 — 서버 관리 콘솔 (/music/guilds/{guild_id}/admin)
 *
 * 기존 "설정 모달"을 대체한다. 사양서 §4.2의 "구림" 해소 기준 8개를 전부 만족시키는 것이 합격 기준이다.
 *   1) 항목마다 한 줄 설명   2) 섹션 묶음 + 섹션 목적   3) 권한 드롭다운 즉시 통과 인원
 *   4) 정렬 모드 대기열 미리보기  5) 변경분 강조 + 되돌리기 + 이탈 확인
 *   6) 숫자는 슬라이더 + 직접입력 + 단위 + 허용범위  7) 섹션 단위 부분 저장 + 토스트
 *   8) 1024px 이하에서 좌측 네비가 상단 가로 스크롤 탭으로 (네비는 어떤 폭에서도 사라지지 않는다)
 *
 * v3 추가분 (docs/REMOTE-API-V3.md)
 *   §1   권한 8개가 각각 자기 지정 역할(`ruleRoleIds`)을 갖는다. 관리자 역할(`managerRoleIds`)은 별도 카드.
 *   §4   진단에 봇의 서버/음성 참가 여부와 듣는 사람 수를 노출한다.
 *   §8.5 "순서와 재생"에 자동 재생 기준 곡(시드) 목록 + 드래그 정렬 + 삭제.
 *   말투  UI에 나가는 모든 문구는 ~해요체.
 *
 * 렌더링은 전부 클라이언트. innerHTML을 쓰지 않고 core.js의 h()로만 DOM을 만든다(XSS 차단).
 *
 * ── core.js 계약 ───────────────────────────────────────────────────────────────
 *   api(path, opts?)        : /music/api/guilds/{guildId} 기준 상대 경로. CSRF 헤더 자동.
 *                             opts.body 는 평범한 객체(내부에서 JSON 직렬화). 실패 시 throw(Error.message).
 *   h(tag, props?, ...kids) : 하이퍼스크립트. on* 핸들러 · class · dataset · aria-/data- 속성 지원.
 *   list(box, items, keyOf, render) : 키 기반 리스트 diff. 노드 재사용.
 *   store                   : 전역 상태 스토어. store.subscribe(key, fn) → 해제 함수. (여기선 연결상태만 본다)
 *   connect(guildId, opts)  : WebSocket 연결. onAny(topic, data) 로 모든 프레임을 받는다.
 *   tooltip()               : [data-tip] 전역 위임 툴팁. 인자는 무시된다.
 *   toast(msg, kind?)       : kind = 'ok' | 'warn' | 'danger' | 'info'.
 *   sheet(opts)             : { title, desc, body, actions, danger, dismissValue } → { close, result }.
 *   confirmSheet(opts)      : { title, desc, confirmText, cancelText, danger } → Promise<boolean>.
 *                             ※ confirmSheet 는 body 노드를 받지 않는다. 폼이 필요하면 sheet() 를 직접 쓴다.
 *   theme.toggle()          : 다크/라이트 토글. 현재 값은 documentElement.dataset.theme 로 읽는다.
 *   fmtAgo(utc)             : "3분 전".  fmtTime(seconds) : "3:25".
 * ──────────────────────────────────────────────────────────────────────────────
 */

import { store, connect, api, h, list, tooltip, toast, sheet, confirmSheet, theme, fmtAgo, fmtTime } from './core.js';

/* ═══════════════════════════ 부트스트랩 ═══════════════════════════ */

const M = window.MACHAM || {};
const GUILD_ID = String(M.guildId || '');
const IS_OWNER = M.tier === 'owner';
const CAN_MANAGE = IS_OWNER || M.tier === 'manager';

/* ═══════════════════════════ 상수 테이블 ═══════════════════════════ */

/** 좌측 네비 = 섹션 정의. desc 는 섹션 목적(구림 해소 #2). */
const SECTIONS = [
  { id: 'order',   icon: '🎚', label: '순서와 재생', desc: '대기열이 어떤 기준으로 줄을 서는지, 재생이 어떻게 이어지는지 정해요.' },
  { id: 'perms',   icon: '🛡', label: '권한',        desc: '어떤 사람이 어떤 조작을 할 수 있는지 기능별로 정해요. 고르면 지금 몇 명이 통과하는지 바로 보여드려요.' },
  { id: 'limits',  icon: '📐', label: '제한값',      desc: '한 사람이 얼마나 쓸 수 있는지, 기록을 얼마나 보관할지 숫자로 정해요.' },
  { id: 'users',   icon: '👥', label: '유저 관리',   desc: '리모컨을 써 본 사람 목록과 접속 상태예요. 문제를 일으킨 사람은 기능별·기간제로 정지할 수 있어요.' },
  { id: 'chat',    icon: '💬', label: '채팅과 제안', desc: '웹 채팅과 제안 게시판을 켜고 꺼요. 신고된 메시지와 들어온 제안도 여기서 처리해요.' },
  { id: 'audit',   icon: '📜', label: '활동 기록',   desc: '이 서버에서 일어난 모든 조작 기록이에요. 누가 언제 무엇을 바꿨는지 남아요.' },
  { id: 'diag',    icon: '🩺', label: '진단',        desc: '봇 연결·인텐트·버전 상태예요. 뭔가 안 될 때 여기부터 보시면 돼요.' },
];

/** 권한 규칙 5종. desc 는 드롭다운 옆 한 줄 설명(구림 해소 #1). */
const RULE_OPTIONS = [
  { value: 'guildMember',      label: '모든 멤버',      desc: '이 Discord 서버에 있는 사람이면 누구나 쓸 수 있어요.' },
  { value: 'sameVoiceChannel', label: '같은 음성 채널', desc: '봇이 들어가 있는 음성 채널에 같이 있는 사람만 쓸 수 있어요.' },
  { value: 'configuredRole',   label: '지정 역할',      desc: '아래에서 고른 역할을 가진 사람만 쓸 수 있어요. 항목마다 역할을 따로 골라요.' },
  { value: 'administrator',    label: '관리자',         desc: '서버 관리자와 봇 주인만 쓸 수 있어요.' },
  { value: 'disabled',         label: '사용 안 함',     desc: '아무도 못 써요. 기능 자체를 끄는 선택이에요.' },
];

/**
 * 권한 8종. `key` 는 설정 JSON의 필드명, `permKey` 는 v3 §1의 권한 키다.
 * `permKey` 는 `ruleRoleIds` 의 키이자 `permission-preview?key=` 로 보내는 값이라 철자가 정확해야 한다.
 */
const PERM_FIELDS = [
  { key: 'searchRule',       permKey: 'search',       label: '곡 검색·신청',       desc: '검색해서 대기열에 곡을 넣는 동작이에요. 막으면 아무도 새 곡을 못 넣어요.' },
  { key: 'voteRule',         permKey: 'vote',         label: '좋아요·슈퍼 좋아요', desc: '대기열 곡에 점수를 주는 동작이에요. 점수제일 때만 실제 순서에 영향을 줘요.' },
  { key: 'chatRule',         permKey: 'chat',         label: '채팅 쓰기',          desc: '웹 채팅에 글·반응·답장을 쓰는 동작이에요. 읽기는 멤버라면 언제나 돼요.' },
  { key: 'playbackRule',     permKey: 'playback',     label: '재생·일시정지·스킵', desc: '지금 나오는 곡을 직접 조작하는 동작이에요. 모두에게 즉시 영향이 가요.' },
  { key: 'seekRule',         permKey: 'seek',         label: '구간 이동(시크)',    desc: '진행바를 끌어 재생 위치를 옮기는 동작이에요.' },
  { key: 'volumeRule',       permKey: 'volume',       label: '볼륨 조절',          desc: '음성 채널 전체에 들리는 볼륨을 바꾸는 동작이에요. 웹에서 듣기의 개인 볼륨은 여기 해당하지 않아요.' },
  { key: 'queueEditRule',    permKey: 'queueEdit',    label: '대기열 편집',        desc: '남의 곡을 지우거나 순서를 바꾸는 동작이에요. 자기가 넣은 곡을 빼는 건 언제나 돼요.' },
  { key: 'autoplaySeedRule', permKey: 'autoplaySeed', label: '자동 재생 기준 곡',  desc: '자동 재생이 참고할 기준 곡을 등록·삭제하는 동작이에요. 등록된 목록은 누구나 볼 수 있어요.' },
];

/** 정렬 모드 3종. 각 모드에 한 문단 설명(요구사항). */
const SORT_MODES = [
  {
    value: 'score', label: '점수제',
    desc: '좋아요를 많이 받은 곡이 먼저 나와요. 오래 기다린 곡에는 대기 점수가 자동으로 붙어서 언젠가는 순서가 와요. ' +
          '분위기에 맞는 곡이 빨리 나오는 대신, 한 사람이 곡을 몰아 넣고 친구들이 눌러 주면 그 사람 곡만 계속 나올 수 있어요.',
  },
  {
    value: 'fifo', label: '시간제',
    desc: '먼저 신청한 순서 그대로 나와요. 좋아요는 표시만 되고 순서를 바꾸지 않아요. ' +
          '규칙이 가장 단순하고 예측하기 쉬운 대신, 미리 여러 곡을 넣어 둔 사람이 오래 독점해요.',
  },
  {
    value: 'fair', label: '공평제',
    desc: '사람별로 돌아가며 한 곡씩 재생해요. 미리 다섯 곡을 넣어 둬도 첫 바퀴에서는 한 곡만 나가고, ' +
          '늦게 들어온 사람도 다음 차례에 바로 들어와요. 사람이 많고 신청이 몰릴 때 가장 덜 싸워요.',
  },
];

const REPEAT_MODES = [
  { value: 'off',   label: '반복 없음', desc: '대기열이 비면 재생이 멈춰요.' },
  { value: 'track', label: '한 곡 반복', desc: '지금 곡을 계속 다시 틀어요. 대기열은 그대로 기다려요.' },
  { value: 'queue', label: '전체 반복', desc: '대기열 끝까지 가면 처음으로 돌아가요.' },
];

/** 숫자 항목 정의 — 슬라이더 + 직접입력 + 단위 + 허용범위 + 한 줄 설명(구림 해소 #1, #6). */
const NUM_SPECS = {
  minVolume:          { label: '최소 볼륨',      min: 0,  max: 100,    step: 5,  unit: '%',  desc: '이 아래로는 볼륨을 못 내려요. 0이면 음소거까지 허용해요.' },
  maxVolume:          { label: '최대 볼륨',      min: 10, max: 200,    step: 5,  unit: '%',  desc: '멤버가 올릴 수 있는 볼륨 상한이에요. 관리자도 이 값을 넘기지 못해요.' },
  defaultVolume:      { label: '기본 볼륨',      min: 0,  max: 200,    step: 5,  unit: '%',  desc: '봇이 음성 채널에 새로 들어갈 때 시작하는 볼륨이에요.' },
  maxQueuePerUser:    { label: '1인 대기열 수',  min: 1,  max: 100,    step: 1,  unit: '곡', desc: '한 사람이 동시에 대기열에 넣어 둘 수 있는 곡 수예요. 작을수록 골고루 돌아가요.' },
  maxQueuePerGuild:   { label: '서버 대기열 수', min: 1,  max: 1000,   step: 10, unit: '곡', desc: '서버 전체 대기열 상한이에요. 넘으면 새 신청이 거절돼요.' },
  maxTrackSeconds:    { label: '곡 최대 길이',   min: 60, max: 86400,  step: 60, unit: '초', desc: '이보다 긴 곡은 신청할 수 없어요. 몇 시간짜리 라이브 통짜 등록을 막아요.', pretty: prettySeconds },
  auditRetentionDays: { label: '로그 보관일',    min: 1,  max: 3650,   step: 1,  unit: '일', desc: '활동 기록을 며칠 보관할지 정해요. 지난 기록은 하루 한 번 정리돼요.' },
  chatRetentionDays:  { label: '채팅 보관일',    min: 1,  max: 365,    step: 1,  unit: '일', desc: '웹 채팅을 며칠 보관할지 정해요. 기본은 30일이에요.' },
};

/** 섹션이 소유하는 설정 키 — 부분 저장(구림 해소 #7)의 단위. 한 키는 정확히 한 섹션에만 속한다. */
const SECTION_KEYS = {
  order:  ['sortMode', 'autoBgmEnabled', 'repeatMode', 'defaultVolume'],
  perms:  PERM_FIELDS.map((field) => field.key).concat(['ruleRoleIds', 'managerRoleIds']),
  limits: ['minVolume', 'maxVolume', 'maxQueuePerUser', 'maxQueuePerGuild', 'maxTrackSeconds', 'auditRetentionDays', 'chatRetentionDays'],
  chat:   ['chatEnabled', 'suggestionEnabled'],
};

/** 정지 범위 · 기간 (사양서 결정 #14). */
const SUSPEND_SCOPES = [
  { value: 'all',   label: '전체',    desc: '리모컨의 모든 조작을 막아요. 보기만 돼요.' },
  { value: 'chat',  label: '채팅만',  desc: '채팅·반응·답장만 막아요. 곡 신청은 계속 돼요.' },
  { value: 'queue', label: '신청만',  desc: '곡 신청·좋아요만 막아요. 채팅은 계속 돼요.' },
];
const SUSPEND_DURATIONS = [
  { minutes: 5,    label: '5분' },
  { minutes: 30,   label: '30분' },
  { minutes: 180,  label: '3시간' },
  { minutes: null, label: '무기한' },
];

/** 제안 상태 (스키마 §6.2 remote_suggestions.status). */
const SUGGESTION_STATUS = [
  { value: 'open',      label: '접수됨',   chip: '' },
  { value: 'reviewing', label: '검토중',   chip: 'chip--info' },
  { value: 'planned',   label: '반영예정', chip: 'chip--accent' },
  { value: 'done',      label: '반영됨',   chip: 'chip--ok' },
  { value: 'declined',  label: '보류',     chip: 'chip--warn' },
];

/** 접속 상태 우선순위 배지 (사양서 §2.2 + v3 §4의 "다른 채널에 있어요"). */
const PRESENCE_LABEL = {
  listening:    ['🎧', '듣는 중',      'dot--listening'],
  inOtherVoice: ['🎙', '다른 채널',    'dot--idle'],
  viewing:      ['🖥', '보는 중',      'dot--viewing'],
  online:       ['🟢', '온라인',       'dot--online'],
  idle:         ['🌙', '자리비움',     'dot--idle'],
  dnd:          ['⛔', '다른 용무',    'dot--dnd'],
  offline:      ['⚪', '오프라인',     'dot--offline'],
};

/** 자동 재생 기준 곡 상한. 서버가 `max` 를 주면 그 값이 이긴다 (v3 §8.1). */
const SEED_MAX_FALLBACK = 10;

/* ═══════════════════════════ 상태 ═══════════════════════════ */

const S = {
  activeSection: 'order',
  saved: null,      // 서버가 준 마지막 저장본 (baseline)
  draft: null,      // 편집 중인 사본
  roles: [],        // 길드 역할 목록
  saving: false,
  /** 섹션별 지연 로드 데이터 */
  queuePreview: { mode: null, data: null, loading: false },
  permPreview: {},  // 설정 키 → { passCount, memberCount, ... }
  seeds: { items: null, max: SEED_MAX_FALLBACK, canEdit: false, error: null, loading: false },
  participants: null,
  suspensions: null,
  reports: null,
  suggestions: null,
  audit: { items: [], cursor: null, done: false, loading: false, query: '' },
  diag: null,
};

/** 섹션 루트 DOM. 한 번에 한 섹션만 DOM에 존재한다(저장 버튼 testid 중복 방지 + 렌더 비용 절감). */
let sectionBox = null;
let navBox = null;
let dirtyBadge = null;

/* ═══════════════════════════ 유틸 ═══════════════════════════ */

const clone = (value) => JSON.parse(JSON.stringify(value === undefined ? null : value));
const same = (a, b) => JSON.stringify(a === undefined ? null : a) === JSON.stringify(b === undefined ? null : b);

/** 초 → "1시간 30분" 같은 사람이 읽는 길이. */
function prettySeconds(total) {
  const seconds = Math.max(0, Math.round(Number(total) || 0));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (hours && minutes) return `${hours}시간 ${minutes}분`;
  if (hours) return `${hours}시간`;
  if (minutes) return `${minutes}분`;
  return `${seconds}초`;
}

/** 만료 UTC → "2시간 14분 남았어요". null 이면 무기한. */
function untilLabel(expiresUtc) {
  if (!expiresUtc) return '무기한';
  const left = Date.parse(expiresUtc) - Date.now();
  if (!Number.isFinite(left) || left <= 0) return '곧 풀려요';
  const minutes = Math.floor(left / 60000);
  if (minutes < 60) return `${Math.max(1, minutes)}분 남았어요`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}시간 ${minutes % 60}분 남았어요`;
  return `${Math.floor(hours / 24)}일 ${hours % 24}시간 남았어요`;
}

function scopeLabel(scope) {
  const found = SUSPEND_SCOPES.find((item) => item.value === scope);
  return found ? found.label : scope;
}

function ruleDesc(value) {
  const found = RULE_OPTIONS.find((item) => item.value === value);
  return found ? found.desc : '';
}

/** 역할 ID 목록 → "@DJ, @스태프". 이름을 모르면 ID를 그대로 보여준다(거짓말보다 낫다). */
function roleNames(ids) {
  return (ids || []).map((id) => {
    const found = S.roles.find((role) => String(role.id) === String(id));
    return found ? `@${found.name}` : `역할 ${id}`;
  });
}

/** list()는 core.js 계약. 여기 한 곳만 거치게 해서 시그니처가 달라져도 수정 지점을 하나로 묶는다. */
function renderList(box, items, keyOf, render) {
  list(box, items, keyOf, render);
}

/**
 * 서버 응답 → 콘솔이 다루는 형태.
 * v3 §1의 마이그레이션(비어 있으면 `configuredRoleIds` 폴백)을 클라이언트에서도 한 번 더 태운다.
 * 서버가 아직 v2 형태로 답해도 화면이 통짜 역할을 8개 항목에 그대로 보여 주므로 동작이 조용히 바뀌지 않는다.
 * force 는 부팅 시 baseline 을 만들 때만 true — 부분 저장 응답에는 권한 키가 없을 수 있다.
 */
function normalizeSettings(raw, force) {
  const next = Object.assign({}, raw || {});
  const touchesPerms = force
    || 'ruleRoleIds' in next || 'managerRoleIds' in next || 'configuredRoleIds' in next;
  if (!touchesPerms) return next;

  const legacy = (next.configuredRoleIds || []).map(String);
  const source = next.ruleRoleIds && typeof next.ruleRoleIds === 'object' ? next.ruleRoleIds : {};
  const anySet = Object.values(source).some((ids) => Array.isArray(ids) && ids.length);

  const map = {};
  PERM_FIELDS.forEach((field) => {
    const ids = source[field.permKey];
    if (Array.isArray(ids) && ids.length) map[field.permKey] = ids.map(String);
    else map[field.permKey] = anySet ? [] : legacy.slice();
  });
  next.ruleRoleIds = map;

  next.managerRoleIds = Array.isArray(next.managerRoleIds) && next.managerRoleIds.length
    ? next.managerRoleIds.map(String)
    : legacy.slice();

  // v2 필드는 더 이상 주고받지 않는다 (v3 §1).
  delete next.configuredRoleIds;
  if (next.autoplaySeedRule === undefined) next.autoplaySeedRule = 'administrator';
  return next;
}

/* ═══════════════════════════ 변경 추적 ═══════════════════════════ */

/** 지금 섹션에서 baseline과 달라진 키들. */
function dirtyKeys(sectionId) {
  const keys = SECTION_KEYS[sectionId] || [];
  return keys.filter((key) => !same(S.draft[key], S.saved[key]));
}

/** 전체 섹션 통틀어 변경이 있는지. beforeunload / 네비 가드가 본다. */
function anyDirty() {
  return Object.keys(SECTION_KEYS).some((id) => dirtyKeys(id).length > 0);
}

/** 변경된 항목만 강조 + 섹션 푸터 갱신 (구림 해소 #5). */
function refreshDirty() {
  if (!sectionBox) return;
  const changed = new Set(dirtyKeys(S.activeSection));
  sectionBox.querySelectorAll('[data-field]').forEach((node) => {
    node.classList.toggle('is-changed', changed.has(node.dataset.field));
  });
  const foot = sectionBox.querySelector('.sec__foot');
  if (foot) {
    const count = changed.size;
    foot.classList.toggle('is-active', count > 0);
    const label = foot.querySelector('.sec__footnote');
    if (label) label.textContent = count ? `바꾼 항목 ${count}개예요` : '아직 바꾼 항목이 없어요';
    foot.querySelectorAll('button').forEach((button) => { button.disabled = !count || S.saving; });
  }
  if (dirtyBadge) {
    const total = Object.keys(SECTION_KEYS).reduce((sum, id) => sum + dirtyKeys(id).length, 0);
    dirtyBadge.hidden = total === 0;
    dirtyBadge.textContent = `저장 안 한 변경 ${total}개`;
  }
  // 네비에도 섹션별 변경 점을 찍는다.
  if (navBox) {
    navBox.querySelectorAll('.nav__item').forEach((node) => {
      const count = (SECTION_KEYS[node.dataset.section] || []).length ? dirtyKeys(node.dataset.section).length : 0;
      node.classList.toggle('has-change', count > 0);
    });
  }
}

/** draft 갱신 진입점. 모든 컨트롤이 여기만 부른다. */
function setValue(key, value) {
  S.draft[key] = value;
  refreshDirty();
  validate();
}

/** 항목 하나만 원래대로. */
function revertField(key) {
  S.draft[key] = clone(S.saved[key]);
  renderSection(S.activeSection);
}

/* ── 교차 검증 ── 슬라이더끼리 모순되는 값을 저장 전에 잡는다. */
function validate() {
  const errors = {};
  if (Number(S.draft.minVolume) > Number(S.draft.maxVolume)) {
    errors.minVolume = '최소 볼륨이 최대 볼륨보다 커요. 두 값을 뒤집어 주세요.';
  }
  if (Number(S.draft.defaultVolume) < Number(S.draft.minVolume) || Number(S.draft.defaultVolume) > Number(S.draft.maxVolume)) {
    errors.defaultVolume = `기본 볼륨은 최소~최대(${S.draft.minVolume}~${S.draft.maxVolume}%) 안에 있어야 해요.`;
  }
  PERM_FIELDS.forEach((field) => {
    if (S.draft[field.key] !== 'configuredRole') return;
    const ids = (S.draft.ruleRoleIds || {})[field.permKey] || [];
    if (!ids.length) errors[field.key] = '"지정 역할"인데 고른 역할이 없어요. 이대로 저장하면 아무도 못 써요.';
  });
  if (!sectionBox) return errors;
  sectionBox.querySelectorAll('[data-field]').forEach((node) => {
    const message = errors[node.dataset.field];
    const slot = node.querySelector('.fld__err');
    node.classList.toggle('is-invalid', Boolean(message));
    if (slot) {
      slot.hidden = !message;
      slot.textContent = message || '';
    }
  });
  const foot = sectionBox.querySelector('.sec__foot');
  if (foot) {
    const blocked = (SECTION_KEYS[S.activeSection] || []).some((key) => errors[key]);
    const save = foot.querySelector('[data-testid="settings-save"]');
    if (save && blocked) save.disabled = true;
  }
  return errors;
}

/* ═══════════════════════════ 필드 위젯 ═══════════════════════════ */

/** 모든 필드의 공통 껍데기 — 라벨 · 변경 배지 · 항목별 되돌리기 · 설명 · 오류 슬롯. */
function fieldShell(key, label, desc, control, extra) {
  return h('div', { class: 'fld', 'data-field': key },
    h('div', { class: 'fld__head' },
      h('span', { class: 'fld__label' }, label),
      h('span', { class: 'fld__badge' }, '바꿨어요'),
      h('button', {
        class: 'fld__undo', type: 'button', 'data-tip': '이 항목만 저장 전 값으로 되돌려요',
        'aria-label': `${label} 되돌리기`,
        onclick: () => revertField(key),
      }, '↺'),
    ),
    h('div', { class: 'fld__ctl' }, control),
    desc ? h('p', { class: 'fld__desc' }, desc) : null,
    h('p', { class: 'fld__err', hidden: true }),
    extra || null,
  );
}

/** 세그먼트 선택 — 정렬 모드처럼 선택지가 적고 설명이 긴 경우. 방향키 이동 지원. */
function segmentControl(key, options, onPick) {
  const box = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': '선택' });
  options.forEach((option, index) => {
    const selected = S.draft[key] === option.value;
    box.append(h('button', {
      class: 'seg__btn' + (selected ? ' is-on' : ''),
      type: 'button', role: 'radio',
      'aria-checked': selected ? 'true' : 'false',
      tabindex: selected ? '0' : '-1',
      'data-value': option.value,
      onclick: () => { setValue(key, option.value); onPick && onPick(option.value); renderSection(S.activeSection); },
      onkeydown: (event) => {
        const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
        if (!step) return;
        event.preventDefault();
        const next = options[(index + step + options.length) % options.length];
        setValue(key, next.value);
        onPick && onPick(next.value);
        renderSection(S.activeSection);
      },
    }, option.label));
  });
  return box;
}

/** 드롭다운. */
function selectControl(key, options, onPick) {
  const select = h('select', {
    class: 'field', 'aria-label': '선택',
    onchange: (event) => { setValue(key, event.target.value); onPick && onPick(event.target.value); },
  });
  options.forEach((option) => {
    select.append(h('option', { value: option.value, selected: S.draft[key] === option.value }, option.label));
  });
  return select;
}

/** 체크 스위치. */
function toggleControl(key, onText, offText) {
  const on = Boolean(S.draft[key]);
  const button = h('button', {
    class: 'sw' + (on ? ' is-on' : ''), type: 'button', role: 'switch',
    'aria-checked': on ? 'true' : 'false',
    onclick: () => { setValue(key, !S.draft[key]); renderSection(S.activeSection); },
  }, h('span', { class: 'sw__knob' }));
  return h('div', { class: 'sw__row' }, button, h('span', { class: 'sw__text' }, on ? onText : offText));
}

/**
 * 숫자 — 슬라이더 + 직접입력 + 단위 + 허용범위 (구림 해소 #6).
 * bounds 로 min/max 를 런타임에 덮을 수 있다(기본 볼륨은 최소~최대 볼륨을 따라간다).
 */
function numberField(key, bounds) {
  const spec = NUM_SPECS[key];
  const min = bounds && bounds.min != null ? bounds.min : spec.min;
  const max = bounds && bounds.max != null ? bounds.max : spec.max;
  const value = Number(S.draft[key]);

  const readout = h('span', { class: 'num__pretty' }, spec.pretty ? spec.pretty(value) : '');
  const number = h('input', {
    class: 'field num__input', type: 'number', inputmode: 'numeric',
    min: String(spec.min), max: String(spec.max), step: String(spec.step), value: String(value),
    'aria-label': spec.label,
  });
  const range = h('input', {
    class: 'num__range', type: 'range',
    min: String(min), max: String(max), step: String(spec.step), value: String(value),
    'aria-label': `${spec.label} 슬라이더`, tabindex: '-1',
  });

  const apply = (raw, syncRange, syncNumber) => {
    let next = Number(raw);
    if (!Number.isFinite(next)) next = S.saved[key];
    next = Math.min(spec.max, Math.max(spec.min, Math.round(next)));
    if (syncRange) range.value = String(next);
    if (syncNumber) number.value = String(next);
    readout.textContent = spec.pretty ? spec.pretty(next) : '';
    setValue(key, next);
  };
  range.addEventListener('input', (event) => apply(event.target.value, false, true));
  number.addEventListener('input', (event) => apply(event.target.value, true, false));
  number.addEventListener('blur', (event) => apply(event.target.value, true, true));

  const control = h('div', { class: 'num' },
    range,
    h('div', { class: 'num__side' },
      number,
      h('span', { class: 'num__unit' }, spec.unit),
    ),
    h('div', { class: 'num__meta' },
      h('span', { class: 'num__range-label' }, `${spec.min}~${spec.max}${spec.unit} 안에서 고를 수 있어요`),
      readout,
    ),
  );
  return fieldShell(key, spec.label, spec.desc, control);
}

/* ═══════════════════════════ 섹션 1 · 순서와 재생 ═══════════════════════════ */

function sectionOrder() {
  const mode = S.draft.sortMode;
  const modeSpec = SORT_MODES.find((item) => item.value === mode) || SORT_MODES[0];

  const previewBox = h('div', { class: 'prev' });
  const modeField = fieldShell('sortMode', '대기열 정렬 방식',
    null,
    segmentControl('sortMode', SORT_MODES, (value) => loadQueuePreview(value)),
    h('div', { class: 'sortnote' },
      h('p', { class: 'sortnote__body' }, modeSpec.desc),
      previewBox,
    ),
  );

  const seedsBox = h('div', { class: 'card seeds' });

  const body = h('div', { class: 'sec__body' },
    modeField,
    fieldShell('autoBgmEnabled', '자동 BGM',
      '대기열이 비면 아래 기준 곡과 비슷한 곡을 알아서 이어 틀어요. 끄면 조용해져요.',
      toggleControl('autoBgmEnabled', '대기열이 비면 알아서 이어 틀어요', '대기열이 비면 재생을 멈춰요')),
    fieldShell('repeatMode', '반복',
      REPEAT_MODES.find((item) => item.value === S.draft.repeatMode)?.desc || null,
      segmentControl('repeatMode', REPEAT_MODES)),
    numberField('defaultVolume', { min: Number(S.draft.minVolume), max: Number(S.draft.maxVolume) }),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '📻 자동 재생 기준 곡'),
      h('p', { class: 'grp__desc' },
        '자동 재생이 이 곡들과 비슷한 곡을 찾아와요. 여러 곡을 넣으면 돌아가며 참고하니 한 장르로 쏠리지 않아요. ' +
        '여기서 바꾼 건 저장 버튼 없이 바로 반영돼요.'),
      seedsBox,
    ),
  );

  renderQueuePreview(previewBox);
  loadQueuePreview(mode);
  paintSeeds(seedsBox);
  if (!S.seeds.items && !S.seeds.loading) loadSeeds().then(() => repaintSeeds());
  return body;
}

/** 지금 대기열에 그 모드를 적용하면 순서가 어떻게 되는지 (구림 해소 #4). */
async function loadQueuePreview(mode) {
  if (S.queuePreview.mode === mode && S.queuePreview.data) return;
  S.queuePreview = { mode, data: null, loading: true };
  const box = sectionBox && sectionBox.querySelector('.prev');
  if (box) renderQueuePreview(box);
  try {
    const data = await api(`/admin/queue-preview?mode=${encodeURIComponent(mode)}`);
    if (S.queuePreview.mode !== mode) return;   // 그 사이 모드가 또 바뀌었으면 버린다
    S.queuePreview = { mode, data, loading: false };
  } catch (error) {
    if (S.queuePreview.mode !== mode) return;
    S.queuePreview = { mode, data: { error: error.message }, loading: false };
  }
  const target = sectionBox && sectionBox.querySelector('.prev');
  if (target) renderQueuePreview(target);
}

function renderQueuePreview(box) {
  box.replaceChildren();
  box.append(h('div', { class: 'prev__head' },
    h('strong', null, '지금 대기열에 적용하면'),
    h('span', { class: 'hint' }, '저장해야 실제로 바뀌어요'),
  ));

  const preview = S.queuePreview;
  if (preview.loading) {
    box.append(h('div', { class: 'prev__rows' },
      h('div', { class: 'skel', style: 'height:34px' }),
      h('div', { class: 'skel', style: 'height:34px' }),
      h('div', { class: 'skel', style: 'height:34px' }),
    ));
    return;
  }
  if (!preview.data) return;
  if (preview.data.error) {
    box.append(h('p', { class: 'hint' }, `미리보기를 못 불러왔어요 — ${preview.data.error}`));
    return;
  }
  const items = preview.data.items || [];
  if (!items.length) {
    box.append(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '🎵'),
      h('div', { class: 'empty__title' }, '대기열이 비어 있어요'),
      h('div', { class: 'empty__desc' }, '곡이 쌓이면 여기서 순서가 어떻게 바뀌는지 미리 보실 수 있어요.'),
    ));
    return;
  }

  const rows = h('ol', { class: 'prev__rows' });
  items.slice(0, 10).forEach((item) => {
    const delta = Number(item.delta || 0);   // 양수 = 지금보다 위로 올라감
    const arrow = delta > 0 ? '↑' : delta < 0 ? '↓' : '·';
    const tone = delta > 0 ? 'is-up' : delta < 0 ? 'is-down' : 'is-flat';
    rows.append(h('li', { class: 'prev__row' },
      h('span', { class: 'prev__rank' }, String(item.previewPosition)),
      h('span', { class: 'prev__title mq' }, h('span', { class: 'mq__i' }, item.title || '(제목이 없어요)')),
      h('span', { class: 'prev__who' }, item.roundLabel || item.requestedBy || ''),
      h('span', { class: `prev__delta ${tone}` },
        delta === 0 ? '그대로예요' : `지금 ${item.currentPosition}위 ${arrow}`),
    ));
  });
  box.append(rows);
  if (items.length > 10) {
    box.append(h('p', { class: 'hint' }, `아래로 ${items.length - 10}곡 더 있어요.`));
  }
}

/* ── 자동 재생 기준 곡 (v3 §8.5) ── 저장 버튼을 거치지 않고 바로 서버에 반영한다. */

/** 드래그 중인 시드의 cacheKey. 드롭 대상이 이 값을 보고 자리를 계산한다. */
let seedDragKey = null;

async function loadSeeds() {
  if (S.seeds.loading) return;
  S.seeds.loading = true;
  try {
    const data = await api('/autoplay/seeds');
    S.seeds = {
      items: (data.seeds || []).slice(),
      max: Number(data.max) || SEED_MAX_FALLBACK,
      canEdit: data.canEdit !== false,
      error: null,
      loading: false,
    };
  } catch (error) {
    S.seeds = { items: [], max: S.seeds.max, canEdit: false, error: error.message, loading: false };
  }
}

/** 지금 화면에 붙어 있는 시드 목록 컨테이너를 찾아 다시 그린다. */
function repaintSeeds() {
  const box = sectionBox && sectionBox.querySelector('.seeds');
  if (box) paintSeeds(box);
}

function paintSeeds(box) {
  const state = S.seeds;
  if (state.loading || state.items === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:76px;margin:12px' }));
    return;
  }
  if (state.error) {
    box.replaceChildren(h('p', { class: 'hint', style: 'padding:12px' },
      `기준 곡을 못 불러왔어요 — ${state.error}`));
    return;
  }

  const items = state.items;
  const max = state.max || SEED_MAX_FALLBACK;
  const head = h('div', { class: 'seeds__head' },
    h('span', { class: 'seeds__count' + (items.length >= max ? ' is-full' : '') },
      `${items.length} / ${max}곡`),
    h('span', { class: 'hint' },
      items.length >= max
        ? '자리가 다 찼어요. 새로 넣으려면 하나를 빼 주세요.'
        : `${max}곡까지 넣을 수 있어요.`),
  );

  if (!items.length) {
    box.replaceChildren(head, h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '📻'),
      h('div', { class: 'empty__title' }, '기준 곡이 없어서 최근에 튼 곡을 참고해요'),
      h('div', { class: 'empty__desc' },
        '리모컨의 대기열이나 검색 결과에서 "📻 기준으로 삼기"를 누르면 여기에 쌓여요. ' +
        '한 곡만 넣어도 자동 재생이 그 곡을 기준으로 골라요.'),
    ));
    return;
  }

  const rows = h('ol', { class: 'seeds__rows' });
  items.forEach((item, index) => rows.append(seedRow(item, index, items.length, state.canEdit)));

  const notes = [];
  if (!state.canEdit) {
    notes.push(h('p', { class: 'hint' },
      '이 서버에서는 기준 곡을 바꿀 권한이 없어서 목록만 보여 드려요. 권한 섹션의 "자동 재생 기준 곡"에서 바꿀 수 있어요.'));
  }
  if (!S.draft.autoBgmEnabled) {
    notes.push(h('p', { class: 'hint' },
      '지금은 자동 BGM이 꺼져 있어서 기준 곡이 쓰이지 않아요. 위에서 켜 주시면 바로 참고하기 시작해요.'));
  }
  box.replaceChildren(head, rows, ...notes);
  tooltip(box);
}

function seedRow(item, index, total, canEdit) {
  const track = item.track || {};
  const seconds = Number(track.durationSeconds || 0);
  const sub = [
    track.artist || '아티스트 미상',
    seconds > 0 ? fmtTime(seconds) : null,
    item.addedByDisplayName ? `${item.addedByDisplayName}이 넣었어요` : null,
    item.addedUtc ? fmtAgo(item.addedUtc) : null,
  ].filter(Boolean).join(' · ');

  const row = h('li', {
    class: 'seed',
    'data-key': String(item.cacheKey),
    draggable: canEdit ? 'true' : null,
  },
    h('span', {
      class: 'seed__handle',
      'aria-hidden': 'true',
      'data-tip': canEdit ? '끌어서 순서를 바꿔요' : '순서를 바꿀 권한이 없어요',
    }, '⠿'),
    h('span', { class: 'seed__idx' }, String(index + 1)),
    h('div', { class: 'seed__main' },
      h('div', { class: 'seed__title mq' }, h('span', { class: 'mq__i' }, track.title || '(제목이 없어요)')),
      h('div', { class: 'seed__sub' }, sub),
    ),
    canEdit ? h('div', { class: 'seed__acts' },
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button',
        disabled: index === 0,
        'data-tip': '한 칸 위로 올려요',
        'aria-label': `${track.title || '이 곡'} 한 칸 위로`,
        onclick: () => moveSeedBy(index, -1),
      }, '↑'),
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button',
        disabled: index === total - 1,
        'data-tip': '한 칸 아래로 내려요',
        'aria-label': `${track.title || '이 곡'} 한 칸 아래로`,
        onclick: () => moveSeedBy(index, 1),
      }, '↓'),
      h('button', {
        class: 'btn btn--sm btn--danger', type: 'button',
        'data-tip': '기준 곡에서 빼요',
        onclick: () => removeSeed(item),
      }, '빼기'),
    ) : null,
  );

  if (!canEdit) return row;

  row.addEventListener('dragstart', (event) => {
    seedDragKey = String(item.cacheKey);
    row.classList.add('is-drag');
    if (event.dataTransfer) {
      event.dataTransfer.effectAllowed = 'move';
      try { event.dataTransfer.setData('text/plain', seedDragKey); } catch { /* Firefox 예외 무시 */ }
    }
  });
  row.addEventListener('dragend', () => {
    seedDragKey = null;
    const box = sectionBox && sectionBox.querySelector('.seeds');
    if (box) box.querySelectorAll('.seed').forEach((node) => node.classList.remove('is-drag', 'is-over'));
  });
  row.addEventListener('dragover', (event) => {
    if (!seedDragKey || seedDragKey === String(item.cacheKey)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    row.classList.add('is-over');
  });
  row.addEventListener('dragleave', () => row.classList.remove('is-over'));
  row.addEventListener('drop', (event) => {
    event.preventDefault();
    row.classList.remove('is-over');
    const moving = seedDragKey;
    seedDragKey = null;
    if (moving) moveSeedTo(moving, index);
  });
  return row;
}

/** 드래그한 곡을 대상 자리로 옮긴다. */
function moveSeedTo(cacheKey, toIndex) {
  const items = (S.seeds.items || []).slice();
  const from = items.findIndex((item) => String(item.cacheKey) === String(cacheKey));
  if (from < 0 || from === toIndex) return;
  const [moved] = items.splice(from, 1);
  items.splice(toIndex, 0, moved);
  commitSeedOrder(items);
}

/** 위/아래 버튼 — 키보드만으로도 순서를 바꿀 수 있어야 한다. */
function moveSeedBy(index, step) {
  const items = (S.seeds.items || []).slice();
  const next = index + step;
  if (next < 0 || next >= items.length) return;
  const [moved] = items.splice(index, 1);
  items.splice(next, 0, moved);
  commitSeedOrder(items);
}

/** 낙관적 반영 후 저장. 실패하면 서버 값으로 되돌린다 — 화면이 거짓말하면 안 된다. */
async function commitSeedOrder(items) {
  const before = (S.seeds.items || []).slice();
  S.seeds.items = items;
  repaintSeeds();
  try {
    await api('/autoplay/seeds/reorder', {
      method: 'POST',
      body: { cacheKeys: items.map((item) => String(item.cacheKey)) },
    });
  } catch (error) {
    S.seeds.items = before;
    repaintSeeds();
    toast(`순서를 못 바꿨어요 — ${error.message}`, 'danger');
  }
}

async function removeSeed(item) {
  const title = (item.track && item.track.title) || '이 곡';
  const ok = await confirmSheet({
    title: '기준 곡에서 빼기',
    desc: `"${title}"을(를) 자동 재생 기준에서 빼요. 대기열에 있는 곡은 그대로 남아요.`,
    confirmText: '빼기',
    cancelText: '그냥 둘게요',
    danger: true,
  });
  if (!ok) return;
  try {
    await api('/autoplay/seeds/remove', { method: 'POST', body: { cacheKey: String(item.cacheKey) } });
    S.seeds.items = (S.seeds.items || []).filter((entry) => String(entry.cacheKey) !== String(item.cacheKey));
    repaintSeeds();
    toast('기준 곡에서 뺐어요.', 'ok');
  } catch (error) {
    toast(`빼지 못했어요 — ${error.message}`, 'danger');
  }
}

/* ═══════════════════════════ 섹션 2 · 권한 ═══════════════════════════ */

function sectionPerms() {
  const body = h('div', { class: 'sec__body' });

  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '기능별 권한'),
    h('p', { class: 'grp__desc' },
      '각 동작을 누가 할 수 있는지 따로 정해요. "지정 역할"을 고른 항목에만 역할 선택기가 펼쳐지고, ' +
      '고른 역할은 그 항목에만 적용돼요. 검색용으로 @DJ를 줬다고 볼륨까지 열리지 않아요.'),
  );

  PERM_FIELDS.forEach((field) => group.append(permField(field)));
  body.append(group);

  // 관리자 지정 역할 — 기능 권한과 완전히 분리한다 (v3 §1).
  const managerIds = S.draft.managerRoleIds || [];
  body.append(h('div', { class: 'grp grp--manager' },
    h('h3', { class: 'grp__title' }, '🛡 관리자 지정 역할'),
    h('p', { class: 'grp__desc' },
      '여기서 고른 역할을 가진 사람은 서버 관리자로 대우해요. 이 관리 콘솔에 들어오고, 설정 변경·유저 정지까지 할 수 있어요.'),
    h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null,
        '위쪽 "지정 역할"과는 완전히 별개예요. 예전에는 이 둘이 한 목록이라 검색 권한으로 역할을 하나 줬더니 ' +
        '그 사람이 관리자가 되는 일이 있었어요. 이제는 관리자로 삼을 역할만 여기에 넣어 주세요.'),
    ),
    fieldShell('managerRoleIds', '관리자로 대우할 역할',
      '비워 두면 Discord의 "관리자"·"서버 관리" 권한을 가진 사람과 봇 주인만 관리자예요.',
      roleChecklist(
        () => S.draft.managerRoleIds || [],
        (next) => setValue('managerRoleIds', next),
      ),
      managerIds.length
        ? h('p', { class: 'hint' }, `지금 관리자 대우: ${roleNames(managerIds).join(', ')}`)
        : h('p', { class: 'hint' }, '지금은 따로 지정한 역할이 없어요.'),
    ),
  ));

  return body;
}

/** 권한 한 줄 — 규칙 드롭다운 + 그 항목만의 역할 선택기 + 통과 인원 미리보기. */
function permField(field) {
  const value = S.draft[field.key];
  const preview = h('div', { class: 'permprev' });

  const warn = h('div', { class: 'warnbox', hidden: value !== 'disabled' },
    h('span', null, '⚠'),
    h('span', null, '"사용 안 함"은 예외가 없어요. 서버 관리자와 봇 주인도 이 기능을 쓸 수 없어요. 기능을 완전히 끄는 선택이에요.'),
  );

  const rolesNote = h('p', { class: 'permroles__note' });
  const rolesBox = h('div', { class: 'permroles', hidden: value !== 'configuredRole' },
    h('div', { class: 'permroles__head' },
      h('span', { class: 'permroles__title' }, `${field.label}에 쓸 역할`),
      h('span', { class: 'hint' }, '이 항목에만 적용돼요'),
    ),
    roleChecklist(
      () => (S.draft.ruleRoleIds || {})[field.permKey] || [],
      (next) => setRuleRoles(field.permKey, next),
      () => { paintRolesNote(rolesNote, field); loadPermPreview(field, S.draft[field.key], preview); },
    ),
    rolesNote,
  );
  paintRolesNote(rolesNote, field);

  const control = h('div', { class: 'permrow' },
    selectControl(field.key, RULE_OPTIONS, (next) => {
      warn.hidden = next !== 'disabled';
      rolesBox.hidden = next !== 'configuredRole';
      const note = control.querySelector('[data-rule-note]');
      if (note) note.textContent = ruleDesc(next);
      paintRolesNote(rolesNote, field);
      loadPermPreview(field, next, preview);
    }),
    h('span', { class: 'permrow__note', 'data-rule-note': field.key }, ruleDesc(value)),
  );

  const shell = fieldShell(field.key, field.label, field.desc, control,
    h('div', null, rolesBox, preview, warn));
  loadPermPreview(field, value, preview);
  return shell;
}

/** 지정 역할 목록 아래 한 줄 — 고른 게 없으면 그대로 말해 준다. */
function paintRolesNote(node, field) {
  const ids = (S.draft.ruleRoleIds || {})[field.permKey] || [];
  node.classList.toggle('is-warn', ids.length === 0);
  node.textContent = ids.length
    ? `고른 역할: ${roleNames(ids).join(', ')}`
    : '아직 고른 역할이 없어요. 이대로 저장하면 관리자 말고는 아무도 못 써요.';
}

/** 권한별 역할 목록을 draft 에 반영한다. 맵을 통째로 갈아 끼워야 변경 감지가 걸린다. */
function setRuleRoles(permKey, ids) {
  const map = Object.assign({}, S.draft.ruleRoleIds || {});
  map[permKey] = ids;
  setValue('ruleRoleIds', map);
}

/**
 * 역할 다중 선택 칩.
 * read/write 로 어느 값에 붙을지 정한다 — 권한별 역할과 관리자 역할이 같은 위젯을 쓴다.
 * 칩 하나를 눌렀다고 섹션 전체를 다시 그리면 8개 미리보기가 전부 다시 날아가므로 제자리에서 토글한다.
 */
function roleChecklist(read, write, onChange) {
  const box = h('div', { class: 'roles' });
  if (!S.roles.length) {
    box.append(h('p', { class: 'hint' },
      '역할 목록이 비어 있어요. 봇에게 역할을 볼 권한이 없거나 아직 불러오는 중이에요.'));
    return box;
  }
  const picked = new Set((read() || []).map(String));
  S.roles.forEach((role) => {
    const id = String(role.id);
    const on = picked.has(id);
    const chip = h('button', {
      class: 'role' + (on ? ' is-on' : ''), type: 'button',
      'aria-pressed': on ? 'true' : 'false',
      'data-tip': `멤버 ${role.memberCount != null ? role.memberCount : '?'}명`,
    },
      h('span', { class: 'role__dot', style: role.color ? `background:${role.color}` : '' }),
      h('span', { class: 'role__name' }, role.name),
      h('span', { class: 'role__count' }, role.memberCount != null ? `${role.memberCount}명` : ''),
    );
    chip.addEventListener('click', () => {
      const next = new Set((read() || []).map(String));
      const was = next.has(id);
      if (was) next.delete(id); else next.add(id);
      write(Array.from(next));
      chip.classList.toggle('is-on', !was);
      chip.setAttribute('aria-pressed', was ? 'false' : 'true');
      onChange && onChange();
    });
    box.append(chip);
  });
  return box;
}

/** 고른 규칙이 "지금 이 서버에서 몇 명을 통과시키는지" (구림 해소 #3). */
const permPreviewTimers = {};
function loadPermPreview(field, rule, box) {
  clearTimeout(permPreviewTimers[field.key]);
  box.replaceChildren(h('span', { class: 'permprev__skel skel' }));
  permPreviewTimers[field.key] = setTimeout(async () => {
    try {
      // key 를 같이 보내야 서버가 그 권한의 역할 목록으로 정확한 인원을 센다 (v3 §1).
      const params = new URLSearchParams({
        rule,
        key: field.permKey,
        roleIds: ((S.draft.ruleRoleIds || {})[field.permKey] || []).join(','),
      });
      const data = await api(`/admin/permission-preview?${params.toString()}`);
      S.permPreview[field.key] = data;
      paintPermPreview(box, data, rule);
    } catch (error) {
      box.replaceChildren(h('span', { class: 'permprev__fail' }, `통과 인원을 못 셌어요 — ${error.message}`));
    }
  }, 180);
}

function paintPermPreview(box, data, rule) {
  const pass = Number(data.passCount || 0);
  const total = Number(data.memberCount || 0);
  const tone = rule === 'disabled' ? 'is-none' : pass === 0 ? 'is-none' : pass === total ? 'is-all' : 'is-some';
  const kids = [
    h('span', { class: `permprev__count ${tone}` },
      rule === 'disabled' ? '지금 통과: 0명 — 아무도 못 써요' : `지금 통과: ${pass}명 / 멤버 ${total}명`),
  ];
  if (data.managerBypassCount) {
    kids.push(h('span', { class: 'permprev__note' }, `그중 ${data.managerBypassCount}명은 관리자라서 통과해요`));
  }
  if (data.note) kids.push(h('span', { class: 'permprev__note' }, data.note));
  const sample = data.sample || [];
  if (sample.length) {
    const faces = h('span', { class: 'permprev__faces' });
    sample.slice(0, 8).forEach((person) => {
      faces.append(person.avatarUrl
        ? h('img', { class: 'ava ava--sm', src: person.avatarUrl, alt: person.displayName, 'data-tip': person.displayName })
        : h('span', { class: 'ava ava--sm permprev__blank', 'data-tip': person.displayName }));
    });
    if (pass > sample.length) faces.append(h('span', { class: 'permprev__more' }, `+${pass - sample.length}`));
    kids.push(faces);
  }
  box.replaceChildren(...kids);
  tooltip(box);
}

/* ═══════════════════════════ 섹션 3 · 제한값 ═══════════════════════════ */

function sectionLimits() {
  return h('div', { class: 'sec__body' },
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '볼륨'),
      h('p', { class: 'grp__desc' }, '멤버가 조절할 수 있는 범위예요. 기본 볼륨은 "순서와 재생"에 있어요.'),
      numberField('minVolume'),
      numberField('maxVolume'),
    ),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '대기열'),
      h('p', { class: 'grp__desc' }, '한 사람이 얼마나 넣을 수 있고 서버 전체로는 얼마까지 받을지 정해요.'),
      numberField('maxQueuePerUser'),
      numberField('maxQueuePerGuild'),
      numberField('maxTrackSeconds'),
    ),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '보관 기간'),
      h('p', { class: 'grp__desc' }, '오래된 기록은 자동으로 지워요. 길게 잡으면 DB가 커져요.'),
      numberField('auditRetentionDays'),
      numberField('chatRetentionDays'),
    ),
  );
}

/* ═══════════════════════════ 섹션 4 · 유저 관리 ═══════════════════════════ */

function sectionUsers() {
  const body = h('div', { class: 'sec__body' });

  const activeBox = h('div', { class: 'card suslist' });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '지금 정지 중'),
    h('p', { class: 'grp__desc' }, '기간이 지나면 자동으로 풀려요. 무기한은 직접 풀어 주셔야 해요.'),
    activeBox,
  ));

  const listBox = h('div', { class: 'card userlist' });
  const filter = h('input', {
    class: 'field', type: 'search', placeholder: '이름으로 찾아요',
    oninput: (event) => paintParticipants(listBox, event.target.value),
  });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '참여자'),
    h('p', { class: 'grp__desc' }, '이 서버에서 리모컨으로 채팅했거나 곡을 신청해 본 사람이에요. 접속 상태는 실시간으로 바뀌어요.'),
    h('div', { class: 'grp__tools' }, filter),
    listBox,
  ));

  paintSuspensions(activeBox);
  paintParticipants(listBox, '');
  loadUsers().then(() => {
    paintSuspensions(activeBox);
    paintParticipants(listBox, filter.value);
  });
  return body;
}

async function loadUsers() {
  try {
    const [participants, suspensions] = await Promise.all([
      api('/admin/participants'),
      api('/admin/suspensions'),
    ]);
    S.participants = participants.members || [];
    S.suspensions = suspensions.items || [];
  } catch (error) {
    toast(`참여자 목록을 못 불러왔어요 — ${error.message}`, 'danger');
    S.participants = S.participants || [];
    S.suspensions = S.suspensions || [];
  }
}

function paintSuspensions(box) {
  if (S.suspensions === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:52px;margin:12px' }));
    return;
  }
  if (!S.suspensions.length) {
    box.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '✅'),
      h('div', { class: 'empty__title' }, '정지 중인 사람이 없어요'),
    ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  S.suspensions.forEach((item) => {
    rows.append(h('li', { class: 'row' },
      avatarOf(item),
      h('div', { class: 'row__main' },
        h('div', { class: 'row__name' }, item.displayName || item.userId),
        h('div', { class: 'row__sub' },
          `${scopeLabel(item.scope)} 정지 · ${untilLabel(item.expiresUtc)}`,
          item.reason ? ` · 사유: ${item.reason}` : '',
          item.byDisplayName ? ` · ${item.byDisplayName}이 처리했어요` : ''),
      ),
      h('button', {
        class: 'btn btn--sm', type: 'button',
        onclick: () => liftSuspension(item),
      }, '풀어 주기'),
    ));
  });
  box.replaceChildren(rows);
}

function avatarOf(person) {
  return person.avatarUrl
    ? h('img', { class: 'ava', src: person.avatarUrl, alt: '' })
    : h('span', { class: 'ava' });
}

function paintParticipants(box, query) {
  if (S.participants === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:64px;margin:12px' }));
    return;
  }
  const needle = String(query || '').trim().toLowerCase();
  const items = S.participants.filter((person) =>
    !needle || String(person.displayName || '').toLowerCase().includes(needle));
  if (!items.length) {
    box.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '👥'),
      h('div', { class: 'empty__title' }, '해당하는 사람이 없어요'),
      h('div', { class: 'empty__desc' }, '리모컨에서 채팅하거나 곡을 신청하면 여기에 나타나요.'),
    ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  box.replaceChildren(rows);
  renderList(rows, items, (person) => String(person.userId), (person) => userRow(person));
  tooltip(box);
}

function userRow(person) {
  const [icon, label, dotClass] = PRESENCE_LABEL[person.presence] || PRESENCE_LABEL.offline;
  const suspensions = person.suspensions || [];
  // 관리자는 다른 관리자를 정지할 수 없다. Owner만 가능 (사양서 §1.2 마지막 줄).
  const targetIsStaff = person.tier === 'manager' || person.tier === 'owner';
  const blocked = targetIsStaff && !IS_OWNER;
  const blockReason = person.tier === 'owner'
    ? '봇 주인은 정지할 수 없어요.'
    : '관리자는 다른 관리자를 정지할 수 없어요. 봇 주인만 할 수 있어요.';

  return h('li', { class: 'row row--user' },
    avatarOf(person),
    h('div', { class: 'row__main' },
      h('div', { class: 'row__name' },
        person.displayName || String(person.userId),
        targetIsStaff ? h('span', { class: `tier tier--${person.tier}` }, person.tier === 'owner' ? '봇 주인' : '관리자') : null,
      ),
      h('div', { class: 'row__sub' },
        h('span', { class: `dot ${dotClass}` }),
        h('span', null, `${icon} ${label}`),
        h('span', null, `· 신청 ${person.queueCount || 0}곡 · 채팅 ${person.chatCount || 0}건`),
        person.lastSeenUtc ? h('span', null, `· ${fmtAgo(person.lastSeenUtc)} 활동했어요`) : null,
      ),
      suspensions.length
        ? h('div', { class: 'row__chips' }, ...suspensions.map((item) =>
            h('span', { class: 'chip chip--danger' }, `${scopeLabel(item.scope)} 정지 · ${untilLabel(item.expiresUtc)}`)))
        : null,
    ),
    h('button', {
      class: 'btn btn--sm' + (blocked ? '' : ' btn--danger'),
      type: 'button',
      disabled: blocked,
      'data-tip': blocked ? blockReason : '무엇을 얼마나 막을지 골라서 정지해요',
      onclick: () => openSuspendSheet(person),
    }, blocked ? '정지 불가' : '정지'),
  );
}

/**
 * 정지 시트 — 범위 × 기간 + 사유.
 * confirmSheet 는 body 노드를 버리므로(desc 문자열만 받는다) sheet 를 직접 쓴다.
 */
async function openSuspendSheet(person) {
  let scope = 'all';
  let minutes = 30;
  let reason = '';

  const scopeNote = h('p', { class: 'hint' }, SUSPEND_SCOPES[0].desc);
  const scopeBox = h('div', { class: 'seg' });
  SUSPEND_SCOPES.forEach((item) => {
    scopeBox.append(h('button', {
      class: 'seg__btn' + (item.value === scope ? ' is-on' : ''), type: 'button',
      onclick: (event) => {
        scope = item.value;
        scopeNote.textContent = item.desc;
        scopeBox.querySelectorAll('.seg__btn').forEach((node) => node.classList.remove('is-on'));
        event.currentTarget.classList.add('is-on');
      },
    }, item.label));
  });

  const durationBox = h('div', { class: 'seg' });
  SUSPEND_DURATIONS.forEach((item) => {
    durationBox.append(h('button', {
      class: 'seg__btn' + (item.minutes === minutes ? ' is-on' : ''), type: 'button',
      onclick: (event) => {
        minutes = item.minutes;
        durationBox.querySelectorAll('.seg__btn').forEach((node) => node.classList.remove('is-on'));
        event.currentTarget.classList.add('is-on');
      },
    }, item.label));
  });

  const reasonInput = h('input', {
    class: 'field', placeholder: '사유 (본인에게 보여요)', maxlength: '120',
    oninput: (event) => { reason = event.target.value; },
  });

  const body = h('div', { class: 'sheetform' },
    h('label', { class: 'sheetform__label' }, '무엇을 막을까요'), scopeBox, scopeNote,
    h('label', { class: 'sheetform__label' }, '얼마나 막을까요'), durationBox,
    h('label', { class: 'sheetform__label' }, '사유'), reasonInput,
  );

  const ok = await sheet({
    title: `${person.displayName || person.userId} 정지`,
    body,
    danger: true,
    dismissValue: false,
    actions: [
      { label: '취소', kind: 'ghost', value: false },
      { label: '정지', kind: 'danger', value: true },
    ],
  }).result;
  if (!ok) return;
  try {
    await api('/admin/suspensions', {
      method: 'POST',
      body: { userId: String(person.userId), scope, minutes, reason },
    });
    toast('정지했어요.', 'ok');
    await loadUsers();
    renderSection('users');
  } catch (error) {
    toast(`정지하지 못했어요 — ${error.message}`, 'danger');
  }
}

async function liftSuspension(item) {
  const ok = await confirmSheet({
    title: '정지 풀기',
    desc: `${item.displayName || item.userId}의 ${scopeLabel(item.scope)} 정지를 지금 풀어요.`,
    confirmText: '풀어 주기',
    cancelText: '그대로 둘게요',
  });
  if (!ok) return;
  try {
    await api('/admin/suspensions/lift', { method: 'POST', body: { userId: String(item.userId), scope: item.scope } });
    toast('정지를 풀었어요.', 'ok');
    await loadUsers();
    renderSection('users');
  } catch (error) {
    toast(`풀지 못했어요 — ${error.message}`, 'danger');
  }
}

/* ═══════════════════════════ 섹션 5 · 채팅과 제안 ═══════════════════════════ */

function sectionChat() {
  const body = h('div', { class: 'sec__body' });

  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '웹 채팅'),
    h('p', { class: 'grp__desc' }, '리모컨 안에서만 오가는 채팅이에요. Discord 채널과는 연결되지 않아요.'),
    fieldShell('chatEnabled', '채팅 사용',
      '끄면 기존 대화는 남지만 아무도 새로 쓸 수 없어요. 읽기까지 막고 싶으시면 권한에서 "채팅 쓰기"를 "사용 안 함"으로 두세요.',
      toggleControl('chatEnabled', '멤버가 채팅을 쓸 수 있어요', '채팅이 꺼져 있어요')),
    // 보관 기간의 실제 값은 "제한값" 섹션이 소유한다. 여기서는 현재 값만 보여주고 그쪽으로 보낸다.
    h('div', { class: 'mirror' },
      h('div', null,
        h('div', { class: 'mirror__label' }, '채팅 보관 기간'),
        h('div', { class: 'mirror__value' }, `${S.draft.chatRetentionDays}일`),
        h('p', { class: 'hint' }, '이 값은 "제한값" 섹션에서 다른 보관 기간들과 함께 관리해요.'),
      ),
      h('button', { class: 'btn btn--sm', type: 'button', onclick: () => goSection('limits') }, '제한값에서 바꾸기 →'),
    ),
  ));

  const reportsBox = h('div', { class: 'card' });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '신고된 메시지'),
    h('p', { class: 'grp__desc' }, '멤버가 신고한 채팅이에요. 지우거나 문제없음으로 넘기시면 돼요.'),
    reportsBox,
  ));

  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '제안 게시판'),
    h('p', { class: 'grp__desc' }, '멤버가 앱 개선 제안을 올리고 공감을 눌러요. 상태는 관리자가 정해요.'),
    fieldShell('suggestionEnabled', '제안 게시판 사용',
      '끄면 새 제안을 받지 않아요. 이미 올라온 제안은 계속 보여요.',
      toggleControl('suggestionEnabled', '멤버가 제안을 올릴 수 있어요', '제안 접수를 닫았어요')),
  ));

  const suggestBox = h('div', { class: 'card' });
  body.append(suggestBox);

  paintReports(reportsBox);
  paintSuggestions(suggestBox);
  loadChatData().then(() => { paintReports(reportsBox); paintSuggestions(suggestBox); });
  return body;
}

async function loadChatData() {
  try {
    const [reports, suggestions] = await Promise.all([
      api('/admin/reports?status=open'),
      api('/admin/suggestions?limit=50'),
    ]);
    S.reports = reports.items || [];
    S.suggestions = suggestions.items || [];
  } catch (error) {
    toast(`채팅·제안을 못 불러왔어요 — ${error.message}`, 'danger');
    S.reports = S.reports || [];
    S.suggestions = S.suggestions || [];
  }
}

function paintReports(box) {
  if (S.reports === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:56px;margin:12px' }));
    return;
  }
  if (!S.reports.length) {
    box.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '🕊'),
      h('div', { class: 'empty__title' }, '처리할 신고가 없어요'),
    ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  S.reports.forEach((report) => {
    rows.append(h('li', { class: 'row row--report' },
      h('div', { class: 'row__main' },
        h('div', { class: 'row__name' }, `${report.messageAuthor}의 메시지`),
        h('blockquote', { class: 'quote' }, report.messageContent || '(내용이 없어요)'),
        h('div', { class: 'row__sub' },
          `${report.reporterDisplayName}이 신고했어요 · ${report.reason || '사유 없음'} · ${fmtAgo(report.createdUtc)}`),
      ),
      h('div', { class: 'row__acts' },
        h('button', { class: 'btn btn--sm btn--danger', type: 'button', onclick: () => resolveReport(report, 'delete') }, '메시지 삭제'),
        h('button', { class: 'btn btn--sm', type: 'button', onclick: () => resolveReport(report, 'dismiss') }, '문제 없어요'),
      ),
    ));
  });
  box.replaceChildren(rows);
}

async function resolveReport(report, action) {
  if (action === 'delete') {
    const ok = await confirmSheet({
      title: '메시지 삭제',
      desc: '이 채팅을 지워요. 되돌릴 수 없어요.',
      confirmText: '삭제', cancelText: '그냥 둘게요', danger: true,
    });
    if (!ok) return;
  }
  try {
    await api(`/admin/reports/${report.id}/resolve`, { method: 'POST', body: { action } });
    S.reports = S.reports.filter((item) => item.id !== report.id);
    toast(action === 'delete' ? '메시지를 지웠어요.' : '신고를 닫았어요.', 'ok');
    renderSection('chat');
  } catch (error) {
    toast(`처리하지 못했어요 — ${error.message}`, 'danger');
  }
}

function paintSuggestions(box) {
  if (S.suggestions === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:56px;margin:12px' }));
    return;
  }
  if (!S.suggestions.length) {
    box.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '💡'),
      h('div', { class: 'empty__title' }, '올라온 제안이 없어요'),
      h('div', { class: 'empty__desc' }, '멤버가 리모컨의 제안 탭에서 글을 올리면 여기 모여요.'),
    ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  S.suggestions.forEach((item) => {
    const statusSelect = h('select', {
      class: 'field field--sm',
      'aria-label': '제안 상태',
      onchange: (event) => changeSuggestionStatus(item, event.target.value),
    });
    SUGGESTION_STATUS.forEach((status) => {
      statusSelect.append(h('option', { value: status.value, selected: item.status === status.value }, status.label));
    });
    const status = SUGGESTION_STATUS.find((entry) => entry.value === item.status) || SUGGESTION_STATUS[0];
    rows.append(h('li', { class: 'row row--suggest' },
      h('div', { class: 'row__main' },
        h('div', { class: 'row__name' },
          item.title,
          h('span', { class: `chip ${status.chip}` }, status.label),
        ),
        h('p', { class: 'row__body' }, item.body),
        h('div', { class: 'row__sub' },
          `${item.displayName} · ${fmtAgo(item.createdUtc)} · 👍 ${item.voteCount || 0}`,
          item.statusNote ? ` · 메모: ${item.statusNote}` : ''),
      ),
      h('div', { class: 'row__acts' }, statusSelect),
    ));
  });
  box.replaceChildren(rows);
}

async function changeSuggestionStatus(item, status) {
  try {
    await api(`/admin/suggestions/${item.id}/status`, { method: 'POST', body: { status } });
    item.status = status;
    toast('제안 상태를 바꿨어요.', 'ok');
    renderSection('chat');
  } catch (error) {
    toast(`상태를 바꾸지 못했어요 — ${error.message}`, 'danger');
  }
}

/* ═══════════════════════════ 섹션 6 · 활동 기록 ═══════════════════════════ */

function sectionAudit() {
  const rows = h('ul', { class: 'rows rows--audit' });
  const sentinel = h('div', { class: 'audit__more' });

  const filter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'audit-filter',
    placeholder: '사람 · 동작 · 곡 제목으로 찾아요',
    value: S.audit.query,
  });
  let timer = null;
  filter.addEventListener('input', (event) => {
    clearTimeout(timer);
    const value = event.target.value;
    timer = setTimeout(() => {
      S.audit = { items: [], cursor: null, done: false, loading: false, query: value };
      paintAudit(rows, sentinel);
      loadAudit(rows, sentinel);
    }, 220);
  });

  const body = h('div', { class: 'sec__body' },
    h('div', { class: 'grp__tools' }, filter),
    h('div', { class: 'card' }, rows, sentinel),
  );

  // 무한 스크롤 — 바닥 센티넬이 보이면 다음 페이지.
  const observer = new IntersectionObserver((entries) => {
    if (entries.some((entry) => entry.isIntersecting)) loadAudit(rows, sentinel);
  }, { rootMargin: '240px' });
  observer.observe(sentinel);
  body.dataset.cleanup = '1';
  body._cleanup = () => observer.disconnect();

  paintAudit(rows, sentinel);
  if (!S.audit.items.length) loadAudit(rows, sentinel);
  return body;
}

async function loadAudit(rows, sentinel) {
  if (S.audit.loading || S.audit.done) return;
  S.audit.loading = true;
  sentinel.replaceChildren(h('div', { class: 'skel', style: 'height:28px' }));
  try {
    const params = new URLSearchParams({ limit: '50' });
    if (S.audit.cursor) params.set('before', String(S.audit.cursor));
    if (S.audit.query) params.set('q', S.audit.query);
    const data = await api(`/admin/audit?${params.toString()}`);
    const items = data.items || [];
    S.audit.items = S.audit.items.concat(items);
    S.audit.cursor = data.nextCursor || null;
    S.audit.done = !data.nextCursor || items.length === 0;
  } catch (error) {
    S.audit.done = true;
    sentinel.replaceChildren(h('p', { class: 'hint' }, `기록을 못 불러왔어요 — ${error.message}`));
    S.audit.loading = false;
    return;
  }
  S.audit.loading = false;
  paintAudit(rows, sentinel);
}

function paintAudit(rows, sentinel) {
  if (!S.audit.items.length && S.audit.done) {
    rows.replaceChildren(h('li', null, h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '📜'),
      h('div', { class: 'empty__title' }, '조건에 맞는 기록이 없어요'),
    )));
  } else {
    renderList(rows, S.audit.items, (entry) => String(entry.id), (entry) => auditRow(entry));
  }
  sentinel.replaceChildren(S.audit.done && S.audit.items.length
    ? h('p', { class: 'hint' }, '여기까지가 전부예요.')
    : h('span'));
}

function auditRow(entry) {
  return h('li', { class: 'row row--audit' + (entry.success ? '' : ' is-fail') },
    h('time', { class: 'audit__time' }, fmtAgo(entry.createdUtc)),
    h('strong', { class: 'audit__who' }, entry.displayName || String(entry.userId)),
    h('span', { class: 'audit__what' }, entry.action),
    h('span', { class: 'audit__detail mq' },
      h('span', { class: 'mq__i' },
        entry.failureReason || entry.target || entry.afterValue || '')),
  );
}

/* ═══════════════════════════ 섹션 7 · 진단 ═══════════════════════════ */

function sectionDiag() {
  const body = h('div', { class: 'sec__body' });
  const box = h('div', { class: 'diag' });

  body.append(intentCards());
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '봇 · 서버 상태'),
    h('p', { class: 'grp__desc' }, '문제 신고를 받으시면 이 값들을 먼저 확인해 주세요.'),
    box,
    h('button', {
      class: 'btn', type: 'button', style: 'margin-top:12px',
      onclick: () => { S.diag = null; renderSection('diag'); },
    }, '다시 확인할게요'),
  ));

  paintDiag(box);
  if (!S.diag) loadDiag().then(() => paintDiag(box));
  return body;
}

/** 특권 인텐트가 꺼져 있으면 켜는 방법까지 안내한다 (사양서 §2.3). */
function intentCards() {
  const status = M.intentStatus || {};
  const rows = [
    { key: 'members',   label: 'Server Members Intent',  what: '전체 멤버 목록과 역할을 읽어요. 꺼져 있으면 참여자 목록이 리모컨을 써 본 사람만으로 줄어들어요.' },
    { key: 'presences', label: 'Presence Intent',        what: 'Discord 온라인/자리비움 표시를 읽어요. 꺼져 있으면 접속 표시가 "보는 중 / 듣는 중"만 남아요.' },
    { key: 'voiceStates', label: 'Voice States',         what: '누가 어느 음성 채널에 있는지 읽어요. "같은 음성 채널" 권한 규칙이 이 값을 써요.' },
  ];
  const box = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '인텐트'),
    h('p', { class: 'grp__desc' }, 'Discord가 봇에게 어떤 정보를 주는지예요. 꺼져 있어도 봇은 죽지 않고 관련 표시만 줄어들어요.'),
  );
  rows.forEach((row) => {
    const on = status[row.key] !== false;
    box.append(h('div', { class: 'diagrow' + (on ? '' : ' is-off') },
      h('span', { class: 'diagrow__flag' }, on ? '✅' : '⚠'),
      h('div', null,
        h('div', { class: 'diagrow__label' }, row.label, h('span', { class: `chip ${on ? 'chip--ok' : 'chip--warn'}` }, on ? '켜져 있어요' : '꺼져 있어요')),
        h('p', { class: 'hint' }, row.what),
        on ? null : h('p', { class: 'hint' },
          'Discord 개발자 포털 → 내 애플리케이션 → Bot → Privileged Gateway Intents 에서 켜고 봇을 다시 시작하시면 돼요.'),
      ),
    ));
  });
  if (status.degradedReason) {
    box.append(h('div', { class: 'warnbox' },
      h('span', null, '⚠'),
      h('span', null, `축소된 이유: ${status.degradedReason}`),
    ));
  }
  return box;
}

async function loadDiag() {
  try {
    S.diag = await api('/admin/diagnostics');
  } catch (error) {
    S.diag = { error: error.message };
  }
}

function paintDiag(box) {
  if (!S.diag) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:120px' }));
    return;
  }
  if (S.diag.error) {
    box.replaceChildren(h('p', { class: 'hint' }, `진단 정보를 못 불러왔어요 — ${S.diag.error}`));
    return;
  }
  const bot = S.diag.bot || {};
  // v3 §4의 presence.bot 과 같은 값들. 예전 서버는 online/voiceConnected 만 주므로 그걸로 흘려 받는다.
  const inGuild = bot.inGuild;
  const inVoice = bot.inVoice != null ? bot.inVoice : bot.voiceConnected;
  const listeners = bot.listenerCount;
  const channel = bot.voiceChannelName || '이름을 모르는 채널';

  const cells = [
    ['봇 연결', bot.online ? '온라인이에요' : '오프라인이에요', bot.online ? 'is-ok' : 'is-bad'],
    ['서버 참가 여부',
      inGuild == null ? '알 수 없어요' : inGuild ? '이 서버에 들어와 있어요' : '이 서버에 없어요',
      inGuild == null ? '' : inGuild ? 'is-ok' : 'is-bad'],
    ['음성 채널 참가 여부',
      inVoice == null ? '알 수 없어요' : inVoice ? `참가 중이에요 (${channel})` : '아직 안 들어갔어요',
      inVoice ? 'is-ok' : ''],
    ['듣는 사람 수',
      listeners == null ? '알 수 없어요' : inVoice ? `${listeners}명이 듣고 있어요` : '0명 (음성 채널에 없어요)',
      listeners > 0 ? 'is-ok' : ''],
    ['게이트웨이 지연', bot.gatewayLatencyMs != null ? `${bot.gatewayLatencyMs}ms` : '알 수 없어요', ''],
    ['빌드 ID', S.diag.buildId || M.buildId || '-', ''],
    ['DB 스키마 버전', S.diag.schemaVersion != null ? `v${S.diag.schemaVersion}` : '-', ''],
    ['가동 시간', S.diag.uptimeSeconds != null ? prettySeconds(S.diag.uptimeSeconds) : '-', ''],
  ];
  const grid = h('div', { class: 'diag__grid' });
  cells.forEach(([label, value, tone]) => {
    grid.append(h('div', { class: `diag__cell ${tone}` },
      h('div', { class: 'diag__label' }, label),
      h('div', { class: 'diag__value' }, String(value)),
    ));
  });
  box.replaceChildren(grid);

  if (inGuild === false) {
    box.append(h('div', { class: 'warnbox' },
      h('span', null, '⚠'),
      h('span', null, '봇이 이 서버에 들어와 있지 않아요. Discord에서 봇을 다시 초대하면 리모컨도 같이 살아나요.'),
    ));
  } else if (inVoice === false) {
    box.append(h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null, '봇이 음성 채널에 없어서 지금은 아무도 못 듣고 있어요. Discord에서 봇을 음성 채널로 불러 주세요.'),
    ));
  }

  if (S.diag.buildId && M.buildId && S.diag.buildId !== M.buildId) {
    box.append(h('div', { class: 'warnbox' },
      h('span', null, '⚠'),
      h('span', null, `열어 두신 화면이 옛 버전이에요(${M.buildId}). 새로고침하면 ${S.diag.buildId}로 바뀌어요.`),
    ));
  }
}

/* ═══════════════════════════ 섹션 셸 · 저장 ═══════════════════════════ */

const SECTION_RENDER = {
  order: sectionOrder,
  perms: sectionPerms,
  limits: sectionLimits,
  users: sectionUsers,
  chat: sectionChat,
  audit: sectionAudit,
  diag: sectionDiag,
};

function renderSection(id) {
  const spec = SECTIONS.find((item) => item.id === id) || SECTIONS[0];
  S.activeSection = spec.id;

  if (sectionBox && sectionBox._cleanup) sectionBox._cleanup();

  const body = SECTION_RENDER[spec.id]();
  const editable = Boolean(SECTION_KEYS[spec.id]);

  const next = h('section', { class: 'sec', 'data-section': spec.id },
    h('header', { class: 'sec__head' },
      h('h2', { class: 'sec__title' }, spec.icon + ' ' + spec.label),
      h('p', { class: 'sec__desc' }, spec.desc),
    ),
    body,
    editable ? h('footer', { class: 'sec__foot' },
      h('span', { class: 'sec__footnote' }, '아직 바꾼 항목이 없어요'),
      h('button', {
        class: 'btn', type: 'button', disabled: true,
        onclick: () => revertSection(spec.id),
      }, '되돌리기'),
      h('button', {
        class: 'btn btn--primary', type: 'button', disabled: true,
        'data-testid': 'settings-save',
        onclick: () => saveSection(spec.id),
      }, '저장'),
    ) : null,
  );

  if (sectionBox && sectionBox.parentNode) sectionBox.replaceWith(next);
  else document.querySelector('.cs__main').append(next);
  sectionBox = next;
  if (body._cleanup) sectionBox._cleanup = body._cleanup;

  navBox.querySelectorAll('.nav__item').forEach((node) => {
    const on = node.dataset.section === spec.id;
    node.classList.toggle('is-on', on);
    node.setAttribute('aria-current', on ? 'page' : 'false');
    if (on) node.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  });

  refreshDirty();
  validate();
  tooltip(sectionBox);
}

function revertSection(id) {
  (SECTION_KEYS[id] || []).forEach((key) => { S.draft[key] = clone(S.saved[key]); });
  renderSection(id);
  toast('저장 전 값으로 되돌렸어요.');
}

async function saveSection(id) {
  const keys = dirtyKeys(id);
  if (!keys.length || S.saving) return;
  const errors = validate();
  const blocked = (SECTION_KEYS[id] || []).filter((key) => errors[key]);
  if (blocked.length) {
    toast('값이 서로 맞지 않아요. 빨간 설명부터 고쳐 주세요.', 'warn');
    return;
  }

  S.saving = true;
  refreshDirty();
  const payload = {};
  (SECTION_KEYS[id] || []).forEach((key) => { payload[key] = S.draft[key]; });

  try {
    const result = await api(`/admin/settings/${id}`, { method: 'PUT', body: payload });
    // 서버가 정규화한 값을 돌려주면 그걸 기준으로 삼는다. 다른 섹션의 편집 중인 값은 건드리지 않는다.
    const applied = normalizeSettings((result && result.settings) || payload);
    (SECTION_KEYS[id] || []).forEach((key) => {
      if (applied[key] === undefined) return;
      S.saved[key] = clone(applied[key]);
      S.draft[key] = clone(applied[key]);
    });
    toast(`저장했어요 · ${keys.length}개 항목`, 'ok');
    if (keys.includes('sortMode')) S.queuePreview = { mode: null, data: null, loading: false };
  } catch (error) {
    toast(`저장하지 못했어요 — ${error.message}`, 'danger');
  } finally {
    S.saving = false;
    renderSection(id);
  }
}

/* ═══════════════════════════ 네비게이션 가드 ═══════════════════════════ */

/** 저장 안 한 변경이 있으면 확인부터. true 면 이동해도 된다. */
async function guardLeave() {
  if (!anyDirty()) return true;
  const count = Object.keys(SECTION_KEYS).reduce((sum, id) => sum + dirtyKeys(id).length, 0);
  return confirmSheet({
    title: '저장하지 않은 변경이 있어요',
    desc: `${count}개 항목을 바꿔 두고 아직 저장하지 않으셨어요. 이대로 나가면 변경은 사라져요.`,
    confirmText: '변경 버리고 갈게요',
    cancelText: '여기 남을게요',
    danger: true,
  });
}

async function goSection(id) {
  if (id === S.activeSection) return;
  const dirty = dirtyKeys(S.activeSection);
  if (dirty.length) {
    const ok = await confirmSheet({
      title: `"${(SECTIONS.find((item) => item.id === S.activeSection) || {}).label}" 섹션에 저장 안 한 변경이 있어요`,
      desc: `${dirty.length}개 항목이 저장 전이에요. 지금 이동하면 이 섹션의 변경은 사라져요.`,
      confirmText: '변경 버리고 갈게요',
      cancelText: '남아서 저장할게요',
      danger: true,
    });
    if (!ok) return;
    revertSectionSilently(S.activeSection);
  }
  renderSection(id);
  history.replaceState(null, '', `#${id}`);
}

function revertSectionSilently(id) {
  (SECTION_KEYS[id] || []).forEach((key) => { S.draft[key] = clone(S.saved[key]); });
}

/* ═══════════════════════════ 셸 ═══════════════════════════ */

function renderShell() {
  const root = document.getElementById('app') || document.body;
  root.replaceChildren();

  dirtyBadge = h('span', { class: 'head__dirty', hidden: true });

  const back = h('a', {
    class: 'btn btn--ghost head__back',
    href: `/music/guilds/${GUILD_ID}`,
    'data-tip': '유저용 리모컨 화면으로 가요',
    onclick: async (event) => {
      event.preventDefault();
      if (await guardLeave()) location.href = `/music/guilds/${GUILD_ID}`;
    },
  }, '← 리모컨으로 돌아가기');

  const head = h('header', { class: 'cs__head' },
    back,
    h('div', { class: 'head__title' },
      h('h1', null, '서버 관리'),
      h('span', { class: 'head__guild' }, (M.guild && M.guild.name) || ''),
    ),
    dirtyBadge,
    h('div', { class: 'head__spacer' }),
    h('span', { class: `tier tier--${M.tier}` }, IS_OWNER ? '🛡 봇 주인' : '🛡 서버 관리자'),
    h('button', {
      class: 'btn btn--icon btn--ghost', type: 'button',
      'data-tip': '밝게 / 어둡게 바꿔요',
      'aria-label': '테마 전환',
      onclick: (event) => {
        theme.toggle();
        event.currentTarget.textContent = document.documentElement.dataset.theme === 'light' ? '🌞' : '🌓';
      },
    }, document.documentElement.dataset.theme === 'light' ? '🌞' : '🌓'),
  );

  navBox = h('nav', { class: 'cs__nav', 'aria-label': '설정 섹션' });
  SECTIONS.forEach((spec) => {
    navBox.append(h('button', {
      class: 'nav__item', type: 'button', 'data-section': spec.id,
      onclick: () => goSection(spec.id),
    },
      h('span', { class: 'nav__icon' }, spec.icon),
      h('span', { class: 'nav__label' }, spec.label),
      h('span', { class: 'nav__dot' }),
    ));
  });

  root.append(h('div', { class: 'cs' },
    head,
    h('div', { class: 'cs__body' },
      navBox,
      h('main', { class: 'cs__main' }),
    ),
  ));
  tooltip(root);
}

/** 권한이 없을 때 — 서버가 403을 주지만 클라에서도 방어적으로 막는다. */
function renderDenied() {
  const root = document.getElementById('app') || document.body;
  root.replaceChildren(h('div', { class: 'cs cs--denied' },
    h('div', { class: 'panel denied' },
      h('div', { class: 'denied__icon' }, '🔒'),
      h('h1', null, '서버 관리 콘솔은 관리자만 들어올 수 있어요'),
      h('p', { class: 'hint' }, '이 서버에서는 관리 권한이 없으세요. 서버 관리자에게 "관리자 지정 역할"을 받으시면 들어올 수 있어요.'),
      h('a', { class: 'btn btn--primary', href: `/music/guilds/${GUILD_ID}` }, '← 리모컨으로 돌아가기'),
    ),
  ));
}

/* ═══════════════════════════ 부팅 ═══════════════════════════ */

async function boot() {
  if (!CAN_MANAGE) { renderDenied(); return; }
  renderShell();

  const main = document.querySelector('.cs__main');
  main.append(h('div', { class: 'skel', style: 'height:60vh' }));

  try {
    const [settings, roles] = await Promise.all([
      api('/admin/settings'),
      api('/admin/roles').catch(() => ({ roles: [] })),   // 역할은 없어도 나머지는 동작해야 한다
    ]);
    S.saved = normalizeSettings(settings.settings || settings, true);
    S.draft = clone(S.saved);
    S.roles = roles.roles || [];
  } catch (error) {
    main.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '⚠'),
      h('div', { class: 'empty__title' }, '설정을 불러오지 못했어요'),
      h('div', { class: 'empty__desc' }, error.message),
      h('button', { class: 'btn btn--primary', onclick: () => location.reload() }, '다시 시도할게요'),
    ));
    return;
  }

  main.replaceChildren();
  const wanted = location.hash.replace('#', '');
  renderSection(SECTIONS.some((spec) => spec.id === wanted) ? wanted : 'order');

  // 저장 안 하고 창을 닫으려 하면 브라우저 확인 (구림 해소 #5).
  window.addEventListener('beforeunload', (event) => {
    if (!anyDirty()) return;
    event.preventDefault();
    event.returnValue = '';
  });

  // 뒤로가기도 같은 가드를 태운다.
  window.addEventListener('popstate', async () => {
    const id = location.hash.replace('#', '') || 'order';
    if (id !== S.activeSection) await goSection(id);
  });

  // WS — 콘솔이 열려 있는 동안 정지/설정/접속/시드 변화를 따라간다.
  // onEvent 는 core.js 가 처리하지 않은 토픽에만 오고, presence/settings/suspension/queue.set 은
  // core.js 가 자체 머지한다. 콘솔은 그 토픽들도 봐야 하므로 모든 프레임을 주는 onAny 를 쓴다.
  connect(GUILD_ID, {
    onAny: (topic, data) => onRemoteEvent(topic, data),
  });

  // 연결 끊김 표시는 core.js 스토어의 conn 값을 그대로 쓴다.
  store.subscribe('conn', (next) => {
    document.body.classList.toggle('is-offline', next.conn && next.conn !== 'live');
  });
}

/** WS 이벤트 머지 — 전체 재조회는 하지 않는다 (성능 계약 §5.2 B). */
function onRemoteEvent(topic, data) {
  if (topic === 'presence' && S.participants) {
    const listening = new Set((data && data.listening) || []);
    const otherVoice = new Set((data && data.inOtherVoice) || []);
    const viewing = new Set((data && data.viewing) || []);
    const online = (data && data.online) || {};
    S.participants.forEach((person) => {
      const id = String(person.userId);
      person.presence = listening.has(id) ? 'listening'
        : otherVoice.has(id) ? 'inOtherVoice'
        : viewing.has(id) ? 'viewing'
        : online[id] || 'offline';
    });
    if (S.activeSection === 'users') {
      const box = sectionBox.querySelector('.userlist');
      const filter = sectionBox.querySelector('input[type="search"]');
      if (box) paintParticipants(box, filter ? filter.value : '');
    }
    return;
  }
  if (topic === 'suspension') {
    S.suspensions = null;
    S.participants = null;
    if (S.activeSection === 'users') renderSection('users');
    return;
  }
  if (topic === 'autoplay') {
    // 다른 사람이 기준 곡을 바꿨다. 목록만 다시 받는다 — 설정 draft 는 건드리지 않는다.
    S.seeds = { items: null, max: S.seeds.max, canEdit: S.seeds.canEdit, error: null, loading: false };
    if (S.activeSection === 'order') {
      repaintSeeds();
      loadSeeds().then(() => repaintSeeds());
    }
    return;
  }
  if (topic === 'settings') {
    // 다른 관리자가 같은 서버 설정을 바꿨다. 내 편집분은 지우지 않고 알리기만 한다.
    if (anyDirty()) {
      toast('다른 관리자가 방금 설정을 바꿨어요. 저장하시면 제 값으로 덮어써요.', 'warn');
    } else {
      api('/admin/settings').then((next) => {
        S.saved = normalizeSettings(next.settings || next, true);
        S.draft = clone(S.saved);
        renderSection(S.activeSection);
      }).catch(() => {});
    }
    return;
  }
  if (topic === 'queue.set' && S.activeSection === 'order') {
    S.queuePreview = { mode: null, data: null, loading: false };
    loadQueuePreview(S.draft.sortMode);
  }
}

boot();
