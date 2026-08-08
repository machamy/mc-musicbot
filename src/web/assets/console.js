/* 마참뮤직 리모컨 v3 — 서버 관리 콘솔 (/music/guilds/{guild_id}/admin)
 *
 * 기존 "설정 모달"을 대체한다. 사양서 §4.2의 "구림" 해소 기준 8개를 전부 만족시키는 것이 합격 기준이다.
 *   1) 항목마다 한 줄 설명   2) 섹션 묶음 + 섹션 목적   3) 권한 드롭다운 즉시 통과 인원
 *   4) 정렬 모드 대기열 미리보기  5) 변경분 강조 + 되돌리기 + 이탈 확인
 *   6) 숫자는 슬라이더 + 직접입력 + 단위 + 허용범위  7) 섹션 단위 부분 저장 + 토스트
 *   8) 1024px 이하에서 좌측 네비가 상단 가로 스크롤 탭으로 (네비는 어떤 폭에서도 사라지지 않는다)
 *
 * v3 추가분 (docs/REMOTE-API-V3.md)
 *   §1    권한 10개가 각각 자기 지정 역할(`ruleRoleIds`)을 갖는다. 관리자 역할(`managerRoleIds`)은 별도 카드.
 *   §4    진단에 봇의 서버/음성 참가 여부와 듣는 사람 수를 노출한다.
 *   §8    자동 재생 방식 3종 · 추천 정책 4종 · 기준 곡(시드) · 최근 N곡 · 장르.
 *   §10   투표 점수 4종 + 미리보기, 붐따(싫어요로 대기열에서 내리기), 투표 스킵, 슈퍼 좋아요 제한.
 *   §15   우리 차트 가중치 + 차트 관리(켜기/끄기·순서·주소).
 *   §18.1 1인 1000곡 / 서버 10000곡.
 *   §19.3 차단 목록 섹션 — 전역 항목은 읽기 전용, 시험 입력창 포함.
 *   §20   아이콘만 있는 버튼은 예외 없이 `data-tip`. 네이티브 `title=` 은 쓰지 않는다.
 *   §23.1 숫자 설정은 전부 `0 = 무제한`. 슬라이더 최댓값 다음 칸이 `∞` 다.
 *   §23.3 막힌 컨트롤에는 "왜 안 되는지" + "누가 되는지(역할 + 인원수)" 를 붙인다.
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
  { id: 'blocked', icon: '🚫', label: '차단 목록',   desc: '이 서버에서 안 나왔으면 하는 곡을 막아요. 봇 전체 규칙은 보이기만 하고 못 지워요.' },
  { id: 'audit',   icon: '📜', label: '활동 기록',   desc: '이 서버에서 일어난 모든 조작 기록이에요. 누가 언제 무엇을 바꿨는지 남아요. 여기서는 합치지 않고 하나하나 다 보여드려요.' },
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
 * 권한 10종 (v3 §1 · §10.5 · §8.3 · §15.4). `key` 는 설정 JSON의 필드명, `permKey` 는 권한 키다.
 * `permKey` 는 `ruleRoleIds` 의 키이자 `permission-preview?key=` 로 보내는 값이라 철자가 정확해야 한다.
 * 관리자 역할(`managerRoleIds`)까지 세면 11종이고, 그건 아래 별도 카드에서 다룬다.
 */
const PERM_FIELDS = [
  { key: 'searchRule',      permKey: 'search',      label: '곡 검색·신청',        desc: '검색해서 대기열에 곡을 한 곡씩 넣는 동작이에요. 막으면 아무도 새 곡을 못 넣어요.' },
  { key: 'voteRule',        permKey: 'vote',        label: '좋아요·싫어요·슈퍼',  desc: '대기열 곡에 점수를 주는 동작이에요. 점수제일 때만 실제 순서에 영향을 줘요.' },
  { key: 'chatRule',        permKey: 'chat',        label: '채팅 쓰기',           desc: '웹 채팅에 글·반응·답장을 쓰는 동작이에요. 읽기는 멤버라면 언제나 돼요.' },
  { key: 'playbackRule',    permKey: 'playback',    label: '재생·일시정지',       desc: '지금 나오는 곡을 멈추고 다시 트는 동작이에요. 모두에게 즉시 영향이 가요.' },
  { key: 'skipRule',        permKey: 'skip',        label: '곡 넘기기',           desc: '지금 곡을 다음으로 넘기는 동작이에요. 재생/일시정지와 성격이 달라서 따로 뒀어요. 투표 스킵을 켜면 여기 통과한 사람들끼리 표를 모아요.' },
  { key: 'seekRule',        permKey: 'seek',        label: '구간 이동(시크)',     desc: '진행바를 끌어 재생 위치를 옮기는 동작이에요.' },
  { key: 'volumeRule',      permKey: 'volume',      label: '볼륨 조절',           desc: '음성 채널 전체에 들리는 볼륨을 바꾸는 동작이에요. 웹에서 듣기의 개인 볼륨은 여기 해당하지 않아요.' },
  { key: 'queueEditRule',   permKey: 'queueEdit',   label: '대기열 편집',         desc: '남의 곡을 지우거나 순서를 바꾸는 동작이에요. 자기가 넣은 곡을 빼는 건 언제나 돼요.' },
  { key: 'autoplayRule',    permKey: 'autoplay',    label: '자동 재생 설정',      desc: '자동 재생을 켜고 끄고, 방식·기준 곡·최근 N곡·장르를 바꾸는 동작이에요. 기본은 모든 멤버예요.' },
  { key: 'bulkEnqueueRule', permKey: 'bulkEnqueue', label: '한 번에 담기',        desc: '재생목록이나 차트를 통째로 담는 동작이에요. 한 번에 수십 곡이 들어가서 대기열을 오래 차지할 수 있어요.' },
];

/** 붐따 동작 (v3 §10.3). */
const BOOMTTA_ACTIONS = [
  { value: 'bottom', label: '맨 뒤로',  desc: '대기열 맨 뒤로 보내요. 언젠가는 나오지만 한참 뒤예요.' },
  { value: 'remove', label: '아예 빼기', desc: '대기열에서 지워요. 되돌리려면 다시 신청해야 해요.' },
];

/** 투표 스킵 모수 (v3 §10.5). */
const VOTE_SKIP_BASIS = [
  { value: 'listeners', label: '듣는 사람', desc: '봇과 같은 음성 채널에 있는 사람만 세요. 실제로 듣는 사람이 정하니 제일 자연스러워요.' },
  { value: 'viewers',   label: '보는 사람', desc: '리모컨을 열어 두고 있는 사람을 세요. 음성엔 안 들어와도 같이 고르는 분위기에 맞아요.' },
  { value: 'either',    label: '둘 중 하나', desc: '듣는 사람이든 보는 사람이든 한쪽만 넘으면 넘어가요. 느슨해서 잘 넘어가요.' },
  { value: 'both',      label: '둘 다',      desc: '듣는 사람과 보는 사람 양쪽 다 넘어야 넘어가요. 엄격해서 잘 안 넘어가요.' },
];

/** 자동 재생 방식 3종 (v3 §8). */
const AUTOPLAY_MODES = [
  { value: 'seed',   label: '기준 곡',   desc: '아래에 등록한 기준 곡들을 돌아가며 참고해요. 취향을 확실히 잡고 싶을 때 좋아요.' },
  { value: 'recent', label: '최근 튼 곡', desc: '최근에 튼 몇 곡 중 하나를 무작위로 골라 참고해요. 지금까지의 동작이라 기본값이에요.' },
  { value: 'genre',  label: '장르',      desc: '고른 장르 차트에서 곡을 뽑아요. 장르를 여러 개 고르면 번갈아 가며 써요.' },
];

/** 자동 재생 추천 정책 4종 (v3 §8.5). 시드를 어디서 고르냐와는 다른 문제다. */
const AUTOPLAY_POLICIES = [
  { value: 'similar',  label: '비슷하게', desc: '후보 상위 3곡 중에서 골라요. 분위기가 잘 유지되지만 곡이 자주 겹쳐요.' },
  { value: 'balanced', label: '적당히',   desc: '후보 상위 10곡 중 앞쪽이 잘 뽑히게 골라요. 비슷하되 매번 달라서 기본값이에요.' },
  { value: 'explore',  label: '새롭게',   desc: '후보 전체에서 균등하게 골라요. 예상 못 한 곡이 자주 나와요.' },
  { value: 'popular',  label: '무난하게', desc: '후보 중 많이 들은 곡 위주로 골라요. 튀는 곡이 적어요.' },
];

/** 차트 분류 (v3 §15.2). 관리 콘솔의 차트 목록을 이 순서로 묶는다. */
const CHART_CATEGORIES = [
  { key: 'ours',       icon: '⭐', label: '우리 차트',  desc: '우리가 실제로 튼 곡으로 만드는 차트예요. 주소가 없고 지울 수도 없어요.' },
  { key: 'popular',    icon: '🔥', label: '인기',       desc: '전세계·한국·오늘 뜨는 곡이에요.' },
  { key: 'region',     icon: '🌏', label: '나라별',     desc: '미국·일본·영국 차트예요.' },
  { key: 'genre',      icon: '🎸', label: '장르',       desc: 'K-Pop·힙합·록 같은 장르 차트예요. 자동 재생의 "장르" 방식이 이 목록을 그대로 써요.' },
  { key: 'karaoke',    icon: '🎤', label: '노래방',     desc: 'TJ·금영 공식 재생목록이에요.' },
  { key: 'soundcloud', icon: '☁', label: 'SoundCloud', desc: 'SoundCloud 인기 차트예요.' },
];

/** 차단 규칙 종류 (models.rs 의 BlacklistKind). */
const BLACKLIST_KINDS = [
  { value: 'TitleContains', label: '제목 포함', desc: '제목에 이 글자가 들어가면 막아요. 가장 많이 쓰는 방식이에요.' },
  { value: 'TitleExact',    label: '제목 일치', desc: '제목이 정확히 같을 때만 막아요. 한 곡만 콕 집을 때 써요.' },
  { value: 'UrlExact',      label: '링크 일치', desc: '이 주소를 그대로 신청하면 막아요.' },
];

