/* 마참뮤직 리모컨 v2 — 서버 관리 콘솔 (/music/guilds/{guild_id}/admin)
 *
 * 기존 "설정 모달"을 대체한다. 사양서 §4.2의 "구림" 해소 기준 8개를 전부 만족시키는 것이 합격 기준이다.
 *   1) 항목마다 한 줄 설명   2) 섹션 묶음 + 섹션 목적   3) 권한 드롭다운 즉시 통과 인원
 *   4) 정렬 모드 대기열 미리보기  5) 변경분 강조 + 되돌리기 + 이탈 확인
 *   6) 숫자는 슬라이더 + 직접입력 + 단위 + 허용범위  7) 섹션 단위 부분 저장 + 토스트
 *   8) 1024px 이하에서 좌측 네비가 상단 가로 스크롤 탭으로 (네비는 어떤 폭에서도 사라지지 않는다)
 *
 * 렌더링은 전부 클라이언트. innerHTML을 쓰지 않고 core.js의 h()로만 DOM을 만든다(XSS 차단).
 *
 * ── core.js 계약 (다른 에이전트가 작성 중. 여기서 가정한 형태) ────────────────────────
 *   api(path, opts?)        : /music/api/guilds/{guildId} 기준 상대 경로. CSRF 헤더 자동.
 *                             opts.body 는 평범한 객체(내부에서 JSON 직렬화). 실패 시 throw(Error.message).
 *   h(tag, props?, ...kids) : 하이퍼스크립트. props 는 class/id/type/value/min/max/step/
 *                             disabled/hidden/placeholder/href, aria-·data- 속성, on* 핸들러 지원.
 *                             children 은 문자열(텍스트 노드) · 노드 · 배열 · null 허용.
 *   list(box, items, keyOf, render) : 키 기반 리스트 diff. 노드 재사용.
 *   store                   : 전역 상태 스토어. store.subscribe(fn) → 해제 함수. (여기선 연결상태만 본다)
 *   connect(opts)           : WebSocket 연결. { guildId, csrf, onEvent(topic, data) }.
 *   tooltip(root)           : root 하위 [data-tip] 요소에 커스텀 툴팁 바인딩(전역 위임이면 no-op).
 *   toast(msg, kind?)       : kind = 'ok' | 'warn' | 'danger'.
 *   confirmSheet(opts)      : { title, body, confirmText, cancelText, danger } → Promise<boolean>.
 *   theme.toggle()          : 다크/라이트 토글. 현재 값은 documentElement.dataset.theme 로 읽는다.
 *   fmtAgo(utc)             : "3분 전".
 * ──────────────────────────────────────────────────────────────────────────────────
 */

import { store, connect, api, h, list, tooltip, toast, confirmSheet, theme, fmtAgo } from './core.js';

/* ═══════════════════════════ 부트스트랩 ═══════════════════════════ */

const M = window.MACHAM || {};
const GUILD_ID = String(M.guildId || '');
const IS_OWNER = M.tier === 'owner';
const CAN_MANAGE = IS_OWNER || M.tier === 'manager';

/* ═══════════════════════════ 상수 테이블 ═══════════════════════════ */

/** 좌측 네비 = 섹션 정의. desc 는 섹션 목적(구림 해소 #2). */
const SECTIONS = [
  { id: 'order',   icon: '🎚', label: '순서와 재생', desc: '대기열이 어떤 기준으로 줄을 서는지, 재생이 어떻게 시작되는지 정한다.' },
  { id: 'perms',   icon: '🛡', label: '권한',        desc: '어떤 사람이 어떤 조작을 할 수 있는지 기능별로 정한다. 고르면 지금 몇 명이 통과하는지 바로 보여준다.' },
  { id: 'limits',  icon: '📐', label: '제한값',      desc: '한 사람이 얼마나 쓸 수 있는지, 기록을 얼마나 보관할지 숫자로 정한다.' },
  { id: 'users',   icon: '👥', label: '유저 관리',   desc: '리모컨을 써본 사람 목록과 접속 상태. 문제를 일으킨 사람을 기능별·기간제로 정지한다.' },
  { id: 'chat',    icon: '💬', label: '채팅과 제안', desc: '웹 채팅과 제안 게시판을 켜고 끈다. 신고된 메시지와 들어온 제안을 처리한다.' },
  { id: 'audit',   icon: '📜', label: '활동 기록',   desc: '이 서버에서 일어난 모든 조작 기록. 누가 언제 무엇을 바꿨는지 남는다.' },
  { id: 'diag',    icon: '🩺', label: '진단',        desc: '봇 연결·인텐트·버전 상태. 뭔가 안 될 때 여기부터 본다.' },
];

