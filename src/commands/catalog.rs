//! 슬래시 명령 카탈로그 — C# CommandCatalog 1:1 (대표 명령 + 한국어/축약/초성 별칭).

pub struct CommandDef {
    pub name: &'static str,
    pub description: &'static str,
    pub canonical: &'static str,
}

pub const ALL: &[CommandDef] = &[
    CommandDef {
        name: "play",
        description: "곡이나 재생목록 주소를 지금 대기열에 담아요.",
        canonical: "play",
    },
    CommandDef {
        name: "재생",
        description: "곡이나 재생목록 주소를 지금 대기열에 담아요.",
        canonical: "play",
    },
    CommandDef {
        name: "p",
        description: "재생 명령의 영문 축약이에요.",
        canonical: "play",
    },
    CommandDef {
        name: "ㅈㅅ",
        description: "재생 명령의 초성 별칭이에요.",
        canonical: "play",
    },
    CommandDef {
        name: "playnow",
        description: "지금 곡과 대기열을 비우고(반복 중이면 뒤로 보내고) 바로 재생해요.",
        canonical: "playnow",
    },
    CommandDef {
        name: "바로재생",
        description: "지금 곡과 대기열을 비우고(반복 중이면 뒤로 보내고) 바로 재생해요.",
        canonical: "playnow",
    },
    CommandDef {
        name: "ㅂㄹㅈㅅ",
        description: "바로재생 명령의 초성 별칭이에요.",
        canonical: "playnow",
    },
    CommandDef {
        name: "queue",
        description: "지금 대기열과 재생 상태를 보여줘요.",
        canonical: "queue",
    },
    CommandDef {
        name: "대기열",
        description: "지금 대기열과 재생 상태를 보여줘요.",
        canonical: "queue",
    },
    CommandDef {
        name: "q",
        description: "대기열 명령의 영문 축약이에요.",
        canonical: "queue",
    },
    CommandDef {
        name: "ㄷㄱㅇ",
        description: "대기열 명령의 초성 별칭이에요.",
        canonical: "queue",
    },
    CommandDef {
        name: "nowplaying",
        description: "지금 재생 중인 곡을 보여줘요.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "현재곡",
        description: "지금 재생 중인 곡을 보여줘요.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "ㅈㅈㄱ",
        description: "현재곡 명령의 초성 별칭이에요.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "shuffle",
        description: "지금 곡 뒤에 남은 대기열만 무작위로 섞어요.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "셔플",
        description: "지금 곡 뒤에 남은 대기열만 무작위로 섞어요.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "ㅅㅍ",
        description: "셔플 명령의 초성 별칭이에요.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "repeat",
        description: "반복 재생 방식을 바꿔요.",
        canonical: "repeat",
    },
    CommandDef {
        name: "반복",
        description: "반복 재생 방식을 바꿔요.",
        canonical: "repeat",
    },
    CommandDef {
        name: "ㅂㅂ",
        description: "반복 명령의 초성 별칭이에요.",
        canonical: "repeat",
    },
    CommandDef {
        name: "autoplay",
        description: "자동 추천으로 다음 곡을 이어 붙일지 정해요.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "자동추천",
        description: "자동 추천으로 다음 곡을 이어 붙일지 정해요.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "ㅈㄷㅊㅊ",
        description: "자동추천 명령의 초성 별칭이에요.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "pause",
        description: "지금 재생 중인 곡을 잠깐 멈춰요.",
        canonical: "pause",
    },
    CommandDef {
        name: "일시정지",
        description: "지금 재생 중인 곡을 잠깐 멈춰요.",
        canonical: "pause",
    },
    CommandDef {
        name: "ㅇㅅㅈㅈ",
        description: "일시정지 명령의 초성 별칭이에요.",
        canonical: "pause",
    },
    CommandDef {
        name: "resume",
        description: "멈춰 둔 재생을 다시 시작해요.",
        canonical: "resume",
    },
    CommandDef {
        name: "재개",
        description: "멈춰 둔 재생을 다시 시작해요.",
        canonical: "resume",
    },
    CommandDef {
        name: "계속",
        description: "재개 명령의 쉬운 한국어 별칭이에요.",
        canonical: "resume",
    },
    CommandDef {
        name: "ㄱㅅ",
        description: "재개 명령의 초성 별칭이에요.",
        canonical: "resume",
    },
    CommandDef {
        name: "skip",
        description: "지금 곡을 건너뛰고 다음 곡으로 넘어가요.",
        canonical: "skip",
    },
    CommandDef {
        name: "스킵",
        description: "지금 곡을 건너뛰고 다음 곡으로 넘어가요.",
        canonical: "skip",
    },
    CommandDef {
        name: "넘김",
        description: "스킵 명령의 쉬운 한국어 별칭이에요.",
        canonical: "skip",
    },
    CommandDef {
        name: "ㄴㄱ",
        description: "스킵 명령의 초성 별칭이에요.",
        canonical: "skip",
    },
    CommandDef {
        name: "stop",
        description: "재생을 멈추고 플레이어를 정지 상태로 되돌려요.",
        canonical: "stop",
    },
    CommandDef {
        name: "정지",
        description: "재생을 멈추고 플레이어를 정지 상태로 되돌려요.",
        canonical: "stop",
    },
    CommandDef {
        name: "ㅈㅈ",
        description: "정지 명령의 초성 별칭이에요.",
        canonical: "stop",
    },
    CommandDef {
        name: "move",
        description: "대기열에 있는 곡의 순서를 바꿔요.",
        canonical: "move",
    },
    CommandDef {
        name: "이동",
        description: "대기열에 있는 곡의 순서를 바꿔요.",
        canonical: "move",
    },
    CommandDef {
        name: "ㅇㄷ",
        description: "이동 명령의 초성 별칭이에요.",
        canonical: "move",
    },
    CommandDef {
        name: "remove",
        description: "대기열에 있는 곡을 빼요.",
        canonical: "remove",
    },
    CommandDef {
        name: "제거",
        description: "대기열에 있는 곡을 빼요.",
        canonical: "remove",
    },
    CommandDef {
        name: "ㅈㄱ",
        description: "제거 명령의 초성 별칭이에요.",
        canonical: "remove",
    },
    CommandDef {
        name: "clear",
        description: "지금 곡은 두고 대기열만 모두 비워요.",
        canonical: "clear",
    },
    CommandDef {
        name: "큐비우기",
        description: "지금 곡은 두고 대기열만 모두 비워요.",
        canonical: "clear",
    },
    CommandDef {
        name: "비우기",
        description: "큐비우기 명령의 쉬운 한국어 별칭이에요.",
        canonical: "clear",
    },
    CommandDef {
        name: "ㅋㅂㅇㄱ",
        description: "큐비우기 명령의 초성 별칭이에요.",
        canonical: "clear",
    },
    CommandDef {
        name: "previous",
        description: "방금 재생한 곡으로 되돌아가요.",
        canonical: "previous",
    },
    CommandDef {
        name: "이전곡",
        description: "방금 재생한 곡으로 되돌아가요.",
        canonical: "previous",
    },
    CommandDef {
        name: "이전",
        description: "이전곡 명령의 쉬운 한국어 별칭이에요.",
        canonical: "previous",
    },
    CommandDef {
        name: "ㅇㅈㄱ",
        description: "이전곡 명령의 초성 별칭이에요.",
        canonical: "previous",
    },
    CommandDef {
        name: "replay",
        description: "지금 곡을 처음부터 다시 재생해요.",
        canonical: "replay",
    },
    CommandDef {
        name: "다시재생",
        description: "지금 곡을 처음부터 다시 재생해요.",
        canonical: "replay",
    },
    CommandDef {
        name: "다시",
        description: "다시재생 명령의 쉬운 한국어 별칭이에요.",
        canonical: "replay",
    },
    CommandDef {
        name: "ㄷㅅㅈㅅ",
        description: "다시재생 명령의 초성 별칭이에요.",
        canonical: "replay",
    },
    CommandDef {
        name: "seek",
        description: "지금 곡의 원하는 시간으로 이동해요 (예: 1:23).",
        canonical: "seek",
    },
    CommandDef {
        name: "이동시간",
        description: "지금 곡의 원하는 시간으로 이동해요 (예: 1:23).",
        canonical: "seek",
    },
    CommandDef {
        name: "ㅇㄷㅅㄱ",
        description: "이동시간 명령의 초성 별칭이에요.",
        canonical: "seek",
    },
    CommandDef {
        name: "skipto",
        description: "대기열의 정한 순번으로 바로 건너뛰어요.",
        canonical: "skipto",
    },
    CommandDef {
        name: "지정스킵",
        description: "대기열의 정한 순번으로 바로 건너뛰어요.",
        canonical: "skipto",
    },
    CommandDef {
        name: "ㅈㅈㅅㅋ",
        description: "지정스킵 명령의 초성 별칭이에요.",
        canonical: "skipto",
    },
    CommandDef {
        name: "volume",
        description: "서버 재생 볼륨을 바꿔요 (모두에게 적용돼요).",
        canonical: "volume",
    },
    CommandDef {
        name: "볼륨",
        description: "서버 재생 볼륨을 바꿔요 (모두에게 적용돼요).",
        canonical: "volume",
    },
    CommandDef {
        name: "ㅂㄹ",
        description: "볼륨 명령의 초성 별칭이에요.",
        canonical: "volume",
    },
    CommandDef {
        name: "normalize",
        description: "볼륨 평준화를 쓸지 정해요.",
        canonical: "normalize",
    },
    CommandDef {
        name: "평준화",
        description: "볼륨 평준화를 쓸지 정해요.",
        canonical: "normalize",
    },
    CommandDef {
        name: "ㅍㅈㅎ",
        description: "평준화 명령의 초성 별칭이에요.",
        canonical: "normalize",
    },
    CommandDef {
        name: "playlist",
        description: "저장해 둔 재생목록을 관리해요.",
        canonical: "playlist",
    },
    CommandDef {
        name: "플레이리스트",
        description: "저장해 둔 재생목록을 관리해요.",
        canonical: "playlist",
    },
    CommandDef {
        name: "플리",
        description: "플레이리스트 명령의 축약 별칭이에요.",
        canonical: "playlist",
    },
    CommandDef {
        name: "ㅍㄹ",
        description: "플레이리스트 명령의 초성 별칭이에요.",
        canonical: "playlist",
    },
    CommandDef {
        name: "search",
        description: "유튜브에서 찾아 후보 중에 골라 재생해요.",
        canonical: "search",
    },
    CommandDef {
        name: "검색",
        description: "유튜브에서 찾아 후보 중에 골라 재생해요.",
        canonical: "search",
    },
    CommandDef {
        name: "scsearch",
        description: "사운드클라우드에서 찾아 후보 중에 골라 재생해요.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "사클검색",
        description: "사운드클라우드에서 찾아 후보 중에 골라 재생해요.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "ㅅㅋㄱㅅ",
        description: "사클검색 명령의 초성 별칭이에요.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "leave",
        description: "봇을 음성 채널에서 내보내요.",
        canonical: "leave",
    },
    CommandDef {
        name: "나가기",
        description: "봇을 음성 채널에서 내보내요.",
        canonical: "leave",
    },
    CommandDef {
        name: "ㄴㄱㄱ",
        description: "나가기 명령의 초성 별칭이에요.",
        canonical: "leave",
    },
    CommandDef {
        name: "remote",
        description: "웹 리모컨 주소를 나만 보이게 알려줘요.",
        canonical: "remote",
    },
    CommandDef {
        name: "리모컨",
        description: "웹 리모컨 주소를 나만 보이게 알려줘요.",
        canonical: "remote",
    },
    CommandDef {
        name: "ㄹㅁㅋ",
        description: "리모컨 명령의 초성 별칭이에요.",
        canonical: "remote",
    },
    CommandDef {
        name: "status",
        description: "봇 버전과 지금 재생·전역 설정을 보여줘요.",
        canonical: "status",
    },
    CommandDef {
        name: "상태",
        description: "봇 버전과 지금 재생·전역 설정을 보여줘요.",
        canonical: "status",
    },
    CommandDef {
        name: "ㅅㅌ",
        description: "상태 명령의 초성 별칭이에요.",
        canonical: "status",
    },
];

pub fn canonical_of(name: &str) -> &'static str {
    ALL.iter()
        .find(|c| c.name == name)
        .map(|c| c.canonical)
        .unwrap_or("unknown")
}

/// 초성 전용 별칭인가 (이름이 한글 호환 자모로만 이루어짐, 예: "ㅈㅅ", "ㅂㄹㅈㅅ").
/// 실제 명령 이름은 음절 블록(재생/볼륨…)이나 ASCII 라서 자모만으로 된 이름이 아니다.
/// 사용자 요청으로 초성 명령을 비활성화하기 위해 등록 단계에서 이 이름들을 거른다(2026-06-20).
pub fn is_chosung_alias(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| ('\u{3130}'..='\u{318F}').contains(&c))
}