/** 활동 기록 분류 칩 (v3 §13.4). 관리 콘솔은 기본으로 전부 켠다. */
const AUDIT_KINDS = [
  { value: 'song',       icon: '🎵', label: '곡' },
  { value: 'vote',       icon: '👍', label: '투표' },
  { value: 'playback',   icon: '▶',  label: '재생' },
  { value: 'playlist',   icon: '📃', label: '재생목록' },
  { value: 'moderation', icon: '🛡', label: '관리' },
  { value: 'admin',      icon: '⚙',  label: '설정' },
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

/**
 * 숫자 항목 정의 — 슬라이더 + 직접입력 + 단위 + 허용범위 + 한 줄 설명(구림 해소 #1, #6).
 *
 * `unlimited: true` 면 v3 §23.1 의 무제한을 지원한다. 슬라이더 **최댓값 다음 칸이 `∞`** 이고
 * 거기로 밀면 값이 `0` 이 된다. 직접입력에 `0` 이나 빈 값을 넣어도 무제한이다.
 * `zeroLabel` 은 그때 화면에 뜨는 문구다 — 항목마다 "무제한"의 의미가 다르니 정확하게 쓴다.
 *
 * 무제한을 두지 않는 예외는 둘뿐이다(§23.1): 볼륨 3종(0~200 범위가 있어야 의미가 있다)과
 * 투표 스킵 비율(백분율이라 무제한이 말이 안 된다).
 */
const NUM_SPECS = {
  minVolume:          { label: '최소 볼륨',      min: 0,  max: 100,    step: 5,  unit: '%',  desc: '이 아래로는 볼륨을 못 내려요. 0이면 음소거까지 허용해요. 볼륨은 범위가 있어야 의미가 있어서 무제한이 없어요.' },
  maxVolume:          { label: '최대 볼륨',      min: 10, max: 200,    step: 5,  unit: '%',  desc: '멤버가 올릴 수 있는 볼륨 상한이에요. 관리자도 이 값을 넘기지 못해요. 볼륨은 범위가 있어야 의미가 있어서 무제한이 없어요.' },
  defaultVolume:      { label: '기본 볼륨',      min: 0,  max: 200,    step: 5,  unit: '%',  desc: '봇이 음성 채널에 새로 들어갈 때 시작하는 볼륨이에요. 최소~최대 볼륨 사이에서만 고를 수 있어서 무제한이 없어요.' },

  maxQueuePerUser: {
    label: '1인 대기열 수', min: 1, max: 1000, step: 1, unit: '곡', unlimited: true,
    zeroLabel: '무제한 · 한 사람이 얼마든지 넣을 수 있어요',
    desc: '한 사람이 동시에 대기열에 넣어 둘 수 있는 곡 수예요. 작을수록 골고루 돌아가요.',
    hint: (value) => (value === 0 ? '한 사람이 대기열을 통째로 차지할 수 있어요'
      : value <= 50 ? '골고루 돌아가요'
      : value <= 200 ? '넉넉해요'
      : '한 사람이 대기열을 오래 차지할 수 있어요'),
  },
  maxQueuePerGuild: {
    label: '서버 대기열 수', min: 1, max: 10000, step: 10, unit: '곡', unlimited: true,
    zeroLabel: '무제한 · 아무리 쌓여도 안 막아요',
    desc: '서버 전체 대기열 상한이에요. 넘으면 새 신청이 거절돼요.',
    hint: (value) => (value === 0 || value > 500
      ? '500곡을 넘으면 순서를 5초가 아니라 15초마다 다시 정해요'
      : '5초마다 순서를 다시 정해요'),
  },
  maxTrackSeconds: {
    label: '곡 최대 길이', min: 60, max: 86400, step: 60, unit: '초', unlimited: true,
    zeroLabel: '무제한 · 몇 시간짜리도 들어와요',
    desc: '이보다 긴 곡은 신청할 수 없어요. 몇 시간짜리 라이브 통짜 등록을 막아요.',
    pretty: prettySeconds,
  },
  auditRetentionDays: {
    label: '로그 보관일', min: 1, max: 3650, step: 1, unit: '일', unlimited: true,
    zeroLabel: '무제한 · 영원히 남겨요',
    desc: '활동 기록을 며칠 보관할지 정해요. 지난 기록은 하루 한 번 정리돼요. 투표·재생 기록은 3일만 남아요.',
  },
  chatRetentionDays: {
    label: '채팅 보관일', min: 1, max: 365, step: 1, unit: '일', unlimited: true,
    zeroLabel: '무제한 · 지우지 않아요',
    desc: '웹 채팅을 며칠 보관할지 정해요. 기본은 30일이에요.',
  },
  bulkEnqueueLimit: {
    label: '한 번에 담기 상한', min: 10, max: 1000, step: 10, unit: '곡', unlimited: true,
    zeroLabel: '무제한 · 한 번에 다 들어와요',
    desc: '재생목록이나 차트를 통째로 담을 때 한 번에 들어올 수 있는 곡 수예요. 클릭 한 번이 대기열을 몇천 곡으로 만들면 되돌리기가 너무 어려워요.',
  },

  /* ── 투표 점수 (v3 §10.1) ──
   * 여기 `0` 은 "무제한"이 아니라 **"그 투표를 점수에 안 센다"** 는 뜻이다. 음수도 뜻이 있는 값이라
   * §23.1 의 `0 = 무제한` 규약을 적용하면 오히려 의미가 뒤집힌다. 그래서 무제한 칸을 두지 않는다. */
  likePoints:      { label: '좋아요 점수',      min: -10, max: 10, step: 1, unit: '점', desc: '좋아요 하나가 몇 점인지예요. 기본 1점이에요. 0을 넣으면 좋아요를 점수에 안 세요 — 여기서 0은 무제한이 아니에요.' },
  dislikePoints:   { label: '싫어요 점수',      min: -10, max: 10, step: 1, unit: '점', desc: '싫어요 하나가 몇 점인지예요. 기본 -1점이라 순서가 뒤로 밀려요. 0을 넣으면 싫어요를 점수에 안 세요 — 여기서 0은 무제한이 아니에요.' },
  superLikePoints: { label: '슈퍼 좋아요 점수', min: -10, max: 10, step: 1, unit: '점', desc: '슈퍼 좋아요 하나가 몇 점인지예요. 기본 2점이에요. 0을 넣으면 슈퍼 좋아요를 점수에 안 세요 — 여기서 0은 무제한이 아니에요.' },
  waitPoints:      { label: '대기 점수',        min: -10, max: 10, step: 1, unit: '점', desc: '앞 곡이 하나 지날 때마다 붙는 점수예요. 오래 기다린 곡에 언젠가 차례가 오게 해요. 0을 넣으면 기다린 시간을 안 세요 — 여기서 0은 무제한이 아니에요.' },

  boomttaThreshold: {
    label: '붐따 기준', min: 1, max: 20, step: 1, unit: '개', unlimited: true,
    zeroLabel: '무제한 · 아무리 모여도 안 내려가요',
    desc: '싫어요가 이만큼 모이면 곡이 대기열에서 내려가요.',
  },

  /* ── 투표 스킵 (v3 §10.5) ── */
  voteSkipRatio: {
    label: '동의 비율', min: 10, max: 100, step: 5, unit: '%',
    desc: '모수의 몇 %가 동의해야 넘어갈지예요. 백분율이라 무제한은 없어요.',
  },
  voteSkipMin: {
    label: '최소 동의 인원', min: 1, max: 20, step: 1, unit: '명', unlimited: true,
    zeroLabel: '무제한 · 비율만 보고 정해요',
    desc: '비율로 계산한 값이 이보다 작아도 최소한 이만큼은 눌러야 해요. 모수가 1명이면 그 사람 혼자 눌러도 넘어가요.',
  },

  /* ── 슈퍼 좋아요 제한 (v3 §10.6) ── 기본은 둘 다 꺼짐(0)이다. */
  superLikeCooldownSec: {
    label: '슈퍼 좋아요 쿨타임', min: 5, max: 3600, step: 5, unit: '초', unlimited: true,
    zeroLabel: '쿨타임 없음 · 연달아 쓸 수 있어요',
    desc: '슈퍼 좋아요를 한 번 쓰면 이만큼 기다려야 다시 쓸 수 있어요. 취소해도 쿨타임은 안 돌려줘요.',
    pretty: prettySeconds,
  },
  superLikeDailyLimit: {
    label: '하루 슈퍼 좋아요 수', min: 1, max: 100, step: 1, unit: '번', unlimited: true,
    zeroLabel: '무제한 · 하루에 몇 번이든 돼요',
    desc: '한 사람이 하루에 쓸 수 있는 슈퍼 좋아요 횟수예요. UTC 자정에 초기화돼요. 취소하면 횟수는 돌려줘요.',
  },

  /* ── 자동 재생 (v3 §8) ── */
  autoplayRecentCount: {
    label: '참고할 최근 곡 수', min: 1, max: 20, step: 1, unit: '곡',
    desc: '"최근 튼 곡" 방식일 때 최근 몇 곡 중에서 기준을 고를지예요. 적어도 한 곡은 봐야 기준을 뽑을 수 있어서 무제한이 없어요.',
  },
  webSyncOffsetMs: {
    label: '전역 싱크 보정', min: -5000, max: 5000, step: 50, unit: 'ms',
    desc: '디스코드가 웹보다 늘 일정하게 늦거나 빠를 때 여기서 한 번에 맞춰요. 양수는 웹을 늦춰요.',
  },
  skipLeadMs: {
    label: '스킵 여유', min: 0, max: 5000, step: 100, unit: 'ms',
    desc: '스킵·되감기 때 몇 ms 뒤를 시작 시각으로 잡을지예요. 0이면 "지금부터"인데, 그 말이 사람마다 다른 시각에 도착해서 각자 다른 지점에서 시작해요.',
  },
  seekLockoutMs: {
    label: '진행바 잠금 구간', min: 0, max: 10000, step: 500, unit: 'ms',
    desc: '곡이 끝나기 이만큼 전부터는 위치를 못 옮겨요. 옮긴 게 반영되기 전에 다음 곡으로 넘어가면 웹만 옛 곡에 남아요.',
  },
  autoplayArtistCooldown: {
    // **0 은 "무제한"이 아니라 "끔"이다.** 이 값은 *막을 곡 수*라 0이면 아무도 안 막는다
    // (`autoplay.rs` 의 decay_factor 주석과 같은 이야기). 무제한이라고 적어 두면
    // "0을 넣으면 전부 막힌다"로 읽혀서 아무도 0을 안 쓴다.
    label: '같은 가수 쿨다운', min: 0, max: 20, step: 1, unit: '곡', unlimited: true,
    zeroLabel: '끔 · 같은 가수가 연달아 나와도 그냥 둬요',
    desc: '최근 이만큼의 곡 안에 나온 가수는 자동 재생 후보에서 빼요. 0으로 두면 이 제한을 아예 안 걸어요.',
  },
  autoplayRecentDecayHours: {
    label: '최근 곡 회피 시간', min: 1, max: 168, step: 1, unit: '시간', unlimited: true,
    zeroLabel: '무제한 · 한 번 튼 곡은 계속 피해요',
    desc: '최근에 튼 곡을 이 시간 동안 강하게 피하고, 지나면 다시 나와도 괜찮게 봐요. 영원히 빼면 고를 곡이 계속 줄어들어요.',
  },
  autoplaySeedMax: {
    label: '기준 곡 최대 개수', min: 1, max: 50, step: 1, unit: '곡', unlimited: true,
    zeroLabel: '무제한 · 얼마든지 넣을 수 있어요',
    desc: '자동 재생 기준 곡을 몇 곡까지 등록할 수 있는지예요.',
  },

  /* ── 우리 차트 (v3 §15.2b) ── 0 은 "슈퍼 좋아요를 아예 안 센다"는 뜻이라 무제한이 아니다. */
  chartLimit: {
    label: '차트 곡 수', min: 10, max: 100, step: 10, unit: '곡',
    desc: '차트 하나를 열 때 가져올 곡 수예요. 검색으로 만든 차트는 이 숫자만큼 찾아오고, 재생목록으로 만든 차트는 앞에서부터 이만큼만 써요. 많이 가져올수록 처음 여는 데 조금 더 걸려요.',
  },
  chartSuperWeight: {
    label: '슈퍼 좋아요 가중치', min: 0, max: 5, step: 1, unit: '배',
    desc: '"많이 사랑받은 곡" 차트에서 슈퍼 좋아요를 몇 배로 칠지예요. 배수라서 무제한이 없고, 0은 무제한이 아니라 "슈퍼 좋아요를 아예 안 센다"는 뜻이에요.',
  },
};

/** 섹션이 소유하는 설정 키 — 부분 저장(구림 해소 #7)의 단위. 한 키는 정확히 한 섹션에만 속한다. */
const SECTION_KEYS = {
  order: [
    'sortMode', 'autoBgmEnabled', 'repeatMode', 'defaultVolume',
    'likePoints', 'dislikePoints', 'superLikePoints', 'waitPoints',
    'boomttaEnabled', 'boomttaThreshold', 'boomttaAction',
    'voteSkipEnabled', 'voteSkipBasis', 'voteSkipRatio', 'voteSkipMin',
    'superLikeCooldownSec', 'superLikeDailyLimit',
    'autoplayMode', 'autoplayRecentCount', 'autoplayGenres',
    'autoplayPolicy', 'autoplayArtistCooldown', 'autoplayRecentDecayHours', 'autoplaySeedMax',
    'chartSuperWeight', 'chartLimit',
    // 재생 동작 (§31 · §36)
    'requireVoiceForPlayback', 'publicNowPlaying', 'webSyncOffsetMs', 'skipLeadMs', 'seekLockoutMs',
  ],
  perms:  PERM_FIELDS.map((field) => field.key).concat(['ruleRoleIds', 'managerRoleIds']),
  limits: ['minVolume', 'maxVolume', 'maxQueuePerUser', 'maxQueuePerGuild', 'maxTrackSeconds', 'bulkEnqueueLimit', 'auditRetentionDays', 'chatRetentionDays'],
  chat:   ['chatEnabled', 'suggestionEnabled'],
};

/** 설정에 값이 없을 때 콘솔이 가정하는 기본값 — 서버가 아직 v2 응답을 줘도 화면이 비지 않게 한다. */
const SETTING_DEFAULTS = {
  likePoints: 1, dislikePoints: -1, superLikePoints: 2, waitPoints: 1,
  boomttaEnabled: false, boomttaThreshold: 3, boomttaAction: 'bottom',
  voteSkipEnabled: false, voteSkipBasis: 'listeners', voteSkipRatio: 50, voteSkipMin: 2,
  superLikeCooldownSec: 0, superLikeDailyLimit: 0,
  autoplayMode: 'recent', autoplayRecentCount: 5, autoplayGenres: [],
  autoplayPolicy: 'balanced', autoplayArtistCooldown: 3, autoplayRecentDecayHours: 24,
  autoplaySeedMax: 10, chartSuperWeight: 2, chartLimit: 50, bulkEnqueueLimit: 200,
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
  /** 대기열 비우기(§18.2 (5))가 진행 중인가 — 두 번 눌러 두 번 지우는 일을 막는다. */
  clearingQueue: false,
  /** 섹션별 지연 로드 데이터 */
  queuePreview: { mode: null, data: null, loading: false },
  /**
   * 정렬 미리보기에서 지금 보고 있는 탭 — `live`(지금 대기열) · `sample`(샘플 대기열).
   * 섹션을 다시 그려도 보던 탭이 유지돼야 한다. 정렬 모드를 누르면 renderSection 이 통째로
   * 다시 그리는데, 그때마다 "지금 대기열"로 튕기면 샘플로 세 방식을 비교하던 흐름이 끊긴다.
   */
  previewTab: 'live',
  permPreview: {},  // 설정 키 → { passCount, memberCount, ... }
  seeds: { items: null, max: SEED_MAX_FALLBACK, canEdit: false, error: null, loading: false },
  participants: null,
  suspensions: null,
  reports: null,
  suggestions: null,
  /** 관리 콘솔의 활동 기록은 **합치지 않는다**(§13.3) — 분류 필터와 실패만 보기만 있다. */
  audit: {
    items: [], cursor: null, done: false, loading: false, query: '',
    kinds: AUDIT_KINDS.map((kind) => kind.value),   // 관리 콘솔은 기본으로 전부 켠다
    failedOnly: false,
  },
  diag: null,
  /** 서버 통계 (§22.6 `GET /stats/server`). `{available:false}` 도 그대로 담는다. */
  serverStats: null,
  /** 차트 관리 (§15.5). */
  charts: { items: null, error: null, loading: false },
  /** 차단 목록 (§19.3). */
  blocked: { items: null, error: null, loading: false, kind: 'TitleContains' },
  /** 자동 재생 장르 후보 (§8.4) — 장르 차트 목록을 그대로 쓴다. */
  genreOptions: null,
  /**
   * 투표 스킵을 "지금 인원"으로 환산할 모수 (§10.5).
   * WS presence 가 실시간으로 갱신하고, 없으면 진단의 listenerCount 로 채운다.
   * 새 폴링은 만들지 않는다(§23.2).
   */
  basis: { listeners: null, viewers: null },
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
 * 서버가 아직 v2 형태로 답해도 화면이 통짜 역할을 10개 항목에 그대로 보여 주므로 동작이 조용히 바뀌지 않는다.
 * force 는 부팅 시 baseline 을 만들 때만 true — 부분 저장 응답에는 권한 키가 없을 수 있다.
 */
function normalizeSettings(raw, force) {
  const next = Object.assign({}, raw || {});

  // v3 에서 새로 생긴 값들은 서버가 아직 안 보낼 수 있다. 없으면 기본값을 채워 둔다.
  // 여기서 채운 값은 baseline 에도 같이 들어가므로 "안 바꿨는데 바꿈"으로 보이지 않는다.
  if (force) {
    Object.entries(SETTING_DEFAULTS).forEach(([key, value]) => {
      if (next[key] === undefined) next[key] = clone(value);
    });
    // 자동 재생 권한은 v2 의 `autoplaySeedRule` 을 이어받는다 (§8.3 — 기본은 모든 멤버).
    if (next.autoplayRule === undefined) next.autoplayRule = next.autoplaySeedRule || 'guildMember';
    // 스킵은 재생 권한에서 갈라져 나왔다. 기본은 모든 멤버지만, 예전 값이 있으면 그걸 잇는다 (§10.5).
    if (next.skipRule === undefined) next.skipRule = 'guildMember';
    if (next.bulkEnqueueRule === undefined) next.bulkEnqueueRule = 'guildMember';
  }

  const touchesPerms = force
    || 'ruleRoleIds' in next || 'managerRoleIds' in next || 'configuredRoleIds' in next;
  if (!touchesPerms) return next;

  const legacy = (next.configuredRoleIds || []).map(String);
  const source = next.ruleRoleIds && typeof next.ruleRoleIds === 'object' ? next.ruleRoleIds : {};
  const anySet = Object.values(source).some((ids) => Array.isArray(ids) && ids.length);

  const map = {};
  PERM_FIELDS.forEach((field) => {
    // v2 의 `autoplaySeed` 역할은 v3 의 `autoplay` 가 그대로 물려받는다.
    const ids = source[field.permKey]
      || (field.permKey === 'autoplay' ? source.autoplaySeed : null);
    if (Array.isArray(ids) && ids.length) map[field.permKey] = ids.map(String);
    else map[field.permKey] = anySet ? [] : legacy.slice();
  });
  next.ruleRoleIds = map;

  next.managerRoleIds = Array.isArray(next.managerRoleIds) && next.managerRoleIds.length
    ? next.managerRoleIds.map(String)
    : legacy.slice();

  // v2 필드는 더 이상 주고받지 않는다 (v3 §1).
  delete next.configuredRoleIds;
  delete next.autoplaySeedRule;
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
  // 무제한(0)끼리는 비교하지 않는다 — 0은 "상한 없음"이라 언제나 서로 모순이 아니다.
  const perUser = Number(S.draft.maxQueuePerUser);
  const perGuild = Number(S.draft.maxQueuePerGuild);
  if (perUser > 0 && perGuild > 0 && perUser > perGuild) {
    errors.maxQueuePerUser = `1인 상한(${perUser}곡)이 서버 전체 상한(${perGuild}곡)보다 커요. 한 사람도 그만큼 못 넣어요.`;
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

/**
 * 툴팁 문구 — 한 줄, 마침표 없이 (§20.1).
 * 긴 설명문을 그대로 넣으면 툴팁이 아니라 벽이 되므로 첫 문장만 쓰고 마침표를 뗀다.
 */
function tipOf(text, fallback) {
  if (!text) return fallback || null;
  const first = String(text).split(/(?<=요)\.\s/)[0].split('. ')[0];
  return first.replace(/[.\s]+$/, '');
}

/** 모든 필드의 공통 껍데기 — 라벨 · ⓘ · 변경 배지 · 항목별 되돌리기 · 설명 · 오류 슬롯. */
function fieldShell(key, label, desc, control, extra) {
  return h('div', { class: 'fld', 'data-field': key },
    h('div', { class: 'fld__head' },
      h('span', { class: 'fld__label' }, label),
      desc ? h('span', {
        class: 'fld__info', 'aria-hidden': 'true', 'data-tip': tipOf(desc),
      }, 'ⓘ') : null,
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
      'data-tip': tipOf(option.desc, `${option.label}(으)로 바꿔요`),
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
function selectControl(key, options, onPick, label) {
  const select = h('select', {
    class: 'field', 'aria-label': label || '선택',
    'data-tip': label ? `${label}을(를) 누가 할 수 있는지 골라요` : '값을 골라요',
    onchange: (event) => { setValue(key, event.target.value); onPick && onPick(event.target.value); },
  });
  options.forEach((option) => {
    select.append(h('option', { value: option.value, selected: S.draft[key] === option.value }, option.label));
  });
  return select;
}

/** 체크 스위치. 아이콘도 글자도 없는 컨트롤이라 툴팁과 aria-label 이 필수다 (§20.1). */
function toggleControl(key, onText, offText, label) {
  const on = Boolean(S.draft[key]);
  const button = h('button', {
    class: 'sw' + (on ? ' is-on' : ''), type: 'button', role: 'switch',
    'aria-checked': on ? 'true' : 'false',
    'aria-label': label || (on ? offText : onText),
    'data-tip': on ? '눌러서 꺼요' : '눌러서 켜요',
    onclick: () => { setValue(key, !S.draft[key]); renderSection(S.activeSection); },
  }, h('span', { class: 'sw__knob' }));
  return h('div', { class: 'sw__row' }, button, h('span', { class: 'sw__text' }, on ? onText : offText));
}

/** 이 항목의 `0` 이 화면에서 어떻게 읽히는지. */
function zeroText(spec) {
  return spec.zeroLabel || '무제한 · 아무도 안 막아요';
}

/**
 * 숫자 — 슬라이더 + 직접입력 + 단위 + 허용범위 (구림 해소 #6) + 무제한 (v3 §23.1).
 * bounds 로 min/max 를 런타임에 덮을 수 있다(기본 볼륨은 최소~최대 볼륨을 따라간다).
 *
 * 무제한 항목은 슬라이더가 한 칸 더 길다. 그 마지막 칸이 `∞` 이고 값은 `0` 이다.
 * 슬라이더 위치와 실제 값이 다르므로 둘 사이를 `toSlider`/`fromSlider` 로만 오간다.
 */
function numberField(key, bounds) {
  const spec = NUM_SPECS[key];
  // bounds 는 다른 설정값에서 온다(기본 볼륨의 범위 = 최소~최대 볼륨). 그 값이 아직 안 왔으면 NaN 인데,
  // NaN 이 클램프에 섞이면 저장값 자체가 NaN 이 된다. 숫자일 때만 spec 을 덮어쓴다.
  const bound = (value, fallback) => (Number.isFinite(Number(value)) ? Number(value) : fallback);
  const min = bounds ? bound(bounds.min, spec.min) : spec.min;
  const max = Math.max(min, bounds ? bound(bounds.max, spec.max) : spec.max);
  const unlimited = Boolean(spec.unlimited);
  const infinitySlot = max + spec.step;          // 최댓값 다음 칸 = ∞
  const value = Number(S.draft[key]);

  const toSlider = (v) => (unlimited && Number(v) === 0 ? infinitySlot : Number(v));
  const fromSlider = (pos) => (unlimited && Number(pos) > max ? 0 : Number(pos));

  const readout = h('span', { class: 'num__pretty' });
  const hintNode = h('span', { class: 'num__hint' });
  const number = h('input', {
    class: 'field num__input', type: 'number', inputmode: 'numeric',
    min: String(unlimited ? 0 : min), max: String(max), step: String(spec.step),
    value: String(value),
    'aria-label': spec.label,
    'data-tip': unlimited ? '0을 넣으면 무제한이에요' : `${min}~${max}${spec.unit} 사이로 넣어요`,
  });
  const range = h('input', {
    class: 'num__range', type: 'range',
    min: String(min), max: String(unlimited ? infinitySlot : max), step: String(spec.step),
    value: String(toSlider(value)),
    'aria-label': `${spec.label} 슬라이더`, tabindex: '-1',
    'data-tip': unlimited ? '맨 오른쪽 칸(∞)으로 밀면 무제한이에요' : '끌어서 값을 바꿔요',
  });

  const paint = (next) => {
    const isZero = unlimited && next === 0;
    readout.classList.toggle('is-inf', isZero);
    readout.textContent = isZero ? zeroText(spec) : (spec.pretty ? spec.pretty(next) : '');
    hintNode.textContent = spec.hint ? spec.hint(next) : '';
    number.classList.toggle('is-inf', isZero);
  };

  const apply = (raw, syncRange, syncNumber) => {
    let next = Number(raw);
    if (raw === '' || !Number.isFinite(next)) next = unlimited ? 0 : Number(S.saved[key]);
    next = Math.round(next);
    // 슬라이더는 bounds(예: 기본 볼륨 = 최소~최대 볼륨)를 따르는데 직접입력만 spec 범위로 잘라서,
    // 최대 볼륨이 100인데도 150을 타이핑하면 그대로 저장되고 서버에서 튕겼다. 두 입력이 같은 범위를 쓴다.
    if (!(unlimited && next === 0)) next = Math.min(max, Math.max(min, next));
    if (syncRange) range.value = String(toSlider(next));
    if (syncNumber) number.value = String(next);
    paint(next);
    setValue(key, next);
  };
  range.addEventListener('input', (event) => apply(fromSlider(event.target.value), false, true));
  number.addEventListener('input', (event) => apply(event.target.value, true, false));
  number.addEventListener('blur', (event) => apply(event.target.value, true, true));
  paint(value);

  const rangeLabel = unlimited
    ? `${min}~${max}${spec.unit} · 맨 끝은 ∞(무제한)이에요`
    : `${min}~${max}${spec.unit} 안에서 고를 수 있어요`;

  const control = h('div', { class: 'num' + (unlimited ? ' num--inf' : '') },
    range,
    h('div', { class: 'num__side' },
      number,
      h('span', { class: 'num__unit' }, spec.unit),
      unlimited ? h('button', {
        class: 'btn btn--sm num__infbtn', type: 'button',
        'data-tip': '이 항목을 무제한으로 바꿔요',
        'aria-label': `${spec.label} 무제한으로`,
        onclick: () => apply(0, true, true),
      }, '∞') : null,
    ),
    h('div', { class: 'num__meta' },
      h('span', { class: 'num__range-label' }, rangeLabel),
      hintNode,
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
  const sampleBox = h('div', { class: 'smpl' });
  const modeField = fieldShell('sortMode', '대기열 정렬 방식',
    null,
    segmentControl('sortMode', SORT_MODES, () => loadQueuePreview()),
    h('div', { class: 'sortnote' },
      h('p', { class: 'sortnote__body' }, modeSpec.desc),
      previewTabs(previewBox, sampleBox),
    ),
  );

  const body = h('div', { class: 'sec__body' },
    modeField,
    votePointsGroup(previewBox),
    boomttaGroup(),
    voteSkipGroup(),
    superLikeGroup(),
    playbackGroup(),
    syncGroup(),
    autoplayGroup(),
    chartsGroup(),
    clearQueueGroup(),
  );

  renderQueuePreview(previewBox);
  renderSampleQueue(sampleBox);
  loadQueuePreview();
  return body;
}

/* ── 대기열 비우기 (v3 §18.2 (5)) ──
 * 상한을 1000/10000곡으로 100배 열었으면 되돌릴 수단이 반드시 같이 있어야 한다.
 * 없으면 누가 차트 100곡을 여러 번 담았을 때 관리자가 손쓸 방법이 Discord 명령뿐이다.
 */

function clearQueueGroup() {
  const box = h('div', { class: 'clearq' });
  paintClearQueue(box);
  return h('div', { class: 'grp grp--danger' },
    h('h3', { class: 'grp__title' }, '🧹 대기열 비우기'),
    h('p', { class: 'grp__desc' },
      '대기열에 쌓인 곡을 한 번에 전부 지워요. 지금 재생 중인 곡은 그대로 두고 뒤에 줄 서 있는 곡만 지워요. ' +
      '되돌릴 수 없으니 확인 창에서 곡 수를 한 번 더 보여드려요.'),
    box,
  );
}

/** 버튼에 지금 곡 수를 박는다. 숫자는 대기열 미리보기가 이미 받아 온 `totalCount` 를 그대로 쓴다. */
function paintClearQueue(box) {
  const preview = S.queuePreview.data;
  const total = preview && !preview.error ? Number(preview.totalCount) : NaN;
  const known = Number.isFinite(total);
  const empty = known && total === 0;

  const button = h('button', {
    class: 'btn btn--danger', type: 'button',
    disabled: empty || S.clearingQueue ? true : undefined,
    'data-tip': empty
      ? '지금은 대기열이 비어 있어서 지울 게 없어요'
      : '대기열에 줄 서 있는 곡을 전부 지워요',
    onclick: () => clearQueue(total),
  }, S.clearingQueue ? '비우는 중이에요…' : (known ? `🧹 ${total}곡 비우기` : '🧹 대기열 비우기'));

  box.replaceChildren(
    button,
    h('p', { class: 'hint' }, known
      ? (empty ? '지금 대기열에 곡이 없어요.' : `지금 ${total}곡이 줄 서 있어요.`)
      : '지금 몇 곡인지 아직 못 받았어요. 곡 수는 확인 창에서 다시 보여드릴게요.'),
  );
  tooltip(box);
}

async function clearQueue(total) {
  const known = Number.isFinite(total);
  const ok = await confirmSheet({
    title: '대기열을 비울까요',
    desc: known
      ? `${total}곡이 전부 지워져요. 되돌릴 수 없어요. 지금 재생 중인 곡은 그대로 둬요.`
      : '대기열에 줄 서 있는 곡이 전부 지워져요. 되돌릴 수 없어요. 지금 재생 중인 곡은 그대로 둬요.',
    danger: true,
    confirmText: known ? `${total}곡 비우기` : '대기열 비우기',
    cancelText: '그만둘게요',
  });
  if (!ok) return;

  S.clearingQueue = true;
  const box = sectionBox && sectionBox.querySelector('.clearq');
  if (box) paintClearQueue(box);
  try {
    await api('/queue/action', { body: { action: 'clear' } });
    toast('대기열을 비웠어요.', 'ok');
    S.queuePreview = { mode: null, data: null, loading: false };
  } catch (error) {
    // 이 봇이 아직 `clear` 를 모르는 빌드일 수 있다. 원인 불명 에러로 두지 않는다 (§23.3).
    const status = Number(error && error.status);
    toast(status === 400 || status === 404 || status === 422
      ? '이 봇은 아직 대기열 비우기를 지원하지 않아요. 봇을 새 빌드로 올리면 바로 돼요.'
      : `대기열을 비우지 못했어요 — ${error.message}`, 'danger');
  } finally {
    S.clearingQueue = false;
    loadQueuePreview();
    const live = sectionBox && sectionBox.querySelector('.clearq');
    if (live) paintClearQueue(live);
  }
}

/* ── 재생 (반복·기본 볼륨) ── */

function playbackGroup() {
  return h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '▶ 재생'),
    h('p', { class: 'grp__desc' }, '대기열 끝에 닿았을 때 어떻게 할지와, 봇이 새로 들어갈 때의 볼륨이에요.'),
    fieldShell('repeatMode', '반복',
      REPEAT_MODES.find((item) => item.value === S.draft.repeatMode)?.desc || null,
      segmentControl('repeatMode', REPEAT_MODES)),
    numberField('defaultVolume', { min: Number(S.draft.minVolume), max: Number(S.draft.maxVolume) }),
    // 봇이 음성에 없을 때 조작을 받을지 (§36).
    fieldShell('requireVoiceForPlayback', '봇이 음성 채널에 있어야만 조작',
      '켜 두면 봇이 음성에 없을 때 재생·스킵이 거절돼요. 눌러도 아무 일이 안 나는 상태를 막아요. ' +
      '끄면 조작을 미리 받아 두고 봇이 들어오는 순간부터 이어 가요.',
      toggleControl('requireVoiceForPlayback', '없으면 거절해요', '미리 받아 둬요')),
    // 로그인 없이 지금 곡 보기 (§29).
    fieldShell('publicNowPlaying', '로그인 없이 지금 곡 보기',
      '켜면 로그인하지 않은 사람도 지금 무슨 곡인지 볼 수 있어요. ' +
      '곡 제목과 가수만 나가고 신청한 사람·채팅·멤버는 안 나가요.',
      toggleControl('publicNowPlaying', '누구나 곡만 볼 수 있어요', '로그인해야 보여요')),
  );
}

/* ── 재생 싱크 (§31) ──
 * 값이 세 개뿐이지만 화면에 없으면 **존재하지 않는 설정**이 된다.
 * 실제로 저장 핸들러만 만들어 두고 화면에 안 올려서 켜고 끌 방법이 없었다. */
function syncGroup() {
  return h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '⏱ 재생 싱크'),
    h('p', { class: 'grp__desc' },
      '디스코드 소리와 웹에서 듣기가 어긋날 때 맞춰요. 여기 값은 서버 전체에 적용되고, ' +
      '사람마다 남는 차이는 각자 리모컨에서 다듬어요.'),
    numberField('webSyncOffsetMs'),
    numberField('skipLeadMs'),
    numberField('seekLockoutMs'),
  );
}

/* ── 투표 점수 (v3 §10.1) ── 화면의 계산식과 실제 정렬이 같은 값을 써야 한다. */

function votePointsGroup(previewBox) {
  const formula = h('div', { class: 'formula' });
  const paint = () => paintFormula(formula);

  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '👍 투표 점수'),
    h('p', { class: 'grp__desc' },
      '좋아요·싫어요·슈퍼 좋아요·기다린 시간이 각각 몇 점인지 정해요. ' +
      '리모컨 화면에 뜨는 계산식도 이 값을 그대로 쓰니까 화면이 거짓말하지 않아요.'),
    numberField('likePoints'),
    numberField('superLikePoints'),
    numberField('dislikePoints'),
    numberField('waitPoints'),
    formula,
  );
  paint();

  // 점수를 바꾸면 계산식도, 아래 대기열 미리보기도 같이 따라간다.
  // 샘플은 서버를 안 거치니 지연 없이 즉시 다시 계산한다 — 슬라이더를 끄는 동안 순서가 실시간으로 갈린다.
  group.addEventListener('input', () => { paint(); repaintSampleQueue(); scheduleQueuePreview(); });
  group.addEventListener('click', () => { paint(); repaintSampleQueue(); });
  if (S.draft.sortMode === 'fifo') {
    group.append(h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null,
        '지금은 시간제라 점수가 순서를 바꾸지 않아요. 화면에는 계속 보이지만 신청한 순서대로 나가요.'),
    ));
  }
  if (previewBox) {
    // 이 줄은 서버가 실제로 무엇을 했는지에 따라 문구가 바뀐다 — paintPreviewNote 가 채운다.
    const note = h('p', { class: 'hint prevnote' });
    paintPreviewNote(note);
    group.append(note);
  }
  return group;
}

/** `👍3 × 1 + ⭐1 × 2 + 👎0 × -1 + 대기 2 × 1 = 7` — 예시 한 곡으로 지금 설정을 보여준다. */
function paintFormula(box) {
  const like = Number(S.draft.likePoints) || 0;
  const superLike = Number(S.draft.superLikePoints) || 0;
  const dislike = Number(S.draft.dislikePoints) || 0;
  const wait = Number(S.draft.waitPoints) || 0;
  const total = 3 * like + 1 * superLike + 0 * dislike + 2 * wait;
  box.replaceChildren(
    h('span', { class: 'formula__label' }, '예를 들면'),
    h('code', { class: 'formula__body' },
      `👍3 × ${like} + ⭐1 × ${superLike} + 👎0 × ${dislike} + 대기 2 × ${wait} = ${total}점`),
    h('span', { class: 'hint' }, '좋아요 3개·슈퍼 1개를 받고 두 곡을 기다린 곡이에요'),
  );
}

/* ── 붐따 (v3 §10.3) ── */

function boomttaGroup() {
  const on = Boolean(S.draft.boomttaEnabled);
  const action = BOOMTTA_ACTIONS.find((item) => item.value === S.draft.boomttaAction) || BOOMTTA_ACTIONS[0];

  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '💥 붐따 — 싫어요가 모이면 내리기'),
    h('p', { class: 'grp__desc' },
      '싫어요가 일정 수 이상 모인 곡을 대기열에서 내려요. 꺼 두면 싫어요는 점수에만 영향을 주고 곡이 사라지지 않아요. ' +
      '지금 재생 중인 곡에는 적용하지 않아요.'),
    fieldShell('boomttaEnabled', '붐따 사용',
      '켜면 기준을 넘는 순간 바로 실행되고, 신청한 분에게 알려 드려요. 조용히 사라지지 않아요.',
      toggleControl('boomttaEnabled', '기준을 넘으면 대기열에서 내려요', '싫어요는 점수에만 반영해요')),
  );
  if (!on) return group;

  group.append(
    numberField('boomttaThreshold'),
    fieldShell('boomttaAction', '내릴 때 어떻게 할까요', action.desc,
      segmentControl('boomttaAction', BOOMTTA_ACTIONS)),
    h('p', { class: 'hint' }, Number(S.draft.boomttaThreshold) === 0
      ? '기준을 무제한으로 두셔서 싫어요가 아무리 모여도 곡이 내려가지 않아요. 사실상 꺼 둔 것과 같아요.'
      : `지금 설정이면 싫어요 ${S.draft.boomttaThreshold}개가 모이는 순간 그 곡을 ${action.label} 처리해요.`),
  );
  return group;
}

