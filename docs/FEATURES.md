# OpusDM - Feature highlights (marketing copy)

Raw material for the promotional website: taglines, section copy, feature
blurbs and ready-to-use lists. Every claim below maps to something OpusDM does
today (see `docs/usage.md` for the technical detail). Copy is in English; trim,
reorder and re-word freely.

---

## 1. Positioning

### One-liners (pick one for the hero)

- **The desktop that works like a file manager.**
- **Your files, your way. Many windows, one brain.**
- **A complete Linux desktop built around the best way ever invented to move files.**
- **Directory Opus power, reborn for Linux. Rust fast, GTK4 smooth.**
- **Stop dragging files between windows. Tell OpusDM where they go.**

### Elevator pitch (50 words)

OpusDM is a native Linux desktop environment and file manager inspired by the
legendary Directory Opus. Open as many file windows as you like, mark one as
SOURCE and another as DEST, and copy, move, rename and sync with a single key.
Built in Rust for speed, with a panel, dock, themes and tools included.

### Elevator pitch (short, 20 words)

A blazing fast Rust file manager and desktop for Linux: any number of windows, SOURCE and DEST roles,
and total control from the keyboard.

### Who it is for

- **Power users** who live in the file manager and hate repeating themselves.
- **Developers and sysadmins** who want a terminal, remote servers, checksums and search one key away.
- **Directory Opus veterans** who miss the real thing on Linux.
- **Tinkerers** who want to bend the desktop to their workflow, with themes, hotkeys and scripts.
- **Anyone tired of heavy desktops** and looking for a fast, light, coherent alternative.

### Key numbers and facts

- Written in **Rust** with **GTK4**: native, no Electron, no web runtime.
- **Unlimited** file windows (called Listers), not just two panes.
- **7 snap zones** for every window, **5+ theme presets**, **4 archive families** browsable as folders.
- **FTP, FTPS and SFTP** built in. **XChaCha20-Poly1305** encryption built in.
- Everything is a **Function**: one engine behind every button, hotkey, menu, file type and script.
- Settings are plain **JSON** files in `~/.config/opusdm/`, with automatic backups of damaged files.
- Runs on **Linux with X11**.

---

## 2. The big ideas (homepage sections)

### 2.1 SOURCE and DEST: the idea that changes everything

Forget a fixed two-pane window. In OpusDM every file window can be the
**SOURCE** (where files come from) or the **DEST** (where they go). The role
follows your focus: click a window and it becomes the source, the last one
becomes the destination. Press **F5** and the job is done.

- Any number of windows, placed anywhere on any monitor.
- **Pin a role** and it stays put while you work elsewhere.
- **Multi-destination fan-out**: lock several DEST windows and one Copy writes to all of them at once.
- Jobs run in the background with a progress bar, an **Abort** button and a clear overwrite / skip prompt when a name already exists.

> *Two folders. One key. Zero drag and drop.*

### 2.2 Keyboard first, mouse friendly

- **Command Palette** (`Ctrl+P`): every command in OpusDM, found by typing a few letters.
- **Command Line**: type `copy`, `rename`, or your own function name and run it with arguments.
- **Type-to-find** right on the file list, and an **instant fuzzy filter** with glob support (`cfg` finds `config.rs`, `*.png` finds every picture).
- **Hotkeys for everything**, including **global hotkeys** that work from any application.
- Classic function keys (**F2** rename, **F3** preview, **F4** terminal, **F5** copy, **F6** move, **Del** delete) so your hands already know the way.
- Tooltips on every button tell you the shortcut. Learn as you go.

### 2.3 A full desktop, not just a window

OpusDM brings its own **desktop, panel and dock**, all themed together, so the
file manager is the center of your whole session.

- **Desktop** with icons, a right click menu, and the files of your Desktop folder.
- **Panel** with application menu, virtual desktops pager, window list, system tray, clock and live **CPU, memory and network monitors**.
- **Dock** of application launchers on any screen edge, with drag and drop reordering.
- **Multi-monitor** aware: work areas follow the real struts of the panel and dock, monitor by monitor.
- Lightweight enough for an old laptop, polished enough for a new workstation.

---

## 3. File management

### 3.1 Everyday operations, done right

- **Copy, Move, Delete, New Folder, New File, Rename**: one key each.
- **Copy As / Move As**: send files to the other window under a new name, with `*` standing for the original (`*.bak`) and a **live preview** before you confirm.
- **Copy newer only**, **Duplicate**, **Recursive filter** copies, and **Check Fit** to see if a job will fit on the destination before you start it.
- **Rename that thinks**: F2 pre-selects the name and leaves the extension alone.
- **Links, Join and Split**: create symbolic links, glue split files together, or chop a huge file into pieces.
- **Permissions and timestamps** editing for one file or a whole tree.
- Safe by design: **atomic config saves**, no half-written files, and a prompt before anything destructive.

