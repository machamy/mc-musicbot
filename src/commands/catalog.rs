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
        name: "join",
        description: "부른 사람이 있는 음성 채널로 봇을 불러요.",
        canonical: "join",
    },
    CommandDef {
        name: "참여",
        description: "부른 사람이 있는 음성 채널로 봇을 불러요.",
        canonical: "join",
    },
    CommandDef {
        name: "부르기",
        description: "참여 명령의 쉬운 한국어 별칭이에요.",
        canonical: "join",
    },
    CommandDef {
        name: "입장",
        description: "참여 명령의 쉬운 한국어 별칭이에요.",
        canonical: "join",
    },
    CommandDef {
        name: "ㅊㅇ",
        description: "참여 명령의 초성 별칭이에요.",
        canonical: "join",
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

// ───────── 명령 그룹 (서버별 on/off) ─────────

/// 디스코드 명령을 서버별로 켜고 끄는 단위.
///
/// **명령 하나씩이 아니라 그룹으로 묶는 이유.** "리모컨이 있으니 디스코드로는 좀 적게
/// 하고 싶다" 는 요구는 `/제거` 하나만 끄는 식으로 풀리지 않는다. 하고 싶은 말은
/// "대기열은 웹에서만 만지게 해라" 쪽이라, 켜고 끄는 단위도 그 덩어리여야 한다.
/// 명령마다 스위치를 두면 스물일곱 개짜리 화면이 되고, 새 명령이 생길 때마다
/// 어느 서버는 켜지고 어느 서버는 꺼지는 상태가 생긴다.
pub struct CommandGroup {
    /// 설정 JSON 에 저장되는 키 (camelCase). **바꾸면 저장된 설정이 조용히 풀린다.**
    pub key: &'static str,
    /// 사람이 읽는 이름. 거절 문구와 관리 콘솔이 같은 말을 쓴다.
    pub label: &'static str,
    /// 무엇이 막히는지. 끄기 전에 읽을 문장이라 결과를 그대로 적는다.
    pub description: &'static str,
    /// 이 그룹에 속한 canonical 명령. **모든 canonical 은 정확히 한 그룹에 있어야 한다**
    /// (`every_canonical_belongs_to_exactly_one_group` 가 못 박는다).
    pub commands: &'static [&'static str],
}

/// 그룹 표 — 새 명령을 만들면 여기에 한 줄 넣는 것으로 끝난다.
/// 빠뜨리면 그 명령은 어떤 그룹에도 안 들어가 **영영 못 끄는 명령**이 되므로
/// 테스트가 컴파일이 아니라 실패로 알려 준다.
pub const GROUPS: &[CommandGroup] = &[
    CommandGroup {
        key: "voice",
        label: "음성 연결",
        description: "봇을 음성 채널로 부르고 내보내는 명령이에요.",
        commands: &["join", "leave"],
    },
    CommandGroup {
        key: "enqueue",
        label: "곡 담기",
        description: "곡을 찾아 대기열에 담는 명령이에요. 끄면 디스코드로는 곡을 못 넣어요.",
        commands: &["play", "playnow", "search", "scsearch"],
    },
    CommandGroup {
        key: "queueEdit",
        label: "대기열 편집",
        description: "대기열의 곡을 빼거나 순서를 바꾸는 명령이에요.",
        commands: &["remove", "clear", "shuffle", "move", "skipto"],
    },
    CommandGroup {
        key: "playback",
        label: "재생 조작",
        description: "지금 나오는 소리를 바꾸는 명령이에요 (재생·스킵·볼륨 등).",
        commands: &[
            "pause",
            "resume",
            "skip",
            "previous",
            "replay",
            "seek",
            "stop",
            "volume",
            "normalize",
            "repeat",
        ],
    },
    CommandGroup {
        key: "autoplay",
        label: "자동 재생",
        description: "자동 추천을 켜고 끄는 명령이에요.",
        commands: &["autoplay"],
    },
    CommandGroup {
        key: "library",
        label: "재생목록",
        description: "저장해 둔 재생목록을 관리하는 명령이에요.",
        commands: &["playlist"],
    },
    CommandGroup {
        key: "info",
        label: "조회",
        description: "보여 주기만 하는 명령이에요. \
                      **이것까지 끄면 디스코드에서는 봇이 아무 말도 안 해요** — \
                      리모컨 주소를 알려 주는 `/리모컨` 도 같이 막혀요.",
        commands: &["queue", "nowplaying", "status", "remote"],
    },
];

/// canonical 명령이 속한 그룹. 카탈로그에 없는 이름이면 `None` —
/// 그때는 **막지 않는다.** 모르는 명령을 그룹 판정이 조용히 거절하면
/// 새 명령을 붙인 사람이 원인을 못 찾는다 (없는 명령은 dispatch 가 따로 말해 준다).
pub fn group_of(canonical: &str) -> Option<&'static CommandGroup> {
    GROUPS
        .iter()
        .find(|group| group.commands.contains(&canonical))
}