/* ── 투표 스킵 (v3 §10.5) ── 고른 기준을 지금 인원으로 환산해서 보여준다. */

function voteSkipGroup() {
  const on = Boolean(S.draft.voteSkipEnabled);
  const basis = VOTE_SKIP_BASIS.find((item) => item.value === S.draft.voteSkipBasis) || VOTE_SKIP_BASIS[0];
  const convert = h('div', { class: 'convert' });

  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '⏭ 투표 스킵'),
    h('p', { class: 'grp__desc' },
      '혼자 계속 넘기면 남의 곡이 다 날아가요. 켜면 여러 명이 동의해야 곡이 넘어가요. ' +
      '관리자·봇 주인·그 곡을 신청한 본인은 투표 없이 바로 넘길 수 있어요.'),
    fieldShell('voteSkipEnabled', '투표로 넘기기',
      '끄면 "곡 넘기기" 권한이 있는 사람은 누구나 혼자서 바로 넘길 수 있어요.',
      toggleControl('voteSkipEnabled', '여러 명이 동의해야 넘어가요', '권한이 있으면 혼자서도 넘어가요')),
  );
  if (!on) return group;

  group.append(
    fieldShell('voteSkipBasis', '누구를 세나요', basis.desc,
      segmentControl('voteSkipBasis', VOTE_SKIP_BASIS, () => paintSkipConvert(convert))),
    numberField('voteSkipRatio'),
    numberField('voteSkipMin'),
    convert,
  );
  paintSkipConvert(convert);
  // 숫자를 끌면 환산도 같이 따라간다.
  group.addEventListener('input', () => paintSkipConvert(convert));
  // 아직 인원을 모르면 진단을 한 번만 불러 채운다. 그 뒤로는 WS presence 가 알아서 갱신한다.
  if (S.basis.listeners === null && !S.diag) {
    loadDiag().then(() => {
      const live = sectionBox && sectionBox.querySelector('.convert');
      if (live) paintSkipConvert(live);
    });
  }
  return group;
}

