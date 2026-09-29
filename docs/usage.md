# OpusDM User Guide

OpusDM is a high-performance, dual-pane/multi-pane file manager for Linux inspired by the classic **Directory Opus**, built natively with Rust, GTK4, and an asynchronous Unix domain socket IPC hub architecture.

---

## 1. Core Architecture & Concepts

OpusDM is divided into two primary cooperating processes:

1. **`opusdm-hub`**: The background coordinator and centralized operations engine.
   - Manages global state, Lister window tracking, and background worker threads.
   - Executes filesystem operations asynchronously with non-blocking concurrency caps, progress tracking, conflict handling (overwrite/skip), and cancellation tokens.
   - Hosts a top-docked toolbar panel displaying active job progress, role indicators, and new window controls.
2. **`opusdm-lister`**: Individual file manager windows.
   - Lightweight, ultra-responsive GTK4 windows.
   - Communicates with the hub over a local Unix domain socket.
   - Can hold roles such as **SOURCE** or **DEST**, or operate as independent windows.

### The SOURCE / DEST Paradigm
Unlike conventional dual-pane managers locked into a single split window, OpusDM allows any number of Lister windows to be placed anywhere on your desktop:
- **SOURCE**: The active window from which files are read during Copy/Move operations.
- **DEST**: The destination window(s) into which files are written.
- **Pin / Lock Role**: Pinning a role ensures that changing window focus will not reassign SOURCE or DEST automatically.
- **Multi-Destination Fan-Out**: You can lock multiple DEST windows; clicking **Copy** will fan out copies to all locked destinations simultaneously.

---

## 2. Navigation & Interface

### Header Stat Bar
At the top of each Lister window:
- **D / F / B Counter**: Live counts of Directories, Files, and total Bytes in the current directory and active selection.
- **Role Badge**: Visual indicator of window role (`SOURCE` in green, `DEST` in red/amber, or `NONE`). Click the badge directly to cycle between roles.
- **Pin Button**: Locks the current role against focus-triggered changes.

### Main File View & Columns
The central view displays items sorted by directories first, with columns for:
- **Name**: File/folder name.
- **Size**: Formatted size (`B`, `K`, `M`, `G`, `T`). Directories show their count or calculated total size.
- **Date**: Last modification date and time.
- **User & Group**: File owner and group names.
- **Access**: Standard Unix permission bits (e.g., `rwxr-xr-x`).

Click any column header to sort ascending or descending.

### Action Toolbar
- **Copy (`c`)**: Copies selected items from SOURCE to DEST.
- **Move (`m`)**: Moves selected items from SOURCE to DEST.
- **Delete (`Del`)**: Prompts for confirmation and permanently deletes selected items.
- **MakeDir (`F7`)**: Prompts to create a new directory in current view.
- **NewFile (`Shift+F7`)**: Creates a new blank file.
- **Rename (`F2`)**: 
  - If a single item is selected: opens an inline prompt to rename it.
  - If multiple items are selected: opens the **Batch Renamer**.
- **Hidden Files Toggle (`Ctrl+H`)**: Toggles visibility of dotfiles (`.git`, `.config`, etc.).
- **Quick Look (`👁` / `F3`)**: Opens the instant file previewer.
- **Terminal Drawer (`>_` / `F4`)**: Drops down the embedded console drawer.

### Bottom Navigation
- **Home (`~`)**: Jumps to user's home directory.
- **Parent (`..` / `Backspace`)**: Navigates to parent directory (or up one level within a virtual archive).
- **Path Bar**: Editable path field. Press `Enter` to navigate directly to any path or virtual archive.

---

## 3. Advanced Features

### 3.1 Instant Fuzzy Finder (`/` or `Ctrl+F`)
Filter file listings in real time with subsequence fuzzy matching and glob patterns:
- Press `/` or `Ctrl+F` to reveal the inline filter bar.
- Type any partial characters (e.g., `cfg` matches `config.rs`).
- Supports glob wildcards such as `*.png`, `test_?.*`.
- Displays real-time match counters (e.g., `12/150`).
- Press **`Enter`** to focus and select the first match in the list.
- Press **`Esc`** at any time to immediately close the filter bar and restore the full directory listing.

