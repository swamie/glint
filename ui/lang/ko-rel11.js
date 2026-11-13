"use strict";
// Korean: the 0.1.11 and 0.1.12 changelogs.
glintAddLanguage("ko", "한국어", {
  "Settings → Components checks for ffmpeg and Blur and installs anything missing. If ffmpeg is missing, the status bar has an Install ffmpeg button.": "설정 → 구성 요소에서 ffmpeg와 Blur가 있는지 확인하고 없는 것을 설치해요. ffmpeg가 없으면 상태 표시줄에 ffmpeg 설치 버튼이 나와요.",
  "Edit together: select several clips, right-click, and they are joined, oldest first, into one new clip that opens in the editor with a split where each clip starts. Your original clips stay as they are.": "함께 편집: 클립을 여러 개 선택하고 우클릭하면 오래된 순서대로 하나의 새 클립으로 이어 붙이고, 각 클립이 시작하는 곳에 분할을 넣어 편집기에서 열어요. 원본 클립은 그대로 남아요.",
  "Separate game audio track (Capture, off until you switch it on): a track with only the game's sound. In the editor, mute Desktop and keep Game to drop voice chat and music.": "게임 오디오 트랙 분리(캡처 화면, 켜기 전까지는 꺼져 있어요): 게임 소리만 담긴 트랙이에요. 편집기에서 데스크톱을 음소거하고 게임만 남기면 음성 채팅과 음악을 뺄 수 있어요.",
  "The taskbar icon can take your theme's colour: Settings, next to the themes.": "작업 표시줄 아이콘을 테마 색으로 바꿀 수 있어요. 설정의 테마 옆에 있어요.",
  "On Windows 10, Auto capture no longer shows the yellow border around the screen.": "Windows 10에서 자동 캡처를 쓸 때 화면 둘레에 노란 테두리가 더 이상 나오지 않아요.",
  "Clip cards: more room around the text, and the folder is shown only when it differs from the game.": "클립 카드: 글자 둘레 여백이 넓어졌고, 폴더는 게임 이름과 다를 때만 보여요.",
  "A trimmed clip rendered with Blur no longer has its sound out of step with the picture.": "잘라 낸 클립을 Blur로 렌더링해도 소리와 화면이 어긋나지 않아요.",
  "Clip thumbnails always show the whole frame, also with a custom theme and for stretched, ultrawide and portrait clips.": "클립 썸네일이 항상 화면 전체를 보여 줘요. 사용자 테마를 쓸 때도, 늘린 화면·울트라와이드·세로 클립도 마찬가지예요.",
  "A trimmed clip that is exported again, compressed for Discord or joined no longer has its picture ahead of the sound. The trim now starts on the frame that was on screen, so it can begin a moment earlier than you asked.": "잘라 낸 클립을 다시 내보내거나, Discord용으로 압축하거나, 이어 붙여도 화면이 소리보다 앞서지 않아요. 이제 자르기는 그때 화면에 있던 프레임에서 시작해서, 고른 시점보다 아주 조금 일찍 시작할 수 있어요.",
  "What's new: the text is set closer together.": "새 소식: 글자 간격을 좁혔어요.",
  "Edit together: the clips now open in the editor as separate clips, in the order you selected them. Drag a clip in the Clips strip to move it, or right-click it to move it or cut it out. Nothing is added to your library until you export.": "한 번에 편집: 이제 클립이 선택한 순서대로, 각각 따로 편집기에 열려요. 클립 줄에서 클립을 끌어 옮기거나, 우클릭해서 옮기거나 잘라 낼 수 있어요. 내보내기 전에는 라이브러리에 아무것도 추가되지 않아요.",
  "Edit together keeps Desktop, Microphone and Game as separate tracks, even when the clips were recorded with different tracks.": "한 번에 편집에서 클립마다 녹음된 트랙이 달라도 데스크톱, 마이크, 게임 트랙을 따로 유지해요.",
  "A trim starts exactly where you set it: sound from before the start no longer comes back.": "자르기가 정확히 설정한 곳에서 시작해요. 시작점 앞의 소리가 다시 들어오지 않아요.",
  "Settings → Components: ffmpeg can be installed again in one session, a failed install never leaves Glint without ffmpeg, and the replay buffer restarts only when it has to.": "설정 → 구성 요소: 앱을 켠 채로 ffmpeg를 다시 설치할 수 있고, 설치가 실패해도 ffmpeg가 없어지지 않으며, 꼭 필요할 때만 리플레이 버퍼를 다시 시작해요.",
  "With only the microphone and the game sound recorded, the main audio track now has both, so Discord and other players hear the game.": "마이크와 게임 소리만 녹음한 경우에도 기본 오디오 트랙에 둘 다 들어가서, Discord나 다른 플레이어에서 게임 소리가 들려요.",
  "Settings → Components no longer stays on Installing after the window was opened again, and finds ffprobe beside an ffmpeg you chose yourself.": "설정 → 구성 요소가 창을 다시 연 뒤 설치 중에 멈춰 있지 않고, 직접 고른 ffmpeg 옆의 ffprobe를 찾아요.",
  "A clip that is waiting to be edited together can no longer be renamed or moved in the meantime.": "한 번에 편집을 기다리는 클립은 그동안 이름을 바꾸거나 옮길 수 없어요.",
  "If Blur's timing fix fails, the clip still renders.": "Blur의 타이밍 보정이 실패해도 클립은 렌더링돼요.",
  "After an update Glint starts itself again directly, without a hidden PowerShell step in between. That step could make antivirus programs suspicious.": "업데이트 후 Glint가 숨겨진 PowerShell 단계를 거치지 않고 직접 다시 시작해요. 그 단계 때문에 백신 프로그램이 Glint를 의심할 수 있었어요.",
});