/** 모수 × 비율을 지금 접속 인원으로 환산해 "몇 명 중 몇 명"으로 보여준다. */
function paintSkipConvert(box) {
  const ratio = Number(S.draft.voteSkipRatio) || 0;
  const minimum = Number(S.draft.voteSkipMin) || 0;
  const listeners = S.basis.listeners;
  const viewers = S.basis.viewers;
  // 서버(`models.rs` `VoteSkipBasis::votes_needed`)와 **같은 식**이어야 한다.
  // 모수 상한(`.min(population)`)이 빠져 있어서, 듣는 사람 1명 · 최소 동의 2명이면
  // 콘솔은 `1명 중 2명이 눌러야 넘어가요` 라고 말하는데 서버는 1명으로 통과시켰다.
  // 바로 아래 힌트("모수가 1명이면 혼자 눌러도 넘어가요")와도 자기모순이었다.
  const need = (population) => {
    if (population <= 0) return 0;
    const bounded = Math.min(100, Math.max(10, ratio));   // 서버도 비율을 10~100으로 clamp 한다
    const byRatio = Math.max(1, Math.ceil((population * bounded) / 100));
    return Math.min(population, Math.max(minimum, byRatio));
  };
  const line = (label, population) => {
    if (population == null) {
      return h('div', { class: 'convert__row' },
        h('span', { class: 'convert__label' }, label),
        h('span', { class: 'hint' }, '지금 인원을 아직 못 받았어요. 잠시 뒤 다시 보여드릴게요.'));
    }
    if (population === 0) {
      return h('div', { class: 'convert__row' },
        h('span', { class: 'convert__label' }, label),
        h('span', { class: 'convert__value' }, '0명 — 아무도 없어서 투표 없이 바로 넘어가요'));
    }
    return h('div', { class: 'convert__row' },
      h('span', { class: 'convert__label' }, label),
      h('span', { class: 'convert__value' }, `${population}명 중 ${need(population)}명이 눌러야 넘어가요`));
  };

  const basis = String(S.draft.voteSkipBasis || 'listeners');
  const rows = [];
  if (basis === 'listeners' || basis === 'either' || basis === 'both') rows.push(line('🎧 듣는 사람', listeners));
  if (basis === 'viewers' || basis === 'either' || basis === 'both') rows.push(line('🖥 보는 사람', viewers));
  if (basis === 'either') rows.push(h('p', { class: 'hint' }, '둘 중 한쪽만 채워도 넘어가요.'));
  if (basis === 'both') rows.push(h('p', { class: 'hint' }, '두 줄을 모두 채워야 넘어가요.'));

  box.replaceChildren(
    h('div', { class: 'convert__head' }, '지금 이 서버 인원으로 환산하면'),
    ...rows,
    h('p', { class: 'hint' }, '모수가 1명이면 그 사람 혼자 눌러도 넘어가요. 혼자 듣는데 투표를 시키면 괴롭힘이니까요.'),
  );
}

/* ── 슈퍼 좋아요 제한 (v3 §10.6) ── */

function superLikeGroup() {
  const cooldown = Number(S.draft.superLikeCooldownSec) || 0;
  const daily = Number(S.draft.superLikeDailyLimit) || 0;
  return h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '⭐ 슈퍼 좋아요 제한'),
    h('p', { class: 'grp__desc' },
      '슈퍼 좋아요는 점수를 크게 움직여요. 한 사람이 자기 취향 곡마다 박으면 대기열이 그 사람 것이 돼요. ' +
      '기본은 둘 다 꺼져 있고, 필요한 서버만 켜시면 돼요. 관리자와 봇 주인도 똑같이 적용돼요.'),
    numberField('superLikeCooldownSec'),
    numberField('superLikeDailyLimit'),
    h('p', { class: 'hint' },
      daily > 0
        ? '하루 기준은 UTC 자정이에요. 한국 시간으로는 오전 9시에 초기화돼요. 취소하면 횟수는 돌려드려요.'
        : '하루 제한을 켜면 UTC 자정(한국 시간 오전 9시)에 초기화돼요.'),
    cooldown > 0
      ? h('p', { class: 'hint' }, '쿨타임 중에는 ⭐ 버튼에 남은 시간이 숫자로 떠요. 회색으로만 두면 고장인 줄 알거든요.')
      : null,
  );
}

/* ── 자동 재생 (v3 §8) ── 방식 3종 · 정책 4종 · 기준 곡 · 최근 N곡 · 장르 ── */

function autoplayGroup() {
  const on = Boolean(S.draft.autoBgmEnabled);
  const mode = String(S.draft.autoplayMode || 'recent');
  const modeSpec = AUTOPLAY_MODES.find((item) => item.value === mode) || AUTOPLAY_MODES[1];
  const policy = AUTOPLAY_POLICIES.find((item) => item.value === S.draft.autoplayPolicy) || AUTOPLAY_POLICIES[1];

  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '📻 자동 재생'),
    h('p', { class: 'grp__desc' },
      '대기열이 비었을 때 무엇을 기준으로 다음 곡을 고를지 정해요. ' +
      '기본값은 "최근 튼 곡"이라 지금까지의 동작 그대로예요.'),
    fieldShell('autoBgmEnabled', '자동 재생 사용',
      '끄면 대기열이 비는 순간 조용해져요. 아래 설정은 그대로 남아 있다가 다시 켜면 이어서 써요.',
      toggleControl('autoBgmEnabled', '대기열이 비면 알아서 이어 틀어요', '대기열이 비면 재생을 멈춰요')),
  );
  if (!on) {
    group.append(h('p', { class: 'hint' }, '자동 재생이 꺼져 있어서 아래 설정은 지금 쓰이지 않아요. 위에서 켜시면 바로 나타나요.'));
  }

  group.append(fieldShell('autoplayMode', '무엇을 기준으로 고를까요', modeSpec.desc,
    segmentControl('autoplayMode', AUTOPLAY_MODES)));

  if (mode === 'recent') {
    group.append(numberField('autoplayRecentCount'));
  }
  if (mode === 'genre') {
    const genreBox = h('div', { class: 'genres' });
    group.append(fieldShell('autoplayGenres', '어떤 장르를 쓸까요',
      '고른 장르 차트에서 곡을 뽑아요. 여러 개 고르면 번갈아 가며 써서 한 장르로 쏠리지 않아요.',
      genreBox));
    paintGenres(genreBox);
    if (S.genreOptions === null) loadGenreOptions().then(() => {
      const live = sectionBox && sectionBox.querySelector('.genres');
      if (live) paintGenres(live);
    });
  }

  // 기준 곡 목록은 방식과 무관하게 늘 보여준다 — `seed` 가 아니어도 폴백 사슬에서 쓰인다.
  const seedsBox = h('div', { class: 'card seeds' });
  group.append(
    h('h4', { class: 'grp__sub' }, '기준 곡'),
    h('p', { class: 'grp__desc' },
      '자동 재생이 이 곡들과 비슷한 곡을 찾아와요. 여러 곡을 넣으면 돌아가며 참고해요. ' +
      '여기서 바꾼 건 저장 버튼 없이 바로 반영돼요.'),
    seedsBox,
    numberField('autoplaySeedMax'),
  );
  paintSeeds(seedsBox);
  if (!S.seeds.items && !S.seeds.loading) loadSeeds().then(() => repaintSeeds());

  group.append(
    h('h4', { class: 'grp__sub' }, '추천 정책'),
    h('p', { class: 'grp__desc' },
      '기준 곡을 정하는 것과, 그 라디오 결과에서 어떤 곡을 집어 오는지는 다른 문제예요. ' +
      '아래에서 얼마나 비슷하게 고를지 정해요.'),
    fieldShell('autoplayPolicy', '얼마나 비슷하게 고를까요', policy.desc,
      segmentControl('autoplayPolicy', AUTOPLAY_POLICIES)),
    numberField('autoplayArtistCooldown'),
    numberField('autoplayRecentDecayHours'),
    h('p', { class: 'hint' },
      '정책을 바꾸면 지금 잡혀 있는 다음 추천곡을 바로 다시 뽑아요. 안 그러면 언제 반영됐는지 알 수 없으니까요.'),
  );
  return group;
}

/** 장르 칩 — 장르 차트 목록을 그대로 쓴다. 새 크롤러도 새 목록도 만들지 않는다. */
function paintGenres(box) {
  if (S.genreOptions === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:30px;width:240px' }));
    return;
  }
  if (!S.genreOptions.length) {
    box.replaceChildren(h('p', { class: 'hint' },
      '고를 수 있는 장르 차트가 없어요. 아래 "차트 관리"에서 장르 차트를 켜시면 여기에 나타나요.'));
    return;
  }
  const picked = new Set((S.draft.autoplayGenres || []).map(String));
  box.replaceChildren();
  S.genreOptions.forEach((option) => {
    const key = String(option.key);
    const on = picked.has(key);
    const chip = h('button', {
      class: 'role' + (on ? ' is-on' : ''), type: 'button',
      'aria-pressed': on ? 'true' : 'false',
      'data-tip': on ? '이 장르를 빼요' : '이 장르를 넣어요',
    }, h('span', { class: 'role__name' }, option.label || key));
    chip.addEventListener('click', () => {
      const next = new Set((S.draft.autoplayGenres || []).map(String));
      const was = next.has(key);
      if (was) next.delete(key); else next.add(key);
      setValue('autoplayGenres', Array.from(next));
      chip.classList.toggle('is-on', !was);
      chip.setAttribute('aria-pressed', was ? 'false' : 'true');
    });
    box.append(chip);
  });
  if (!picked.size) {
    box.append(h('p', { class: 'hint' }, '아직 고른 장르가 없어요. 이대로 두면 "최근 튼 곡"으로 대신 골라요.'));
  }
  tooltip(box);
}

/**
 * 장르 후보. 전용 엔드포인트가 있으면 그걸 쓰고, 없으면 차트 목록의 `genre` 분류에서 뽑는다.
 * 둘 다 없으면 빈 목록을 그대로 보여준다 — 없는 걸 있는 척하지 않는다.
 */
async function loadGenreOptions() {
  try {
    const data = await api('/autoplay');
    if (Array.isArray(data.genreOptions)) { S.genreOptions = data.genreOptions; return; }
  } catch { /* 아래 차트 목록으로 폴백한다 */ }
  try {
    const data = await api('/charts');
    const category = (data.categories || []).find((item) => item.key === 'genre');
    S.genreOptions = (category ? category.charts || [] : [])
      .map((chart) => ({ key: String(chart.id), label: chart.name }));
  } catch {
    S.genreOptions = [];
  }
}

/* ── 차트 관리 (v3 §15.5) ── 켜기/끄기 · 순서 · 주소 수정 · 차트 추가 ── */

function chartsGroup() {
  const box = h('div', { class: 'card charts' });
  const group = h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '📈 차트 관리'),
    h('p', { class: 'grp__desc' },
      '리모컨의 차트 탭에 무엇을 보여줄지 정해요. 기본 제공 차트는 지울 수 없고 끄기만 돼요.'),
    h('div', { class: 'urlhelp' },
      h('strong', null, '주소는 세 가지 방식이 있어요'),
      h('dl', { class: 'urlhelp__list' },
        h('dt', null, 'ytsearch50:검색어'),
        h('dd', null, '유튜브에서 검색해 가져와요. 재생목록 ID 와 달리 죽지 않아서 기본값으로 써요. ' +
          '앞의 숫자는 위 "차트 곡 수" 설정으로 자동으로 바뀌니 그대로 두셔도 돼요. ' +
          '예: ytsearch50:TJ노래방 발라드'),
        h('dt', null, 'https://…playlist?list=…'),
        h('dd', null, '실제 재생목록이라 더 정확한데 ID 가 자주 바뀌어요. ' +
          '실제로 유튜브 뮤직 인기곡 재생목록 두 개가 죽어서 빈 차트가 나간 적이 있어요. ' +
          '넣으신 뒤 아래 곡 수가 0 이 아닌지 꼭 확인해 주세요.'),
        h('dt', null, 'internal:…'),
        h('dd', null, '우리가 실제로 튼 기록으로 만드는 차트예요. 바깥을 안 부르고 주소도 못 바꿔요.')),
      h('p', { class: 'hint' },
        '가져온 목록은 6시간 동안 저장해 두고 다시 써요. 여러 명이 같은 차트를 눌러도 한 번만 가져와요. ' +
        '바로 새로 받고 싶으시면 각 줄의 ↻ 를 눌러 주세요.')),
    numberField('chartLimit'),
    numberField('chartSuperWeight'),
    box,
    h('button', {
      class: 'btn', type: 'button', style: 'align-self:flex-start',
      'data-tip': '주소를 직접 넣어 차트를 하나 추가해요',
      onclick: () => openChartSheet(null),
    }, '+ 차트 추가'),
  );
  paintCharts(box);
  if (S.charts.items === null && !S.charts.loading) loadCharts().then(() => repaintCharts());
  return group;
}

async function loadCharts() {
  if (S.charts.loading) return;
  S.charts.loading = true;
  try {
    const data = await api('/admin/charts');
    S.charts = { items: data.items || [], error: null, loading: false };
  } catch (error) {
    S.charts = { items: [], error: error.message, loading: false };
  }
}

function repaintCharts() {
  const box = sectionBox && sectionBox.querySelector('.charts');
  if (box) paintCharts(box);
}

function paintCharts(box) {
  if (S.charts.loading || S.charts.items === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:120px;margin:12px' }));
    return;
  }
  if (S.charts.error) {
    box.replaceChildren(h('p', { class: 'hint', style: 'padding:12px' },
      `차트 목록을 못 불러왔어요 — ${S.charts.error}`));
    return;
  }
  if (!S.charts.items.length) {
    box.replaceChildren(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '📈'),
      h('div', { class: 'empty__title' }, '차트가 하나도 없어요'),
      h('div', { class: 'empty__desc' }, '아래 "차트 추가"로 재생목록 주소를 넣으시면 리모컨의 차트 탭에 나타나요.'),
    ));
    return;
  }

  box.replaceChildren();
  CHART_CATEGORIES.forEach((category) => {
    const items = chartsIn(category.key);
    if (!items.length) return;
    box.append(h('div', { class: 'charts__cat' },
      h('span', { class: 'charts__caticon' }, category.icon),
      h('span', { class: 'charts__catname' }, category.label),
      h('span', { class: 'hint' }, category.desc),
    ));
    const rows = h('ul', { class: 'rows' });
    items.forEach((item, index) => rows.append(chartRow(item, index, items.length)));
    box.append(rows);
  });
  tooltip(box);
}

