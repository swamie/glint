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
    ("No clip to edit yet", "아직 편집할 클립이 없어요"),
    ("Save a clip first", "먼저 클립을 저장하세요"),
    ("Clipping is paused", "클리핑 일시 정지됨"),
    ("Resume clipping to save clips", "클립을 저장하려면 클리핑을 재개해 주세요"),
    ("Clipping resumed", "클리핑 재개됨"),
    ("Clipping paused", "클리핑 일시 정지됨"),
    // plugins.rs: a plugin's process ended by itself; a keybind for a process that was never set up
    ("Stopped", "중지됨"),
    ("Open Glint and set it up first", "Glint를 열어 먼저 설정해 주세요"),
    ("Replay buffer is filling", "리플레이 버퍼 채우는 중"),
    ("Replay buffer cleared", "리플레이 버퍼 비움"),
    ("Captured last {len} of {game}", "{game} · 최근 {len}"),
    ("Captured last {len}", "최근 {len}"),
    ("Captured {game}", "{game}"),
    ("the desktop", "데스크톱"),
    // Game mode (0.1.27)
    ("No game is running", "실행 중인 게임이 없어요"),
    ("Glint records the game as soon as one starts", "게임을 시작하면 Glint가 바로 녹화해요"),
    // Tray menu
    ("Capture monitor", "캡처 모니터"),
    ("Open Glint", "Glint 열기"),
    ("Save clip now", "지금 클립 저장"),
    ("Pause / resume clipping", "클리핑 일시 정지/재개"),
    ("Quit Glint", "Glint 종료"),
];

/// Greek.
const EL: &[(&str, &str)] = &[
    // Clip overlay
    ("Clip saved", "Το κλιπ αποθηκεύτηκε"),
    ("Clip failed", "Το κλιπ απέτυχε"),
    ("Nothing to clip yet", "Δεν υπάρχει ακόμα κάτι για κλιπ"),
    ("No clip to edit yet", "Δεν υπάρχει ακόμα κλιπ για επεξεργασία"),
    ("Save a clip first", "Αποθηκεύστε πρώτα ένα κλιπ"),
    ("Clipping is paused", "Η καταγραφή κλιπ είναι σε παύση"),
    ("Resume clipping to save clips", "Συνεχίστε την καταγραφή για να αποθηκεύσετε κλιπ"),
    ("Clipping resumed", "Η καταγραφή κλιπ συνεχίζεται"),
    ("Clipping paused", "Η καταγραφή κλιπ σε παύση"),
    ("Stopped", "Διακόπηκε"),
    ("Open Glint and set it up first", "Άνοιξε το Glint και ρύθμισέ το πρώτα"),
    ("Replay buffer is filling", "Το buffer γεμίζει"),
    ("Replay buffer cleared", "Το buffer άδειασε"),
    ("Captured last {len} of {game}", "{game} · Τελευταία {len}"),
    ("Captured last {len}", "Τελευταία {len}"),
    ("Captured {game}", "{game}"),
    ("the desktop", "Επιφάνεια εργασίας"),
    // Game mode (0.1.27)
    ("No game is running", "Δεν τρέχει κανένα παιχνίδι"),
    ("Glint records the game as soon as one starts", "Το Glint καταγράφει το παιχνίδι μόλις ξεκινήσει"),
    // Tray menu
    ("Capture monitor", "Οθόνη καταγραφής"),
    ("Open Glint", "Άνοιγμα Glint"),
    ("Save clip now", "Αποθήκευση κλιπ τώρα"),
    ("Pause / resume clipping", "Παύση / συνέχιση καταγραφής"),
    ("Quit Glint", "Έξοδος από το Glint"),
];

/// `text` in language `lang` ("en", "ko", "el"), or `text` itself if there is no translation.
pub fn tr<'a>(lang: &str, text: &'a str) -> &'a str {
    let table: &'static [(&'static str, &'static str)] = match lang {
        "ko" => KO,
        "el" => EL,
        _ => return text,
    };
    table.iter().find(|(en, to)| *en == text && !to.is_empty()).map(|(_, to)| *to).unwrap_or(text)
}
