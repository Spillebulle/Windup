# Changelog

## Unreleased

- Recordings with sound play in every player. The sound track was stamped with the time of day, so the MP4 claimed to be decades long, and GStreamer players such as COSMIC's and GNOME's found nothing to play in it. Recordings made with 0.3.0 can be fixed with `ffmpeg -i old.mp4 -c:v copy -af asetpts=PTS-STARTPTS fixed.mp4`.
- After a selection is made, the Area, Window and Full screen dropdown stays on the bar, in the editor and before recording. Dragging outside the selection draws a new area, whatever tool is in hand; with Window or Full screen chosen, a click outside picks the window or screen under the pointer, which is highlighted as it moves. The edges and corners still resize the selection, and the edits stay where they are.

## 0.3.0

- Screen recording. Win+Shift+R (Super+Shift+R on Linux) opens the capture bar ready to record an area, a window or the whole screen, to MP4, WebM, MOV, MKV or a GIF, with the computer's sound and the microphone if you turn them on. A small bar with a timer, pause and stop stays out of the recording; the same keys stop it, Win+Shift+P pauses it and Esc throws it away. A notice says where the file went, with a button that opens the folder.
- Recordings are encoded on the graphics card where it can (NVENC, AMF, Quick Sync, VA-API), and Windup only offers the encoders it has seen work on your computer. FFmpeg comes with Windup, with its licence and a note saying where its source is.
- A Video settings pane: the shortcuts, the save folder and file name, the container, codec and encoder, the frame rate and bitrate (or the colours of a GIF), and the sound devices. A setting that cannot work here is greyed out and says why.
- On Linux, `windup --record`, `--record-pause`, `--record-stop` and `--record-cancel`, for desktops where the keys are bound in their own settings. KDE and GNOME ask once to share the screen; the Linux page of the docs says how each desktop records and where Esc cancels.
- The colour wheel's triangle stays pointing right instead of turning with the hue.

## 0.2.2

- On Linux, an update no longer leaves the old Windup answering the shortcuts until it is killed. Windup notices when a package update has replaced it and starts the new version within a few seconds, once no popup or capture is open. The first update to this version still needs the old one stopped once, since it predates this.

## 0.2.1

- Print Screen opens the capture bar and Ctrl+Shift+S takes a screenshot at once, for new settings. Settings from 0.2.0 keep their keys; Revert on each shortcut row sets the new ones.
- Print Screen, Scroll Lock and Pause can be chosen for any shortcut: the shortcut editor offers them as buttons, since the settings window cannot hear them pressed.
- Redo in the editor is Ctrl+Y by default; Ctrl+Shift+Z still redoes too.
- Blur has an eye-off icon in the capture bar.

## 0.2.0

- Screenshots. Win+Shift+S (Super+Shift+S on Linux) opens the capture bar over a frozen copy of the screen: drag an area, click a window or take the whole screen, then draw on it, blur or mosaic parts of it, and copy or save it. Print Screen takes a screenshot at once and opens it in the editor.
- Two new settings panes, Capture launcher and Screenshot: the shortcuts, the save folder, the file name and format, and the editor's keys.
- The shortcuts you set in the settings window are used now: on Windows the keyboard hook follows them, and on X11 they are grabbed again when you change them. A shortcut that two tools share is marked in the settings.
- On Linux, `windup --capture` and `windup --screenshot` open the capture bar and take a screenshot, for desktops where the keys are bound in their own settings. The Linux page of the docs says which route each desktop uses.

## 0.1.2

- On COSMIC, the clipboard history opens at the pointer instead of in the middle of the screen, even when the mouse is not moving.
- The clipboard history shows when each entry was copied as a clock time, such as 14:32, or 27 Oct 14:32 for an earlier day.
- The settings window shows Windup's version at the bottom left, with a link to the source on GitHub.

## 0.1.1

- On COSMIC, Windup binds Super+V and Super+Shift+V itself the first time it starts, in COSMIC's own custom shortcuts, where they can be changed or removed. A key you have already bound is left as it is.
- On Linux, the screen no longer turns black when the clipboard history opens. Only the card is drawn by the graphics card now; the rest of the screen stays as it was, on every driver.

## 0.1.0

- Clipboard history at your cursor with Win+V, images included, replacing the Windows clipboard panel.
- Paste the clipboard as plain text with Win+Shift+V.
- A settings window for both tools, with light and dark themes, an accent colour and custom themes.
- A Windows installer that shows Windup's own window, adds Windup to the Start menu and to Add or remove programs, and starts it when it is done.
- The Windup icon on the program file, the Start menu entry and the taskbar.
- Linux, on Wayland and X11: `.deb`, `.rpm`, Arch and AppImage packages, with Super+V bound in your desktop's keyboard settings where the desktop does not let Windup take it. Each package adds the Spillebulle package archive, so Windup updates with the rest of the system, `pacman -Syu` included.