function chartRow(item, index, total) {
  const builtin = item.builtin || item.guildId == null || Number(item.guildId) === 0;
  const enabled = item.enabled !== false;
  const failed = item.ok === false;
  const fetched = item.lastFetchedUtc
    ? (failed ? `마지막 갱신 실패 · ${fmtAgo(item.lastFetchedUtc)}` : `${fmtAgo(item.lastFetchedUtc)} 갱신했어요`)
    : '아직 한 번도 안 가져왔어요';

  return h('li', { class: 'row row--chart' + (enabled ? '' : ' is-off') },
    h('button', {
      class: 'sw sw--sm' + (enabled ? ' is-on' : ''), type: 'button', role: 'switch',
      'aria-checked': enabled ? 'true' : 'false',
      'data-tip': enabled ? '리모컨 차트 목록에서 숨겨요' : '리모컨 차트 목록에 다시 보여요',
      'aria-label': `${item.name} ${enabled ? '끄기' : '켜기'}`,
      onclick: () => updateChart(item, { enabled: !enabled }),
    }, h('span', { class: 'sw__knob' })),
    h('div', { class: 'row__main' },
      h('div', { class: 'row__name' },
        item.name,
        builtin ? h('span', { class: 'chip' }, '기본 제공') : null,
        failed ? h('span', { class: 'chip chip--warn' }, '가져오기 실패') : null,
      ),
      h('div', { class: 'row__sub' },
        h('span', null, item.provider || '공급자 미상'),
        h('span', null, `· ${fetched}`),
        item.trackCount != null ? h('span', null, `· ${item.trackCount}곡`) : null,
        failed && item.failureReason ? h('span', null, `· ${item.failureReason}`) : null,
      ),
    ),
    h('div', { class: 'row__acts' },
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button', disabled: index === 0,
        'data-tip': index === 0 ? '이미 맨 위예요' : '한 칸 위로 올려요',
        'aria-label': `${item.name} 한 칸 위로`,
        onclick: () => moveChart(item, -1),
      }, '↑'),
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button', disabled: index === total - 1,
        'data-tip': index === total - 1 ? '이미 맨 아래예요' : '한 칸 아래로 내려요',
        'aria-label': `${item.name} 한 칸 아래로`,
        onclick: () => moveChart(item, 1),
      }, '↓'),
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button',
        'data-tip': '캐시를 무시하고 지금 다시 가져와요',
        'aria-label': `${item.name} 새로고침`,
        onclick: () => refreshChart(item),
      }, '↻'),
      item.url ? h('button', {
        class: 'btn btn--sm', type: 'button',
        'data-tip': '이름과 주소를 고쳐요',
        onclick: () => openChartSheet(item),
      }, '수정') : null,
      builtin ? h('button', {
        class: 'btn btn--sm', type: 'button', disabled: true,
        'data-tip': '기본 제공 차트는 지울 수 없어요. 대신 왼쪽 스위치로 끄시면 돼요',
      }, '삭제') : h('button', {
        class: 'btn btn--sm btn--danger', type: 'button',
        'data-tip': '이 차트를 목록에서 지워요',
        onclick: () => removeChart(item),
      }, '삭제'),
    ),
  );
}

async function updateChart(item, patch) {
  try {
    await api(`/admin/charts/${item.id}`, { method: 'PUT', body: patch });
    Object.assign(item, patch);
    repaintCharts();
  } catch (error) {
    toast(`차트를 못 바꿨어요 — ${error.message}`, 'danger');
  }
}

/** 한 분류의 차트를 저장된 순서대로. 화면과 `sortOrder` 가 어긋나면 순서 바꾸기가 먹지 않은 것처럼 보인다. */
function chartsIn(category) {
  return (S.charts.items || [])
    .filter((entry) => entry.category === category)
    .sort((a, b) => (Number(a.sortOrder) || 0) - (Number(b.sortOrder) || 0));
}

/** 같은 분류 안에서만 자리를 옮긴다. 분류를 넘나들면 사용자가 찾던 차트가 사라진 것처럼 보인다. */
async function moveChart(item, step) {
  const siblings = chartsIn(item.category);
  const from = siblings.indexOf(item);
  const to = from + step;
  if (from < 0 || to < 0 || to >= siblings.length) return;
  // 되돌릴 때 배열만 되돌리면 안 된다 — 순서는 항목마다의 `sortOrder` 에 들어 있다.
  const before = siblings.map((entry) => [entry, entry.sortOrder]);
  siblings.splice(from, 1);
  siblings.splice(to, 0, item);
  siblings.forEach((entry, index) => { entry.sortOrder = index; });
  repaintCharts();
  try {
    await api('/admin/charts/reorder', {
      method: 'POST',
      body: { category: item.category, ids: siblings.map((entry) => entry.id) },
    });
  } catch (error) {
    before.forEach(([entry, order]) => { entry.sortOrder = order; });
    repaintCharts();
    toast(`순서를 못 바꿨어요 — ${error.message}`, 'danger');
  }
}

async function refreshChart(item) {
  toast(`"${item.name}"을(를) 다시 가져오는 중이에요. 곡이 많으면 몇 초 걸려요.`, 'info');
  try {
    await api(`/charts/${item.id}/refresh`, { method: 'POST', body: {} });
    await loadCharts();
    repaintCharts();
    toast('차트를 새로 가져왔어요.', 'ok');
  } catch (error) {
    toast(`가져오지 못했어요 — ${error.message}`, 'danger');
  }
}

async function removeChart(item) {
  const ok = await confirmSheet({
    title: '차트 지우기',
    desc: `"${item.name}"을(를) 목록에서 지워요. 다시 쓰시려면 주소를 새로 넣어야 해요.`,
    confirmText: '지울게요', cancelText: '그냥 둘게요', danger: true,
  });
  if (!ok) return;
  try {
    await api(`/admin/charts/${item.id}/remove`, { method: 'POST', body: {} });
    S.charts.items = S.charts.items.filter((entry) => entry.id !== item.id);
    repaintCharts();
    toast('차트를 지웠어요.', 'ok');
  } catch (error) {
    toast(`지우지 못했어요 — ${error.message}`, 'danger');
  }
}

/** 차트 추가·수정 시트. 기본 제공 차트는 이름과 주소만 고칠 수 있다. */
async function openChartSheet(item) {
  const editing = Boolean(item);
  let category = editing ? item.category : 'popular';
  let provider = editing ? item.provider : 'YouTubeMusic';
  const nameInput = h('input', { class: 'field', placeholder: '차트 이름 (예: 한국 인기곡)', value: editing ? item.name : '' });
  const urlInput = h('input', { class: 'field', placeholder: '재생목록 주소 (https://…)', value: editing ? (item.url || '') : '' });

  const categoryBox = h('div', { class: 'seg' });
  CHART_CATEGORIES.filter((entry) => entry.key !== 'ours').forEach((entry) => {
    categoryBox.append(h('button', {
      class: 'seg__btn' + (entry.key === category ? ' is-on' : ''), type: 'button',
      'data-tip': entry.desc,
      onclick: (event) => {
        category = entry.key;
        categoryBox.querySelectorAll('.seg__btn').forEach((node) => node.classList.remove('is-on'));
        event.currentTarget.classList.add('is-on');
      },
    }, entry.label));
  });

  const providerBox = h('div', { class: 'seg' });
  ['YouTubeMusic', 'YouTube', 'SoundCloud'].forEach((value) => {
    providerBox.append(h('button', {
      class: 'seg__btn' + (value === provider ? ' is-on' : ''), type: 'button',
      'data-tip': `${value} 주소로 다뤄요`,
      onclick: (event) => {
        provider = value;
        providerBox.querySelectorAll('.seg__btn').forEach((node) => node.classList.remove('is-on'));
        event.currentTarget.classList.add('is-on');
      },
    }, value));
  });

  const body = h('div', { class: 'sheetform' },
    h('label', { class: 'sheetform__label' }, '이름'), nameInput,
    h('label', { class: 'sheetform__label' }, '주소'), urlInput,
    h('p', { class: 'hint' }, '재생목록 주소면 돼요. 곡 목록은 6시간마다 한 번만 가져오니 서버가 무거워지지 않아요.'),
    editing ? null : h('label', { class: 'sheetform__label' }, '분류'),
    editing ? null : categoryBox,
    editing ? null : h('label', { class: 'sheetform__label' }, '공급자'),
    editing ? null : providerBox,
  );

  const ok = await sheet({
    title: editing ? `"${item.name}" 수정` : '차트 추가',
    body,
    dismissValue: false,
    actions: [
      { label: '취소', kind: 'ghost', value: false },
      { label: editing ? '저장' : '추가', kind: 'primary', value: true },
    ],
  }).result;
  if (!ok) return;

  const name = nameInput.value.trim();
  const url = urlInput.value.trim();
  if (!name) { toast('이름을 넣어 주세요.', 'warn'); return; }
  if (!url) { toast('재생목록 주소를 넣어 주세요.', 'warn'); return; }
  try {
    if (editing) await api(`/admin/charts/${item.id}`, { method: 'PUT', body: { name, url } });
    else await api('/admin/charts', { method: 'POST', body: { category, provider, name, url } });
    await loadCharts();
    repaintCharts();
    toast(editing ? '차트를 고쳤어요.' : '차트를 추가했어요.', 'ok');
  } catch (error) {
    toast(`저장하지 못했어요 — ${error.message}`, 'danger');
  }
}

/* ── 대기열 미리보기 ── */

/** 점수를 끌 때마다 서버를 때리지 않게 한 박자 묶는다 (§23.2 — 새 폴링을 만들지 않는다). */
let queuePreviewTimer = null;
function scheduleQueuePreview() {
  clearTimeout(queuePreviewTimer);
  queuePreviewTimer = setTimeout(() => loadQueuePreview(), 260);
}

/** 지금 화면의 투표 점수 4종 — 서버 `vote_points()` 와 같은 키 이름을 쓴다. */
function draftVotePoints() {
  return {
    like: Number(S.draft.likePoints) || 0,
    dislike: Number(S.draft.dislikePoints) || 0,
    superLike: Number(S.draft.superLikePoints) || 0,
    wait: Number(S.draft.waitPoints) || 0,
  };
}

/**
 * 서버가 미리보기에서 **우리가 보낸 점수를 실제로 썼는가** (v3 §10.1).
 * `null` = 아직 모름 · `true` = 반영됨 · `false` = 저장된 점수로만 계산됨.
 *
 * 옛 서버의 `ModeQuery` 는 `mode` 하나만 역직렬화하고 나머지 쿼리 키를 조용히 버렸다.
 * 그래서 점수를 아무리 끌어도 미리보기 순서가 그대로였는데 화면은 "새 점수로 다시 계산돼요"
 * 라고 말했다. 서버가 쓴 점수를 응답에 되돌려 주면(`points`) 여기서 사실을 확인한다.
 */
let previewHonorsPoints = null;

function previewEchoMatches(data, sent) {
  const echo = data && (data.points || data.votePoints);
  if (!echo || typeof echo !== 'object') return false;
  return ['like', 'dislike', 'superLike', 'wait']
    .every((field) => Number(echo[field]) === Number(sent[field]));
}

/** 지금 대기열에 이 모드·이 점수를 적용하면 순서가 어떻게 되는지 (구림 해소 #4 + v3 §10.1). */
function queuePreviewKey() {
  const parts = [S.draft.sortMode];
  // 서버가 점수를 안 본다는 걸 확인했으면 점수를 키에서 뺀다.
  // 안 그러면 슬라이더를 끌 때마다 **똑같은 응답**을 받으려고 요청이 계속 나간다(§23.2 위반).
  if (previewHonorsPoints !== false) {
    const points = draftVotePoints();
    parts.push(points.like, points.dislike, points.superLike, points.wait);
  }
  return parts.join('|');
}

async function loadQueuePreview() {
  const key = queuePreviewKey();
  if (S.queuePreview.mode === key && S.queuePreview.data) return;
  S.queuePreview = { mode: key, data: null, loading: true };
  const box = sectionBox && sectionBox.querySelector('.prev');
  if (box) renderQueuePreview(box);
  // 점수도 같이 보낸다 — 서버가 이 값들로 계산해야 "저장하면 이렇게 돼요"가 사실이 된다.
  const sent = draftVotePoints();
  const params = new URLSearchParams({
    mode: String(S.draft.sortMode),
    likePoints: String(sent.like),
    dislikePoints: String(sent.dislike),
    superLikePoints: String(sent.superLike),
    waitPoints: String(sent.wait),
  });
  try {
    const data = await api(`/admin/queue-preview?${params.toString()}`);
    if (S.queuePreview.mode !== key) return;   // 그 사이 값이 또 바뀌었으면 버린다
    previewHonorsPoints = previewEchoMatches(data, sent);
    S.queuePreview = { mode: queuePreviewKey(), data, loading: false };
  } catch (error) {
    if (S.queuePreview.mode !== key) return;
    S.queuePreview = { mode: key, data: { error: error.message }, loading: false };
  }
  const target = sectionBox && sectionBox.querySelector('.prev');
  if (target) renderQueuePreview(target);
  const note = sectionBox && sectionBox.querySelector('.prevnote');
  if (note) paintPreviewNote(note);
  const clearBox = sectionBox && sectionBox.querySelector('.clearq');
  if (clearBox) paintClearQueue(clearBox);
}

/** 미리보기가 지금 무엇을 기준으로 계산된 건지 한 줄로 말한다. 모르면 아무 말도 안 한다. */
function paintPreviewNote(node) {
  node.classList.toggle('is-warn', previewHonorsPoints === false);
  node.textContent = previewHonorsPoints === false
    ? '지금 이 봇은 미리보기를 저장된 점수로만 계산해요. 바꾸신 점수는 저장한 뒤에 순서에 반영돼요.'
    : '점수를 바꾸면 위쪽 미리보기가 새 점수로 다시 계산돼요.';
}

function renderQueuePreview(box) {
  box.replaceChildren();
  box.append(h('div', { class: 'prev__head' },
    h('strong', null, '지금 대기열에 적용하면'),
    h('span', { class: 'hint' }, '저장해야 실제로 바뀌어요'),
  ));
  // 서버가 점수를 안 보고 계산했다면 그 사실을 미리보기 안에서 말한다.
  // 아무 말 없이 옛 순서를 보여 주면 "점수를 바꿨는데 순서가 그대로네"가 되고, 그게 §10.1 이 금지한 거짓말이다.
  if (previewHonorsPoints === false) {
    box.append(h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null,
        '이 순서는 지금 저장돼 있는 점수로 계산한 거예요. 바꾸신 점수는 저장한 뒤에 순서에 반영돼요.'),
    ));
  }

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
    // 빈 화면만 보여 주고 끝내면 정렬 방식을 고르는 사람이 아무 판단도 못 한다.
    // 곡이 없을 때야말로 샘플이 필요한 순간이라 그쪽으로 넘어가는 길을 같이 준다.
    box.append(h('div', { class: 'empty' },
      h('div', { class: 'empty__icon' }, '🎵'),
      h('div', { class: 'empty__title' }, '대기열이 비어 있어요'),
      h('div', { class: 'empty__desc' }, '곡이 쌓이면 여기서 순서가 어떻게 바뀌는지 미리 보실 수 있어요.'),
      h('button', {
        class: 'btn btn--sm', type: 'button',
        'data-tip': '예시 곡으로 세 방식의 차이를 대신 보여드려요',
        onclick: () => openSampleTab(),
      }, '샘플 대기열로 비교해 볼게요'),
    ));
    tooltip(box);
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

