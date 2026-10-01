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
- **MakeDir (`Ctrl+N`)**: Prompts to create a new directory in current view.
- **NewFile (`Ctrl+Shift+N`)**: Creates a new blank file.
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

### 3.10b Status bar format
The line at the top of every Lister is built from the **Status bar format** field under
**BEHAVIOUR** in Settings (`status_format` in `settings.json`). Codes: `%d` `%f` `%b` are the
total directories, files and bytes; `%D` `%F` `%B` the same for the selection; `%s` the number of
selected entries; `%p` the current path; `%%` a percent sign. The default is
`D: [%D/%d]  F: [%F/%f]  B: [%B/%b]`; an empty field means the default.

### 3.10c Lister formats and Show/Hide patterns
Open **Lister Format...** from the command palette (`Ctrl+P`).
- **Columns**: tick the ones to show, move them with **Up** / **Down** (Name can move but not be removed). **Sort by** and **Reverse** choose the order, **Separation** puts directories first, last or mixed with the files.
- **Show only** / **Hide** are pattern lists for *files* (directories are always listed): `*` and `?`, separated by `,` or `;`, and `!pattern` excludes (`*.rs,!test*`). **Hide** wins over **Show**.
- **Free space gauge** adds a used-space bar to the top status bar.
- **Use** applies the format to the current directory until you leave it. **Save (this directory)**, **Save (with subdirectories)** and **Save as default** store it in `~/.config/opusdm/formats.json`; every Lister picks the change up at once. Entering a directory applies its own format, else the nearest parent saved with subdirectories, else the default, else the built-in one. **Remove saved format** deletes this directory's entry, **Reset** refills the dialog with the built-in format.
- **Set Show Pattern...** and **Set Hide Pattern...** (palette) edit just the pattern and save it where the current format came from (a new entry for this directory when nothing was saved). An empty answer clears it.
- Clicking a column header still sorts on the spot, but only **Save** keeps it.

### 3.10d Directory cache, history and Cache List
- Each Lister remembers the directories it read recently (20 by default; **Directory cache size** under **BEHAVIOUR** in Settings, 0 turns it off). Going back to one shows its list at once, and the disk is read again in the background: if something changed the list updates without losing your selection. `Ctrl+R` always reads the disk.
- Next to the path bar, **back** and **forward** buttons and a **history** drop-down list the directories you visited in this Lister (newest first, the current one in bold). `Alt+Left` and `Alt+Right` do the same from the keyboard. Going somewhere new after stepping back drops the entries you stepped back over. The history keeps 50 entries and is not saved between sessions.
- **Cache List...** (command palette) shows the cached directories of every Lister. Double click an entry to raise the Lister that holds it and show the directory there; **Open here** opens the selected directory in the Lister you are in; **Free all caches** empties every Lister's cache (also available on its own as **Free Caches**).

### 3.10e Advanced Select
**Advanced Select...** (command palette) selects or deselects the listed entries by several criteria at once. Every filled-in field must hold; empty fields are ignored.
- **Name**: the same pattern lists as Show / Hide (`*.rs, *.toml`, `!test*` excludes).
- **Size at least / at most**: `512K`, `1M`, `2G`. A size limit never matches a directory.
- **Modified after / before**: `30m`, `12h`, `7d`, `2w` (that long ago) or `YYYY-MM-DD [HH:MM[:SS]]` in local time; a bare date means midnight.
- **Type**: files and directories, files only, or directories only.
- **Select** adds the matching entries to the selection, **Deselect** removes them. Entries hidden by the filter bar or a Show / Hide pattern are not touched. A wrong value is reported next to the field and the requester stays open. `+` and `-` keep working as the quick single-pattern versions.