### 3.2 Batch Renamer

Rename hundreds of files in seconds, and see the result *before* you commit.

- **Search and Replace**, plain or full **regular expressions** with capture groups.
- **Prefix and Suffix** that respect extensions.
- **Sequential numbering** with start, step and zero padding (`_001`, `_002`).
- **Case transforms**: lowercase, UPPERCASE, Title Case, snake_case, kebab-case.
- **Live preview table** (`original -> new`) with conflicts and duplicates flagged in red.
- Available from F2, from Copy As and from Move As.

### 3.3 Selection superpowers

- Click, Ctrl+click, Shift+click, **Ins**, **select all**, **invert**.
- **Select and deselect by pattern** (`*.txt`, `report_0[1-3]*`).
- **Advanced Select**: combine name, **size** ("at least 1M"), **age** ("last 7d" or a date) and type (files only, folders only).
- **Reselect**: bring back the selection you just lost.

### 3.4 Find anything

- **Find Files** by name pattern with include and exclude (`*.log, *.rs, !test*`).
- **Search in Files**: look *inside* files, results with file and line number, streamed live while it runs.
- Skips binary files and hidden folders unless you ask, cancels instantly, and tells you what it could not read.
- Jump from any result to its folder, or open it in the Viewer on the first match.

### 3.5 Compare, sync and verify

- **Visual Directory Compare**: only in A, only in B, newer in A, newer in B, different size, identical. One click selects the differences, ready to copy.
- **Checksum calculator** (SHA-256 and MD5) computed on the fly, without leaving the window.
- **Recursive folder sizes** in a flash, remembered between listings, with a force-rescan when you need a fresh number.

### 3.6 Instant Quick Look

Press **Space** and see the file, right now, without opening a heavy app.

- **Images**: PNG, JPG, SVG, WebP, GIF with size and resolution.
- **Text and code**: monospace view with line numbers and counters.
- **Archives**: browse their contents.
- **Binaries**: a proper hex dump with ASCII column.
- **Folders**: peek at what is inside.
- **Live**: leave the preview open and it follows your cursor through the list.

---

## 4. Beyond the local disk

### 4.1 Archives are just folders

Double click a **ZIP, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, TAR.ZST, 7Z, RAR, ISO or CPIO** and walk inside.
Select what you want and copy it out to the destination window. No extract
dialog, no temporary clutter: scratch folders are private and cleaned up for you.

- Navigate nested folders, go up with Backspace, even out of the archive.
- **Selective extraction** with a normal Copy.
- Hardened against nasty file names (dashes, wildcards, `..`).

### 4.2 FTP, FTPS and SFTP built in

Type `sftp://user@host/path` in the path bar and the server becomes a folder.

- **Address book** of your sites, with host, port, user, start folder and passive mode.
- **SFTP host key verification** with SHA-256 fingerprints, and a loud refusal when a key changes.
- Uses your **ssh-agent and keys** automatically. Passwords stay in memory only.
- Copy, Move and Delete between **local and remote, or two servers**, with a progress window, Cancel and overwrite prompts.
- Double click a remote file to download and open it.

### 4.3 Built-in encryption

Right click, **Encrypt...**, type a password. That is it.

- **XChaCha20-Poly1305** authenticated encryption with **Argon2id** key derivation.
- Detects tampering, truncation and wrong passwords.
- Never overwrites, never leaves half-written files behind.
- Encrypts whole folders and keeps their structure.

### 4.4 Integrated terminal

Press **F4** or the backtick and a **Quake-style terminal drawer** slides in.

- **Two-way directory sync**: `cd` in the terminal and the file window follows; browse in the window and the terminal follows.
- Runs your own `$SHELL`, with command history.
- One click to open a full external terminal at the current folder.

---

## 5. Automation: the Function system

The heart of OpusDM. A **Function** is an ordered list of steps, and *everything*
runs on them: toolbar buttons, hotkeys, menu items, file type actions, script events.

### 5.1 What a Function can do

- Run **internal commands** (copy, move, rename, change directory, set background, encrypt, show output, and many more).
- Run **external programs and bash commands**, with placeholders for the selected files, names, paths and the other window's selection.
- **Ask you a question** (Confirm step) and stop if the answer is no.
- Open a **live output window** that streams a program's output as it runs.
- Handle **name conflicts** with the same Overwrite / Skip prompts as normal jobs.
- Show a **busy indicator** while it runs.
- **Change directory** with options: in a new window, select an item, choose the view mode.