/* ── 샘플 대기열 미리보기 ──
 *
 * 왜 있나: 위쪽 "지금 대기열" 미리보기는 **이 서버의 실제 대기열**을 쓴다. 그런데 설정을 만지는
 * 시점은 보통 아무도 안 듣고 있을 때다. 대기열이 비었거나 한두 곡이면 세 방식이 전부 같은 순서를
 * 내놓기 때문에, 정작 "이 방식으로 바꾸면 뭐가 달라지나"를 볼 수가 없다.
 * 그래서 **고정된 가짜 대기열**을 하나 두고, 세 방식의 결과를 나란히 보여 준다.
 *
 * 서버를 새로 파지 않는 이유: 이 데이터는 고정이고 정렬 규칙도 순수 함수라 클라에서 계산하면 끝이다.
 * 미리보기 하나 보자고 엔드포인트를 늘리면 유휴 상태 쿼리 0회 기준(§23.2)만 갉아먹는다.
 *
 * 대신 **정렬 규칙은 반드시 서버와 같아야 한다**. 아래 비교 함수들은 `src/remote/ranking.rs` 의
 * `compare_score` / `compare_fifo` / `compare_fair` 를 그대로 옮긴 것이고, 점수는 화면에서
 * 편집 중인 점수표(`draftVotePoints`)를 쓴다 — 서버의 `QueueScore::total_score` 와 같은 식이다.
 * 샘플에는 수동 우선순위(핀·붐따)가 없어서 `compare_manual` 단계만 빠져 있다.
 */

/**
 * 샘플 곡 6개 — 신청자 3명. **세 방식이 서로 다른 1위를 내도록** 일부러 이렇게 짰다.
 *   · 여름밤 드라이브 : 제일 먼저 신청 + 오래 기다림 → 시간제 1위
 *   · 떼창 유발 록    : 제일 늦게 신청했지만 좋아요 폭격 → 점수제 1위
 *   · 노래방 18번     : 싫어요를 받아 점수는 꼴찌지만 아직 한 곡도 못 튼 사람 곡 → 공평제 1위
 * 민수가 3곡을 몰아 넣은 것도 의도다. 공평제에서 그 3곡이 라운드별로 흩어지는 게 눈에 보여야 한다.
 * `order` 는 신청 순서(서버의 `original_order`), `wait` 는 기다린 곡 수(`wait_score`)다.
 */
const SAMPLE_QUEUE = [
  { id: 'smpl-1', order: 0, who: '민수', title: '여름밤 드라이브',  like: 0, superLike: 0, dislike: 0, wait: 3 },
  { id: 'smpl-2', order: 1, who: '지훈', title: '출근길 시티팝',    like: 5, superLike: 0, dislike: 0, wait: 2 },
  { id: 'smpl-3', order: 2, who: '민수', title: '새벽 감성 발라드',  like: 1, superLike: 1, dislike: 0, wait: 1 },
  { id: 'smpl-4', order: 3, who: '수연', title: '노래방 18번',      like: 0, superLike: 0, dislike: 2, wait: 1 },
  { id: 'smpl-5', order: 4, who: '민수', title: '떼창 유발 록',      like: 8, superLike: 1, dislike: 0, wait: 0 },
  { id: 'smpl-6', order: 5, who: '지훈', title: '카페 재즈',        like: 2, superLike: 0, dislike: 0, wait: 0 },
];

/**
 * 신청자별 마지막 재생 시각 — 공평제의 2순위 기준(`last_played_utc`).
 * 빈 문자열은 "아직 한 곡도 못 틀었다"는 뜻이고, 서버와 똑같이 **제일 앞에 온다**.
 * 수연이 그 자리라서 같은 1라운드 안에서도 수연 곡이 먼저 나간다.
 */
const SAMPLE_LAST_PLAYED = { 민수: '2025-05-01T21:40:00Z', 지훈: '2025-05-01T21:10:00Z', 수연: '' };

/** 서버 `QueueScore::total_score` 와 같은 식. 배수를 여기서 다시 곱하지 않는다 (§10.1). */
function sampleScore(row, points) {
  return row.wait * points.wait
    + row.like * points.like
    + row.superLike * points.superLike
    + row.dislike * points.dislike;
}

/** 서버 `ranking::request_rounds` — 사람별로 신청 순서대로 줄 세운 0-based 라운드. */
function sampleRounds() {
  const ordered = SAMPLE_QUEUE.slice().sort(sampleTail);
  const next = new Map();
  const rounds = new Map();
  ordered.forEach((row) => {
    const slot = next.get(row.who) || 0;
    rounds.set(row.id, slot);
    next.set(row.who, slot + 1);
  });
  return rounds;
}

/** 모든 모드가 공유하는 마지막 결정자: 신청 순서 → id (서버 `compare_tail`). */
function sampleTail(a, b) {
  return (a.order - b.order) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);
}

/** 샘플 대기열을 이 방식·이 점수표로 정렬한 결과. 원본 배열은 건드리지 않는다. */
function sampleSorted(mode, points, rounds) {
  const rows = SAMPLE_QUEUE.slice();
  if (mode === 'score') {
    // 총점 내림차순 → 신청 순서. 서버 `compare_score` 와 같다.
    rows.sort((a, b) => (sampleScore(b, points) - sampleScore(a, points)) || sampleTail(a, b));
  } else if (mode === 'fair') {
    // 라운드 오름차순 → 마지막 재생 시각 오름차순(못 튼 사람이 먼저) → 신청 순서.
    rows.sort((a, b) => {
      const byRound = (rounds.get(a.id) || 0) - (rounds.get(b.id) || 0);
      if (byRound) return byRound;
      const left = SAMPLE_LAST_PLAYED[a.who] || '';
      const right = SAMPLE_LAST_PLAYED[b.who] || '';
      if (left !== right) return left < right ? -1 : 1;
      return sampleTail(a, b);
    });
  } else {
    // 시간제는 점수를 아예 안 본다.
    rows.sort(sampleTail);
  }
  return rows;
}

/** 그 방식에서 이 곡이 왜 그 자리인지 한 조각으로 말한다. 근거 없는 순위는 설득이 안 된다. */
function sampleReason(mode, row, points, rounds) {
  if (mode === 'score') return `${sampleScore(row, points)}점`;
  if (mode === 'fair') return `${(rounds.get(row.id) || 0) + 1}번째 곡`;
  return `${row.order + 1}번째 신청`;
}

/**
 * 미리보기 두 장을 탭으로 묶는다.
 * 나란히 놓지 않고 탭으로 둔 이유: 정렬 방식 설명 바로 아래라 세로 공간이 이미 빡빡하고,
 * 샘플 쪽은 표가 3열이라 옆에 붙이면 좁은 화면에서 둘 다 못 읽는 폭이 된다.
 */
function previewTabs(liveBox, sampleBox) {
  const bar = h('div', { class: 'prevtabs', role: 'tablist', 'aria-label': '미리보기 종류' });
  const panes = [
    { id: 'live',   label: '지금 대기열', box: liveBox,   tip: '이 서버에 실제로 줄 서 있는 곡에 이 방식을 적용해 봐요' },
    { id: 'sample', label: '샘플 대기열', box: sampleBox, tip: '예시 곡 6개로 세 방식이 어떻게 갈리는지 나란히 봐요' },
  ];
  panes.forEach((pane) => {
    bar.append(h('button', {
      class: 'prevtabs__btn', type: 'button', role: 'tab',
      'data-tab': pane.id, 'data-tip': pane.tip,
      onclick: () => showPreviewTab(pane.id, bar, panes),
    }, pane.label));
  });
  const wrap = h('div', { class: 'prevwrap' }, bar, liveBox, sampleBox);
  showPreviewTab(S.previewTab, bar, panes);
  tooltip(bar);
  return wrap;
}

function showPreviewTab(id, bar, panes) {
  S.previewTab = panes.some((pane) => pane.id === id) ? id : 'live';
  bar.querySelectorAll('.prevtabs__btn').forEach((btn) => {
    const on = btn.dataset.tab === S.previewTab;
    btn.classList.toggle('is-on', on);
    btn.setAttribute('aria-selected', on ? 'true' : 'false');
    btn.tabIndex = on ? 0 : -1;
  });
  panes.forEach((pane) => { pane.box.hidden = pane.id !== S.previewTab; });
}

/** "대기열이 비어 있어요" 자리에서 샘플로 건너뛴다 — 빈 화면만 보여 주고 끝내지 않는다. */
function openSampleTab() {
  const btn = sectionBox && sectionBox.querySelector('.prevtabs__btn[data-tab="sample"]');
  if (btn) btn.click();
}

/** 점수 슬라이더를 끌 때 샘플만 다시 그린다 (서버 왕복 없음). */
function repaintSampleQueue() {
  const box = sectionBox && sectionBox.querySelector('.smpl');
  if (box) renderSampleQueue(box);
}

function renderSampleQueue(box) {
  const points = draftVotePoints();
  const rounds = sampleRounds();
  const current = S.draft.sortMode;
  const orders = new Map(SORT_MODES.map((spec) => [spec.value, sampleSorted(spec.value, points, rounds)]));

  box.replaceChildren();
  box.append(h('div', { class: 'smpl__head' },
    h('strong', null, '예시 곡으로 세 방식을 비교하면'),
    h('span', { class: 'smpl__tag' }, '샘플'),
  ));
  box.append(h('p', { class: 'hint' },
    '이 서버의 진짜 대기열이 아니에요. 신청자 3명·6곡을 지금 화면의 점수 그대로 세 방식에 넣어 본 결과라, ' +
    '대기열이 비어 있어도 차이를 보실 수 있어요.'));

  // 3열 표는 좁은 화면에서 접히면 뜻이 사라진다(세로로 쌓으면 "나란히 비교"가 아니게 된다).
  // 그래서 폭을 지키고 가로로만 스크롤시킨다.
  const grid = h('div', { class: 'smpl__grid' });
  SORT_MODES.forEach((spec) => {
    const on = spec.value === current;
    const col = h('div', { class: `smpl__col${on ? ' is-on' : ''}` });
    col.append(h('div', { class: 'smpl__colhead' },
      h('span', { class: 'smpl__colname' }, spec.label),
      on ? h('span', { class: 'smpl__now' }, '지금 고른 방식') : null,
    ));
    const rows = h('ol', { class: 'smpl__rows' });
    (orders.get(spec.value) || []).forEach((row, index) => {
      rows.append(h('li', { class: 'smpl__row' },
        h('span', { class: 'smpl__rank' }, String(index + 1)),
        h('span', { class: 'smpl__main' },
          h('span', { class: 'smpl__title' }, row.title),
          h('span', { class: 'smpl__meta' }, `${row.who} · ${sampleReason(spec.value, row, points, rounds)}`),
        ),
      ));
    });
    col.append(rows);
    grid.append(col);
  });
  box.append(h('div', { class: 'smpl__scroll' }, grid));

  // 한 줄 요약 — 표를 다 안 읽어도 "뭐가 먼저 나가는지"는 바로 보이게 한다.
  const sum = h('p', { class: 'smpl__sum' }, h('span', { class: 'smpl__sumlabel' }, '맨 앞에 나갈 곡'));
  SORT_MODES.forEach((spec) => {
    const first = (orders.get(spec.value) || [])[0];
    if (!first) return;
    sum.append(h('span', { class: 'smpl__sumitem' },
      h('span', { class: 'smpl__sumname' }, spec.label),
      h('strong', null, first.title)));
  });
  box.append(sum);

  // 점수를 0으로 다 내리면 점수제가 시간제와 같아진다. 그때 "세 방식이 다르다"고 우기면 거짓말이 된다.
  const same = [];
  for (let i = 0; i < SORT_MODES.length; i += 1) {
    for (let j = i + 1; j < SORT_MODES.length; j += 1) {
      const left = (orders.get(SORT_MODES[i].value) || []).map((row) => row.id).join();
      const right = (orders.get(SORT_MODES[j].value) || []).map((row) => row.id).join();
      if (left === right) same.push(`${SORT_MODES[i].label}·${SORT_MODES[j].label}`);
    }
  }
  if (same.length) {
    box.append(h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null, `지금 점수 설정에서는 ${same.join(', ')} 결과가 같아요. 위쪽 투표 점수를 올리면 갈라져요.`),
    ));
  }
  tooltip(box);
}

/* ── 자동 재생 기준 곡 (v3 §8.5) ── 저장 버튼을 거치지 않고 바로 서버에 반영한다. */

/** 드래그 중인 시드의 cacheKey. 드롭 대상이 이 값을 보고 자리를 계산한다. */
let seedDragKey = null;

/** 기준 곡을 못 고칠 때의 이유 — 조건과 대상을 같이 말한다 (§23.3). */
const LOCKED_SEED_TIP = '"자동 재생 설정" 권한이 있어야 고쳐요';

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
  // 상한은 저장한 설정(`autoplaySeedMax`)이 이긴다. 0이면 무제한이라 "가득 참"이 없다 (§23.1).
  const configured = S.draft && S.draft.autoplaySeedMax != null ? Number(S.draft.autoplaySeedMax) : null;
  const max = configured != null ? configured : (state.max || SEED_MAX_FALLBACK);
  const unlimited = max === 0;
  const full = !unlimited && items.length >= max;
  const head = h('div', { class: 'seeds__head' },
    h('span', {
      class: 'seeds__count' + (full ? ' is-full' : ''),
      'data-tip': unlimited ? '상한을 무제한으로 두셨어요' : `${max}곡까지 넣을 수 있어요`,
    }, unlimited ? `${items.length}곡 · 무제한` : `${items.length} / ${max}곡`),
    h('span', { class: 'hint' },
      unlimited ? '몇 곡이든 넣을 수 있어요.'
        : full ? '자리가 다 찼어요. 새로 넣으려면 하나를 빼 주세요.'
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
      '이 서버에서는 기준 곡을 바꿀 권한이 없어서 목록만 보여 드려요. ' +
      '권한 섹션의 "자동 재생 설정"을 관리자나 내 역할이 통과하도록 바꾸시면 편집할 수 있어요.'));
  }
  if (S.draft.autoplayMode !== 'seed') {
    notes.push(h('p', { class: 'hint' },
      '지금 방식은 "기준 곡"이 아니에요. 그래도 다른 방식으로 후보를 못 구하면 이 목록으로 넘어와서 골라요.'));
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
    // 권한이 없어도 버튼을 숨기지 않는다 (§23.3). 회색으로 두되 왜 안 되는지 툴팁에 적는다.
    h('div', { class: 'seed__acts' },
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button',
        disabled: !canEdit || index === 0,
        'data-tip': !canEdit ? LOCKED_SEED_TIP : index === 0 ? '이미 맨 위예요' : '한 칸 위로 올려요',
        'aria-label': `${track.title || '이 곡'} 한 칸 위로`,
        onclick: () => moveSeedBy(index, -1),
      }, '↑'),
      h('button', {
        class: 'btn btn--sm btn--icon', type: 'button',
        disabled: !canEdit || index === total - 1,
        'data-tip': !canEdit ? LOCKED_SEED_TIP : index === total - 1 ? '이미 맨 아래예요' : '한 칸 아래로 내려요',
        'aria-label': `${track.title || '이 곡'} 한 칸 아래로`,
        onclick: () => moveSeedBy(index, 1),
      }, '↓'),
      h('button', {
        class: 'btn btn--sm' + (canEdit ? ' btn--danger' : ''), type: 'button',
        disabled: !canEdit,
        'data-tip': canEdit ? '기준 곡에서 빼요' : LOCKED_SEED_TIP,
        onclick: () => removeSeed(item),
      }, '빼기'),
    ),
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
    h('h3', { class: 'grp__title' }, `기능별 권한 ${PERM_FIELDS.length}종`),
    h('p', { class: 'grp__desc' },
      '각 동작을 누가 할 수 있는지 따로 정해요. "지정 역할"을 고른 항목에만 역할 선택기가 펼쳐지고, ' +
      '고른 역할은 그 항목에만 적용돼요. 검색용으로 @DJ를 줬다고 볼륨까지 열리지 않아요.'),
    h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null,
        '여기서 막은 기능은 리모컨에서 그냥 회색이 되지 않아요. ' +
        '"왜 안 되는지"와 "지금 누가 쓸 수 있는지(역할 이름 + 인원수)"가 버튼 툴팁에 같이 떠요. ' +
        '그래서 아래 통과 인원이 0명이면 멤버에게도 0명이라고 보여요.'),
    ),
  );

  PERM_FIELDS.forEach((field) => group.append(permField(field)));
  body.append(group);
  body.append(roleViewBox());

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
/* 특정 역할로 보기 (§37).
 *
 * 관리자는 **자기 화면만** 볼 수 있다. 관리자는 모든 규칙을 우회하니까 뭘 잠가 놔도
 * 자기한테는 다 열려 보인다. 그래서 "일반 멤버한테 실제로 어떻게 보이지"를 확인할 방법이
 * 없었다. Discord 의 "역할로 보기"와 같은 목적이다.
 *
 * 판정은 **서버가 실제 경로로** 한다. 여기서 규칙을 다시 구현하면 미리보기와 실제가
 * 갈라져서, 미리보기를 믿고 설정한 게 틀리는 최악이 된다.
 */