### 3.10f Viewer and slideshow
Quick Look (`F3` / `Space`) is still the light preview that follows the selection. For real reading, select one or more files and use the command palette (`Ctrl+P`):
- **Read (SmartRead)** picks the mode: pictures by extension, ANSI art for `.ans` / `.diz` / `.asc` files or when the first 4 KiB hold a colour escape sequence, text for readable files, hex for everything else. **Read as Text**, **Read as ANSI** and **Read as Hex** force a mode.
- The Viewer pages through the selected files (directories are skipped; with nothing selected it opens the file under the cursor). Keys: `n` / `p` next / previous file (wrapping), `t` `a` `x` `s` switch to Text / ANSI / Hex / Smart, `Ctrl+F` opens the search bar, `Esc` closes the search bar and then the window.
- **Search**: type to highlight every match (case-insensitive) with a `3/17` counter; `Enter` / `F3` jump to the next match, `Shift+Enter` / `Shift+F3` to the previous, both wrapping around. In hex mode the search runs on the dump text.
- ANSI mode decodes CP437 and understands the colour, bold and cursor-forward sequences; the SAUCE record after a `Ctrl+Z` byte is not shown.
- Text and ANSI show up to the first 16 MiB of a file, hex the first 1 MiB; the header says "showing the first N" when a file is cut.
- **Show (image slideshow)** takes the pictures among the selected files. `Left` / `Right` move, `Delete` (or `m`) marks or unmarks the current picture (`MARKED` and a counter show in the header). Closing the window asks "Delete N marked file(s)?"; nothing is removed unless you answer Delete.

### 3.10g Find Files and Search in Files
**Find Files...** and **Search in Files...** (command palette) open the same **Find** dialog; they only differ in where the cursor starts.
- **Name**: file name patterns, the same as Show / Hide (`*.log, *.rs`, `!test*` excludes). Empty matches every name.
- **Contains**: text to look for inside files. Empty means a search by name only (folders that match the name are listed too); filled, only files whose name matches and whose content holds the text are listed, as `path:LINE  text`, with the number of matches when there are several.
- **Folder** starts as the Lister's folder. Hidden names (starting with `.`) are skipped with everything below them unless **Include hidden** is on, symbolic links are not followed, and binary files are never searched. Text is searched in the first 16 MiB of each file; a search stops at 5000 results.
- Results appear while the search runs; **Stop** or closing the dialog cancels it. The status line ends with how many entries were scanned and how many could not be read.
- **Show in folder** takes the Lister to the folder that holds the selected result and selects it. **View** opens the file in the Viewer, on the first match of the text. Double click does View for content results and Show in folder for name results.

### 3.10h File Types
**Edit File Types...** (command palette) edits the list kept in `~/.config/opusdm/filetypes.json`. The first type (top to bottom) whose rule matches a file wins; use the arrows to reorder.
- **Match**: one or more conditions combined with "all of" or "any of": file name patterns, bytes at an offset (hex, e.g. `89 50 4E 47`), text in the first N bytes, size range (`10K`, `4M`) and modified-time range (`7d`, `2026-01-01`). Content is read from the first 64 KiB of the file.
- **Actions**: the function run on a **double click** (instead of the default application) and the functions listed in **Open With** in the right-click menu of matching files. The function works on the double clicked or right-clicked file, which becomes the only selected row. Folders and archives keep their usual behaviour; a file with no matching type opens with the default application as before.
- **Drop** is stored in `filetypes.json` but not wired to drag and drop yet.
- **Filetype Creator (from selected file)...** opens the editor with a new type derived from the selected file: `*.ext` for a file with an extension, its first bytes for a binary file without one, the exact name otherwise.
- Changes apply at once to every Lister; Save is disabled while a type has a problem (shown under the form). A nested rule written by hand in the JSON is shown read-only and kept as it is.

**Conflicts inside functions.** When a Copy or Move step finds a name already at the destination, the Lister that started the function shows the usual Overwrite / Skip / Overwrite All / Skip All / Cancel requester and the step goes on with the answer. A function started with no Lister to ask (a script event or a Shutdown hook) skips the existing entry. Delete steps never conflict.

### 3.10i FTP, FTPS and SFTP
Type `sftp://user@host[:port]/path`, `ftp://...` or `ftps://...` in the path bar (or use **FTP / SFTP Address Book...** in the palette) to browse a server like a folder. Backspace, the parent button and double click work as usual.
- **Address book** (`~/.config/opusdm/ftp.json`): name, protocol, host, port, user, start folder, FTP passive mode and, for FTPS, certificate verification. **Connect** opens the site; the box at the top opens any address.
- **Passwords** are asked when needed and kept in memory only. SFTP tries your ssh-agent and `~/.ssh/id_*` keys first. FTP without a user logs in as anonymous.
- **Host keys** (SFTP) are checked against `~/.config/opusdm/known_hosts.json`. A new host shows its SHA-256 fingerprint and asks before connecting; a changed key is refused.
- **Copy**, **Move** and **Delete** work with a remote folder as source, as DEST, or both (two servers). Remote to local downloads, local to remote uploads, remote to remote goes through a scratch folder in the system temp directory (cleaned afterwards; stale ones older than a day are removed). **New Folder** and **New File** work in a remote folder too. A Move deletes its sources only after the copy succeeded.
- A small window shows the file being transferred and has **Cancel** (closing it cancels too). When some targets already exist you are asked to Overwrite, Skip the existing ones or Cancel; folders merge, and with Overwrite nested files are replaced.
- Double click on a remote file downloads it and opens it with the default application.
- Not available on remote folders: Rename, Duplicate, permissions, timestamps, links and Join/Split. **FTP Quit** closes every connection.