### 5.2 Where Functions plug in

- **Hotkeys**, local or **global**, bound to a Function, a bash command or a program.
- **User menu** in every file window.
- **Script events**: run something on **Startup, Shutdown, Lister opened, Lister closed**, or a **double click on the desktop** with any mouse button.
- **File Types** (below).
- **Button Banks** (below).
- **Command Line** and **Command Palette**.

### 5.3 File Types

Teach OpusDM what a file *is*, not just what it is called.

- Match by **name pattern, magic bytes, text content, size range and date range**, combined with AND / OR.
- Per type: a **double click action** and your own **Open With** menu.
- **Filetype Creator**: select a file and OpusDM derives the rule for you.
- Reorderable, live-applied, safe to edit.

### 5.4 Button Banks

Build floating **toolbars of your own**.

- Buttons with a label and/or icon, and **three actions each: left, right and middle click**.
- Horizontal or vertical, any number of columns, borderless if you like.
- Remembered position, restored at the next start, hidden together with everything else on demand.
- Open, close and toggle them from hotkeys.

### 5.5 Bookmarks

Give any folder a short name and jump there from the path bar: `work:`, `photos:2026/trip`.
Inspired by the Amiga "assign", fully managed from a friendly dialog.

---

## 6. Desktop and window management

### 6.1 Leave Out

Keep any file or folder on the desktop as a **permanent icon with an arrow badge**,
wherever it really lives. Drag it where you want and it stays there. Remove the
icon any time; the file is never touched.

### 6.2 Program Groups

Gather programs, files and folders under **one desktop icon**. Drop things on it,
double click to open the group, double click a member to launch it.

### 6.3 Hide and Reveal

One command, one hotkey: **hide every window, the panel and the dock** and show
only the desktop. Press it again and everything comes back where it was.

### 6.4 Edge snapping, KDE style

Drag **any** window near a screen edge and a **translucent preview with rounded corners** shows where it will land.

- Top edge: maximize. Corners: quarters. Left and right: halves or thirds.
- **Un-snap** by dragging away and the window gets its old size back under the pointer.
- Works for every X11 application, not just OpusDM, and on every monitor.
- Power-friendly: it sleeps until you actually press a mouse button.

### 6.5 Backgrounds that move

- A **rotating list of backgrounds**: pictures, **videos**, flat colors and gradients, with fit modes.
- Pick the interval, a **cross-fade** between entries, and **shuffle**.
- Videos pause automatically while a full screen window covers the desktop.
- Add the wallpapers shipped with your theme in one click.

### 6.6 Window transparency

Set the opacity of **active and inactive windows** separately (live, compositor-powered).
Turn it off with one switch and your values are kept for later.

### 6.7 Startup apps

A simple list of programs to start with your session, each with an optional delay.

---

## 7. Panel, dock and system tray

### 7.1 Panel

- **Application menu** built from your installed apps, or from a menu you design.
- **Pager** for virtual desktops, **window list**, **system tray**, **clock**.
- **Quick apps**: a strip of icon-only launchers, reorderable by drag and drop.
- **Live CPU, memory and network monitors**.
- Move it to any edge from the settings.

### 7.2 Dock

- Launchers with tooltips, a busy cursor while the app starts, and the app's own **desktop actions** on right click.
- **Drag to reorder**, drag out to remove, drop a `.desktop` file to add.
- Sizes 32, 48 and 64 on any edge that the panel does not use.

### 7.3 Audio tray

Everything you expect from a modern volume applet.

- **Volume, mute and default device** for **output and input**.
- Reacts instantly to keyboard volume keys, other mixers and **hot-plugged USB headsets**.
- The slider never jumps under your finger.
- Works with **PulseAudio and PipeWire**. Reconnects by itself.

### 7.4 Network tray

- **Ethernet and Wi-Fi switches**, and a list of nearby networks sorted by signal.
- Click to connect, with lock icons for protected networks and a **password prompt with show/hide**.
- Wrong password? The half-saved connection is cleaned up for you.
- **VPN switches** for WireGuard and OpenVPN, a lock on the icon while a VPN is up.
- **Network Connections** (`opusdm-network-manager`), like the KDE Connections page: edit Wi-Fi,
  Ethernet, WireGuard and OpenVPN connections with their IPv4 and IPv6 settings, and **import
  `.conf` and `.ovpn` files**. Passwords and keys stay in the login keyring.
- Powered by **NetworkManager**.

### 7.5 Device notifier