function roleViewBox() {
  const picked = new Set();
  let sameVoice = false;
  const out = h('div', { class: 'roleview__out' });

  const refresh = async () => {
    out.replaceChildren(h('div', { class: 'skel', style: 'height:60px' }));
    try {
      const params = new URLSearchParams();
      if (picked.size) params.set('roles', [...picked].join(','));
      if (sameVoice) params.set('sameVoice', 'true');
      const data = await api(`/admin/roleview?${params.toString()}`);
      const rows = (data.permissions || []).map((row) => h('div', {
        class: 'roleview__row' + (row.allowed ? ' is-on' : ''),
      },
        h('span', null, row.allowed ? '✅' : '🚫'),
        h('span', { class: 'roleview__key' }, PERM_LABEL[row.key] || row.key),
        h('span', { class: 'roleview__rule' }, row.ruleLabel)));
      out.replaceChildren(
        h('p', { class: 'hint' },
          picked.size
            ? `${(data.roleNames || []).map((n) => '@' + n).join(' · ')} 역할만 가진 사람이 보는 화면이에요.`
            : '역할이 하나도 없는 사람이 보는 화면이에요.'),
        h('div', { class: 'roleview__grid' }, ...rows));
    } catch (error) {
      out.replaceChildren(h('p', { class: 'hint' }, `못 불러왔어요 — ${error.message}`));
    }
  };

  const chips = (S.roles || []).map((role) => {
    const chip = h('button', {
      class: 'chipbtn', type: 'button', 'aria-pressed': 'false',
      onClick: () => {
        const id = String(role.id);
        if (picked.has(id)) picked.delete(id); else picked.add(id);
        chip.setAttribute('aria-pressed', String(picked.has(id)));
        refresh();
      },
    }, `@${role.name}`);
    return chip;
  });

  const voiceToggle = h('button', {
    class: 'chipbtn', type: 'button', 'aria-pressed': 'false',
    // 같은 음성 채널 규칙은 역할과 무관하게 결과를 바꾼다. 같이 켜 봐야 진짜가 보인다.
    title: '봇과 같은 음성 채널에 있다고 치고 봐요',
    onClick: () => {
      sameVoice = !sameVoice;
      voiceToggle.setAttribute('aria-pressed', String(sameVoice));
      refresh();
    },
  }, '🔊 같은 음성 채널에 있음');

  refresh();
  return h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '👁 특정 역할로 보기'),
    h('p', { class: 'grp__desc' },
      '고른 역할만 가진 사람에게 무엇이 열리는지 그대로 보여줘요. ' +
      '관리자는 모든 규칙을 우회하기 때문에 자기 화면만 봐서는 확인이 안 돼요.'),
    h('div', { class: 'roleview__pick' }, ...chips, voiceToggle),
    out);
}

/** 권한 키 → 사람이 읽는 이름. 미리보기 표에서 쓴다. */
const PERM_LABEL = {
  search: '곡 검색·신청', vote: '좋아요·싫어요', playback: '재생 / 일시정지',
  skip: '곡 넘기기', seek: '재생 위치 이동', volume: '서버 볼륨', queueEdit: '대기열 편집',
  chat: '채팅 쓰기', autoplay: '자동 재생', bulkEnqueue: '전부 담기',
};

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
    }, field.label),
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
 * 칩 하나를 눌렀다고 섹션 전체를 다시 그리면 10개 미리보기가 전부 다시 날아가므로 제자리에서 토글한다.
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
      paintPermPreview(box, data, rule, field);
    } catch (error) {
      box.replaceChildren(h('span', { class: 'permprev__fail' }, `통과 인원을 못 셌어요 — ${error.message}`));
    }
  }, 180);
}

function paintPermPreview(box, data, rule, field) {
  const pass = Number(data.passCount || 0);
  const total = Number(data.memberCount || 0);
  const tone = rule === 'disabled' ? 'is-none' : pass === 0 ? 'is-none' : pass === total ? 'is-all' : 'is-some';
  const kids = [
    h('span', {
      class: `permprev__count ${tone}`,
      'data-tip': rule === 'disabled'
        ? '규칙이 사용 안 함이라 관리자도 못 써요'
        : '지금 이 서버에서 이 규칙을 통과하는 사람 수예요',
    }, rule === 'disabled' ? '지금 통과: 0명 — 아무도 못 써요' : `지금 통과: ${pass}명 / 멤버 ${total}명`),
  ];
  if (data.managerBypassCount) {
    kids.push(h('span', { class: 'permprev__note' }, `그중 ${data.managerBypassCount}명은 관리자라서 통과해요`));
  }
  // §23.3 — 막힌 사람에게 보여줄 "누가 되는지" 문구를 관리자도 미리 확인한다.
  // 서버가 `allowedRoleNames` 를 안 주는 빌드면 콘솔이 이미 들고 있는 역할 이름으로 채운다.
  // 그게 없으면 "지정 역할" 규칙일 때 이 줄이 통째로 안 나와서, 관리자가 멤버에게 뭐라고 보이는지
  // 미리 확인할 방법이 사라진다.
  const fromServer = Array.isArray(data.allowedRoleNames) ? data.allowedRoleNames : [];
  const allowedRoles = fromServer.length
    ? fromServer.map((name) => `@${name}`)
    : (rule === 'configuredRole' && field
      ? roleNames((S.draft.ruleRoleIds || {})[field.permKey] || [])
      : []);
  if (allowedRoles.length) {
    kids.push(h('span', { class: 'permprev__note' },
      `멤버에게는 "지금은 ${allowedRoles.join(' · ')}이 쓸 수 있어요 (${data.allowedCount != null ? data.allowedCount : pass}명)"로 보여요`));
  } else if (rule === 'administrator') {
    kids.push(h('span', { class: 'permprev__note' },
      `멤버에게는 "서버 관리자 ${data.allowedCount != null ? data.allowedCount : pass}명이 쓸 수 있어요"로 보여요`));
  } else if (rule === 'disabled') {
    kids.push(h('span', { class: 'permprev__note' }, '멤버에게는 "이 기능은 서버에서 꺼 뒀어요"로 보여요'));
  } else if (rule === 'sameVoiceChannel') {
    kids.push(h('span', { class: 'permprev__note' }, '멤버에게는 "봇과 같은 음성 채널에 있어야 눌러요"로 보여요'));
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
      h('p', { class: 'grp__desc' },
        '한 사람이 얼마나 넣을 수 있고 서버 전체로는 얼마까지 받을지 정해요. ' +
        '모든 칸은 맨 끝으로 밀면 ∞(무제한)이 되고, 직접입력에 0을 넣어도 같아요.'),
      numberField('maxQueuePerUser'),
      numberField('maxQueuePerGuild'),
      numberField('maxTrackSeconds'),
      numberField('bulkEnqueueLimit'),
    ),
    h('div', { class: 'grp' },
      h('h3', { class: 'grp__title' }, '보관 기간'),
      h('p', { class: 'grp__desc' },
        '오래된 기록은 자동으로 지워요. 길게 잡으면 DB가 커져요. ' +
        '투표와 재생 조작 기록은 양이 많아서 여기 값과 상관없이 3일만 남아요.'),
      numberField('auditRetentionDays'),
      numberField('chatRetentionDays'),
    ),
  );
}

/* ═══════════════════════════ 섹션 3b · 차단 목록 (v3 §19) ═══════════════════════════ */

function sectionBlocked() {
  const body = h('div', { class: 'sec__body' });

  // 시험 입력창 — 규칙을 넣고 나서 "왜 안 막히지?"를 남기지 않는다.
  const testResult = h('div', { class: 'testbox__result' });
  const testInput = h('input', {
    class: 'field', type: 'search',
    placeholder: '곡 제목이나 주소를 넣어 보세요',
    'data-tip': '지금 규칙으로 막히는지 바로 확인해요',
  });
  let testTimer = null;
  testInput.addEventListener('input', (event) => {
    clearTimeout(testTimer);
    const query = event.target.value;
    if (!query.trim()) { testResult.replaceChildren(); return; }
    testTimer = setTimeout(() => runBlacklistTest(query, testResult), 260);
  });

  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '🔎 막히는지 시험해 보기'),
    h('p', { class: 'grp__desc' },
      '지금 규칙으로 이 곡이 막히는지, 막힌다면 어떤 규칙에 걸렸는지 바로 알려드려요.'),
    h('div', { class: 'testbox' }, testInput, testResult),
  ));

  // 추가 폼 — 모달까지 갈 일이 아니라 인라인이다.
  const kindBox = h('div', { class: 'seg' });
  BLACKLIST_KINDS.forEach((kind) => {
    kindBox.append(h('button', {
      class: 'seg__btn' + (kind.value === S.blocked.kind ? ' is-on' : ''), type: 'button',
      'data-tip': kind.desc,
      onclick: (event) => {
        S.blocked.kind = kind.value;
        kindBox.querySelectorAll('.seg__btn').forEach((node) => node.classList.remove('is-on'));
        event.currentTarget.classList.add('is-on');
        const note = body.querySelector('[data-kind-note]');
        if (note) note.textContent = kind.desc;
        repaintBlocked();
      },
    }, kind.label));
  });
  const patternInput = h('input', { class: 'field', placeholder: '막을 제목이나 주소', maxlength: '200' });
  const noteInput = h('input', { class: 'field', placeholder: '메모 (왜 막았는지 · 선택)', maxlength: '120' });

  const listBox = h('div', { class: 'card blocklist' });
  body.append(h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '차단 규칙'),
    h('p', { class: 'grp__desc' },
      '종류를 고르고 패턴을 넣으시면 돼요. 여기서 만든 규칙은 이 서버에만 적용돼요. ' +
      '봇 전체 규칙은 목록에 보이지만 지울 수 없어요.'),
    kindBox,
    h('p', { class: 'hint', 'data-kind-note': '1' },
      (BLACKLIST_KINDS.find((kind) => kind.value === S.blocked.kind) || BLACKLIST_KINDS[0]).desc),
    h('div', { class: 'addform' },
      patternInput,
      noteInput,
      h('button', {
        class: 'btn btn--primary', type: 'button',
        'data-tip': '이 서버의 차단 목록에 넣어요',
        onclick: () => addBlocked(patternInput, noteInput),
      }, '추가'),
    ),
    listBox,
  ));

  paintBlocked(listBox);
  if (S.blocked.items === null && !S.blocked.loading) loadBlocked().then(() => repaintBlocked());
  return body;
}

async function runBlacklistTest(query, box) {
  box.replaceChildren(h('div', { class: 'skel', style: 'height:20px;width:200px' }));
  try {
    const data = await api('/admin/blacklist/test', { method: 'POST', body: { query } });
    if (data.blocked) {
      const rule = data.rule || {};
      box.replaceChildren(h('div', { class: 'testbox__hit is-blocked' },
        h('span', null, '🚫 막혀요'),
        h('span', { class: 'hint' },
          `${(BLACKLIST_KINDS.find((kind) => kind.value === rule.kind) || {}).label || rule.kind || '규칙'}` +
          ` · "${rule.pattern || ''}"` +
          `${rule.scope === 'global' ? ' · 봇 전체 규칙이에요' : ' · 이 서버 규칙이에요'}`),
      ));
    } else {
      box.replaceChildren(h('div', { class: 'testbox__hit is-pass' },
        h('span', null, '✅ 안 막혀요'),
        h('span', { class: 'hint' }, '지금 규칙으로는 이 곡이 그대로 들어와요.'),
      ));
    }
  } catch (error) {
    box.replaceChildren(h('p', { class: 'hint' }, `시험해 보지 못했어요 — ${error.message}`));
  }
}

async function loadBlocked() {
  if (S.blocked.loading) return;
  S.blocked.loading = true;
  try {
    const data = await api('/admin/blacklist');
    S.blocked = {
      items: data.items || [],
      // 전체 차단 규칙은 봇 주인만 본다 (§19.2). 여기서는 "있다"는 사실만 받는다.
      globalNote: data.globalNote || null,
      error: null,
      loading: false,
      kind: S.blocked.kind,
    };
  } catch (error) {
    S.blocked = { items: [], error: error.message, loading: false, kind: S.blocked.kind };
  }
}

function repaintBlocked() {
  const box = sectionBox && sectionBox.querySelector('.blocklist');
  if (box) paintBlocked(box);
}

function paintBlocked(box) {
  if (S.blocked.loading || S.blocked.items === null) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:80px;margin:12px' }));
    return;
  }
  if (S.blocked.error) {
    box.replaceChildren(h('p', { class: 'hint', style: 'padding:12px' },
      `차단 목록을 못 불러왔어요 — ${S.blocked.error}`));
    return;
  }
  // 전체 규칙이 걸려 있으면 **내용 없이** 그 사실만 알린다. 이걸 빼면 규칙에 걸렸을 때
  // 자기 목록엔 아무것도 없는데 왜 막히는지 알 길이 없다.
  const globalHint = S.blocked.globalNote
    ? h('p', { class: 'hint', style: 'padding:8px 12px' }, `🔒 ${S.blocked.globalNote}`)
    : null;
  const items = S.blocked.items.filter((item) => item.kind === S.blocked.kind);
  if (!items.length) {
    box.replaceChildren(
      globalHint,
      h('div', { class: 'empty' },
        h('div', { class: 'empty__icon' }, '🕊'),
        h('div', { class: 'empty__title' }, '이 종류로 막아 둔 게 없어요'),
        h('div', { class: 'empty__desc' },
          '리모컨의 대기열이나 검색 결과에서 곡의 ⋯ 메뉴를 열면 "차단 목록에 넣기"로 바로 넣을 수도 있어요.'),
      ));
    return;
  }
  const rows = h('ul', { class: 'rows' });
  items.forEach((item) => {
    const global = item.scope === 'global';
    rows.append(h('li', { class: 'row row--block' + (global ? ' is-global' : '') },
      h('div', { class: 'row__main' },
        h('div', { class: 'row__name' },
          global ? h('span', { 'data-tip': '봇 주인이 만든 전체 규칙이라 여기서는 못 지워요' }, '🔒') : null,
          h('code', { class: 'block__pattern' }, item.pattern),
          global ? h('span', { class: 'chip chip--warn' }, '봇 전체 규칙') : null,
        ),
        h('div', { class: 'row__sub' },
          item.note ? h('span', null, item.note) : h('span', null, '메모가 없어요'),
          h('span', null, `· ${item.createdByName || '누군가'}이 넣었어요`),
          item.createdUtc ? h('span', null, `· ${fmtAgo(item.createdUtc)}`) : null,
        ),
      ),
      h('button', {
        class: 'btn btn--sm' + (global ? '' : ' btn--danger'), type: 'button',
        disabled: global,
        'data-tip': global
          ? '봇 전체 규칙이에요. 봇 주인만 지울 수 있어요'
          : '이 규칙을 지워요',
        onclick: () => removeBlocked(item),
      }, global ? '못 지워요' : '지우기'),
    ));
  });
  box.replaceChildren(globalHint, rows);
  tooltip(box);
}

async function addBlocked(patternInput, noteInput) {
  const pattern = patternInput.value.trim();
  if (!pattern) { toast('막을 제목이나 주소를 넣어 주세요.', 'warn'); return; }
  try {
    await api('/admin/blacklist', {
      method: 'POST',
      body: { kind: S.blocked.kind, pattern, note: noteInput.value.trim() },
    });
    patternInput.value = '';
    noteInput.value = '';
    await loadBlocked();
    repaintBlocked();
    toast('차단 목록에 넣었어요.', 'ok');
  } catch (error) {
    toast(`넣지 못했어요 — ${error.message}`, 'danger');
  }
}