### 3.10j Encrypt and Decrypt
**Encrypt...** and **Decrypt...** (command palette) ask for a password and write into the DEST Lister (or this folder when there is no DEST).
- Encrypt writes `name.enc` for every selected file; folders keep their tree. Decrypt turns `name.enc` back into `name` (a file not ending in `.enc` becomes `name.dec`). Existing files are never overwritten.
- Right click on a file also offers **Encrypt...**, and **Decrypt...** on `.enc` files. They apply to the selection when the clicked file is selected, otherwise to that file.
- Format: XChaCha20-Poly1305 in 64 KiB chunks, key from the password with Argon2id and a random salt per file. Tampering, truncation and a wrong password are detected; a failed file leaves nothing behind and the rest still run. **There is no password recovery.**
- The password travels from the Lister to the hub over the local socket, in memory only; it is never written to disk or to logs.

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
`~/.config/opusdm/desktop.json` and take effect the next time the hub starts. Icon positions of the
Desktop folder are not saved; only left-out icons (see 3.21) remember where they were dragged.

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

### 3.16 Copy and Move Variants, Recursive Filter

Command Palette entries next to Copy and Move:

- **Copy As... / Move As...** send the selection to the DEST lister under new
  names. In the dialog `*` stands for the original name (`*.bak` turns `a.txt`
  into `a.txt.bak`), a pattern without `*` only works for one entry, and a live
  preview shows every result before you confirm.
- **Duplicate Selected Items** copies each entry next to itself as `name.copy`,
  then `name.copy2`, and so on.
- **Copy (newer only)** copies only files that are missing at the destination
  or newer than the destination copy.

In the Function Editor the internal `Copy` and `Move` commands also offer
"only files that already exist at the destination" (UPDATE), "only files
missing or older" (NEWER) and, for `Copy`, "move instead of copy on the same
device"; `Copy As`, `Move As` and `Duplicate` are internal commands too. Every
Copy, Move, Delete, As and Duplicate step takes an optional filter such as
`*.rs;*.toml`.

**Recursive filter** (Theme Config, Behaviour): when on, a Copy, Move or Delete
that involves a directory first asks for a file pattern. Only matching files
are affected, directories are always walked, and a directory with no matching
file is not created at the destination. A filtered Move deletes only the files
it copied. Leave the pattern empty to process everything, Cancel to drop the
job.

### 3.17 Permissions and Timestamps

- **Change Permissions...** shows an rwx grid for owner, group and other with a
  synchronised octal field, starting from the first selected entry's mode.
  "Apply recursively" walks directories; files get the chosen mode and
  directories also get `x` wherever `r` is set, so they stay traversable.
  Symbolic links are skipped. Changing the owner is not supported.
- **Set Timestamp...** sets the access and modification time of the selection
  to a local `YYYY-MM-DD HH:MM:SS` value ("Now" fills in the current time);
  "Apply to contents of directories" recurses. A symbolic link gets the time
  itself, its target is left alone.

With the recursive filter on (see 3.16), a recursive run asks for a pattern
first and then changes only matching files, never directories.

### 3.18 Links, Join and Split

- **Make Link...** creates a symbolic or hard link to every selected entry in
  the DEST lister. The name pattern works like Copy As (`*` = original name).
  An existing target is reported and never overwritten; hard links to
  directories and across devices are refused by the system and reported.
- **Join Files...** concatenates the selection, in the order it is listed,
  into one new file. The name starts as the first part without its `.001`
  suffix. An existing file is never overwritten.
