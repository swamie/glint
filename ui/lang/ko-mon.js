"use strict";
// Korean: a monitor per clip keybind (Keybinds, Capture, status bar) and the 0.1.9 changelog.
glintAddLanguage("ko", "한국어", {
  "Which monitor this keybind clips": "이 단축키로 클립할 모니터",
  "Capture monitor ({name})": "캡처 모니터 ({name})",
  "Its monitor is not connected, so this keybind clips the capture monitor until it is back.": "지정한 모니터가 연결되어 있지 않아서, 다시 연결될 때까지 캡처 모니터를 클립해요.",
  "Buffered at once: {list}. Each monitor is its own capture, so each one uses the graphics card's encoder, memory and disk space again.": "동시에 버퍼링하는 모니터: {list}. 모니터마다 따로 캡처하기 때문에 그래픽 카드 인코더, 메모리, 디스크 공간을 모니터 수만큼 더 써요.",
  "Also buffered for a keybind: {list}": "단축키 때문에 함께 버퍼링하는 모니터: {list}",
  "Buffered monitors:": "버퍼링 중인 모니터:",
  "(capture monitor)": "(캡처 모니터)",
  "+1 monitor": "+모니터 1개",
  "+{n} monitors": "+모니터 {n}개",
  "Two keys for two monitors: in Keybinds, each clip keybind can pick its own monitor. Glint keeps a replay buffer for every monitor a keybind uses.": "모니터 두 개에 단축키 두 개: 단축키 화면에서 클립 단축키마다 모니터를 따로 고를 수 있어요. Glint는 단축키가 쓰는 모니터마다 리플레이 버퍼를 유지해요.",
});