### 3.2 Command Palette HUD (`Ctrl+P` or `Ctrl+Shift+P`)
A modal HUD dialog providing keyboard-driven access to all OpusDM commands:
- Search through commands by typing partial names.
- Navigate commands using **Up / Down** arrow keys and press **Enter** to execute.
- Includes quick selection tools (*Invert Selection*, *Select by Pattern*, *Deselect All*), filesystem operations, directory comparison, checksum calculations, and role configuration.
- Press **`Esc`** to dismiss.

### 3.3 Instant Quick Look Previewer (`F3` or `Space`)
Inspect files without launching heavy external applications:
- Select any file and press **`Space`** or **`F3`** (or click `👁`).
- **Images**: Crisp rendering of `.png`, `.jpg`, `.svg`, `.webp`, `.gif`, with dimension and resolution info.
- **Text & Code**: Syntax-friendly monospace viewer with line numbering, total line counts, and byte counters.
- **Archives**: Interactive listing of archive entries inside `.zip`, `.tar.gz`, `.7z`.
- **Binaries**: Formatted 16-byte hex dump with printable ASCII column and file offset markers.
- **Directories**: Quick inspection of folder contents.
- **Live Updating**: Leaving the Quick Look window open while moving the selection cursor in the Lister automatically updates the preview to the newly focused item.