/** 권한 규칙 5종. desc 는 드롭다운 아래 한 줄 설명(구림 해소 #1). */
const RULE_OPTIONS = [
  { value: 'guildMember',      label: '모든 멤버',      desc: '이 Discord 서버에 있는 사람이면 누구나 쓸 수 있다.' },
  { value: 'sameVoiceChannel', label: '같은 음성 채널', desc: '봇이 들어가 있는 음성 채널에 같이 있는 사람만 쓸 수 있다.' },
  { value: 'configuredRole',   label: '지정 역할',      desc: '아래에서 고른 역할을 가진 사람만 쓸 수 있다.' },
  { value: 'administrator',    label: '관리자',         desc: '서버 관리자와 봇 주인만 쓸 수 있다.' },
  { value: 'disabled',         label: '사용 안 함',     desc: '아무도 못 쓴다. 기능 자체를 끄는 선택이다.' },
];

/** 권한 7종. 사양서 §1.2 기능별 권한 매트릭스의 "규칙" 칸에 해당하는 항목들. */
const PERM_FIELDS = [
  { key: 'searchRule',    label: '곡 검색·신청',      desc: '검색해서 대기열에 곡을 넣는 동작. 막으면 아무도 새 곡을 못 넣는다.' },
  { key: 'voteRule',      label: '좋아요·슈퍼 좋아요', desc: '대기열 곡에 점수를 주는 동작. 점수제일 때만 실제 순서에 영향을 준다.' },
  { key: 'chatRule',      label: '채팅 쓰기',         desc: '웹 채팅에 글·반응·답장을 쓰는 동작. 읽기는 멤버라면 항상 된다.' },
  { key: 'playbackRule',  label: '재생·일시정지·스킵', desc: '지금 나오는 곡을 직접 조작하는 동작. 모두에게 즉시 영향이 간다.' },
  { key: 'seekRule',      label: '구간 이동(시크)',    desc: '진행바를 끌어 재생 위치를 옮기는 동작.' },
  { key: 'volumeRule',    label: '볼륨 조절',         desc: '음성 채널 전체에 들리는 볼륨을 바꾸는 동작.' },
  { key: 'queueEditRule', label: '대기열 편집',       desc: '남의 곡을 지우거나 순서를 바꾸는 동작. 자기가 넣은 곡을 빼는 건 항상 된다.' },
];

/** 정렬 모드 3종. 각 모드에 한 문단 설명(요구사항). */
const SORT_MODES = [
  {
    value: 'score', label: '점수제',
    desc: '좋아요를 많이 받은 곡이 먼저 나온다. 오래 기다린 곡에는 대기 점수가 자동으로 붙어서 언젠가는 순서가 온다. ' +
          '분위기에 맞는 곡이 빨리 나오는 대신, 한 사람이 곡을 몰아서 넣고 친구들이 눌러주면 그 사람 곡만 계속 나올 수 있다.',
  },
  {
    value: 'fifo', label: '시간제',
    desc: '먼저 신청한 순서 그대로 나온다. 좋아요는 표시만 되고 순서를 바꾸지 않는다. ' +
          '규칙이 가장 단순하고 예측 가능한 대신, 미리 여러 곡을 넣어둔 사람이 오래 독점한다.',
  },
  {
    value: 'fair', label: '공평제',
    desc: '사람별로 돌아가며 한 곡씩 재생한다. 미리 다섯 곡을 넣어둬도 첫 바퀴에서는 한 곡만 나가고, ' +
          '늦게 들어온 사람도 다음 차례에 바로 들어온다. 사람이 많고 신청이 몰릴 때 가장 덜 싸운다.',
  },
];

const REPEAT_MODES = [
  { value: 'off',   label: '반복 없음', desc: '대기열이 비면 재생이 멈춘다.' },
  { value: 'track', label: '한 곡 반복', desc: '지금 곡을 계속 다시 튼다. 대기열은 그대로 대기한다.' },
  { value: 'queue', label: '전체 반복', desc: '대기열 끝까지 가면 처음으로 돌아간다.' },
];