- **Split File...** cuts the selected file into parts of a given size
  (`512K`, `4M`, `1G`, powers of 1024) named `file.001`, `file.002`, ... If the
  parts already exist nothing is written. Join and Split write into the DEST
  lister, or the current directory when there is none.

### 3.19 Check Fit and Reselect

- **Check Fit (Source -> Dest free space)...** sizes the selection (regular
  files, recursively, symbolic links not followed) and compares it with the
  space available to you at the DEST lister's directory. The requester says
  whether everything fits, or what percentage does, with both sizes.
- **Reselect** (`Ctrl+Shift+A`) selects again the entries of the last
  selection that a job (Copy, Move, Delete, ...) or Deselect All / Esc cleared,
  as long as they are still listed. `Ctrl+A` still selects everything.

### 3.20 Change Directory and Set Background (function steps)

Two internal commands of the Function Editor act on the desktop instead of on files:

- **Change Directory** navigates the SOURCE lister to a folder. Its **Directory** field takes an
  absolute path, `~` (your home) or a path relative to the folder the lister is in, and may hold the
  usual placeholders (`{f}`, `{o}`, ...). The folder must exist, otherwise the step fails and the
  function stops. It also moves the working directory of every later step: after
  `Change Directory  ~/Projects`, a `Make Directory` step creates its folder in `~/Projects` and an
  external program with "run in the SOURCE folder" starts there.
- **Set Background** installs a picture as the desktop background. **Image file** takes the same
  kind of path (`png`, `jpg`, `jpeg`, `webp`, `bmp` or `gif`); the optional **Fit** is `fill`, `fit`,
  `center` or `tile` (case does not matter), and empty keeps the fit already set in Settings. The
  change is applied at once and saved in `settings.json`. A wrong extension, a missing file or an
  unknown fit stops the step and leaves the background as it was.

To run either from the Command Palette, save a function that contains the step, for example
`Set Background` with `{f}` as the image. Right-clicking an image file in a lister also offers
**Set as Desktop Background** directly (not for names that contain a `{`).

### 3.21 Leave Out (icons on the desktop)

Any file or folder can sit on the desktop as a permanent icon with a small arrow badge, wherever it really lives:
- Right-click it in a lister and choose **Leave Out on Desktop**. If the clicked entry is part of the selection, the whole selection is left out.
- Or use the **Leave Out** and **Put Away** steps of the Function Editor on the SOURCE selection, so they can be bound to a hotkey.
- **Put Away** is also in the context menu of a left-out icon on the desktop. It only removes the icon; the file is never touched.

Drag a left-out icon to place it: the position is remembered across restarts (icons of the Desktop folder itself are still auto-arranged). A left-out entry whose file disappeared (deleted, or its disk not mounted) shows as a broken icon and stays until you put it away; the desktop only notices a change outside the Desktop folder the next time it reloads (for instance with **Desktop -> Rearrange icons**). The icon menu of a left-out icon offers Open, Put Away and Copy, but not Cut, Rename or Delete, since those would act on the real file somewhere else: do them from a lister. Double click opens a folder in a new lister and a file with its default application. Left-out icons appear on the primary monitor only. Stored in `~/.config/opusdm/leftout.json`.

### 3.22 Program Groups

A group gathers programs, files or folders under one desktop icon.
- Right-click the desktop and choose **New Group...**, then type a name (names must be unique, upper and lower case do not count). The group appears as a folder icon with a small group badge; drag it wherever you like, the position is remembered.
- Drag files or applications from a lister (or the desktop) onto the group icon to add them. Something already in the group is skipped.
- Double click the icon to open the group window. Double click a member to start it: a folder opens in a lister, a `.desktop` file starts that application, a binary or a `#!` script that is executable runs, anything else opens with its default application (the exec bit alone is not trusted: on vfat or SMB mounts every file has it). A launch that fails shows a message. You can also drop files on the open window, and right-click a member and choose **Remove from Group**.
- The icon menu offers **Open**, **Rename Group...** and **Delete Group...**. Deleting a group never touches the files in it. A member whose file disappeared shows as a broken icon until you remove it.

Each group is stored in `~/.config/opusdm/groups/<name>.json`. Groups appear on the primary monitor only.

### 3.23 Hide and Reveal