### 3.4 Quake-Style Dropdown Terminal Drawer (`F4` or `` ` ``)
An embedded console drawer located directly at the bottom of the Lister:
- Press **`F4`** or the backtick/tilde key (`` ` `` / `~`) to slide the terminal open or closed.
- Runs commands asynchronously via the user's `$SHELL` without blocking the GTK UI.
- Command history navigation using **Up / Down** arrow keys.
- **Bi-directional Directory Sync**:
  - Typing `cd <dir>` in the terminal drawer updates the terminal and **automatically commands the Lister to navigate to that directory**.
  - Navigating in the Lister automatically updates the terminal's working directory (`$CWD`) and prompt.
- **External Launcher**: Click the "External" button in the drawer header to spawn a full detached terminal emulator (`xterm`) at the current directory.

### 3.5 Virtual Archive Filesystem (`.zip`, `.tar`, `.tgz`, `.7z`)
Browse and extract archives as if they were native folders:
- **Double-click** any supported archive (`.zip`, `.tar`, `.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar.zst`, `.7z`) to step inside it.
- Seamlessly navigate subdirectories within the archive.
- Breadcrumb paths are represented as `archive.zip#sub/dir`.
- Press **Parent** (`Backspace`) to navigate up through archive folders and back out to the filesystem.
- **Selective Extraction**: Select files or directories inside an archive and click **Copy** (`c`)—OpusDM extracts them into a staging area and queues a hub transfer directly into the DEST Lister!

### 3.6 Powerful Batch Renamer (`F2` on multi-selection)
Bulk renaming with real-time preview and conflict detection:
- Select multiple files and press **`F2`** (or select "Batch Rename" in the Command Palette).
- **Search & Replace**: Plain text matching or full regular expressions with group capture (`$1`, `$2`).
- **Prefix & Suffix**: Append or prepend text while preserving the file extension.
- **Sequential Numbering**: Automatic zero-padded numbering (e.g., `_001`, `_002`) with configurable start, step, and digit padding.
- **Case Transformations**: Convert to `lowercase`, `UPPERCASE`, `Title Case`, `snake_case`, or `kebab-case`.
- **Live Preview Table**: Side-by-side view of `Original Name ➜ New Name`. Conflicting or duplicate target names are highlighted in bold red with an alert.
- **Atomic Execution**: Renames items on disk and reloads the Lister instantly.

### 3.7 Visual Directory Compare & Sync
Compare two folders and identify divergences:
- Open the Command Palette (`Ctrl+P`) and choose **"Compare / Sync Directories (Visual Diff)..."**.
- Compares the current Lister directory against a target directory.
- Categorizes all items:
  - **Only in A (New)**
  - **Only in B**
  - **Newer in A / Newer in B**
  - **Different Size**
  - **Identical**
- Click **"Select Differences in Lister"** to automatically select all new and modified files in the current view for quick synchronization or copying.

### 3.8 On-The-Fly Checksum Calculator
Generate file hashes on demand:
- Select one or more files and choose **"Calculate Checksums"** in the Command Palette.
- Computes **SHA-256** and **MD5** hashes in streaming background threads.
- Copy individual hashes to the clipboard with one click.

### 3.9 Fast Recursive Folder Sizing (`Space` on Directories)
- Select any directory in the Lister and press **`Space`** (or choose "Calculate Directory Sizes" in the palette).
- Recursively traverses the directory tree in background threads.
- Replaces the directory's size column with the computed total (e.g., `3.4 GB`) and flashes the overall size in the status bar.

### 3.10 Real-Time Auto-Refresh (inotify)
The open directory is watched with Linux `inotify`, so the list follows what happens on disk:
- A download finishing, an editor saving, a `git checkout` or a build writing output all show up
  within a few hundred milliseconds - no manual `Ctrl+R`.
- Bursts are coalesced: a build that writes thousands of files reloads the list once, not once per
  file.
- An active filter survives the refresh, caret position included, so a reload never eats what you
  are typing.
- Virtual archive paths are not watched (there is nothing on disk to watch).
- Turn it off with **"Auto-refresh the list when the directory changes"** under **BEHAVIOUR** in
  Settings.

### 3.11 Theme Presets & File Icons
Open **Settings** on the hub toolbar; the **THEME** section at the top drives the whole look.
- **Preset dropdown**: one click fills every colour at once.
  - `Default (light)` - the original white list with a green SOURCE and pink DEST badge.
  - `Amiga Workbench 1.3` - deep blue ground with white and amber text.
  - `Directory Opus 4/5 Classic` - grey panels with navy accents.
  - `Cyberpunk / Tokyo Night` - neon cyan and pink on dark slate.
  - `Monochrome Phosphor (green)` and `(amber)` - vintage CRT terminal looks.
- Editing any colour by hand afterwards is fine; the stored preset then reads `custom`.
- **SELECTION** colours are now configurable too, instead of following the GTK theme.
- **File type icons**: images from the active theme (Breeze Dark is always installed and is the
  default). No Nerd Font is needed.
- **Type indicators**: appends an `ls -F` style character to each name - `/` directory,
  `*` executable, `@` symlink, `=` socket, `|` fifo.
- **Monospace file list**: renders every column in the system monospace font.
- **Apply theme colours to the whole window**: extends the file colours to the toolbar and
  navigation chrome. Leave it off to keep your system GTK theme around the list.

Settings live in `~/.config/opusdm/settings.json` and are pushed to every open Lister the moment
you press Save. A settings file written by an older build still loads: missing keys fall back to
their defaults.

### 3.12 Desktop Configuration
Right-click the desktop and pick **Desktop -> Settings** to open **opus-desktop-config**, a small
standalone window with three sections:
- **Background**: choose **Solid colour**, **Gradient** (two colours plus an angle in degrees), or
  **Image** (pick a file, then a fit mode - `Fill`, `Fit`, `Center`, `Tile`). An image that cannot
  be read falls back to the solid colour and the window shows a warning.
- **Icon grid**: the icon size and the grid spacing, each with `S` / `M` / `L` preset buttons and a
  free spin box.
- **Desktop folder**: override which directory the desktop shows. Empty means the default
  (`$XDG_DESKTOP_DIR`, then `~/Desktop`). **Reset to default** clears the override.

**Apply** sends the change to the running hub, which repaints the desktop immediately; **Save** does
the same and closes. When no hub is running the settings are written to
`~/.config/opusdm/desktop.json` and take effect the next time the hub starts. Per-folder desktop
state (icon layout) lives in a `.opusdm/` directory inside the folder.

---

### 3.13 User Hotkeys

Any saved function (see Manage Functions) can be bound to a key. Open the
Command Palette and choose **Edit Hotkeys...**. Each row has a key (click it,
then press the combination; Escape cancels), the function to run and a
**Global** check box. Rules:

- A user hotkey wins over a built-in binding; the editor notes when it does.
- Two hotkeys cannot share a key, whatever their Global flag: the row shows an
  error and Save stays disabled.
- While a text field has focus, a user hotkey only fires if it has Ctrl, Alt
  or Super, or is a function key, so a plain letter never steals a character.
- A **Global** hotkey works while any window has focus. It needs at least one
  of Ctrl, Alt or Super. It uses the X11 key grab, so it needs an X11 session:
  on Wayland, global hotkeys are not supported (local ones work everywhere).
  A key another program already owns cannot be grabbed and is logged.
- A global hotkey runs the function on the current SOURCE lister's selection;
  with no lister open the selection is empty.

**Find Key...** (Command Palette) asks for a key combination and tells you
what it does: a function, a built-in command, or nothing.

Bindings are stored in `~/.config/opusdm/hotkeys.json`.

### 3.14 User Menu

The **Menu** drop-down in the lister button row shows your own menu. Choose
**Edit User Menu...** at the bottom of it (or the Command Palette entry of the
same name) to add entries (each runs a saved function), submenus and
separators, rename and reorder them. Changes show up in every open lister
without a restart, and an entry whose function was deleted disappears from
the menu (the editor marks it). Stored in `~/.config/opusdm/usermenu.json`.

### 3.15 Script Events

**Edit Script Events...** (Command Palette) runs saved functions on system
events: `Startup`, `Shutdown` (the hub waits up to 10 seconds for these to
finish), `Lister opened`, `Lister closed`, and double clicks on the desktop
background with the primary, middle and right buttons. Functions started by an
event run without a lister asking for them: they use the current SOURCE
lister's selection, or an empty selection in your home directory.

To stop a hook that opens a lister from looping, an event is ignored while its
previous run is still going and never fires twice within 2 seconds. Stored in
`~/.config/opusdm/scripts.json`.

## 4. Keyboard Shortcuts Reference

Every binding below works whenever the file list has the focus. While you are typing in the filter
bar, the path bar or the terminal drawer, single letters are just text - only `Esc`, `F1`-`F12` and
the `Ctrl` chords stay active.

### Modal navigation (vim mode)

| Shortcut | Action |
| :--- | :--- |
| **`j`** / **`k`** | Move the cursor down / up one row (arrows work too) |
| **`g`** / **`G`** | Jump to the first / last row |
| **`l`** or **`Enter`** | Enter the directory, step into the archive, or open the file |
| **`h`** or **`Backspace`** | Go to the parent directory |
| **`~`** | Go to user home directory (`$HOME`) |
| **`Ins`** | Toggle the selection of the cursor row, then step down |
| **`Tab`** | Jump to the Lister holding the opposite role (SOURCE <-> DEST) |

### Everything else

| Shortcut | Action |
| :--- | :--- |
| **`/`** or **`Ctrl+F`** | Open inline Fuzzy File Filter |
| **`Esc`** | Close filter bar, close terminal drawer, or deselect all items |
| **`Ctrl+P`** / **`Ctrl+Shift+P`** | Open Command Palette HUD |
| **`F3`** | Toggle Quick Look file preview |
| **`Space`** | Quick Look preview (on files) / Calculate recursive size (on directories) |
| **`F4`** or **`` ` ``** | Toggle Quake-style dropdown terminal drawer |
| **`c`** | Copy selected items from SOURCE to DEST |
| **`m`** | Move selected items from SOURCE to DEST |
| **`d`** or **`Del`** | Delete selected items permanently |
| **`F2`** | Rename single item / Open Batch Renamer on multiple items |
| **`F7`** | Create new directory |
| **`Shift+F7`** | Create new blank file |
| **`Ctrl+A`** | Select all items |
| **`*`** | Invert current selection |
| **`+`** | Select files matching glob pattern (e.g., `*.rs`) |
| **`-`** | Deselect files matching glob pattern |
| **`Ctrl+H`** | Toggle hidden files (dotfiles) |
| **`Ctrl+R`** | Reload current directory |

---

## 5. Drag & Drop Operations

OpusDM supports full Drag & Drop interactions complying with the standard `text/uri-list` specification:
- Drag files between any Lister windows.
- Standard drop executes a **Copy** operation.
- Holding **Shift** while dropping executes a **Move** operation.
- You can also drag files out of or into external applications (text editors, file managers, desktop environments).
