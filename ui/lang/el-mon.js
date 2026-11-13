"use strict";
// Greek: a monitor per clip keybind (Keybinds, Capture, status bar) and the 0.1.9 changelog.
glintAddLanguage("el", "Ελληνικά", {
  "Which monitor this keybind clips": "Από ποια οθόνη αποθηκεύει κλιπ αυτή η συντόμευση",
  "Capture monitor ({name})": "Οθόνη λήψης ({name})",
  "Its monitor is not connected, so this keybind clips the capture monitor until it is back.": "Η οθόνη της δεν είναι συνδεδεμένη, οπότε αυτή η συντόμευση αποθηκεύει κλιπ από την οθόνη λήψης μέχρι να συνδεθεί ξανά.",
  "Buffered at once: {list}. Each monitor is its own capture, so each one uses the graphics card's encoder, memory and disk space again.": "Σε buffer ταυτόχρονα: {list}. Κάθε οθόνη είναι ξεχωριστή καταγραφή, οπότε η καθεμία χρησιμοποιεί ξανά τον κωδικοποιητή της κάρτας γραφικών, μνήμη και χώρο στον δίσκο.",
  "Also buffered for a keybind: {list}": "Επίσης σε buffer για μια συντόμευση: {list}",
  "Buffered monitors:": "Οθόνες σε buffer:",
  "(capture monitor)": "(οθόνη λήψης)",
  "+1 monitor": "+1 οθόνη",
  "+{n} monitors": "+{n} οθόνες",
  "Two keys for two monitors: in Keybinds, each clip keybind can pick its own monitor. Glint keeps a replay buffer for every monitor a keybind uses.": "Δύο πλήκτρα για δύο οθόνες: στις Συντομεύσεις, κάθε συντόμευση κλιπ μπορεί να έχει τη δική της οθόνη. Το Glint κρατά buffer επανάληψης για κάθε οθόνη που χρησιμοποιεί μια συντόμευση.",
});