**Hide All** in the desktop context menu minimizes every lister and hides the panel, the dock and the group windows, leaving only the desktop; the same menu entry now reads **Reveal** and brings everything back. A lister you had already minimized yourself stays minimized after a reveal. Opening a new lister while hidden (**New Lister**) reveals everything too. After a reveal the lister that was the SOURCE is raised again, so SOURCE and DEST stay as they were. A `Hide` step in a `Startup` script also hides the first lister OpusDM opens.

To do it from the keyboard, make a function with the **Hide**, **Reveal** or **Toggle Hide** step (Manage Functions) and bind it in **Edit Hotkeys...** with **Global** ticked: the hotkey then works from any window and toggles the desktop. Not persisted: OpusDM always starts revealed.

### 3.24 Button Banks

A button bank is a small floating window of buttons. Each button has a label
and/or an icon and up to three Functions: one for the left, one for the right
and one for the middle mouse button. Bank windows do not appear in the window
list or the pager.

- **Manage banks:** "Button Banks..." in the desktop menu or in the Command
  Palette. The manager lists the banks (New, Edit, Delete, Open, Close Bank).
- **Edit a bank:** the preview on the left shows the buttons; click one to
  select it. Add, Insert (before the selection), Delete, Copy, Paste, Up and
  Down work on the selected button. The form sets its label, icon file and the
  three Functions, and the bank's name, orientation (Horizontal fills rows,
  Vertical fills columns), columns and Borderless. Save is disabled while
  something is invalid; the reason is shown under the form.
- **Commands:** the Function steps `Open Bank`, `Close Bank`, `Toggle Bank` and
  `Hide Bank` take a bank name, so a hotkey, a user menu entry or a script event
  can drive a bank. `Hide Bank` hides the window but keeps the bank open.
- **Restore:** a bank that was open when the hub quit opens again at the same
  position at the next start. `Hide All` hides the bank windows too.
- **Files:** one JSON file per bank in `~/.config/opusdm/buttons/`. A file that
  cannot be read is copied to `<name>.json.bak` and left alone.

## 4. Keyboard Shortcuts Reference

Every binding below works whenever the file list has the focus. While you are typing in the filter
bar, the path bar or the terminal drawer, typing is just text - only `Esc`, `F1`-`F12` and
the `Ctrl` chords stay active.

### Navigation

| Shortcut | Action |
| :--- | :--- |
| **`Enter`** | Enter the directory, step into the archive, or open the file |
| **`Backspace`** | Go to the parent directory |
| **`~`** | Go to user home directory (`$HOME`) |
| **`Ins`** | Toggle the selection of the cursor row, then step down |
| **`Tab`** | Jump to the Lister holding the opposite role (SOURCE <-> DEST) |
| **`Alt+Left`** / **`Alt+Right`** | Back / forward in this Lister's history |
| *type a name* | Type-to-find, see below |

### Type-to-find

Typing a letter, a digit, `.` or `_` over the file list opens a small search: the cursor jumps to
the first entry whose name starts with what you typed (case is ignored), and the status line shows
`Find: car`. Once it is open any printable character is accepted (`my-file`), `Backspace` deletes
one, `Esc` closes it, and it closes by itself after 1.5 s of inactivity. A character that matches
nothing is ignored. `Enter`, the arrows and the function keys close it and then do their usual job.
There are no single-letter commands in OpusDM.

### Everything else

| Shortcut | Action |
| :--- | :--- |
| **`/`** or **`Ctrl+F`** | Open inline Fuzzy File Filter |
| **`Esc`** | Close filter bar, close terminal drawer, or deselect all items |
| **`Ctrl+P`** / **`Ctrl+Shift+P`** | Open Command Palette HUD |
| **`F3`** | Toggle Quick Look file preview |
| **`Space`** | Quick Look preview (on files) / Calculate recursive size (on directories) |
| **`F4`** or **`` ` ``** | Toggle Quake-style dropdown terminal drawer |
| **`F5`** | Copy selected items from SOURCE to DEST |
| **`F6`** | Move selected items from SOURCE to DEST |
| **`Del`** | Delete selected items permanently |
| **`F2`** | Rename single item / Open Batch Renamer on multiple items |
| **`Ctrl+N`** | Create new directory |
| **`Ctrl+Shift+N`** | Create new blank file |
| **`Ctrl+A`** | Select all items |
| **`Ctrl+Shift+A`** | Reselect the previous selection |
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
