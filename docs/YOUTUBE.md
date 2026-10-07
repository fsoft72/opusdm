# OpusDM - YouTube video flow

A shooting script for a demo video of OpusDM. It follows a story (from "what is
this" to "I want it") instead of listing menus, and every scene says what to
show, what to do on screen and what to say. Features are the ones described in
`docs/usage.md` and `docs/FEATURES.md`.

Target length: **8 to 10 minutes** for the main video. Section 12 has a
**60 second cut** for Shorts, social media and the website hero.

---

## 0. Before you record

### Setup

- Record in a clean **X11 session** (or the nested one from `./run-x11.sh :2 1920x1080`) at **1920x1080**.
- Use a **dark theme** (Cyberpunk / Tokyo Night or Breeze Dark look best on video) and switch to **Amiga Workbench 1.3** for the closing scene.
- Large UI: icon size `L`, readable font. Hide private files and notifications.
- Turn on **picom** so snapping previews, rounded corners and transparency show up.
- Enable a **key overlay** (for example `screenkey`) so viewers see every shortcut.
- Hide the mouse jitter: move the pointer slowly and deliberately.
- Record the screen with something that captures system audio (OBS) if you show the player.

### Demo data (create once)

```bash
mkdir -p ~/opusdm-tour/{sorgente,destinazione,archivi,immagini}
cd ~/opusdm-tour/sorgente
for i in 01 02 03 04 05; do echo "File $i" > "document_$i.txt"; done
echo "TODO: record the video" > note.txt
echo "int main(void) { return 0; }" > main.c
dd if=/dev/urandom of=big.bin bs=1M count=200 2>/dev/null
mkdir -p project/src project/docs
echo "fn main() {}" > project/src/main.rs
cd ~/opusdm-tour && zip -qr archivi/example.zip sorgente
```

Also put **5 or 6 nice photos** in `immagini/`, one short video, and one MP3.
For the SFTP scene, have a test server ready (a VPS or a local `sshd`).

### Things to check on the day

- The `big.bin` copy is big enough to show a progress bar for about 3 seconds.
- Wi-Fi, volume and a USB stick are available for the panel scene.
- No real passwords or hostnames appear on screen.

---

## 1. The flow at a glance

| # | Scene | Time | Wow factor |
| --- | --- | --- | --- |
| 1 | Cold open | 0:00 - 0:20 | Fast montage |
| 2 | What is OpusDM | 0:20 - 0:50 | Whole desktop |
| 3 | SOURCE and DEST | 0:50 - 2:00 | The core idea |
| 4 | Keyboard and Command Palette | 2:00 - 2:50 | Speed |
| 5 | Batch Renamer | 2:50 - 3:40 | Live preview |
| 6 | Find, filter, select | 3:40 - 4:30 | Search in files |
| 7 | Archives, SFTP, encryption | 4:30 - 6:00 | "Everything is a folder" |
| 8 | Quick Look, terminal, viewer | 6:00 - 7:00 | Instant previews |
| 9 | Functions, hotkeys, banks | 7:00 - 8:00 | Automation |
| 10 | The desktop around it | 8:00 - 9:00 | Snap, panel, themes |
| 11 | Closing | 9:00 - 9:30 | Call to action |

---

## 2. Scene by scene

### Scene 1 - Cold open (0:00 - 0:20)

**Goal:** hook in the first 5 seconds. No logo, no intro music build-up.

**Show (fast cuts, 2 seconds each):**
1. Two Listers, press **F5**, progress bar sweeps, files appear on the right.
2. Batch Renamer preview table filling with new names.
3. A ZIP opening like a folder.
4. A window dragged to the corner, rounded snap preview, release.
5. The same desktop switching between three themes.

**Say:**
> "This is a file manager. It is also a desktop. And it is the fastest way I know to move files on Linux. This is OpusDM."

**Title card (1 second):** `OpusDM - Directory Opus power for Linux`.

---

### Scene 2 - What is OpusDM (0:20 - 0:50)

**Show:** the full desktop. Slowly point at: the **panel** (menu, pager, window list, tray, clock, CPU and network monitors), the **dock**, two **Lister** windows, desktop icons.

**Say:**
> "OpusDM is written in Rust with GTK4, inspired by the legendary Directory Opus from the Amiga. It gives you a file manager with as many windows as you want, plus its own desktop, panel and dock. It is light, fast, and it runs on Linux with X11."

**On screen text:** `Rust`, `GTK4`, `no Electron`, `Linux / X11`.

---

### Scene 3 - SOURCE and DEST (0:50 - 2:00)

**Goal:** make the viewer understand the one idea that sells the product.

**Do:**
1. Right click the desktop, **New Lister**. Place it next to the first one.
2. Open `sorgente` in the left window and `destinazione` in the right one.
3. Click the left window: point at the green **SOURCE** badge. Click the right one: the badge turns **DEST** and the old one switches.
4. Press **Tab** to jump between the two. Click the badge to cycle SOURCE, DEST, NONE.
5. Select `document_01.txt` and `document_02.txt`, press **F5**. Show the files appear on the right.
6. Copy `big.bin` to show the **progress bar** and the **Abort** button. Let it finish.
7. Copy it again: the **overwrite / skip** prompt appears. Choose Skip.
8. **Pin the DEST role** (pin button). Open a third Lister, lock it as DEST too, press F5: **both destinations receive the files** (fan-out).

**Say:**
> "Most file managers give you two fixed panes. OpusDM gives you any number of windows. Each one can be the source or the destination, and the role simply follows your focus. Press F5 and the files go from one to the other. Lock a role and it stays, and if you lock two destinations, one copy writes to both at once."

**Tip:** keep this scene slow. Everything later builds on it.

---

### Scene 4 - Keyboard first and Command Palette (2:00 - 2:50)

**Do:**
1. Type `gra` in the list: the cursor jumps to `big.bin`, the status bar shows `Find: gra`.
2. Press `/`, type `cfg` style text on a folder with source files (fuzzy match), then `*.txt` (glob) and show the match counter `12/150`. Esc to close.
3. Press **Ctrl+P**: the **Command Palette**. Type `checksum`, run it, show SHA-256 and MD5 appearing.
4. In the palette search `dir sizes`, run **Calculate Directory Sizes** and watch the Size column fill.
5. Open the **Command Line** from the palette and type a command with arguments.

**Say:**
> "Everything in OpusDM is reachable from the keyboard. Just start typing to find a file. Slash gives you a fuzzy filter. And Control P opens a palette with every single command in the program. If you can name it, you can run it."

**On screen:** key overlay must be visible here.

---

### Scene 5 - Batch Renamer (2:50 - 3:40)

**Do:**
1. Select `document_*` with **+** and the pattern `document_*`.
2. Press **F2**: the Batch Renamer opens with the live preview table.
3. **Search and Replace**: `document` becomes `doc`. Toggle **regex** and use a group (`$1`).
4. Add a **suffix**, then **numbering** (`_001`, `_002`).
5. Switch the case to `snake_case`, then `kebab-case`.
6. Type a name that creates a **duplicate**: show it flash in bold red and the Apply button blocked.
7. Fix it, confirm. The Lister updates instantly.
8. Bonus (10 seconds): select one file, **F2**: only the name is selected, the extension is left alone.
9. Bonus: **Copy As...** with `*.bak` and the live preview.

**Say:**
> "Renaming a hundred files is usually a script. Here it is a dialog with a live preview. Search and replace, regular expressions, numbering, case conversions. Duplicates are flagged before you can make a mess."

---

### Scene 6 - Find, filter, select (3:40 - 4:30)

**Do:**
1. **Advanced Select**: Name `*.bin`, Size at least `1M`, files only, then **Select**. Only `big.bin` lights up. Try `Modified after: 7d`.
2. **Reselect** (`Ctrl+Shift+A`): press Esc to lose the selection, then bring it back.
3. **Search in Files**: open the dialog on the project folder, type `main` in **Contains**, watch the results stream in as `path:line`. Click one: the **Viewer** opens on the first match.
4. **Compare / Sync Directories**: compare `sorgente` and `destinazione`, show the categories (only in A, newer in A, identical...) and click **Select Differences in Lister**.
5. Press **Ctrl+H** to toggle hidden files.

**Say:**
> "Selecting is powerful too. Combine name, size and age. Search inside files and watch the results appear as it goes. Compare two folders and select only what differs, ready to copy."

---

### Scene 7 - Archives, SFTP, encryption (4:30 - 6:00)

**Goal:** the "everything is a folder" payoff.

**Archives (4:30 - 5:00):**
1. Double click `example.zip`: you are inside. Show the path `example.zip#sorgente`.
2. Walk into a subfolder, press **Backspace** a few times to walk back out.
3. Select two files inside, **F5** to the DEST window. No extract dialog.

**SFTP (5:00 - 5:30):**
1. In the path bar type `sftp://user@host/` and press Enter.
2. Show the **host key fingerprint** prompt (explain it is the SHA-256 and a changed key is refused).
3. Copy a local file **up** to the server, then a remote file **down**. Show the transfer window with **Cancel**.
4. Mention the **FTP / SFTP Address Book** from the palette.

**Encryption (5:30 - 6:00):**
1. Right click a file, **Encrypt...**, enter a password twice. A `.enc` file appears.
2. Right click it, **Decrypt...** with a wrong password: show the error. Then the right one.

**Say:**
> "A ZIP is just a folder. Open it, look around, copy what you need. A server is just a folder too: type an SFTP address in the path bar and copy as usual. And if you need to protect something, right click, encrypt. Authenticated encryption, tamper detection, and it never overwrites your files."

**On screen text:** `ZIP, TAR, 7Z`, `FTP / FTPS / SFTP`, `XChaCha20-Poly1305 + Argon2id`.

---

### Scene 8 - Quick Look, terminal, viewer, player (6:00 - 7:00)

**Do:**
1. **Space** on an image: Quick Look. Move the cursor down the list with the preview still open: it **follows the selection**.
2. Same on a source file (line numbers), on a binary (**hex dump**), on an archive and on a folder.
3. Press **F4**: the **Quake-style terminal** slides down. Type `cd project/src`: the Lister **navigates by itself**. Browse in the Lister: the terminal prompt changes. Press F4 to close.
4. Double click a photo: the **Viewer**. Zoom with the wheel, rotate with **r**, toggle sharp pixels with **.**, press **s** for the **slideshow**, **f** for full screen, Esc to leave.
5. Double click the MP3 or video: the **Media Player** with cover art and the control bar.

**Say:**
> "Press space and you see the file, instantly. Press F4 and a terminal drops down, and it stays in sync with the folder you are looking at. Pictures, video and music open in built-in viewers, so you rarely need to leave."

---

### Scene 9 - Functions, hotkeys, banks, file types (7:00 - 8:00)

**Goal:** show depth without drowning the viewer. Build ONE small thing live.

**Do:**
1. Open **Manage Functions** and create a function: step 1 **Confirm** ("Backup these files?"), step 2 **Copy** to a backup folder, step 3 **Run** a bash command that shows output in a window.
2. Open **Edit Hotkeys** and bind it to a key, tick **Global** to show it works from any app.
3. Select files, press the key: the confirm prompt, the copy, and the **live output window** streaming.
4. Open **Button Banks**: make a floating bank with 3 buttons, show **left, middle and right click** doing different things.
5. Open **Edit File Types**: show the Audio file type, then **Filetype Creator** on a selected file.
6. Bookmarks: **Add Bookmark for This Folder** named `tour`, then type `tour:` in the path bar from somewhere else.

**Say:**
> "Under every button there is a Function: an ordered list of steps. Copy, ask a question, run a program, show its output. Bind it to a hotkey, a button, a menu or a file type. Build once, use everywhere."

**On screen text:** `One engine: buttons, hotkeys, menus, file types, events`.

---

### Scene 10 - The desktop around it (8:00 - 9:00)

**Do:**
1. **Edge snapping**: drag a *non-OpusDM* window (a browser) to the top edge, to a corner, to the left edge. Show the **rounded translucent preview**. Drag it away: it **regains its old size**.
2. **Right click a file, Leave Out on Desktop**: a permanent icon with an arrow badge. Make a **Program Group** and drop two apps in.
3. **Hide All**: everything vanishes except the desktop. **Reveal** brings it back.
4. **Panel**: click the **audio** icon (change volume, switch device), the **network** icon (Wi-Fi list), plug in a **USB stick** and show the popup with **Mount and open**.
5. **Rotating backgrounds** with a video wallpaper, then adjust **window transparency**.
6. **Themes**: open the theme settings and flip through *Cyberpunk*, *Monochrome Phosphor* and finally *Amiga Workbench 1.3*. Every open window changes live.
7. Quick flash of the **Screenshot tool** annotating an arrow, and the **Run window** (`opusdm-run`).

**Say:**
> "OpusDM is not only a file manager. It brings its own desktop. Snap any window to the edges with a live preview. Leave out the files you need, group your programs, hide everything with one key. The panel handles volume, Wi-Fi and USB drives, and the whole thing is themeable, from modern dark to Amiga blue."

---

### Scene 11 - Closing (9:00 - 9:30)

**Show:** the desktop in the Amiga theme with two Listers mid-copy. Fade to a title card.

**Say:**
> "That is OpusDM. A fast, native file manager and desktop for people who live in their files. It is open for you to try. Link in the description. If you want a deeper dive into Functions, file types or the desktop, tell me in the comments."

**End card (5 seconds):** project link, "Subscribe", suggested next video.

---

## 3. Optional mini-sections (cut or add to taste)

Use for a longer video, a series, or follow-ups.

- **Encrypt, hash and verify a backup** end to end.
- **Duplicate, Copy newer only, Check Fit** before filling a USB stick.
- **Links, Join and Split** a big file for a FAT drive.
- **Permissions and timestamps** across a whole tree.
- **Input settings**: mouse, touchpad, up to four keyboard layouts, live apply and rollback.
- **Apps Editor**: build your own application menu with drag and drop.
- **Script events**: run a function on Startup, Lister opened, or desktop double click.
- **Lister formats and show/hide patterns**, size formats, type indicators.
- **View Font** browser.
- **Screenshot tool** deep dive: line, arrow, text, undo, WebP, delay.

### Series idea

1. *OpusDM in 10 minutes* (this video)
2. *Functions and hotkeys: automate your file manager*
3. *Make your desktop yours: themes, backgrounds, banks, groups*
4. *Remote work: SFTP, archives and encryption*
5. *From Amiga to Linux: the Directory Opus story*

---

## 4. Titles, thumbnails, description

### Title options

- **I Built a Directory Opus for Linux (OpusDM in 10 Minutes)**
- **The File Manager That Replaces Your Whole Desktop**
- **Copy Files Faster: SOURCE and DEST Explained (OpusDM)**
- **A Rust File Manager With Unlimited Windows**
- **Directory Opus Is Back, on Linux**

### Thumbnail ideas

- Two Listers with big green **SOURCE** and pink **DEST** badges and an arrow between them. Text: `ONE KEY`.
- Split screen: a cluttered traditional file manager vs OpusDM. Text: `UNLIMITED WINDOWS`.
- A ZIP icon opening like a folder. Text: `ZIP = FOLDER`.

Keep the text to 2 or 3 words, high contrast, a face optional.

### Description template

```
OpusDM is a native Linux file manager and desktop written in Rust and GTK4,
inspired by Directory Opus. Any number of windows, SOURCE and DEST roles,
batch rename, archives and SFTP as folders, built-in encryption, a function
system, and a complete desktop with panel, dock and themes.

Chapters:
0:00 Intro
0:20 What is OpusDM
0:50 SOURCE and DEST
2:00 Keyboard and Command Palette
2:50 Batch Renamer
3:40 Find, filter, select
4:30 Archives, SFTP, encryption
6:00 Quick Look, terminal, viewer
7:00 Functions, hotkeys, banks
8:00 The desktop around it
9:00 Closing

Project: <link>
User guide: <link>
Requirements: Linux with X11
```

### Pinned comment idea

> "Which feature would you like a full deep dive on? Functions, desktop, or remote work?"

---

## 5. Pacing and style rules

- **Show, then say.** The screen does the work; the narration adds one sentence of "why".
- **One idea per scene.** If a scene needs two ideas, split it.
- **Never wait.** Cut or speed up (2x) every progress bar after the first second, except the one in Scene 3.
- **Show the keys.** Every shortcut visible on screen, and repeat the main ones (F5, F2, Ctrl+P, Space, F4).
- **Zoom in** on small UI (badges, preview table, snap preview) with a smooth 1.5x crop.
- **Use the same demo files** throughout, so viewers recognize them.
- **Honest limits** (mention once, in Scene 11 or in the pinned comment): Linux and X11 only, remote folders cannot rename or change permissions, the network tray does not do VPN yet.
- Music: low, instrumental, drops under the voice; a short synth sting on the cold open fits the retro vibe.

---

## 6. The 60 second cut (Shorts, social, website hero)

| Time | Show | Say |
| --- | --- | --- |
| 0:00 - 0:05 | Two Listers, F5, files fly across | "One key. That's all it takes to copy." |
| 0:05 - 0:15 | SOURCE and DEST badges switching on focus | "Any window can be the source or the destination." |
| 0:15 - 0:25 | Batch Renamer preview with red duplicate | "Rename a hundred files with a live preview." |
| 0:25 - 0:35 | ZIP opens as a folder, then an `sftp://` path | "Archives and servers are just folders." |
| 0:35 - 0:42 | Ctrl+P palette, type, run | "Every command is one shortcut away." |
| 0:42 - 0:50 | Window snapping with rounded preview, theme switch | "A full desktop, in any theme you like." |
| 0:50 - 1:00 | Title card with logo and link | "OpusDM. Directory Opus power for Linux." |

Vertical version: crop to the Lister and use large captions for the shortcuts.

---

## 7. Production checklist

- [ ] Demo data created and checked
- [ ] Theme, icon size and font set for readability
- [ ] Key overlay on, notifications off, private data hidden
- [ ] Each scene recorded in a separate clip (easier to re-take)
- [ ] Voice-over recorded separately and cleaned
- [ ] Zoom-ins, speed-ups and on-screen text added
- [ ] Chapters written from the real final timestamps
- [ ] Thumbnail (3 variants) and title A/B ready
- [ ] Captions or subtitles (Italian and English)
- [ ] Description, links and pinned comment prepared
