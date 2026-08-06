//! 슬래시 명령 카탈로그 — C# CommandCatalog 1:1 (대표 명령 + 한국어/축약/초성 별칭).

pub struct CommandDef {
    pub name: &'static str,
    pub description: &'static str,
    pub canonical: &'static str,
}

pub const ALL: &[CommandDef] = &[
    CommandDef {
        name: "play",
        description: "곡 또는 플레이리스트 URL을 현재 대기열에 추가합니다.",
        canonical: "play",
    },
    CommandDef {
        name: "재생",
        description: "곡 또는 플레이리스트 URL을 현재 대기열에 추가합니다.",
        canonical: "play",
    },
    CommandDef {
        name: "p",
        description: "재생 명령의 영문 축약입니다.",
        canonical: "play",
    },
    CommandDef {
        name: "ㅈㅅ",
        description: "재생 명령의 초성 별칭입니다.",
        canonical: "play",
    },
    CommandDef {
        name: "playnow",
        description: "현재 곡과 대기열을 비우고(반복 모드면 뒤로 보내고) 즉시 재생합니다.",
        canonical: "playnow",
    },
    CommandDef {
        name: "바로재생",
        description: "현재 곡과 대기열을 비우고(반복 모드면 뒤로 보내고) 즉시 재생합니다.",
        canonical: "playnow",
    },
    CommandDef {
        name: "ㅂㄹㅈㅅ",
        description: "바로재생 명령의 초성 별칭입니다.",
        canonical: "playnow",
    },
    CommandDef {
        name: "queue",
        description: "현재 대기열과 재생 상태를 보여 줍니다.",
        canonical: "queue",
    },
    CommandDef {
        name: "대기열",
        description: "현재 대기열과 재생 상태를 보여 줍니다.",
        canonical: "queue",
    },
    CommandDef {
        name: "q",
        description: "대기열 명령의 영문 축약입니다.",
        canonical: "queue",
    },
    CommandDef {
        name: "ㄷㄱㅇ",
        description: "대기열 명령의 초성 별칭입니다.",
        canonical: "queue",
    },
    CommandDef {
        name: "nowplaying",
        description: "현재 재생 중인 곡 정보를 보여 줍니다.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "현재곡",
        description: "현재 재생 중인 곡 정보를 보여 줍니다.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "ㅈㅈㄱ",
        description: "현재곡 명령의 초성 별칭입니다.",
        canonical: "nowplaying",
    },
    CommandDef {
        name: "shuffle",
        description: "현재 곡 뒤에 남아 있는 대기열만 무작위로 섞습니다.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "셔플",
        description: "현재 곡 뒤에 남아 있는 대기열만 무작위로 섞습니다.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "ㅅㅍ",
        description: "셔플 명령의 초성 별칭입니다.",
        canonical: "shuffle",
    },
    CommandDef {
        name: "repeat",
        description: "반복 재생 모드를 변경합니다.",
        canonical: "repeat",
    },
    CommandDef {
        name: "반복",
        description: "반복 재생 모드를 변경합니다.",
        canonical: "repeat",
    },
    CommandDef {
        name: "ㅂㅂ",
        description: "반복 명령의 초성 별칭입니다.",
        canonical: "repeat",
    },
    CommandDef {
        name: "autoplay",
        description: "자동 추천으로 다음 곡을 이어 붙일지 설정합니다.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "자동추천",
        description: "자동 추천으로 다음 곡을 이어 붙일지 설정합니다.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "ㅈㄷㅊㅊ",
        description: "자동추천 명령의 초성 별칭입니다.",
        canonical: "autoplay",
    },
    CommandDef {
        name: "pause",
        description: "현재 재생 중인 곡을 일시정지합니다.",
        canonical: "pause",
    },
    CommandDef {
        name: "일시정지",
        description: "현재 재생 중인 곡을 일시정지합니다.",
        canonical: "pause",
    },
    CommandDef {
        name: "ㅇㅅㅈㅈ",
        description: "일시정지 명령의 초성 별칭입니다.",
        canonical: "pause",
    },
    CommandDef {
        name: "resume",
        description: "일시정지된 재생을 다시 시작합니다.",
        canonical: "resume",
    },
    CommandDef {
        name: "재개",
        description: "일시정지된 재생을 다시 시작합니다.",
        canonical: "resume",
    },
    CommandDef {
        name: "계속",
        description: "재개 명령의 쉬운 한국어 별칭입니다.",
        canonical: "resume",
    },
    CommandDef {
        name: "ㄱㅅ",
        description: "재개 명령의 초성 별칭입니다.",
        canonical: "resume",
    },
    CommandDef {
        name: "skip",
        description: "현재 곡을 건너뛰고 다음 곡으로 이동합니다.",
        canonical: "skip",
    },
    CommandDef {
        name: "스킵",
        description: "현재 곡을 건너뛰고 다음 곡으로 이동합니다.",
        canonical: "skip",
    },
    CommandDef {
        name: "넘김",
        description: "스킵 명령의 쉬운 한국어 별칭입니다.",
        canonical: "skip",
    },
    CommandDef {
        name: "ㄴㄱ",
        description: "스킵 명령의 초성 별칭입니다.",
        canonical: "skip",
    },
    CommandDef {
        name: "stop",
        description: "재생을 멈추고 플레이어를 정지 상태로 되돌립니다.",
        canonical: "stop",
    },
    CommandDef {
        name: "정지",
        description: "재생을 멈추고 플레이어를 정지 상태로 되돌립니다.",
        canonical: "stop",
    },
    CommandDef {
        name: "ㅈㅈ",
        description: "정지 명령의 초성 별칭입니다.",
        canonical: "stop",
    },
    CommandDef {
        name: "move",
        description: "대기열에 있는 특정 곡의 순서를 바꿉니다.",
        canonical: "move",
    },
    CommandDef {
        name: "이동",
        description: "대기열에 있는 특정 곡의 순서를 바꿉니다.",
        canonical: "move",
    },
    CommandDef {
        name: "ㅇㄷ",
        description: "이동 명령의 초성 별칭입니다.",
        canonical: "move",
    },
    CommandDef {
        name: "remove",
        description: "대기열에 있는 특정 곡을 제거합니다.",
        canonical: "remove",
    },
    CommandDef {
        name: "제거",
        description: "대기열에 있는 특정 곡을 제거합니다.",
        canonical: "remove",
    },
    CommandDef {
        name: "ㅈㄱ",
        description: "제거 명령의 초성 별칭입니다.",
        canonical: "remove",
    },
    CommandDef {
        name: "clear",
        description: "현재 곡은 두고 대기열만 모두 비웁니다.",
        canonical: "clear",
    },
    CommandDef {
        name: "큐비우기",
        description: "현재 곡은 두고 대기열만 모두 비웁니다.",
        canonical: "clear",
    },
    CommandDef {
        name: "비우기",
        description: "큐비우기 명령의 쉬운 한국어 별칭입니다.",
        canonical: "clear",
    },
    CommandDef {
        name: "ㅋㅂㅇㄱ",
        description: "큐비우기 명령의 초성 별칭입니다.",
        canonical: "clear",
    },
    CommandDef {
        name: "previous",
        description: "직전에 재생한 곡으로 되돌아갑니다.",
        canonical: "previous",
    },
    CommandDef {
        name: "이전곡",
        description: "직전에 재생한 곡으로 되돌아갑니다.",
        canonical: "previous",
    },
    CommandDef {
        name: "이전",
        description: "이전곡 명령의 쉬운 한국어 별칭입니다.",
        canonical: "previous",
    },
    CommandDef {
        name: "ㅇㅈㄱ",
        description: "이전곡 명령의 초성 별칭입니다.",
        canonical: "previous",
    },
    CommandDef {
        name: "replay",
        description: "현재 곡을 처음부터 다시 재생합니다.",
        canonical: "replay",
    },
    CommandDef {
        name: "다시재생",
        description: "현재 곡을 처음부터 다시 재생합니다.",
        canonical: "replay",
    },
    CommandDef {
        name: "다시",
        description: "다시재생 명령의 쉬운 한국어 별칭입니다.",
        canonical: "replay",
    },
    CommandDef {
        name: "ㄷㅅㅈㅅ",
        description: "다시재생 명령의 초성 별칭입니다.",
        canonical: "replay",
    },
    CommandDef {
        name: "seek",
        description: "현재 곡의 특정 시간으로 이동합니다 (예: 1:23).",
        canonical: "seek",
    },
    CommandDef {
        name: "이동시간",
        description: "현재 곡의 특정 시간으로 이동합니다 (예: 1:23).",
        canonical: "seek",
    },
    CommandDef {
        name: "ㅇㄷㅅㄱ",
        description: "이동시간 명령의 초성 별칭입니다.",
        canonical: "seek",
    },
    CommandDef {
        name: "skipto",
        description: "대기열의 지정한 순번으로 바로 건너뜁니다.",
        canonical: "skipto",
    },
    CommandDef {
        name: "지정스킵",
        description: "대기열의 지정한 순번으로 바로 건너뜁니다.",
        canonical: "skipto",
    },
    CommandDef {
        name: "ㅈㅈㅅㅋ",
        description: "지정스킵 명령의 초성 별칭입니다.",
        canonical: "skipto",
    },
    CommandDef {
        name: "volume",
        description: "전역 재생 볼륨을 조정합니다.",
        canonical: "volume",
    },
    CommandDef {
        name: "볼륨",
        description: "전역 재생 볼륨을 조정합니다.",
        canonical: "volume",
    },
    CommandDef {
        name: "ㅂㄹ",
        description: "볼륨 명령의 초성 별칭입니다.",
        canonical: "volume",
    },
    CommandDef {
        name: "normalize",
        description: "볼륨 평준화 사용 여부를 설정합니다.",
        canonical: "normalize",
    },
    CommandDef {
        name: "평준화",
        description: "볼륨 평준화 사용 여부를 설정합니다.",
        canonical: "normalize",
    },
    CommandDef {
        name: "ㅍㅈㅎ",
        description: "평준화 명령의 초성 별칭입니다.",
        canonical: "normalize",
    },
    CommandDef {
        name: "playlist",
        description: "저장된 플레이리스트를 관리합니다.",
        canonical: "playlist",
    },
    CommandDef {
        name: "플레이리스트",
        description: "저장된 플레이리스트를 관리합니다.",
        canonical: "playlist",
    },
    CommandDef {
        name: "플리",
        description: "플레이리스트 명령의 축약 별칭입니다.",
        canonical: "playlist",
    },
    CommandDef {
        name: "ㅍㄹ",
        description: "플레이리스트 명령의 초성 별칭입니다.",
        canonical: "playlist",
    },
    CommandDef {
        name: "search",
        description: "유튜브에서 검색해 후보 중 골라 재생합니다.",
        canonical: "search",
    },
    CommandDef {
        name: "검색",
        description: "유튜브에서 검색해 후보 중 골라 재생합니다.",
        canonical: "search",
    },
    CommandDef {
        name: "scsearch",
        description: "사운드클라우드에서 검색해 후보 중 골라 재생합니다.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "사클검색",
        description: "사운드클라우드에서 검색해 후보 중 골라 재생합니다.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "ㅅㅋㄱㅅ",
        description: "사클검색 명령의 초성 별칭입니다.",
        canonical: "scsearch",
    },
    CommandDef {
        name: "leave",
        description: "봇을 음성 채널에서 내보냅니다.",
        canonical: "leave",
    },
    CommandDef {
        name: "나가기",
        description: "봇을 음성 채널에서 내보냅니다.",
        canonical: "leave",
    },
    CommandDef {
        name: "ㄴㄱㄱ",
        description: "나가기 명령의 초성 별칭입니다.",
        canonical: "leave",
    },
    CommandDef {
        name: "remote",
        description: "웹 리모컨 주소를 나만 보이게 알려 줍니다.",
        canonical: "remote",
    },
    CommandDef {
        name: "리모컨",
        description: "웹 리모컨 주소를 나만 보이게 알려 줍니다.",
        canonical: "remote",
    },
    CommandDef {
        name: "ㄹㅁㅋ",
        description: "리모컨 명령의 초성 별칭입니다.",
        canonical: "remote",
    },
    CommandDef {
        name: "status",
        description: "봇 버전과 현재 재생/전역 설정을 보여 줍니다.",
        canonical: "status",
    },
    CommandDef {
        name: "상태",
        description: "봇 버전과 현재 재생/전역 설정을 보여 줍니다.",
        canonical: "status",
    },
    CommandDef {
        name: "ㅅㅌ",
        description: "상태 명령의 초성 별칭입니다.",
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