async function removeBlocked(item) {
  const ok = await confirmSheet({
    title: '차단 규칙 지우기',
    desc: `"${item.pattern}" 규칙을 지워요. 지금부터 이 곡이 다시 들어올 수 있어요.`,
    confirmText: '지울게요', cancelText: '그냥 둘게요', danger: true,
  });
  if (!ok) return;
  try {
    await api('/admin/blacklist/remove', { method: 'POST', body: { id: item.id } });
    S.blocked.items = S.blocked.items.filter((entry) => entry.id !== item.id);
    repaintBlocked();
    toast('규칙을 지웠어요.', 'ok');
  } catch (error) {
    toast(`지우지 못했어요 — ${error.message}`, 'danger');
  }
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
    'data-tip': '적은 글자가 들어간 사람만 남겨요',
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
        'data-tip': '지금 바로 정지를 풀어요',
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
      'data-tip': tipOf(item.desc),
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
      'data-tip': item.minutes === null ? '직접 풀 때까지 계속 막아요' : `${item.label} 동안 막아요`,
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
      h('button', {
        class: 'btn btn--sm', type: 'button',
        'data-tip': '보관 기간을 모아 둔 제한값 섹션으로 가요',
        onclick: () => goSection('limits'),
      }, '제한값에서 바꾸기 →'),
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
        h('button', {
          class: 'btn btn--sm btn--danger', type: 'button',
          'data-tip': '이 채팅을 지우고 신고를 닫아요',
          onclick: () => resolveReport(report, 'delete'),
        }, '메시지 삭제'),
        h('button', {
          class: 'btn btn--sm', type: 'button',
          'data-tip': '메시지는 두고 신고만 닫아요',
          onclick: () => resolveReport(report, 'dismiss'),
        }, '문제 없어요'),
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
      'data-tip': '이 제안의 처리 상태를 바꿔요',
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

  /** 조건이 바뀌면 목록을 처음부터 다시 받는다. 커서 기반이라 이어 붙이면 안 된다. */
  const reload = () => {
    S.audit.items = [];
    S.audit.cursor = null;
    S.audit.done = false;
    S.audit.loading = false;
    paintAudit(rows, sentinel);
    loadAudit(rows, sentinel);
  };

  const filter = h('input', {
    class: 'field', type: 'search', 'data-testid': 'audit-filter',
    placeholder: '사람 · 동작 · 곡 제목으로 찾아요',
    value: S.audit.query,
    'data-tip': '적은 글자가 들어간 기록만 남겨요',
  });
  let timer = null;
  filter.addEventListener('input', (event) => {
    clearTimeout(timer);
    const value = event.target.value;
    timer = setTimeout(() => { S.audit.query = value; reload(); }, 220);
  });

  // 분류 칩 — 관리 콘솔은 기본으로 전부 켠다. 관리자는 다 봐야 하는 화면이니까.
  const chips = h('div', { class: 'chips' });
  AUDIT_KINDS.forEach((kind) => {
    const on = S.audit.kinds.includes(kind.value);
    const chip = h('button', {
      class: 'kindchip' + (on ? ' is-on' : ''), type: 'button',
      'aria-pressed': on ? 'true' : 'false',
      'data-tip': on ? `${kind.label} 기록을 숨겨요` : `${kind.label} 기록을 다시 보여요`,
    }, `${kind.icon} ${kind.label}`);
    chip.addEventListener('click', () => {
      const next = new Set(S.audit.kinds);
      const was = next.has(kind.value);
      if (was) next.delete(kind.value); else next.add(kind.value);
      S.audit.kinds = Array.from(next);
      chip.classList.toggle('is-on', !was);
      chip.setAttribute('aria-pressed', was ? 'false' : 'true');
      reload();
    });
    chips.append(chip);
  });

  const failedOnly = h('button', {
    class: 'kindchip' + (S.audit.failedOnly ? ' is-on' : ''), type: 'button',
    'aria-pressed': S.audit.failedOnly ? 'true' : 'false',
    'data-tip': '거부되거나 실패한 기록만 남겨요',
  }, '⚠ 실패만 보기');
  failedOnly.addEventListener('click', () => {
    S.audit.failedOnly = !S.audit.failedOnly;
    failedOnly.classList.toggle('is-on', S.audit.failedOnly);
    failedOnly.setAttribute('aria-pressed', S.audit.failedOnly ? 'true' : 'false');
    reload();
  });

  const body = h('div', { class: 'sec__body' },
    h('div', { class: 'grp__tools' }, filter, failedOnly),
    chips,
    h('p', { class: 'hint' },
      '관리 콘솔의 기록은 합치지 않아요. 누가 무엇을 넣었는지 하나하나 다 보여드려요. ' +
      '멤버가 보는 로그 탭에서만 "곡 7개를 담았어요"처럼 묶여요.'),
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
  // 칩을 전부 끄면 볼 게 없다. 그걸 "전부 보기"로 해석하면 필터가 거짓말을 한다.
  if (!S.audit.kinds.length) {
    S.audit.done = true;
    paintAudit(rows, sentinel);
    return;
  }
  S.audit.loading = true;
  sentinel.replaceChildren(h('div', { class: 'skel', style: 'height:28px' }));
  try {
    const params = new URLSearchParams({ limit: '50' });
    if (S.audit.cursor) params.set('before', String(S.audit.cursor));
    if (S.audit.query) params.set('q', S.audit.query);
    // 전부 켜져 있으면 아예 안 보낸다 — 서버가 필터를 모르는 버전이어도 똑같이 동작한다.
    if (S.audit.kinds.length && S.audit.kinds.length < AUDIT_KINDS.length) {
      params.set('kinds', S.audit.kinds.join(','));
    }
    if (S.audit.failedOnly) params.set('success', 'false');
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
  // 서버가 `kinds` 를 모르는 버전이어도 화면은 칩과 일치해야 한다. 분류를 모르는 줄은 남긴다.
  const wanted = new Set(S.audit.kinds);
  S.audit.items = S.audit.items.filter((entry) => !entry.kind || wanted.has(entry.kind));
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

/**
 * 기록 한 줄. 서버가 사람 문장(`text`)을 완성해서 주면 그걸 그대로 쓴다 (§13.5) —
 * 클라이언트가 액션명을 문장으로 바꾸는 로직을 갖지 않는다.
 * 관리 콘솔에만 전후값과 실패 사유가 같이 내려온다 (§13.2).
 */
function auditRow(entry) {
  const failed = entry.success === false;
  const kind = AUDIT_KINDS.find((item) => item.value === entry.kind);
  const changed = entry.beforeValue != null || entry.afterValue != null;
  const detail = failed
    ? (entry.failureReason || '이유를 남기지 못했어요')
    : changed
      ? `${entry.beforeValue == null ? '(없음)' : entry.beforeValue} → ${entry.afterValue == null ? '(없음)' : entry.afterValue}`
      : (entry.text || entry.target || '');

  return h('li', {
    class: 'row row--audit' + (failed ? ' is-fail' : ''),
    'data-tip': entry.text || entry.action,
  },
    h('time', { class: 'audit__time' }, fmtAgo(entry.createdUtc)),
    h('strong', { class: 'audit__who' }, entry.actorName || entry.displayName || String(entry.userId)),
    h('span', { class: 'audit__what' },
      kind ? `${kind.icon} ` : '', entry.action),
    h('span', { class: 'audit__detail mq' }, h('span', { class: 'mq__i' }, detail)),
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
      'data-tip': '봇 상태를 지금 다시 물어봐요',
    }, '다시 확인할게요'),
  ));

  body.append(serverStatsGroup());

  paintDiag(box);
  if (!S.diag) loadDiag().then(() => paintDiag(box));
  return body;
}

/* ── 서버 통계 (v3 §22.6 `GET /stats/server` — "관리 콘솔용") ──
 * 만들어만 두고 아무도 안 쓰던 엔드포인트다. 관리자가 "우리 서버에서 뭐가 많이 나갔나"를
 * 볼 유일한 자리라 여기에 붙인다. 통계가 꺼져 있으면 **0으로 꾸미지 않고** 그대로 말한다.
 */

function serverStatsGroup() {
  const box = h('div', { class: 'srvstats' });
  paintServerStats(box);
  if (!S.serverStats) loadServerStats().then(() => paintServerStats(box));
  return h('div', { class: 'grp' },
    h('h3', { class: 'grp__title' }, '📊 이번 달 서버 기록'),
    h('p', { class: 'grp__desc' },
      '최근 30일 동안 이 서버에서 사람이 신청해 실제로 나간 곡이에요. 자동 재생으로 나간 건 순위에 안 세요. ' +
      '값은 60초 동안 캐시돼요.'),
    box,
  );
}

async function loadServerStats() {
  try {
    S.serverStats = await api('/stats/server');
  } catch (error) {
    S.serverStats = { error: error.message };
  }
}

function paintServerStats(box) {
  const data = S.serverStats;
  if (!data) {
    box.replaceChildren(h('div', { class: 'skel', style: 'height:96px' }));
    return;
  }
  if (data.error) {
    box.replaceChildren(h('p', { class: 'hint' }, `서버 기록을 못 불러왔어요 — ${data.error}`));
    return;
  }
  // 통계가 꺼져 있는 것과 "기록이 0" 은 완전히 다른 이야기다 (§22.6).
  if (data.available === false) {
    box.replaceChildren(h('div', { class: 'warnbox warnbox--info' },
      h('span', null, 'ℹ'),
      h('span', null, data.message || '통계 기록이 꺼져 있어서 보여 드릴 게 없어요.'),
    ));
    return;
  }

  const trackTitle = (row) => {
    const track = row.track || {};
    const title = track.title || row.cacheKey || '(제목이 없어요)';
    return track.artist ? `${track.artist} - ${title}` : title;
  };
  const table = (heading, rows, valueOf, tipOfRow) => {
    const list = h('ol', { class: 'srvstats__rows' });
    (rows || []).slice(0, 10).forEach((row) => {
      list.append(h('li', { class: 'srvstats__row', 'data-tip': tipOfRow(row) },
        h('span', { class: 'srvstats__rank' }, `${row.rank}`),
        h('span', { class: 'srvstats__title' }, trackTitle(row)),
        h('span', { class: 'srvstats__value' }, valueOf(row)),
      ));
    });
    return h('div', { class: 'srvstats__col' },
      h('h4', { class: 'srvstats__head' }, heading),
      (rows || []).length ? list
        : h('p', { class: 'hint' }, '이번 달에는 아직 기록이 없어요.'),
    );
  };

  box.replaceChildren(
    table('많이 나간 곡', data.topPlayed,
      (row) => `${Number(row.plays) || 0}회`,
      (row) => `${Number(row.requesters) || 0}명이 신청했고 자동 재생으로 ${Number(row.playsAutoplay) || 0}회 더 나갔어요`),
    table('많이 사랑받은 곡', data.topLoved,
      (row) => `${Number(row.loveScore) || 0}점`,
      (row) => row.loveFormula || `슈퍼 좋아요를 ${data.superWeight}배로 셌어요`),
  );
  tooltip(box);
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
    // 투표 스킵 환산의 모수도 여기서 한 번 채운다. 그 뒤로는 WS presence 가 갱신한다.
    const bot = S.diag.bot || {};
    const inVoice = bot.inVoice != null ? bot.inVoice : bot.voiceConnected;
    if (bot.listenerCount != null) S.basis.listeners = inVoice ? Number(bot.listenerCount) : 0;
    else if (inVoice === false) S.basis.listeners = 0;
    if (S.diag.viewerCount != null) S.basis.viewers = Number(S.diag.viewerCount);
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
    ['봇 연결', bot.online ? '온라인이에요' : '오프라인이에요', bot.online ? 'is-ok' : 'is-bad',
      'Discord 게이트웨이에 붙어 있는지예요'],
    ['서버 참가 여부',
      inGuild == null ? '알 수 없어요' : inGuild ? '이 서버에 들어와 있어요' : '이 서버에 없어요',
      inGuild == null ? '' : inGuild ? 'is-ok' : 'is-bad',
      '봇이 이 Discord 서버의 멤버인지예요'],
    ['음성 채널 참가 여부',
      inVoice == null ? '알 수 없어요' : inVoice ? `참가 중이에요 (${channel})` : '아직 안 들어갔어요',
      inVoice ? 'is-ok' : '',
      'Discord 가 지금 알려 주는 값이라 저장된 값과 어긋나지 않아요'],
    ['듣는 사람 수',
      listeners == null ? '알 수 없어요' : inVoice ? `${listeners}명이 듣고 있어요` : '0명 (음성 채널에 없어요)',
      listeners > 0 ? 'is-ok' : '',
      '봇과 같은 음성 채널에 있는 사람 수예요'],
    ['게이트웨이 지연', bot.gatewayLatencyMs != null ? `${bot.gatewayLatencyMs}ms` : '알 수 없어요', '',
      'Discord 와 주고받는 데 걸리는 시간이에요'],
    ['빌드 ID', S.diag.buildId || M.buildId || '-', '', '지금 돌고 있는 봇의 빌드예요'],
    ['DB 스키마 버전', S.diag.schemaVersion != null ? `v${S.diag.schemaVersion}` : '-', '',
      '마이그레이션이 어디까지 적용됐는지예요'],
    ['가동 시간', S.diag.uptimeSeconds != null ? prettySeconds(S.diag.uptimeSeconds) : '-', '',
      '마지막으로 재시작한 뒤 지난 시간이에요'],
  ];
  const grid = h('div', { class: 'diag__grid' });
  cells.forEach(([label, value, tone, tip]) => {
    grid.append(h('div', { class: `diag__cell ${tone}`, 'data-tip': tipOf(tip) },
      h('div', { class: 'diag__label' }, label),
      h('div', { class: 'diag__value' }, String(value)),
    ));
  });
  box.replaceChildren(grid);
  tooltip(box);

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
  blocked: sectionBlocked,
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
        'data-tip': '이 섹션에서 바꾼 걸 전부 저장 전 값으로 돌려요',
      }, '되돌리기'),
      h('button', {
        class: 'btn btn--primary', type: 'button', disabled: true,
        'data-testid': 'settings-save',
        'data-tip': '이 섹션에서 바꾼 항목만 서버에 보내요',
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

  dirtyBadge = h('span', {
    class: 'head__dirty', hidden: true,
    'data-tip': '아직 저장하지 않은 항목이 있어요',
  });

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
    h('span', {
      class: `tier tier--${M.tier}`,
      'data-tip': IS_OWNER ? '봇을 돌리는 사람이라 모든 서버를 볼 수 있어요' : '이 서버의 관리자라 여기 설정을 바꿀 수 있어요',
    }, IS_OWNER ? '🛡 봇 주인' : '🛡 서버 관리자'),
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
      'data-tip': tipOf(spec.desc),
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
      h('a', {
        class: 'btn btn--primary', href: `/music/guilds/${GUILD_ID}`,
        'data-tip': '유저용 리모컨 화면으로 가요',
      }, '← 리모컨으로 돌아가기'),
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
      h('button', {
        class: 'btn btn--primary', type: 'button',
        'data-tip': '화면을 새로고침해서 설정을 다시 불러와요',
        onclick: () => location.reload(),
      }, '다시 시도할게요'),
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
  if (topic === 'presence') {
    // 투표 스킵 환산(§10.5)의 모수. 새 요청 없이 이미 오는 프레임에서 세기만 한다.
    const bot = (data && data.bot) || {};
    S.basis.listeners = bot.inVoice === false
      ? 0
      : (bot.listenerCount != null ? Number(bot.listenerCount) : ((data && data.listening) || []).length);
    S.basis.viewers = ((data && data.viewing) || []).length;
    if (S.activeSection === 'order') {
      const box = sectionBox && sectionBox.querySelector('.convert');
      if (box) paintSkipConvert(box);
    }
  }
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
  if (topic === 'charts') {
    S.charts = { items: null, error: null, loading: false };
    S.genreOptions = null;
    if (S.activeSection === 'order') loadCharts().then(() => repaintCharts());
    return;
  }
  if (topic === 'blacklist') {
    S.blocked = { items: null, error: null, loading: false, kind: S.blocked.kind };
    if (S.activeSection === 'blocked') loadBlocked().then(() => repaintBlocked());
    return;
  }
  if (topic === 'queue.set' && S.activeSection === 'order') {
    S.queuePreview = { mode: null, data: null, loading: false };
    loadQueuePreview();
  }
}

boot();
