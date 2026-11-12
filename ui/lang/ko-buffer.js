"use strict";
// Korean, part 4: where the replay buffer is kept (Capture page, status bar, Keybinds page).
glintAddLanguage("ko", "한국어", {
  "Replay buffer": "리플레이 버퍼",
  "Keep the buffer in": "버퍼 저장 위치",
  "RAM + disk": "RAM + 디스크",
  "RAM only": "RAM만",
  "Disk only": "디스크만",
  "Fastest, and nothing is written to disk. Uses up to {ram} of RAM": "가장 빠르고 디스크에 아무것도 쓰지 않아요. RAM을 최대 {ram}까지 사용해요",
  "Lowest RAM use: the whole buffer is kept in files. Up to {disk} on disk": "RAM을 가장 적게 써요. 버퍼 전체를 파일로 보관해요. 디스크를 최대 {disk}까지 사용해요",
  "Your longest clip is short enough to stay in RAM, so nothing is written to disk. Up to {ram} of RAM": "가장 긴 클립이 RAM에 들어갈 만큼 짧아서 디스크에 쓰지 않아요. RAM을 최대 {ram}까지 사용해요",
  "The newest {secs} stay in RAM and the rest is kept in files. Up to {ram} of RAM and {disk} on disk": "최근 {secs}는 RAM에, 나머지는 파일로 보관해요. RAM 최대 {ram}, 디스크 최대 {disk}",
  "Kept in RAM": "RAM에 보관할 길이",
  "Clips up to this length are saved straight from RAM": "이 길이 이하의 클립은 RAM에서 바로 저장돼요",
  "Buffer folder": "버퍼 폴더",
  "Temporary files, deleted as the buffer moves on. An SSD is best": "임시 파일이며, 버퍼가 지나가면 지워져요. SSD를 권장해요",
  "Default": "기본값",
  "Buffer <b>{now}</b> / {max} · {size} RAM + {disk} disk": "버퍼 <b>{now}</b> / {max} · RAM {size} + 디스크 {disk}",
  "Fixed. Moving pictures use all of it; the {length} buffer can reach {size}": "고정. 움직이는 화면은 전부 사용해요. {length} 버퍼는 최대 {size}까지 커질 수 있어요",
  "Buffer length follows the longest clip: {length} · at most {size}, usually far less": "버퍼 길이는 가장 긴 클립을 따라가요: {length} · 최대 {size}, 보통은 훨씬 적어요",
});