- Plug in a USB stick and a small popup offers **Mount**, **Mount and open**, or **Ignore**.
- A panel icon lists removable volumes with **Mount, Open, Unmount and Eject** (safe power-off included).
- **No password, no sudo**: uses udisks2.

---

## 8. Media and graphics

### 8.1 Viewer

A fast image and video viewer that knows its way around a folder.

- **Zoom** from 5% to 1600%, pan, fit, 100%, **rotate**, mouse wheel and keyboard.
- **Pixel-perfect mode** (sharp pixels instead of smoothing) for pixel art and fine detail.
- **Thumbnail strip**, natural sort order (`img2` before `img10`), wrap-around gallery.
- **Slideshow** with automatic timing, videos play to the end.
- Full screen, and **Delete to Trash** with a second confirmation if there is no trash.
- GStreamer-powered video playback inside the same window.

### 8.2 Media Player

- **Audio and video**, with cover art, title, artist and album from the tags.
- Seek slider, volume, mute, loop, full screen on double click.
- Complete keyboard control: Space, arrows, M, F, Q.
- Registered automatically as the default action for audio and video file types.

### 8.3 Screenshot tool

- **Freeze the screen, drag the area, annotate, save.**
- **Line, Arrow (with 15 degree lock), Text and Select** tools, six colors, three sizes, **undo and redo**.
- Save as **PNG, JPG or WebP** (with quality slider and lossless option).
- **Quick save** to `~/Pictures/Screenshots`, or **Copy** to the clipboard.
- **Capture delay** (0 to 60 seconds) for menus and hover states.

### 8.4 View Font

Browse every installed font family with a searchable list, adjustable size,
bold and italic toggles and your own sample text.

---

## 9. Look and feel

### 9.1 Themes

One click and the whole desktop changes.

- **Default light**, **Amiga Workbench 1.3**, **Directory Opus 4/5 Classic**, **Cyberpunk / Tokyo Night**, **Monochrome Phosphor** (green and amber CRT).
- **Breeze Dark icon set** included, no icon font required.
- Every color editable, including selection colors. Edit a preset and it becomes your own.
- **Theme Editor** and shareable **theme packs**.
- Live: change a color and every open window updates immediately.

### 9.2 Customizable file lists

- Columns: **Name, Size, Date, User, Group, Access** (permissions as `rwxr-xr-x`).
- **Icon view** and list view.
- **Lister formats** and **show/hide patterns** to control what each window displays.
- **Size format** switching (bytes, human readable) without re-reading the folder.
- Type indicators like `ls -F` (`/` folders, `*` executables, `@` links).
- **Monospace** option for every column.
- Dates that respect **daylight saving time** per file.

### 9.3 Always fresh

- **Real-time auto-refresh** with inotify: create a file in a terminal and it appears in the window almost instantly.
- **Directory cache and history** for instant back and forward, with a cache list and a Free Caches command.

---

## 10. System settings

- **Input settings**: mouse and touchpad speed, tap to click, natural scrolling, left handed, **up to four keyboard layouts**, layout switching key, key repeat delay and rate. Every change **applies live** and is rolled back if you cancel. Reapplied automatically when you plug in a device.
- **Apps Editor**: a KDE-style menu editor with a tree, drag and drop, and a properties form. Edits are written as standard `.desktop` files and never touch system files.
- **Run window** (`opusdm-run`): a tiny launcher with ranked suggestions from apps and commands on your PATH.
- **Desktop settings**: icon size and spacing, desktop folder, backgrounds, startup apps, snapping, transparency.

---

## 11. Quality and engineering (for the "Why trust it" section)

- **Rust** end to end: memory safe, fast startup, low footprint.
- **Hub and Lister architecture**: a background hub runs every file job asynchronously with progress, cancellation and conflict handling, while each window stays responsive.
- **Local IPC** over a Unix socket: no network service, nothing exposed.
- **Extensive automated tests**: archive handling, encryption, FTP and SFTP (against a real `sshd`), remote transfers, formats, file types, snapping, audio and more.
- **Fail-safe configuration**: atomic writes, malformed files saved as `.bak`, older settings files keep loading.
- **Security minded**: archive names never read as options, private 0700 scratch folders, authenticated encryption, SSH host key checking, passwords never written to disk or logs.
- **Open architecture**: settings are readable JSON you can version, share and script.

---

## 12. Feature comparison snippets