/** 숫자 항목 정의 — 슬라이더 + 직접입력 + 단위 + 허용범위 + 한 줄 설명(구림 해소 #1, #6). */
const NUM_SPECS = {
  minVolume:          { label: '최소 볼륨',      min: 0,  max: 100,    step: 5,  unit: '%',  desc: '이 아래로는 볼륨을 못 내린다. 0이면 음소거까지 허용.' },
  maxVolume:          { label: '최대 볼륨',      min: 10, max: 200,    step: 5,  unit: '%',  desc: '멤버가 올릴 수 있는 볼륨 상한. 관리자도 이 값을 넘기지 못한다.' },
  defaultVolume:      { label: '기본 볼륨',      min: 0,  max: 200,    step: 5,  unit: '%',  desc: '봇이 음성 채널에 새로 들어갈 때 시작하는 볼륨.' },
  maxQueuePerUser:    { label: '1인 대기열 수',  min: 1,  max: 100,    step: 1,  unit: '곡', desc: '한 사람이 동시에 대기열에 넣어둘 수 있는 곡 수. 작을수록 골고루 돌아간다.' },
  maxQueuePerGuild:   { label: '서버 대기열 수', min: 1,  max: 1000,   step: 10, unit: '곡', desc: '서버 전체 대기열 상한. 넘으면 새 신청이 거절된다.' },
  maxTrackSeconds:    { label: '곡 최대 길이',   min: 60, max: 86400,  step: 60, unit: '초', desc: '이보다 긴 곡은 신청할 수 없다. 몇 시간짜리 라이브 통짜 등록을 막는다.', pretty: prettySeconds },
  auditRetentionDays: { label: '로그 보관일',    min: 1,  max: 3650,   step: 1,  unit: '일', desc: '활동 기록을 며칠 보관할지. 지난 기록은 하루 한 번 정리된다.' },
  chatRetentionDays:  { label: '채팅 보관일',    min: 1,  max: 365,    step: 1,  unit: '일', desc: '웹 채팅을 며칠 보관할지. 기본 30일.' },
};

/** 섹션이 소유하는 설정 키 — 부분 저장(구림 해소 #7)의 단위. 한 키는 정확히 한 섹션에만 속한다. */
const SECTION_KEYS = {
  order:  ['sortMode', 'autoBgmEnabled', 'repeatMode', 'defaultVolume'],
  perms:  PERM_FIELDS.map((field) => field.key).concat(['configuredRoleIds']),
  limits: ['minVolume', 'maxVolume', 'maxQueuePerUser', 'maxQueuePerGuild', 'maxTrackSeconds', 'auditRetentionDays', 'chatRetentionDays'],
  chat:   ['chatEnabled', 'suggestionEnabled'],
};

