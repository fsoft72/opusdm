# OpusDM - Guida Completa all'Ottimizzazione & Roadmap Evolutiva

Documento di analisi approfondita del codebase di **OpusDM** (l'erede spirituale e moderno di *Directory Opus* su Linux / X11 / GTK4).
L'analisi copre tre pilastri fondamentali:
1. **Aspetti da migliorare nel codice (Code Quality, Affidabilità & Architettura)**
2. **Ottimizzazioni di velocità & I/O (Throughput, Syscall & UI Smoothness)**
3. **Cosa fare per rendere OpusDM FIGHISSIMO (Funzionalità Killer, Estetica & UX)**

---

## Indice
- [1. Aspetti da Migliorare nel Codice](#1-aspetti-da-migliorare-nel-codice)
  - [1.1 Robustezza I/O e Race Conditions (TOCTOU)](#11-robustezza-io-e-race-conditions-toctou)
  - [1.2 Gestione Errori e Feedback Utente](#12-gestione-errori-e-feedback-utente)
  - [1.3 Gestione Connessioni, Reader Threads e Lifecycle dei Lister](#13-gestione-connessioni-reader-threads-e-lifecycle-dei-lister)
  - [1.4 Decoupling e Testing della UI](#14-decoupling-e-testing-della-ui)
  - [1.5 Supporto X11 / Wayland](#15-supporto-x11--wayland)
- [2. Ottimizzazioni di Velocità & Throughput](#2-ottimizzazioni-di-velocit--throughput)
  - [2.1 Zero-Copy & Kernel Fast-Paths (copy_file_range / io_uring / reflinks)](#21-zero-copy--kernel-fast-paths-copy_file_range--io_uring--reflinks)
  - [2.2 Directory Reading & Caching dei Metadati](#22-directory-reading--caching-dei-metadati)
  - [2.3 Throttling degli Eventi IPC di Progresso & Saturazione Canali](#23-throttling-degli-eventi-ipc-di-progresso--saturazione-canali)
  - [2.4 UI Batching & Lazy Column Sorting](#24-ui-batching--lazy-column-sorting)
  - [2.5 Memory Allocation & Buffer Recycling](#25-memory-allocation--buffer-recycling)
- [3. Come Rendere OpusDM FIGHISSIMO 🚀](#3-come-rendere-opusdm-fighissimo-)
  - [3.1 Estetica Amiga Classic & Cyberpunk Moderno (Theme Engine)](#31-estetica-amiga-classic--cyberpunk-moderno-theme-engine)
  - [3.2 Dual/Multi-Pane Tiling Intelligente & Window Snapping](#32-dualmulti-pane-tiling-intelligente--window-snapping)
  - [3.3 File Previewer Istantaneo (Quick Look / F3)](#33-file-previewer-istantaneo-quick-look--f3)
  - [3.4 Fuzzy Finder & Command Palette (Ctrl+P / /)](#34-fuzzy-finder--command-palette-ctrlp--)
  - [3.5 Keybindings Modali / Vim-Mode](#35-keybindings-modali--vim-mode)
  - [3.6 Inotify / File Watcher Real-Time](#36-inotify--file-watcher-real-time)
  - [3.7 Visual Diff, Checksums & Calcolo Dimensioni Cartelle Istantaneo](#37-visual-diff-checksums--calcolo-dimensioni-cartelle-istantaneo)
  - [3.8 Terminale a Scomparsa (Quake-style Dropdown Drawer)](#38-terminale-a-scomparsa-quake-style-dropdown-drawer)
  - [3.9 Batch Rename Potente con Regex & Numerazione](#39-batch-rename-potente-con-regex--numerazione)
  - [3.10 Supporto Archivi Integrato (Virtual FS per Zip, Tar, 7z)](#310-supporto-archivi-integrato-virtual-fs-per-zip-tar-7z)

---

## 1. Aspetti da Migliorare nel Codice

### 1.1 Robustezza I/O e Race Conditions (TOCTOU)
> ✅ **FATTO (parziale)** - commit `6bb4cfd`: create_new(true) atomico + retry TOCTOU.
> Restano aperti: preservazione xattr/ACL/setuid e rollback transazionale del Move cross-filesystem.

* **Controllo Esistenza vs Sovrascrittura**: In [`crates/fsops/src/copy.rs`](crates/fsops/src/copy.rs#L101), viene eseguito `if dst.exists()` prima di chiedere la risoluzione del conflitto o di creare il file. Questa operazione soffre di **Time-Of-Check to Time-Of-Use (TOCTOU)**: un file può essere creato o rimosso da un processo terzo tra il check e la `create`.
  * *Miglioramento*: Utilizzare `OpenOptions::new().write(true).create_new(true)` per rilevare atomicamente se la destinazione esiste già (`ErrorKind::AlreadyExists`), evitando la race condition.
* **Rimozione Directory Ricorsiva (`delete.rs`)**: In [`crates/fsops/src/delete.rs`](crates/fsops/src/delete.rs#L37-L54), `remove_recursive` attraversa la directory via `read_dir` e chiama `symlink_metadata`. Se ci sono file con permessi readonly o link ricorsivi/modificati durante l'eliminazione, la gestione dei retry con `set_permissions` è rudimentale.
  * *Miglioramento*: Passare a un loop robusto su `openat` / `unlinkat` (o utilizzare la crate standardizzata `remove_dir_all` che gestisce correttamente le privilege escalation / symlink races su POSIX).
* **Gestione Attributi Estesi (xattrs) e ACL**: Attualmente `copy.rs` preserva solo `permissions` e `mtime`. Non vengono mantenuti extended attributes (`xattr`), permission bit speciali (setuid/setgid/sticky) in modo completo, né timestamp di accesso (`atime` o birthtime dove supportato).
* **Rollback in caso di fallimento Move**: In [`crates/fsops/src/moveop.rs`](crates/fsops/src/moveop.rs), se il rename `EXDEV` ripiega su `copy_tree(..., true)` e l'operazione fallisce a metà (es. disco pieno), una parte dei file è copiata e non eliminata, mentre un'altra parte è rimasta all'origine, senza un report chiaro di transazionalità.

### 1.2 Gestione Errori e Feedback Utente
> ✅ **FATTO** - commit `8376d24` (errori tipizzati con thiserror + dialog errori scrollabile).

* **Stringhe di Errore Grezze**: Spesso gli errori I/O vengono convertiti direttamente con `.to_string()`, ad esempio in `scan.rs` (`io::Error::other("path outside its source base")`).
  * *Miglioramento*: Adottare `thiserror` (già presente nelle workspace dependencies!) per strutturare gerarchie di errore tipizzate (`CopyError`, `ScanError`, `IpcError`), rendendo possibile per la UI distinguere `PermissionDenied`, `NoSpaceLeft`, `Cancelled`, `NotFound`.
* **Truncation degli Errori in Dialog**: In [`crates/lister/src/actions.rs`](crates/lister/src/actions.rs#L206), solo i primi 20 errori vengono visualizzati in un AlertDialog di testo piatto. Se una copia massiva fallisce per 500 file (es. permessi), l'utente non ha modo di salvare un report o scorrere l'elenco.
  * *Miglioramento*: Fornire una finestra dialog con `gtk::ScrolledWindow`, `gtk::ListView` e un pulsante "Copia log errori negli appunti" o "Esporta log".

### 1.3 Gestione Connessioni, Reader Threads e Lifecycle dei Lister
> ✅ **FATTO** - commit `41472c3` (lifecycle connessioni e gestione id).

* **Thread per Connessione (`server.rs`)**: In [`crates/hub/src/server.rs`](crates/hub/src/server.rs#L180), ogni Lister accetta un thread OS dedicato (`opusdm-reader-{cid}`). Anche se per 5-10 Lister l'overhead è trascurabile, le pipe Unix domain possono essere gestite in modo asincrono o tramite polling non-blocking (ad esempio con `epoll` / `mio` o un runtime asincrono), riducendo le risorse e semplificando la sincronizzazione con un unico event loop.
* **`next_conn` Overflow & Riuso ID**: `next_conn` è un `u64` progressivo. Sebbene l'overflow sia teorico, l'ID non viene riciclato.
* **Reconnection & Crash Recovery**: Se un Lister crasha, il server pulisce lo stato tramite `SIGCHLD` o EOF (`drop_conn`). Tuttavia, se l'Hub muore o viene riavviato, tutti i Lister chiudono immediatamente (`Inbound::HubGone -> window.close()`). Sarebbe eccellente se i Lister potessero rimanere aperti in modalità autonoma e riconnettersi al nuovo Hub quando riparte.

### 1.4 Decoupling e Testing della UI
> ✅ **FATTO** - commit `1c4c10d` (window.rs suddiviso in sottomoduli + test UI).

* **Monolite `window.rs`**: Con oltre 660 righe, `window.rs` gestisce setup GTK, callbacks, costruttori di factory, aggiornamenti CSS al volo, gestione dello stato della selezione e rendering delle colonne.
  * *Miglioramento*: Suddividere in sottomoduli:
    * `columns/`: factory e render per Name, Size, Mode, Date, User, Group.
    * `navigation/`: gestione di breadcrumb, history stack e path entry.
    * `statbar/`: rendering e logica di role cycling del banner D/F/B.
    * `theme/`: generatore CSS per le palette dei colori.
* **Copertura di Test UI**: Attualmente i test coprono solo la parte pura (`model.rs`, `scan.rs`, `frame.rs`). La logica di reazione ai messaggi IPC (`apply_inbound`) e la sincronizzazione della selezione nei reload non hanno harness di test automatizzati headless.

### 1.5 Supporto X11 / Wayland
* **Dipendenza Stretta da X11 nel Dock Hub**: In [`crates/hub/src/dock.rs`](crates/hub/src/dock.rs), il toolbar è vincolato a X11 (`x11rb`, `gdk4_x11`, `_NET_WM_WINDOW_TYPE_DOCK`, `_NET_WM_STRUT_PARTIAL`). Sotto una sessione Wayland pura (es. Sway, Hyprland, GNOME Wayland), `x11_window_id` restituisce `None`, loggando un avviso e lasciando la finestra fluttuante.
  * *Miglioramento*: Aggiungere supporto a `gtk4-layer-shell` per Wayland (wlr-layer-shell protocol), permettendo a OpusDM di funzionare come dock top-panel nativo sia su X11 che su Wayland moderno!

---

## 2. Ottimizzazioni di Velocità & Throughput

### 2.1 Zero-Copy & Kernel Fast-Paths (copy_file_range / io_uring / reflinks)
> ✅ **FATTO** - commit `dcdcb7b` (copy_file_range zero-copy) + worker pool per file piccoli.
> `crates/fsops/src/pool.rs`: `WorkerPool` a dimensione fissa, un thread per core (max 8, override con
> `OPUSDM_COPY_WORKERS`). File <= 4 MiB (`SMALL_FILE_MAX`) vanno al pool, i file grandi restano inline
> per non frammentare l'I/O sequenziale. Directory, symlink e prompt di conflitto restano sul thread
> orchestratore, quindi l'utente non vede mai due dialog insieme.
> Misurato su 20.000 file da 8 KiB: **175 ms -> 62 ms (2.8x)**.

* **Sostituire il buffer loop userspace (`copy_file`)**:
  In [`crates/fsops/src/copy.rs`](crates/fsops/src/copy.rs#L27-L42), i file vengono copiati tramite loop `read()` + `write_all()` con buffer di 256 KiB.
  * Su Linux moderno, la syscall **`copy_file_range(2)`** permette di copiare dati in-kernel tra due file descriptor senza trasferire dati nello spazio utente, e supporta automaticamente i **reflink / CoW (Copy-on-Write)** su filesystem come Btrfs, XFS e ZFS! Una copia di 10 GB su Btrfs diventa istantanea (0 millisecondi) e non occupa spazio su disco aggiuntivo.
  * Su file system diversi (dove `copy_file_range` dà `EXDEV` o fallback), si può usare **`splice(2)`** o `sendfile(2)` per ridurre i cambi di contesto (context switches).
* **Parallelismo nei Trasferimenti Multi-File**:
  Attualmente, anche se la coda supporta fino a `MAX_CONCURRENT = 4` job distinti, ciascun `copy_tree` processa la lista dei file **sequenzialmente** in un unico thread.
  * Quando si copiano migliaia di piccoli file (es. un progetto `node_modules` o `target/`), il throughput è strozzato dalla latenza di seek e metadati su un solo core.
  * *Ottimizzazione*: Utilizzare un worker pool interno (es. `rayon` o thread pool bilanciato) con canali dedicati per parallelizzare la copia dei piccoli file (< 4 MB) mantenendo sequenziali i file enormi per non frammentare l'I/O rotazionale.

### 2.2 Directory Reading & Caching dei Metadati
> ✅ **FATTO** - commit `dffcc9f` (d_type al posto di stat doppia, cache NSS condivisa).

* **Syscall Ridondanti in `read_dir_rows`**:
  In [`crates/lister/src/model.rs`](crates/lister/src/model.rs#L45-L71):
  1. Si chiama `std::fs::symlink_metadata(entry.path())`
  2. Si chiama `std::fs::metadata(entry.path())` (che re-interroga il filesystem seguendo il link)
  * Su cartelle con 50.000 file, questo causa **100.000 syscall `stat`**!
  * *Ottimizzazione*: `entry.file_type()?` ottiene il tipo direttamente dalla voce `d_type` restituita da `getdents64` senza eseguire una `lstat` separata per la maggior parte dei file regolari e directory! La `stat` di follow va fatta esclusivamente se l'elemento è effettivamente un symlink.
* **Name Lookup NSS (`getpwuid_r` / `getgrgid_r`)**:
  In `window.rs`, la cache `name_cache` è allocata per ogni factory di colonna ed è locale alla lista.
  * Se l'utente naviga tra cartelle diverse, la cache viene ripopolata da zero chiamando `libc::getpwuid_r`.
  * *Ottimizzazione*: Spostare la cache `Uid/Gid -> String` a livello globale/thread-safe o condivisa tra i Lister, pre-popolando gli account locali standard (`0=root`, ecc.).

### 2.3 Throttling degli Eventi IPC di Progresso & Saturazione Canali
> ✅ **FATTO** - commit `113022e`, poi reso globale al job (non piu per-file) dal worker pool 2.1.

* **Frequenza di Invio Progresso Byte per Byte**:
  In `copy_file`:
  ```rust
  let mut on_bytes = |n: u64| {
      done_bytes += n;
      let _ = tx.send(Progress::File { ... });
  };
  ```
  Con un buffer da 256 KiB, copiando un file da 10 GB a 1 GB/s (NVMe), vengono inviati **4.000 messaggi `Progress::File` al secondo** attraverso canali mpsc, serializzati in JSON via IPC frame socket, deserializzati dal server e ri-spediti al GTK main thread. Questo satura la CPU per pura serializzazione IPC e fa laggare il thread UI di GTK.
  * *Ottimizzazione*: **Rate-limiting temporale** del progresso. Inviare l'evento di progresso solo se sono passati almeno **50-100 ms** dall'ultimo evento (oppure quando il file corrente è completato).

### 2.4 UI Batching & Lazy Column Sorting
> ✅ **FATTO** - commit `28227e8` (store.splice + totali precalcolati).

* **Popolamento `gio::ListStore`**:
  In [`crates/lister/src/window.rs`](crates/lister/src/window.rs#L481-L489):
  `self.store.remove_all();` seguito da un loop di `self.store.append(&boxed);` per ogni riga.
  * Inserire 20.000 elementi uno per uno via `.append()` emette 20.000 segnali `items-changed` in GTK4, costringendo il `SortListModel` a continui ricalcoli parziali.
  * *Ottimizzazione*: Utilizzare `store.splice(0, old_len, &new_items)` per sostituire l'intero blocco in **una sola operazione atomica**, azzerando il freeze della UI su directory immense.
* **Calcolo Statistiche D/F/B con Iteratore Diretto**:
  In `update_status()`: viene eseguita un'iterazione completa su tutti gli item nel selection model chiamando `self.row_at(i)` e `self.selection.is_selected(i)`.
  * Memorizzare i conteggi totali (directory, file, byte totali) direttamente durante il caricamento in `read_dir_rows`, evitando di ricalcolare la somma di tutti i file a ogni click di selezione.

### 2.5 Memory Allocation & Buffer Recycling
> ✅ **FATTO** - commit `4ce263e` (buffer di copia riciclato per thread).

* **Allocazione Buffer nel Loop di Scrittura**:
  In `copy_file`, `let mut buf = vec![0u8; COPY_BUF];` alloca 256 KiB sullo heap per ogni singolo file. Se si copiano 100.000 piccoli file, ci sono 100.000 allocazioni/deallocazioni di buffer da 256 KiB.
  * *Ottimizzazione*: Riutilizzare lo stesso buffer o un `BufferPool` condiviso tra i thread di lavoro.

---

## 3. Come Rendere OpusDM FIGHISSIMO 🚀

Directory Opus sull'Amiga è leggendario non solo per il dual-pane, ma per la velocità fulminea, l'ergonomia totale della tastiera, la programmabilità e l'estetica rétro-futuristica. Ecco come portare OpusDM a un livello "God Tier":

### 3.1 Estetica Amiga Classic & Cyberpunk Moderno (Theme Engine)
> ✅ **FATTO** - preset temi + icone Nerd Font + indicatori di tipo.
> `crates/ipc/src/theme.rs`: `ThemePreset` con 6 preset (Default, Amiga Workbench 1.3,
> Directory Opus 4/5 Classic, Cyberpunk/Tokyo Night, Phosphor verde e ambra); selezionabili dal
> menu a tendina in Settings, riempiono tutti i colori in un click.
> `ColorSettings` estesa con i colori di selezione (prima erano bloccati sul tema GTK);
> nuovo `Appearance { preset, file_icons, type_indicators, monospace_list, theme_window }`.
> `crates/lister/src/glyphs.rs`: glifi Nerd Font per estensione/tipo e indicatori stile `ls -F`
> (`/ * @ = |`), entrambi opzionali e spenti di default (i glifi richiedono un Nerd Font).
> Vecchi settings.json restano leggibili: i campi mancanti tornano ai default.

* **Preset di Temi Iconici Out-of-the-Box**:
  Oltre al color picker RGB attuale, includere preset con un click:
  * 📼 **Amiga Workbench 1.3**: Blu profondo, arancione ambra, bianco e nero con font bitmap stile Topaz.
  * 💽 **Directory Opus 4/5 Classic**: Il caratteristico grigio/azzurro/nero con pulsanti 3D bevel classici di DOpus.
  * 🕶️ **Cyberpunk / Tokyo Night**: Toni neon viola, ciano per SOURCE, rosa acceso per DEST, sfondi dark-slate traslucidi.
  * 🍏 **Monochrome Phosphor (Green Screen / Amber CRT)**: Look hacker da terminale vintage.
* **Font Monospazio e Icone Nerd Font**:
  * Visualizzare icone file accanto ai nomi (directory, rust, python, video, archivi, audio) usando i glifi UTF-8 di **Nerd Fonts** senza dipendere da pesanti librerie grafiche.
  * Indicatori visivi rapidi per file eseguibili (`*`), link (`@`), socket (`=`), pipe (`|`).

### 3.2 Dual/Multi-Pane Tiling Intelligente & Window Snapping
* **Layout Integrato Split Dual-Pane (in aggiunta alle finestre separate)**:
  * Attualmente i Lister sono finestre GTK separate collegate al dock. Sarebbe micidiale poter premere `F6` o un pulsante "Split" per affiancare due pannelli (Left=Source, Right=Dest) **all'interno della stessa finestra**, o sincronizzarne il posizionamento con snapping automatico sul desktop.
* **Layout Presets**:
  * Tasti di scelta rapida per auto-organizzare i Lister attivi: `Alt+1` (Affiancati 50/50), `Alt+2` (Sovrapposti Alto/Basso), `Alt+3` (3 Lister Source + 2 Dest a griglia).

### 3.3 File Previewer Istantaneo (Quick Look / F3)
> ✅ **FATTO** - commit `d5568dd` (quick look con F3/Spazio).

* **Pannello di Anteprima Rapida o Finestra HUD Fluttuante**:
  * Premendo `Spazio` o `F3` sull'elemento selezionato:
    * **Immagini**: Rendering istantaneo con zoom e metadati risoluzione/canali.
    * **Testo / Codice**: Syntax highlighting immediato (usando ad es. `syntect` o GtkSourceView).
    * **Audio**: Minigiocatore con spettrogramma / waveform e tasto play/pause.
    * **Archivi (.zip, .tar.gz, .rar)**: Visualizzazione immediata della tabella dei contenuti senza decomprimere.
    * **PDF / Markdown**: Rendering formattato.

### 3.4 Fuzzy Finder & Command Palette (Ctrl+P / /)
> ✅ **FATTO** - commit `ed1e3f6` (fuzzy finder + command palette).

* **Filtro Istantaneo nella Vista Corrente**:
  * Premendo `/` o digitando direttamente mentre la lista ha il focus: filtro fuzzy istantaneo stile fzf (evidenziando i caratteri matchati) senza dover usare il mouse.
* **Command Palette globale (`Ctrl+Shift+P`)**:
  * Accesso a tutte le funzioni: "Invert Selection", "Calculate Directory Sizes", "Select All by Pattern (*.rs)", "Open Terminal Here", "Git Diff", ecc.

### 3.5 Keybindings Modali / Vim-Mode
> ✅ **FATTO** - `crates/lister/src/keys.rs`: tabella di binding pura (`resolve`) separata da GTK e
> coperta da test, `window.rs` esegue solo l'azione risolta.
> `j`/`k` su/giu, `g`/`G` prima/ultima riga, `l`/`Enter` entra, `h`/`Backspace` esce, `~` home,
> `Ins` seleziona e avanza, `Tab` salta al Lister con il ruolo opposto (nuovo `ToHub::PresentLister`),
> `c`/`m`/`d` copia/sposta/elimina.
> Nota: `c`, `m`, `Del`, `Backspace` e `~` erano documentati ma **non** erano mai stati collegati;
> ora lo sono. `` ` `` apre il terminale e `~` va alla home (prima entrambi aprivano il terminale).
> Le lettere singole sono inattive mentre il focus e in un campo di testo.

* **Controllo Totale da Tastiera (Zero Mouse Needed)**:
  * `j` / `k` (o frecce) per navigare.
  * `h` / `l` per uscire/entrare nelle directory (o freccia sinistra/destra).
  * `Space` o `Ins` per selezionare/deselezionare e passare al successivo.
  * `Tab` per scambiare il focus tra Lister Source e Lister Dest.
  * `c` per Copiare (Source -> Dest), `m` per Muovere, `d` per Eliminare.
  * `*` per invertire la selezione; `+` / `-` per selezionare/deselezionare per glob (es. `*.png`).

### 3.6 Inotify / File Watcher Real-Time
* **Aggiornamento Automatico senza Premere Refresh**:
  * Usare `notify` o chiamate dirette a Linux `inotify` per osservare la directory aperta nel Lister.
  * Se un terminale, un editor o un download scrive o crea un file, il Lister si aggiorna in millisecondi in tempo reale senza dover ricaricare a mano.

### 3.7 Visual Diff, Checksums & Calcolo Dimensioni Cartelle Istantaneo
> ✅ **FATTO** - commit `63f7f53` (feat: visual dir compare, checksums, fast folder sizing)

* **Calcolo Spazio Cartelle Rapido (`Space` su Directory)**:
  * Invece di mostrare stringa vuota nella colonna Size per le cartelle, un click o shortcut avvia un calcolo in background che mostra la dimensione totale (es. `4.2G`) con una mini barra di occupazione.
* **Calcolo Checksum On-the-Fly**:
  * Colonna o tooltip con MD5/SHA256 calcolato in streaming.
* **Tool "Sync / Compare Dirs"**:
  * Un pulsante "Compare" che confronta il Lister SOURCE con il Lister DEST ed evidenzia automaticamente con colori differenti:
    * File presenti solo a sinistra (Nuovi)
    * File più recenti
    * File con contenuto identico o modificato

### 3.8 Terminale a Scomparsa (Quake-style Dropdown Drawer)
> ✅ **FATTO** - commit `45d2588` (terminale embedded quake-style).

* Premendo il tasto tilde `` ` `` o `F4`, compare un terminale embedded (`vte-4`) nella parte inferiore del Lister attivo, già aperto nel percorso corrente (`$CWD`).
* Quando si naviga nel Lister, la shell sincronizza opzionalmente il suo percorso, o viceversa quando si esegue `cd` nel terminale, il Lister naviga in quella cartella.

### 3.9 Batch Rename Potente con Regex & Numerazione
> ✅ **FATTO** - commit `c0b2e33` (feat: batch renamer con regex e live preview)

* Uno dei punti di forza di Directory Opus originale è il rinominatore batch:
  * Interfaccia a due colonne (Nome Attuale -> Nuovo Nome con preview in tempo reale).
  * Supporto per:
    * Regex search & replace.
    * Numerazione automatica con padding zero (`file_001.jpg`, `file_002.jpg`).
    * Cambio maiuscole/minuscole (snake_case, kebab-case, CamelCase).
    * Inserimento tag EXIF (data scatto per foto) o tag ID3 per MP3.

### 3.10 Supporto Archivi Integrato (Virtual FS per Zip, Tar, 7z)
> ✅ **FATTO** - commit `1ed0bea` (feat: virtual archive navigation e selective extraction)

* Trattare archivi `.zip`, `.tar.gz`, `.tar.zst` come se fossero normali directory:
  * Doppio click per "entrarci" ed esplorarne il contenuto.
  * Drag & Drop o "Copy" da dentro l'archivio verso il Lister DEST per estrarre direttamente i singoli file selezionati.

---

## Tabella di Riepilogo e Priorità Suggerite

| Categoria | Intervento | Impatto | Complessità | Priorità Consigliata |
|---|---|---|---|---|
| **Velocità** | `copy_file_range` / reflink CoW per Copy pass | 🔥 Enorme | Bassa | ✅ **FATTO** (`dcdcb7b`) |
| **Velocità** | Worker pool parallelo per file piccoli (< 4 MiB) | 🔥 Alto | Media | ✅ **FATTO** (2.8x su 20k file) |
| **Velocità** | Throttling eventi di progresso IPC (50ms) | 🔥 Alto | Bassa | ✅ **FATTO** (`113022e`) |
| **Velocità** | Syscall reduction su `read_dir_rows` (`d_type`) | 🔥 Alto | Bassa | ✅ **FATTO** (`dffcc9f`) |
| **Velocità** | Batching `store.splice` per ListStore GTK4 | 🔥 Alto | Media | ✅ **FATTO** (`28227e8`) |
| **Codice** | Errori tipizzati con `thiserror` | 🛡️ Stabilità | Media | ✅ **FATTO** (`8376d24`) |
| **Codice** | Supporto Wayland (wlr-layer-shell) oltre a X11 | 🌐 Portabilità | Media | **Media** |
| **Codice** | Fix TOCTOU su creazione destinazione | 🛡️ Stabilità | Bassa | ✅ **FATTO** (`6bb4cfd`) |
| **Fighissimo** | Preset Temi Vintage (Amiga, DOpus 4, Cyberpunk) | ✨ Wow | Bassa | ✅ **FATTO** |
| **Fighissimo** | Icone Nerd Font + indicatori tipo (`/ * @ = \|`) | ✨ Wow | Bassa | ✅ **FATTO** |
| **Fighissimo** | Quick Look / Previewer Istantaneo (Spazio/F3) | 🚀 Produttività | Media | ✅ **FATTO** (`d5568dd`) |
| **Fighissimo** | File Watcher `inotify` (auto-refresh reattivo) | 🚀 Produttività | Media | **Alta** |
| **Fighissimo** | Navigazione Tastiera / Vim Mode (`j`,`k`,`h`,`l`, Tab) | ⌨️ UX | Media | ✅ **FATTO** |
| **Fighissimo** | Terminale embedded a scomparsa | 🚀 Wow | Media | ✅ **FATTO** (`45d2588`) |
| **Fighissimo** | Batch Rename avanzato con live preview | 🛠️ Funzionale | Media | ✅ **FATTO** (`c0b2e33`) |
| **Fighissimo** | Dir Sync / Compare interattivo Source <-> Dest | 🛠️ Funzionale | Media | ✅ **FATTO** (`63f7f53`) |
| **Fighissimo** | Virtual FS archivi (zip, tar) + estrazione selettiva | 🛠️ Funzionale | Media | ✅ **FATTO** (`1ed0bea`) |
