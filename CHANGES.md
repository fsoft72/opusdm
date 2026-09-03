# Changes

## 2026-09-03

- OPTIMIZE 3.6 - Auto-refresh in tempo reale con inotify. Nuovo `crates/lister/src/watcher.rs`: inotify diretto tramite `libc`, senza aggiungere dipendenze. Un thread per Lister possiede il file descriptor e blocca su `poll`; la directory osservata viene sostituita ad ogni navigazione.
- Le raffiche di eventi vengono accorpate (debounce 250 ms, tetto di 2 s): un `cargo build` che scrive migliaia di file provoca un solo reload invece di uno per file. Le notifiche raggiungono il thread GTK su un `async_channel`, lo stesso schema gia usato dal reader dell'hub.
- Nuovo blocco `Behavior { auto_refresh, auto_refresh_debounce_ms }` in `Settings`, con interruttore nella sezione BEHAVIOUR del dialogo Settings. Acceso di default. `debounce_ms()` limita il valore a 50-5000 ms, cosi un settings.json modificato a mano non puo far girare a vuoto il watcher ne farlo sembrare bloccato.
- I path virtuali degli archivi non vengono osservati. Se il kernel rifiuta l'istanza inotify (limite di watch raggiunto) il Lister lo segnala su stderr e continua a funzionare senza auto-refresh.
- Il filtro attivo viene salvato e ripristinato attorno al reload (testo, posizione del cursore e focus): `reload()` passa da `navigate_to()`, che azzera la barra del filtro, quindi senza questo l'auto-refresh cancellava quello che l'utente stava digitando.
- Test: parsing del buffer inotify grezzo (compresa una coda troncata) piu cinque test di integrazione sul watcher vero - creazione file, accorpamento di 200 scritture, eventi di altre directory ignorati, `unwatch`, e sostituzione della directory osservata.
- OPTIMIZE 3.5 - Keybindings modali (vim mode). Nuovo `crates/lister/src/keys.rs`: la tabella dei binding e una funzione pura `resolve(key, ctrl, shift, focus) -> Option<Action>`, testabile senza GTK; `window.rs` si limita a eseguire l'azione risolta. La chiusura inline di ~130 righe che gestiva i tasti e sparita.
- Nuovi tasti: `j`/`k` giu/su, `g`/`G` prima/ultima riga, `l` o `Enter` entra (directory, archivio virtuale o apertura file), `h` o `Backspace` risale al parent, `~` va alla home, `Ins` inverte la selezione della riga sotto il cursore e avanza, `Tab` porta il focus sul Lister con il ruolo opposto (SOURCE <-> DEST).
- Bug preesistente corretto: `c`, `m`, `Del`, `Backspace` e `~` erano elencati nella palette e in `docs/usage.md` ma non erano mai stati collegati a nessun handler. Ora funzionano davvero, insieme a `d` come alias di `Del`.
- `` ` `` apre il terminale e `~` va alla home: prima entrambi i keyval aprivano il terminale, rendendo irraggiungibile la scorciatoia per la home documentata.
- Le lettere singole non rubano piu i caratteri ai campi di testo: il chiamante passa il `Focus` (lista o campo editabile) e in modalita editing restano attivi solo `Esc`, i tasti funzione e le combinazioni `Ctrl`.
- Nuovo cursore di tastiera nel Lister (`Ui.cursor`), sincronizzato con il mouse tramite `selection-changed` e azzerato ad ogni cambio directory. Lo scorrimento passa dall'azione `list.scroll-to-item` di `GtkListBase` perche `ColumnView::scroll_to` richiede GTK 4.12 mentre il progetto e fermo a 4.10.
- Nuovo messaggio `ToHub::PresentLister(ListerId)` gestito dal server per alzare la finestra di un altro Lister.
- OPTIMIZE 3.1 - Theme engine con preset. Nuovo `crates/ipc/src/theme.rs` con `ThemePreset`: Default (light), Amiga Workbench 1.3, Directory Opus 4/5 Classic, Cyberpunk/Tokyo Night, Monochrome Phosphor verde e ambra. Il menu a tendina nella nuova sezione THEME del dialogo Settings riempie tutti i colori in un click; modificando poi un colore a mano il preset memorizzato diventa `custom`.
- `ColorSettings` estesa con `selected_bg`/`selected_fg`: la selezione non e piu inchiodata ai colori del tema GTK. Nuovo blocco `Appearance { preset, file_icons, type_indicators, monospace_list, theme_window }`. Entrambe le struct usano `#[serde(default)]`, quindi un settings.json scritto da una versione precedente continua a caricarsi e i campi mancanti tornano ai default (coperto da test).
- Nuovo `crates/lister/src/glyphs.rs` (puro, senza GTK): glifo Nerd Font per riga scelto per tipo di entry e poi per estensione (rust, python, immagini, audio, video, archivi, shell, ...) e indicatore di tipo stile `ls -F` (`/` directory, `*` eseguibile, `@` symlink, `=` socket, `|` fifo). Entrambi opzionali e spenti di default: i glifi richiedono un Nerd Font installato, altrimenti si vedono quadrati vuoti.
- La CSS del Lister ora dipinge esplicitamente lo sfondo della lista, altrimenti con un preset scuro l'area sotto l'ultima riga restava del colore chiaro del tema GTK. Opzione "Apply theme colours to the whole window" (spenta di default) che estende i colori anche alla cornice ridefinendo `@theme_bg_color`/`@theme_fg_color`.
- Nuovo test che fa analizzare ogni foglio di stile generato dal vero parser CSS di GTK: un selettore sbagliato verrebbe altrimenti scartato in silenzio a runtime. Si salta da solo se non c'e un display.
- OPTIMIZE 2.1 - Copia parallela dei file piccoli. Nuovo modulo `crates/fsops/src/pool.rs` con un `WorkerPool` a dimensione fissa (un thread per core, massimo 8, override via la variabile d'ambiente `OPUSDM_COPY_WORKERS`). I file fino a `SMALL_FILE_MAX` (4 MiB) vengono accodati al pool; i file piu grandi restano sul thread orchestratore per non frammentare l'I/O sequenziale.
- `copy_tree` riscritta attorno a `CopyShared`: contatori atomici (copied/skipped/done_bytes), lista errori sotto `Mutex`, flag sticky `overwrite_all`/`skip_all` e un `conflict_lock` che serializza i prompt, cosi l'utente non vede mai due dialog di conflitto insieme. Directory, symlink e decisioni di conflitto restano sequenziali.
- Il throttling degli eventi `Progress::File` e ora globale al job (50 ms) invece che per singolo file: copiare 100.000 file non genera piu 100.000 eventi IPC forzati.
- `copy_tree_with_workers` interna permette ai test di esercitare sia il percorso sequenziale sia quello con pool; nuovi test su file misti grandi/piccoli, invarianza del risultato al variare dei worker e throttling del progresso.
- Misura su 20.000 file da 8 KiB: 175 ms con 1 worker, 62 ms con 8 (2.8x).

- SVG Icon Toolbar (Directory Opus 5 style): I pulsanti testuali della toolbar e i controlli di navigazione (Home, Parent, Pin) sono stati sostituiti con icone vettoriali SVG ad alta definizione tratte dal set Tabler Icons (salvate in `assets/icons/` ed incorporate a tempo di compilazione senza dipendenze a runtime). Zero emoji.
- Nuovi bottoni icone con tooltip descrittivi: Copy (`copy.svg`), Move (`file-arrow-right.svg`), Delete (`trash.svg`), MakeDir (`folder-plus.svg`), NewFile (`file-plus.svg`), Rename (`edit.svg`), Hidden toggle (`eye.svg` / `eye-off.svg`), Home (`home.svg`), Parent (`arrow-up.svg`), Pin (`pin.svg`).
- Modulo `crates/lister/src/icons.rs`: helper per istanziare `GtkImage` e bottoni GTK4 `flat` con icone SVG scalate nitidamente.

## 2026-09-01

- Lister stat bar: SOURCE/DEST badge is now clickable, cycles SOURCE -> DEST -> NONE (ToHub::SetRole). Pin button (was "Lock" in the button row) moved to the stat bar, left of the badge; same LockRole/UnlockRole semantics. Removed the separate Source/Dest toggle buttons from the button row - role is set by clicking the badge now, not two extra buttons.
- Lister: "Hidden" toggle button in the button row, off by default; filters dotfile rows out of navigate_to(). Ui.show_hidden: Cell<bool>.
- Lister layout redesign: top stat bar (D/F/B counts + SOURCE/DEST/lock), button row, file list (Name/Size/Date/User/Group/Access), bottom navigation (home/parent/path).
- model.rs: Row extended with uid, gid, mode fields; format_user/format_group/format_access functions added.
- window.rs: rebuild_window refactored for new layout; add_column expanded for new column types (User, Group, Access with monospace font).
- update_status() now computes directory/file/byte counts (D/F/B format).
- Role-based stat bar coloring: SOURCE=green, DEST=light red; CSS classes toggle in update_status().
- Hub settings: new settings.rs module (load/save ~/.config/opusdm/settings.json); Settings button opens dialog with SOURCE/DEST color picker (preview box clickable for ColorDialog, hex input).
- IPC: ToLister::Welcome extended with source_color/dest_color fields; new ColorUpdate message to push color changes to connected Listers.
- Server: HubCmd::UpdateColors broadcasts ColorUpdate to all registered Listers when settings saved.
- Lister: receives colors from Welcome + ColorUpdate; applies them dynamically via CSS; update_css_for_colors() generates role-based CSS at runtime.
- Settings model restructured for growth: `opusdm_ipc::Settings { colors: ColorSettings }` is now the single struct used as the wire format (Welcome/SettingsUpdate), the on-disk JSON shape, and the hub's in-memory state - one place to extend when a new setting is added, no more per-field IPC messages. `ColorUpdate` renamed `SettingsUpdate`, carries the whole `Settings`. `HubCmd::UpdateColors` renamed `UpdateSettings`.
- ColorSettings expanded to 4 categories x background+text: source_bg/fg, dest_bg/fg, dir_bg/fg, file_bg/fg. Directory/file rows in the Lister's file list now get their own CSS classes (row-dir/row-file) applied per cell in every column.
- Hub settings dialog: 4 sections (SOURCE/DEST/DIRECTORY/FILE), each with Background + Text color rows; scrollable.

## 2026-08-31

- Task 1: workspace scaffold, opusdm-ipc frame module (length-prefixed JSON frames).

- Task 2: opusdm-ipc protocol enums (ToHub/ToLister), socket path resolution, probe_live_hub.
- Task 3: opusdm-fsops CancelToken, JobSpec/Progress, scan().
- Task 4: opusdm-fsops copy_tree (mode/mtime preserved, conflict + cancel), run_job.
- Task 5: opusdm-fsops run_move, run_delete, make_entry. fsops crate complete.
- Task 6: opusdm-hub crate skeleton, apply_role_event() role machine.
- Task 7: opusdm-hub JobEngine (validate, MAX_CONCURRENT=4, map_progress).
- Task 8: opusdm-hub server.rs - accept loop, owner loop, HubEvent/HubCmd bridge.
- Task 9: opusdm-hub startup - acquire_socket, spawn_lister, install_sigchld; headless main.
- Task 11: opusdm-lister crate, Client::connect + reader thread, env guard in main.
- Task 12: opusdm-lister model.rs (pure): rows, format_size, format_mtime, dir_first_cmp.
- Task 16 (partial): hub lib.rs + integration_copy smoke test (passes), run-x11.sh launches opusdm-hub, QA-checklist.md. GUI tasks 10/13/14/15 pending libgtk-4-dev.
- Task 10: opusdm-hub toolbar.rs GTK window (New Lister, SRC/DST indicators, job rows, Quit); main runs the GTK app. gtk4 pinned to 0.10 + v4_10 (0.11 needs rustc 1.92).
- Task 13: opusdm-lister window.rs - ColumnView (BoxedAnyObject rows), path bar, sort buttons, navigation, focus, status line, inbound pump. actions/dnd stubs.
- Task 14: opusdm-lister actions.rs - Copy/Move/Delete/MakeDir/NewFile/Rename, role toggles, conflict dialog, error dialog. Hub now resolves Copy/Move dest_dir from the DEST Lister path (server.rs); resolve_copy_target is a precondition check.
- Task 15: opusdm-lister dnd.rs - text/uri-list drag source + drop target (Shift=Move). RunJob.dest_dir is now Option: None=button Copy/Move (hub resolves), Some=explicit (DnD/Delete/MakeDir/NewFile).
- Task 16: release build verified; GUI smoke (hub spawns Lister at $HOME, single-instance handoff works); integration_copy passes. v1 feature-complete.
- Post-v1: hub toolbar is now a top-docked X11 panel - undecorated, full monitor width, _NET_WM_WINDOW_TYPE_DOCK + _NET_WM_STRUT_PARTIAL (top=42) so Listers tile below it. New crates/hub/src/dock.rs; deps gdk4-x11, x11rb.
- Fix: openbox was re-centering the DOCK panel. Now sets WM_NORMAL_HINTS user-specified position (0,0) + re-asserts geometry on map. Panel sits flush top-left, full width.
- Feature: Lock button disabled unless the Lister holds SOURCE or DEST.
- Feature: multiple DEST Listers. Role model reworked to a per-Lister map (RoleState.source()/dests()); a locked DEST persists, one unlocked DEST is transient. Button Copy/Move fans out to every DEST (one job per destination). Wire: ToLister RoleChanged/Welcome carry dests: Vec<ListerId>.
