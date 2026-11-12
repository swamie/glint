//! The few texts the backend itself shows: the clip overlay and the tray menu.
//! Everything else is translated in the UI (`ui/i18n.js`, `ui/lang/*.js`).
//!
//! The English text is the key, as in the UI. A text with no translation is
//! shown in English.

/// Korean. Each pair is (English, Korean). `{len}` and `{game}` are filled in after translating.
const KO: &[(&str, &str)] = &[
    // Clip overlay
    ("Clip saved", "클립 저장됨"),
    ("Clip failed", "클립 저장 실패"),
    ("Nothing to clip yet", "아직 저장할 영상이 없어요"),
    ("Clipping is paused", "클리핑 일시 정지됨"),
    ("Resume clipping to save clips", "클립을 저장하려면 클리핑을 재개해 주세요"),
    ("Clipping resumed", "클리핑 재개됨"),
    ("Clipping paused", "클리핑 일시 정지됨"),
    ("Replay buffer is filling", "리플레이 버퍼 채우는 중"),
    ("Replay buffer cleared", "리플레이 버퍼 비움"),
    ("Captured last {len} of {game}", "{game} · 최근 {len}"),
    ("Captured last {len}", "최근 {len}"),
    ("Captured {game}", "{game}"),
    ("the desktop", "데스크톱"),
    // Tray menu
    ("Open Glint", "Glint 열기"),
    ("Save clip now", "지금 클립 저장"),
    ("Pause / resume clipping", "클리핑 일시 정지/재개"),
    ("Quit Glint", "Glint 종료"),
];

/// `text` in language `lang` ("en", "ko"), or `text` itself if there is no translation.
pub fn tr<'a>(lang: &str, text: &'a str) -> &'a str {
    let table: &'static [(&'static str, &'static str)] = match lang {
        "ko" => KO,
        _ => return text,
    };
    table.iter().find(|(en, to)| *en == text && !to.is_empty()).map(|(_, to)| *to).unwrap_or(text)
}