/// 설정에 저장된 그룹 키 → 그룹. 관리 콘솔이 보낸 키가 진짜인지 볼 때 쓴다.
pub fn group_for_key(key: &str) -> Option<&'static CommandGroup> {
    GROUPS.iter().find(|group| group.key == key)
}

/// 이 명령의 대표 한국어 이름 (`play` → `재생`). 화면에 `/play` 대신 `/재생` 을 보여 주려고 쓴다.
/// 초성 별칭은 등록조차 안 되므로 후보에서 뺀다. 한국어 별칭이 없으면 canonical 그대로.
pub fn korean_alias(canonical: &str) -> &'static str {
    if let Some(def) = ALL.iter().find(|def| {
        def.canonical == canonical && def.name != canonical && !is_chosung_alias(def.name)
    }) {
        return def.name;
    }
    // 한국어 별칭이 없으면 카탈로그가 들고 있는 canonical 을 그대로 (인자는 'static 이 아니다).
    ALL.iter()
        .find(|def| def.canonical == canonical)
        .map(|def| def.canonical)
        .unwrap_or("unknown")
}

/// 초성 전용 별칭인가 (이름이 한글 호환 자모로만 이루어짐, 예: "ㅈㅅ", "ㅂㄹㅈㅅ").
/// 실제 명령 이름은 음절 블록(재생/볼륨…)이나 ASCII 라서 자모만으로 된 이름이 아니다.
/// 사용자 요청으로 초성 명령을 비활성화하기 위해 등록 단계에서 이 이름들을 거른다(2026-06-20).
pub fn is_chosung_alias(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| ('\u{3130}'..='\u{318F}').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **이 테스트가 이 파일에서 제일 중요하다.**
    ///
    /// 그룹에 안 들어간 canonical 이 하나라도 생기면 그 명령은 서버에서 **영영 못 끈다.**
    /// 게다가 조용히 그렇게 된다 — 화면에는 스위치가 다 있고, 끈 서버에서 그 명령만
    /// 계속 먹힌다. 명령을 추가한 사람이 `GROUPS` 를 잊는 것이 기본값이므로, 잊으면
    /// 여기서 이름을 대며 터지게 해 둔다. 반대로 그룹에만 있고 카탈로그엔 없는
    /// 유령 이름도 같이 잡는다(명령을 지웠는데 표에서 안 뺀 경우).
    #[test]
    fn every_canonical_belongs_to_exactly_one_group() {
        let canonicals: BTreeSet<&str> = ALL.iter().map(|def| def.canonical).collect();

        for name in &canonicals {
            let hits: Vec<&str> = GROUPS
                .iter()
                .filter(|group| group.commands.contains(name))
                .map(|group| group.key)
                .collect();
            assert_eq!(
                hits.len(),
                1,
                "'{name}' 이(가) 속한 그룹이 {}개예요 ({hits:?}). GROUPS 를 고쳐 주세요.",
                hits.len()
            );
        }

        for group in GROUPS {
            for name in group.commands {
                assert!(
                    canonicals.contains(name),
                    "그룹 '{}' 에 카탈로그에 없는 명령 '{name}' 이 있어요.",
                    group.key
                );
            }
        }
    }

    /// 그룹 키는 설정 JSON 에 그대로 저장된다 — 겹치면 한 스위치가 두 그룹을 끈다.
    #[test]
    fn group_keys_are_unique_and_resolvable() {
        let keys: BTreeSet<&str> = GROUPS.iter().map(|group| group.key).collect();
        assert_eq!(keys.len(), GROUPS.len(), "그룹 키가 겹쳐요");
        for group in GROUPS {
            assert_eq!(group_for_key(group.key).map(|g| g.key), Some(group.key));
        }
        assert!(group_for_key("없는그룹").is_none());
    }

    /// 새로 붙인 `/참여` 의 별칭이 전부 같은 canonical 로 접히는지.
    /// 별칭이 하나라도 새면 그 별칭만 그룹 차단을 빠져나간다.
    #[test]
    fn join_aliases_all_fold_into_the_same_canonical() {
        for name in ["join", "참여", "부르기", "입장", "ㅊㅇ"] {
            assert_eq!(canonical_of(name), "join", "'{name}' 별칭이 새요");
        }
        assert_eq!(group_of("join").map(|g| g.key), Some("voice"));
        assert_eq!(korean_alias("join"), "참여");
    }

    /// 등록 안 되는 초성 별칭이 대표 한국어 이름으로 뽑히면 안 된다 —
    /// 화면이 `/ㅈㅅ` 을 안내하는데 그 명령은 애초에 등록조차 안 돼 있다.
    #[test]
    fn korean_alias_never_returns_a_chosung_name() {
        for def in ALL {
            let alias = korean_alias(def.canonical);
            assert!(!is_chosung_alias(alias), "'{}' → '{alias}'", def.canonical);
        }
        assert_eq!(korean_alias("play"), "재생");
        assert_eq!(korean_alias("queue"), "대기열");
    }
}