| You want to... | The usual way | The OpusDM way |
| --- | --- | --- |
| Copy files to another folder | Open two windows, drag, hope | Mark SOURCE and DEST, press **F5** |
| Rename 200 files | A script or a separate tool | **F2**, preview, done |
| Peek inside a ZIP | Extract it, look, delete it | Double click, browse, copy what you need |
| Upload to a server | Launch an FTP client | Type `sftp://...` and copy like any folder |
| Find text in a project | Switch to a terminal | **Search in Files**, results stream live |
| Protect a folder | Install an encryption tool | Right click, **Encrypt...** |
| Automate a chore | Write a shell script and hide it somewhere | Build a **Function**, bind it to a key or a button |
| Change the whole look | Hunt for a theme and restart | Pick a preset, it applies instantly |

---

## 13. Ready-to-use copy blocks

### Feature grid (title + one line)

- **SOURCE / DEST roles**: Any number of windows, always clear where files go.
- **Command Palette**: Every command, one shortcut away.
- **Batch Renamer**: Regex, numbering and case, with live preview.
- **Archive browsing**: ZIP, TAR, 7Z, RAR, ISO open like folders.
- **FTP / FTPS / SFTP**: Servers are just folders.
- **Encryption**: XChaCha20-Poly1305, built in.
- **Quick Look**: Images, code, hex and archives on Space.
- **Integrated terminal**: A drawer that follows your folder.
- **Functions**: One engine behind every button and hotkey.
- **File Types**: Rules by name, bytes, size and date.
- **Button Banks**: Your own floating toolbars.
- **Directory Compare**: See exactly what differs.
- **Search in Files**: Text search with live results.
- **Edge snapping**: KDE-style tiling for every window.
- **Rotating backgrounds**: Pictures, videos and gradients.
- **Themes**: From Amiga blue to Tokyo neon.
- **Audio and network tray**: Volume, Wi-Fi and VPN in the panel.
- **Device notifier**: USB sticks, mounted in one click.
- **Screenshot tool**: Capture, annotate, share.
- **Viewer and player**: Pictures, video and music built in.

### Short taglines for feature cards

- *Copy to the other window. That's it.*
- *Rename a hundred files without leaving your chair.*
- *Your keyboard is the fastest mouse you own.*
- *A ZIP is just a folder you have not opened yet.*
- *One desktop. Every tool. All in Rust.*
- *Built by people who really use a file manager.*
- *Small footprint, big reach.*
- *Make it yours: themes, hotkeys, functions, banks.*

### Call to action ideas

- **Try OpusDM today.**
- **Download for Linux.**
- **See it in action** (link a 60 second GIF or video of SOURCE / DEST copy).
- **Read the guide** (link `docs/usage.md`).

---

## 14. Suggested site structure

1. **Hero**: tagline, one-line pitch, screenshot of two Listers with SOURCE and DEST badges, Download button.
2. **The SOURCE / DEST idea**: short animation or GIF of copy between windows.
3. **Feature grid** (section 13).
4. **Power section**: Command Palette, Functions, hotkeys, File Types, Button Banks.
5. **Beyond the disk**: archives, SFTP, encryption, terminal.
6. **Full desktop**: panel, dock, tray, snapping, backgrounds.
7. **Media**: viewer, player, screenshot.
8. **Themes gallery**: one screenshot per preset.
9. **Under the hood**: Rust, tests, security, JSON config.
10. **Comparison table** (section 12).
11. **Download and install**, requirements, link to the user guide.

### Screenshot and demo ideas

- Two Listers side by side with SOURCE and DEST badges and a copy in progress.
- Batch Renamer with the red conflict highlighting.
- Command Palette over a busy desktop.
- Quick Look on an image, a source file and a hex dump.
- A ZIP opened as a folder, next to an SFTP session.
- The same desktop in each theme preset.
- Snap preview with rounded corners over a half-screen zone.
- Panel close-up: pager, quick apps, audio, network and device icons.
- Screenshot editor with arrows and text annotations.

---

## 15. Requirements and honest fine print

Useful for the download page, and for building trust.

- **Linux with an X11 session.** Global hotkeys, snapping and some window features need X11.
- Compositor (picom) recommended for window transparency and the snap preview.
- Optional system pieces by feature: **GStreamer** (video and audio playback), **udisks2** (device notifier), **PulseAudio or PipeWire with pipewire-pulse** (audio tray), **NetworkManager** (network tray and connections; `network-manager-openvpn` for OpenVPN), **xinput, setxkbmap and xset** (input settings), **7z** (creating 7z archives; reading any archive is built in through libarchive, which also needs `libarchive13` at run time).
- Remote folders do not support rename, permissions or links (the server decides).
- Network Connections edits WireGuard and OpenVPN only among the VPN types; hotspots and enterprise Wi-Fi are not managed yet.