/** 정지 범위 · 기간 (사양서 결정 #14). */
const SUSPEND_SCOPES = [
  { value: 'all',   label: '전체',    desc: '리모컨의 모든 조작을 막는다. 보기만 된다.' },
  { value: 'chat',  label: '채팅만',  desc: '채팅·반응·답장만 막는다. 곡 신청은 계속 된다.' },
  { value: 'queue', label: '신청만',  desc: '곡 신청·좋아요만 막는다. 채팅은 계속 된다.' },
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

/** 접속 상태 우선순위 배지 (사양서 §2.2). */
const PRESENCE_LABEL = {
  listening: ['🎧', '듣는 중', 'dot--listening'],
  viewing:   ['🖥', '보는 중', 'dot--viewing'],
  online:    ['🟢', '온라인',  'dot--online'],
  idle:      ['🌙', '자리비움', 'dot--idle'],
  dnd:       ['⛔', '다른 용무', 'dot--dnd'],
  offline:   ['⚪', '오프라인', 'dot--offline'],
};

/* ═══════════════════════════ 상태 ═══════════════════════════ */

const S = {
  activeSection: 'order',
  saved: null,      // 서버가 준 마지막 저장본 (baseline)
  draft: null,      // 편집 중인 사본
  roles: [],        // 길드 역할 목록
  saving: false,
  /** 섹션별 지연 로드 데이터 */
  queuePreview: { mode: null, data: null, loading: false },
  permPreview: {},  // rule → { passCount, memberCount, ... }
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

const clone = (value) => JSON.parse(JSON.stringify(value));
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

/** 만료 UTC → "2시간 14분 남음". null 이면 무기한. */
function untilLabel(expiresUtc) {
  if (!expiresUtc) return '무기한';
  const left = Date.parse(expiresUtc) - Date.now();
  if (!Number.isFinite(left) || left <= 0) return '곧 해제';
  const minutes = Math.floor(left / 60000);
  if (minutes < 60) return `${Math.max(1, minutes)}분 남음`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}시간 ${minutes % 60}분 남음`;
  return `${Math.floor(hours / 24)}일 ${hours % 24}시간 남음`;
}

function scopeLabel(scope) {
  const found = SUSPEND_SCOPES.find((item) => item.value === scope);
  return found ? found.label : scope;
}

function ruleLabel(value) {
  const found = RULE_OPTIONS.find((item) => item.value === value);
  return found ? found.label : value;
}

/** list()는 core.js 계약. 여기 한 곳만 거치게 해서 시그니처가 달라져도 수정 지점을 하나로 묶는다. */
function renderList(box, items, keyOf, render) {
  list(box, items, keyOf, render);
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
    if (label) label.textContent = count ? `변경한 항목 ${count}개` : '변경한 항목이 없다';
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
    errors.minVolume = '최소 볼륨이 최대 볼륨보다 큽니다.';
  }
  if (Number(S.draft.defaultVolume) < Number(S.draft.minVolume) || Number(S.draft.defaultVolume) > Number(S.draft.maxVolume)) {
    errors.defaultVolume = `기본 볼륨은 최소~최대(${S.draft.minVolume}~${S.draft.maxVolume}%) 안이어야 합니다.`;
  }
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
      h('span', { class: 'fld__badge' }, '변경됨'),
      h('button', {
        class: 'fld__undo', type: 'button', 'data-tip': '이 항목만 저장 전 값으로',
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
      h('span', { class: 'num__range-label' }, `허용 ${spec.min}~${spec.max}${spec.unit}`),
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

  const body = h('div', { class: 'sec__body' },
    modeField,
    fieldShell('autoBgmEnabled', '자동 BGM',
      '대기열이 비면 최근에 나온 곡과 비슷한 곡을 알아서 이어 튼다. 끄면 조용해진다.',
      toggleControl('autoBgmEnabled', '대기열이 비면 알아서 이어 튼다', '대기열이 비면 재생을 멈춘다')),
    fieldShell('repeatMode', '반복',
      REPEAT_MODES.find((item) => item.value === S.draft.repeatMode)?.desc || null,
      segmentControl('repeatMode', REPEAT_MODES)),
    numberField('defaultVolume', { min: Number(S.draft.minVolume), max: Number(S.draft.maxVolume) }),
  );

  renderQueuePreview(previewBox);
  loadQueuePreview(mode);
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
    h('span', { class: 'hint' }, '저장해야 실제로 바뀐다'),
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
    box.append(h('p', { class: 'hint' }, `미리보기를 불러오지 못했다 — ${preview.data.error}`));
    return;
  }
  const items = preview.data.items || [];
  if (!items.length) {
    box.append(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '🎵'),
      h('div', { class: 'empty__title' }, '대기열이 비어 있다'),
      h('div', { class: 'empty__desc' }, '곡이 쌓이면 여기서 순서가 어떻게 바뀌는지 미리 볼 수 있다.'),
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
      h('span', { class: 'prev__title mq' }, h('span', { class: 'mq__i' }, item.title || '(제목 없음)')),
      h('span', { class: 'prev__who' }, item.roundLabel || item.requestedBy || ''),
      h('span', { class: `prev__delta ${tone}` },
        delta === 0 ? '그대로' : `지금 ${item.currentPosition}위 ${arrow}`),
    ));
  });
  box.append(rows);
  if (items.length > 10) {
    box.append(h('p', { class: 'hint' }, `아래로 ${items.length - 10}곡 더 있다.`));
  }
}

/* ═══════════════════════════ 섹션 2 · 권한 ═══════════════════════════ */

function sectionPerms() {
  const body = h('div', { class: 'sec__body' });

  PERM_FIELDS.forEach((field) => {
    const value = S.draft[field.key];
    const option = RULE_OPTIONS.find((item) => item.value === value);
    const preview = h('div', { class: 'permprev' });
    const warn = h('div', { class: 'warnbox', hidden: value !== 'disabled' },
      h('span', null, '⚠'),
      h('span', null, '"사용 안 함"은 예외가 없다. 서버 관리자와 봇 주인도 이 기능을 쓸 수 없다. 기능을 완전히 끄는 선택이다.'),
    );

    const control = h('div', { class: 'permrow' },
      selectControl(field.key, RULE_OPTIONS, (next) => {
        warn.hidden = next !== 'disabled';
        const note = body.querySelector(`[data-rule-note="${field.key}"]`);
        if (note) note.textContent = (RULE_OPTIONS.find((item) => item.value === next) || {}).desc || '';
        loadPermPreview(field.key, next, preview);
      }),
      h('span', { class: 'permrow__note', 'data-rule-note': field.key }, option ? option.desc : ''),
    );

    body.append(fieldShell(field.key, field.label, field.desc, control,
      h('div', null, preview, warn)));
    loadPermPreview(field.key, value, preview);
  });

  // 지정 역할 — 규칙에서 "지정 역할"을 골랐을 때 대상이 되는 역할, 그리고 관리자 대우를 받는 역할.
  const roleUsed = PERM_FIELDS.some((field) => S.draft[field.key] === 'configuredRole');
  body.append(fieldShell('configuredRoleIds', '지정 역할 / 관리자 역할',
    '여기서 고른 역할을 가진 사람은 "지정 역할" 규칙을 통과하고, 서버 관리 콘솔에도 들어올 수 있다. 즉 관리자 대우다.',
    roleChecklist(),
    roleUsed ? null : h('p', { class: 'hint' }, '지금은 "지정 역할"을 쓰는 규칙이 없다. 그래도 여기 고른 역할은 관리 콘솔 접근 권한을 갖는다.'),
  ));

  return body;
}

function roleChecklist() {
  const box = h('div', { class: 'roles' });
  if (!S.roles.length) {
    box.append(h('p', { class: 'hint' }, '역할 목록을 불러오는 중이다.'));
    return box;
  }
  const picked = new Set((S.draft.configuredRoleIds || []).map(String));
  S.roles.forEach((role) => {
    const on = picked.has(String(role.id));
    box.append(h('button', {
      class: 'role' + (on ? ' is-on' : ''), type: 'button',
      'aria-pressed': on ? 'true' : 'false',
      'data-tip': `멤버 ${role.memberCount != null ? role.memberCount : '?'}명`,
      onclick: () => {
        const next = new Set((S.draft.configuredRoleIds || []).map(String));
        if (next.has(String(role.id))) next.delete(String(role.id)); else next.add(String(role.id));
        setValue('configuredRoleIds', Array.from(next));
        renderSection('perms');
      },
    },
      h('span', { class: 'role__dot', style: role.color ? `background:${role.color}` : '' }),
      h('span', { class: 'role__name' }, role.name),
      h('span', { class: 'role__count' }, role.memberCount != null ? `${role.memberCount}명` : ''),
    ));
  });
  return box;
}

/** 고른 규칙이 "지금 이 서버에서 몇 명을 통과시키는지" (구림 해소 #3). */
const permPreviewTimers = {};
function loadPermPreview(key, rule, box) {
  clearTimeout(permPreviewTimers[key]);
  box.replaceChildren(h('span', { class: 'permprev__skel skel' }));
  permPreviewTimers[key] = setTimeout(async () => {
    try {
      const roles = (S.draft.configuredRoleIds || []).join(',');
      const data = await api(`/admin/permission-preview?rule=${encodeURIComponent(rule)}&roleIds=${encodeURIComponent(roles)}`);
      S.permPreview[key] = data;
      paintPermPreview(box, data, rule);
    } catch (error) {
      box.replaceChildren(h('span', { class: 'permprev__fail' }, `통과 인원을 못 셌다 — ${error.message}`));
    }
  }, 180);
}

function paintPermPreview(box, data, rule) {
  const pass = Number(data.passCount || 0);
  const total = Number(data.memberCount || 0);
  const tone = rule === 'disabled' ? 'is-none' : pass === 0 ? 'is-none' : pass === total ? 'is-all' : 'is-some';
  const kids = [
    h('span', { class: `permprev__count ${tone}` },
      rule === 'disabled' ? '지금 통과: 0명 (전원 차단)' : `지금 통과: ${pass}명 / 멤버 ${total}명`),
  ];
  if (data.managerBypassCount) {
    kids.push(h('span', { class: 'permprev__note' }, `그중 ${data.managerBypassCount}명은 관리자라서 통과`));
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
      h('p', { class: 'grp__desc' }, '멤버가 조절할 수 있는 범위. 기본 볼륨은 "순서와 재생"에 있다.'),
      numberField('minVolume'),
      numberField('maxVolume'),
    ),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '대기열'),
      h('p', { class: 'grp__desc' }, '한 사람이 얼마나 넣을 수 있고 서버 전체로는 얼마까지 받는지.'),
      numberField('maxQueuePerUser'),
      numberField('maxQueuePerGuild'),
      numberField('maxTrackSeconds'),
    ),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '보관 기간'),
      h('p', { class: 'grp__desc' }, '오래된 기록은 자동으로 지운다. 길게 잡으면 DB가 커진다.'),
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
    h('p', { class: 'grp__desc' }, '기간이 지나면 자동으로 풀린다. 무기한은 직접 풀어야 한다.'),
    activeBox,
  ));

  const listBox = h('div', { class: 'card userlist' });
  const filter = h('input', {
    class: 'field', type: 'search', placeholder: '이름으로 찾기',
    oninput: (event) => paintParticipants(listBox, event.target.value),
  });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '참여자'),
    h('p', { class: 'grp__desc' }, '이 서버에서 리모컨으로 채팅했거나 곡을 신청해 본 사람. 접속 상태는 실시간으로 바뀐다.'),
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
    toast(`참여자 목록을 못 불러왔다 — ${error.message}`, 'danger');
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
      h('div', { class: 'empty__title' }, '정지 중인 사람이 없다'),
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
          item.byDisplayName ? ` · ${item.byDisplayName}이 처리` : ''),
      ),
      h('button', {
        class: 'btn btn--sm', type: 'button',
        onclick: () => liftSuspension(item),
      }, '해제'),
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
      h('div', { class: 'empty__title' }, '해당하는 사람이 없다'),
      h('div', { class: 'empty__desc' }, '리모컨에서 채팅하거나 곡을 신청하면 여기에 나타난다.'),
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
    ? '봇 주인은 정지할 수 없다.'
    : '관리자는 다른 관리자를 정지할 수 없다. 봇 주인만 가능하다.';

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
        person.lastSeenUtc ? h('span', null, `· ${fmtAgo(person.lastSeenUtc)} 활동`) : null,
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
      'data-tip': blocked ? blockReason : '기능과 기간을 골라 정지한다',
      onclick: () => openSuspendSheet(person),
    }, blocked ? '정지 불가' : '정지'),
  );
}

/** 정지 시트 — 범위 × 기간 + 사유. */
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
    class: 'field', placeholder: '사유 (본인에게 보인다)', maxlength: '120',
    oninput: (event) => { reason = event.target.value; },
  });

  const body = h('div', { class: 'sheetform' },
    h('label', { class: 'sheetform__label' }, '무엇을 막을지'), scopeBox, scopeNote,
    h('label', { class: 'sheetform__label' }, '얼마나'), durationBox,
    h('label', { class: 'sheetform__label' }, '사유'), reasonInput,
  );

  const ok = await confirmSheet({
    title: `${person.displayName || person.userId} 정지`,
    body,
    confirmText: '정지',
    cancelText: '취소',
    danger: true,
  });
  if (!ok) return;
  try {
    await api('/admin/suspensions', {
      method: 'POST',
      body: { userId: String(person.userId), scope, minutes, reason },
    });
    toast('정지했다.', 'ok');
    await loadUsers();
    renderSection('users');
  } catch (error) {
    toast(`정지 실패 — ${error.message}`, 'danger');
  }
}

async function liftSuspension(item) {
  const ok = await confirmSheet({
    title: '정지 해제',
    body: `${item.displayName || item.userId}의 ${scopeLabel(item.scope)} 정지를 지금 푼다.`,
    confirmText: '해제',
  });
  if (!ok) return;
  try {
    await api('/admin/suspensions/lift', { method: 'POST', body: { userId: String(item.userId), scope: item.scope } });
    toast('정지를 풀었다.', 'ok');
    await loadUsers();
    renderSection('users');
  } catch (error) {
    toast(`해제 실패 — ${error.message}`, 'danger');
  }
}

/* ═══════════════════════════ 섹션 5 · 채팅과 제안 ═══════════════════════════ */

function sectionChat() {
  const body = h('div', { class: 'sec__body' });

  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '웹 채팅'),
    h('p', { class: 'grp__desc' }, '리모컨 안에서만 오가는 채팅이다. Discord 채널과는 연결되지 않는다.'),
    fieldShell('chatEnabled', '채팅 사용',
      '끄면 기존 대화는 남지만 아무도 새로 쓸 수 없다. 읽기도 막고 싶으면 권한에서 채팅 쓰기를 "사용 안 함"으로 둔다.',
      toggleControl('chatEnabled', '멤버가 채팅을 쓸 수 있다', '채팅이 꺼져 있다')),
    // 보관 기간의 실제 값은 "제한값" 섹션이 소유한다. 여기서는 현재 값만 보여주고 그쪽으로 보낸다.
    h('div', { class: 'mirror' },
      h('div', null,
        h('div', { class: 'mirror__label' }, '채팅 보관 기간'),
        h('div', { class: 'mirror__value' }, `${S.draft.chatRetentionDays}일`),
        h('p', { class: 'hint' }, '이 값은 "제한값" 섹션에서 다른 보관 기간들과 함께 관리한다.'),
      ),
      h('button', { class: 'btn btn--sm', type: 'button', onclick: () => goSection('limits') }, '제한값에서 변경 →'),
    ),
  ));

  const reportsBox = h('div', { class: 'card' });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '신고된 메시지'),
    h('p', { class: 'grp__desc' }, '멤버가 신고한 채팅이다. 지우거나 문제없음으로 넘긴다.'),
    reportsBox,
  ));

  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '제안 게시판'),
    h('p', { class: 'grp__desc' }, '멤버가 앱 개선 제안을 올리고 공감을 누른다. 관리자가 상태를 정한다.'),
    fieldShell('suggestionEnabled', '제안 게시판 사용',
      '끄면 새 제안을 받지 않는다. 이미 올라온 제안은 계속 보인다.',
      toggleControl('suggestionEnabled', '멤버가 제안을 올릴 수 있다', '제안 접수를 닫았다')),
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
    toast(`채팅·제안을 못 불러왔다 — ${error.message}`, 'danger');
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
      h('div', { class: 'empty__title' }, '처리할 신고가 없다'),
    ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  S.reports.forEach((report) => {
    rows.append(h('li', { class: 'row row--report' },
      h('div', { class: 'row__main' },
        h('div', { class: 'row__name' }, `${report.messageAuthor}의 메시지`),
        h('blockquote', { class: 'quote' }, report.messageContent || '(내용 없음)'),
        h('div', { class: 'row__sub' },
          `${report.reporterDisplayName}이 신고 · ${report.reason || '사유 없음'} · ${fmtAgo(report.createdUtc)}`),
      ),
      h('div', { class: 'row__acts' },
        h('button', { class: 'btn btn--sm btn--danger', type: 'button', onclick: () => resolveReport(report, 'delete') }, '메시지 삭제'),
        h('button', { class: 'btn btn--sm', type: 'button', onclick: () => resolveReport(report, 'dismiss') }, '문제 없음'),
      ),
    ));
  });
  box.replaceChildren(rows);
}

async function resolveReport(report, action) {
  if (action === 'delete') {
    const ok = await confirmSheet({
      title: '메시지 삭제', body: '이 채팅을 지운다. 되돌릴 수 없다.',
      confirmText: '삭제', danger: true,
    });
    if (!ok) return;
  }
  try {
    await api(`/admin/reports/${report.id}/resolve`, { method: 'POST', body: { action } });
    S.reports = S.reports.filter((item) => item.id !== report.id);
    toast(action === 'delete' ? '메시지를 지웠다.' : '신고를 닫았다.', 'ok');
    renderSection('chat');
  } catch (error) {
    toast(`처리 실패 — ${error.message}`, 'danger');
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
      h('div', { class: 'empty__title' }, '올라온 제안이 없다'),
      h('div', { class: 'empty__desc' }, '멤버가 리모컨의 제안 탭에서 글을 올리면 여기 모인다.'),
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
    toast('제안 상태를 바꿨다.', 'ok');
    renderSection('chat');
  } catch (error) {
    toast(`상태 변경 실패 — ${error.message}`, 'danger');
  }
}

/* ═══════════════════════════ 섹션 6 · 활동 기록 ═══════════════════════════ */

function sectionAudit() {
  const rows = h('ul', { class: 'rows rows--audit' });
  const sentinel = h('div', { class: 'audit__more' });

  const filter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'audit-filter',
    placeholder: '사람 · 동작 · 곡 제목으로 찾기',
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
    sentinel.replaceChildren(h('p', { class: 'hint' }, `기록을 못 불러왔다 — ${error.message}`));
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
      h('div', { class: 'empty__title' }, '조건에 맞는 기록이 없다'),
    )));
  } else {
    renderList(rows, S.audit.items, (entry) => String(entry.id), (entry) => auditRow(entry));
  }
  sentinel.replaceChildren(S.audit.done && S.audit.items.length
    ? h('p', { class: 'hint' }, '여기까지가 전부다.')
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
    h('p', { class: 'grp__desc' }, '문제 신고를 받으면 이 값들을 먼저 확인한다.'),
    box,
    h('button', {
      class: 'btn', type: 'button', style: 'margin-top:12px',
      onclick: () => { S.diag = null; renderSection('diag'); },
    }, '다시 확인'),
  ));

  paintDiag(box);
  if (!S.diag) loadDiag().then(() => paintDiag(box));
  return body;
}

/** 특권 인텐트가 꺼져 있으면 켜는 방법까지 안내한다 (사양서 §2.3). */
function intentCards() {
  const status = M.intentStatus || {};
  const rows = [
    { key: 'members',   label: 'Server Members Intent',  what: '전체 멤버 목록과 역할을 읽는다. 꺼져 있으면 참여자 목록이 리모컨을 써본 사람만으로 줄어든다.' },
    { key: 'presences', label: 'Presence Intent',        what: 'Discord 온라인/자리비움 표시를 읽는다. 꺼져 있으면 접속 표시가 "보는 중 / 듣는 중"만 남는다.' },
    { key: 'voiceStates', label: 'Voice States',         what: '누가 어느 음성 채널에 있는지 읽는다. "같은 음성 채널" 권한 규칙이 이걸 쓴다.' },
  ];
  const box = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '인텐트'),
    h('p', { class: 'grp__desc' }, 'Discord가 봇에게 어떤 정보를 주는지. 꺼져 있어도 봇은 죽지 않고 관련 표시만 줄어든다.'),
  );
  rows.forEach((row) => {
    const on = status[row.key] !== false;
    box.append(h('div', { class: 'diagrow' + (on ? '' : ' is-off') },
      h('span', { class: 'diagrow__flag' }, on ? '✅' : '⚠'),
      h('div', null,
        h('div', { class: 'diagrow__label' }, row.label, h('span', { class: `chip ${on ? 'chip--ok' : 'chip--warn'}` }, on ? '켜짐' : '꺼짐')),
        h('p', { class: 'hint' }, row.what),
        on ? null : h('p', { class: 'hint' },
          'Discord 개발자 포털 → 내 애플리케이션 → Bot → Privileged Gateway Intents 에서 켠 뒤 봇을 재시작하면 된다.'),
      ),
    ));
  });
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
    box.replaceChildren(h('p', { class: 'hint' }, `진단 정보를 못 불러왔다 — ${S.diag.error}`));
    return;
  }
  const bot = S.diag.bot || {};
  const cells = [
    ['봇 연결', bot.online ? '온라인' : '오프라인', bot.online ? 'is-ok' : 'is-bad'],
    ['음성 채널', bot.voiceConnected ? `연결됨 (${bot.voiceChannelName || '이름 미상'})` : '연결 안 됨', bot.voiceConnected ? 'is-ok' : ''],
    ['게이트웨이 지연', bot.gatewayLatencyMs != null ? `${bot.gatewayLatencyMs}ms` : '알 수 없음', ''],
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

  if (S.diag.buildId && M.buildId && S.diag.buildId !== M.buildId) {
    box.append(h('div', { class: 'warnbox' },
      h('span', null, '⚠'),
      h('span', null, `열어둔 화면이 옛 버전이다(${M.buildId}). 새로고침하면 ${S.diag.buildId}로 바뀐다.`),
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
      h('span', { class: 'sec__footnote' }, '변경한 항목이 없다'),
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
  toast('저장 전 값으로 되돌렸다.');
}

async function saveSection(id) {
  const keys = dirtyKeys(id);
  if (!keys.length || S.saving) return;
  const errors = validate();
  const blocked = (SECTION_KEYS[id] || []).filter((key) => errors[key]);
  if (blocked.length) {
    toast('값이 서로 맞지 않는다. 빨간 설명을 먼저 고쳐라.', 'warn');
    return;
  }

  S.saving = true;
  refreshDirty();
  const payload = {};
  (SECTION_KEYS[id] || []).forEach((key) => { payload[key] = S.draft[key]; });

  try {
    const result = await api(`/admin/settings/${id}`, { method: 'PUT', body: payload });
    // 서버가 정규화한 값을 돌려주면 그걸 기준으로 삼는다.
    const applied = (result && result.settings) || payload;
    Object.keys(applied).forEach((key) => {
      S.saved[key] = clone(applied[key]);
      S.draft[key] = clone(applied[key]);
    });
    toast(`저장했다 · ${keys.length}개 항목`, 'ok');
    if (keys.includes('sortMode')) S.queuePreview = { mode: null, data: null, loading: false };
  } catch (error) {
    toast(`저장 실패 — ${error.message}`, 'danger');
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
    title: '저장하지 않은 변경이 있다',
    body: `${count}개 항목을 바꿔 놓고 아직 저장하지 않았다. 이대로 나가면 변경은 사라진다.`,
    confirmText: '변경 버리고 이동',
    cancelText: '여기 남기',
    danger: true,
  });
}

async function goSection(id) {
  if (id === S.activeSection) return;
  const dirty = dirtyKeys(S.activeSection);
  if (dirty.length) {
    const ok = await confirmSheet({
      title: `"${(SECTIONS.find((item) => item.id === S.activeSection) || {}).label}" 섹션에 저장 안 한 변경이 있다`,
      body: `${dirty.length}개 항목이 저장 전이다. 지금 이동하면 이 섹션의 변경은 사라진다.`,
      confirmText: '변경 버리고 이동',
      cancelText: '남아서 저장',
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
    'data-tip': '유저용 리모컨 화면으로',
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
      'data-tip': '밝게 / 어둡게',
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
      h('h1', null, '서버 관리 콘솔은 관리자만 들어올 수 있다'),
      h('p', { class: 'hint' }, '이 서버에서 관리 권한이 없다. 서버 관리자에게 지정 역할을 받으면 들어올 수 있다.'),
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
    S.saved = settings.settings || settings;
    S.draft = clone(S.saved);
    S.roles = roles.roles || [];
  } catch (error) {
    main.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '⚠'),
      h('div', { class: 'empty__title' }, '설정을 불러오지 못했다'),
      h('div', { class: 'empty__desc' }, error.message),
      h('button', { class: 'btn btn--primary', onclick: () => location.reload() }, '다시 시도'),
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

  // WS — 콘솔이 열려 있는 동안 정지/설정/접속 변화를 따라간다.
  connect({
    guildId: GUILD_ID,
    csrf: M.csrf,
    onEvent: (topic, data) => onRemoteEvent(topic, data),
  });

  // 연결 끊김 표시는 core.js 스토어를 그대로 쓴다.
  if (store && typeof store.subscribe === 'function') {
    store.subscribe((next) => {
      document.body.classList.toggle('is-offline', next && next.connected === false);
    });
  }
}

/** WS 이벤트 머지 — 전체 재조회는 하지 않는다 (성능 계약 §5.2 B). */
function onRemoteEvent(topic, data) {
  if (topic === 'presence' && S.participants) {
    const listening = new Set((data && data.listening) || []);
    const viewing = new Set((data && data.viewing) || []);
    const online = (data && data.online) || {};
    S.participants.forEach((person) => {
      const id = String(person.userId);
      person.presence = listening.has(id) ? 'listening'
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
  if (topic === 'settings') {
    // 다른 관리자가 같은 서버 설정을 바꿨다. 내 편집분은 지우지 않고 알리기만 한다.
    if (anyDirty()) {
      toast('다른 관리자가 방금 설정을 바꿨다. 저장하면 내 값으로 덮어쓴다.', 'warn');
    } else {
      api('/admin/settings').then((next) => {
        S.saved = next.settings || next;
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
