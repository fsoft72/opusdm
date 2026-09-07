# IconView - componente vista a icone riutilizzabile

Data: 2026-09-07
Stato: design approvato, in attesa di piano di implementazione

## Obiettivo

Un componente che mostra file e cartelle come icone, riutilizzabile in due
contesti:

- il **Lister** in modalita icona (in alternativa alla vista dettagli a
  `ColumnView` gia esistente);
- la **finestra desktop** dell'hub (oggi mostra solo una `Label` "OpusDM"
  centrata).

Caratteristiche richieste:

- spostamento icone tramite drag and drop;
- preview di immagini e PDF come icona, thumbnail per i video.

## Decisioni di alto livello

| Ambito        | Scelta |
|---------------|--------|
| Collocazione  | nuovo crate `crates/iconview`, nome pacchetto `opusdm_iconview` |
| Layout        | posizione libera sul desktop, griglia auto-flow nel Lister |
| Thumbnail     | tool esterni (`pdftoppm`/`pdftocairo`, `ffmpegthumbnailer`/`ffmpeg`) + cache freedesktop `~/.cache/thumbnails/` |
| Persistenza   | file per-directory nascosto `.opusdm-layout` |
| Dir desktop   | `$XDG_DESKTOP_DIR` con fallback `~/Desktop`, altrimenti area vuota |
| Confini       | il componente e pura vista: emette segnali, non tocca il filesystem, non lancia job |
| Widget        | approccio "C": un solo widget su `gtk::Fixed` per entrambe le modalita, con un trait interno `Arrange` che produce le posizioni |

### Perche l'approccio "C"

Alternative scartate:

- **A** (`gtk::Widget` custom + `LayoutManager` intercambiabile): implementazione
  davvero unica e virtualizzabile, ma molto boilerplate GObject in gtk4-rs
  (subclass di `Widget` + `LayoutManager` + `LayoutChild`).
- **B** (wrapper su `gtk::GridView` per il Lister + `gtk::Fixed` per il desktop):
  poco codice per la meta Lister ma due percorsi di interazione divergenti; il
  contenitore non e condiviso, deriva nel tempo. Scarta proprio il codice che si
  vuole unificare.

"C" da un unico percorso di interazione (obiettivo "riutilizzabile"), poco
boilerplate, e il seam `Arrange` permette di migrare al `LayoutManager` di "A"
se un domani serve la virtualizzazione. Il costo accettato: nessuna
virtualizzazione (una directory da 10k file crea 10k widget), scala gia
patologica in vista a icone con thumbnail in qualunque file manager.

## Struttura del crate

`crates/iconview`, aggiunto ai `members` del workspace. Dipendenze: `gtk4`,
`serde`/`serde_json`, `dirs`, `libc` (watch inotify ridotto). Non dipende da
`opusdm-lister` ne da `opusdm-hub`.

| File             | Responsabilita |
|------------------|----------------|
| `lib.rs`         | re-export di `IconField`, `IconEntry`, `IconKind`, `LayoutMode`, `ThumbSize` |
| `field.rs`       | il widget `IconField` (subclass di `gtk::Widget` via `glib::wrapper!`, contenitore = un `gtk::Fixed` interno in uno `ScrolledWindow`): stato, segnali, controller di interazione |
| `item.rs`        | `IconItem`: widget foglia (immagine + etichetta), CSS class `icon-item` / stato `selected` / stato `cursor` |
| `arrange.rs`     | trait `Arrange` + `GridArrange` + `FreeArrange`; matematica pura, GTK-free, testabile |
| `dnd.rs`         | `paths_to_uri_list` / `uris_to_paths`, spostate da `crates/lister/src/dnd.rs` |
| `glyphs.rs`      | glyph Nerd Font per tipo/estensione, spostato da `crates/lister/src/glyphs.rs` |
| `scan.rs`        | `read_entries(dir, show_hidden) -> io::Result<Vec<IconEntry>>`, GTK-free |
| `thumbs.rs`      | (fase 2) servizio thumbnail asincrono + cache freedesktop |
| `layout_file.rs` | (fase 3) lettura/scrittura di `.opusdm-layout`, pura e testabile |

## API pubblica

```rust
pub enum IconKind { Dir, File, Symlink, Broken }

pub enum LayoutMode { Grid, Free }

pub enum ThumbSize { Normal, Large } // 128 / 256 px freedesktop

pub struct IconEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: IconKind,
    pub mode: u32,           // per glyph exec/socket/pipe
    pub ext: Option<String>, // estensione lowercase, per glyph e thumbnail
}

impl IconField {
    pub fn new() -> Self;
    pub fn set_entries(&self, entries: &[IconEntry]); // rebuild diffabile
    pub fn set_mode(&self, mode: LayoutMode);
    pub fn set_icon_size(&self, px: i32);             // es. 48 / 64 / 96 / 128
    pub fn set_layout_dir(&self, dir: Option<&Path>); // fase 3: sorgente di .opusdm-layout
    pub fn selected_indices(&self) -> Vec<usize>;
    pub fn selected_paths(&self) -> Vec<PathBuf>;
}
```

### Segnali

| Segnale             | Payload                          | Quando |
|---------------------|----------------------------------|--------|
| `activate`          | `u32` indice                     | doppio click / Invio su un'icona |
| `selection-changed` | -                                | dopo ogni variazione di selezione |
| `rename-requested`  | `u32`, `String` nuovo nome       | fine editing inline dell'etichetta (F2 / click lento) |
| `zoom-requested`    | `i32` delta                      | Ctrl+scroll sulla vista |
| `context-menu`      | `i32` indice o `-1`, `f64`, `f64`| tasto destro (su item o area vuota) |
| `drop-uris`         | `Vec<String>` uri-list, `i32` target o `-1`, `bool` shift | drop di file sulla vista |
| `move-icons`        | -                                | fine drag di riposizionamento in modalita Free |

Il componente non legge la directory, non fa watching, non lancia job: riceve
`IconEntry`, emette segnali, il consumatore fa il resto.

## Il seam `Arrange`

`arrange.rs` e GTK-free. Input: dimensioni degli item e larghezza del viewport.
Output: posizioni assolute in pixel dentro il `Fixed`, piu la dimensione
virtuale totale, che `IconField` setta come `size_request` del `Fixed` cosi lo
`ScrolledWindow` sa quanto scrollare.

```rust
pub struct ItemBox { pub w: f64, pub h: f64 }
pub struct Placement { pub positions: Vec<(f64, f64)>, pub virtual_w: f64, pub virtual_h: f64 }

pub trait Arrange {
    fn place(&self, items: &[ItemBox], viewport_w: f64) -> Placement;
}
```

### GridArrange (Lister)

- cella = `max(icon_size + padding, cap larghezza etichetta)` in larghezza,
  `icon_size + altezza etichetta` in altezza;
- colonne = `floor(viewport_w / cell_w)`, minimo 1;
- gli item riempiono da sinistra a destra, poi vanno a capo, nell'ordine in cui
  `set_entries` li ha ricevuti (il Lister li passa gia ordinati secondo il sort
  attivo, come fa oggi col `ColumnView` tramite `dir_first_cmp`);
- re-flow su cambio di `viewport_w`: `IconField` ascolta il resize e richiama
  `place`.

### FreeArrange (desktop)

- riceve una `HashMap<usize, (f64, f64)>` di posizioni note (da `.opusdm-layout`,
  fase 3; vuota in fase 1);
- item con posizione nota: vanno li;
- item senza posizione (file nuovi): slot libero scandendo una griglia implicita
  dall'alto-sinistra, evitando le celle gia occupate;
- `virtual_w/h` = bounding box di tutte le posizioni + margine, con minimo pari
  al viewport (sul desktop il `Fixed` riempie sempre lo schermo).

`IconField::set_mode` scambia l'implementazione di `Arrange` e rifa un `place`.
In fase 1 il desktop usa `FreeArrange` con mappa vuota: si comporta come una
griglia finche non si trascina qualcosa.

Il drop di un'icona in modalita Free salva la posizione grezza in pixel, senza
snap a griglia. Un eventuale "Align to grid" e fuori scope.

## Interazione (tutta in `field.rs`, scritta una volta sola)

### Stato di selezione

`RefCell<HashSet<usize>>` piu un `anchor: Cell<Option<usize>>` per lo
shift-range. Ogni mutazione applica/rimuove la CSS class `selected` sugli
`IconItem` toccati ed emette `selection-changed`. Un indice `cursor` distinto
dalla selezione porta la CSS class `cursor`.

### Click (`GestureClick`, tasto sinistro)

- click singolo su item, nessun modificatore: selezione esclusiva + anchor;
- `Ctrl`: toggle del singolo, anchor = quello;
- `Shift`: seleziona il range anchor..item;
- click su area vuota senza modificatori: deseleziona tutto;
- doppio click su item: emette `activate(index)`.

### Rubber-band (`GestureDrag` sul `Fixed`, parte solo da area vuota)

Durante il drag `IconField` disegna il rettangolo di selezione in `snapshot()`
(fill semitrasparente + bordo, colori da CSS). A ogni update ricalcola
l'intersezione col bounding box di ogni item. `Ctrl` durante il rubber-band =
additivo rispetto alla selezione preesistente, altrimenti la rimpiazza.

### Tastiera (`EventControllerKey`)

- frecce: spostano il cursore; in `GridArrange` su/giu usano il numero di
  colonne corrente, in `FreeArrange` vanno all'item piu vicino nella direzione
  data;
- `Shift`+frecce: estende il range dall'anchor;
- `Ctrl`+frecce: muove il solo cursore senza toccare la selezione;
- `Space`: toggle dell'item sotto il cursore;
- `Invio`: `activate` sul cursore;
- `Ctrl+A`: seleziona tutto;
- `Esc`: deseleziona;
- `F2`: entra in editing inline sull'etichetta del cursore.

### Editing inline (`item.rs`)

L'etichetta e uno `Stack` `Label` <-> `Entry`. Su F2 o click-lento (secondo
`GestureClick` con delay) passa a `Entry` selezionando il basename senza
estensione. `Invio`/focus-out con testo cambiato: `rename-requested(index,
nuovo_nome)`. `Esc` annulla. Il componente non tocca il filesystem: aspetta che
il consumatore ricarichi via `set_entries`.

### Zoom

`EventControllerScroll` con `Ctrl` premuto: emette `zoom-requested(delta)`. Il
consumatore decide la nuova size e richiama `set_icon_size`.

### Context menu

`GestureClick` tasto destro: se il click e su un item non selezionato, lo
seleziona in esclusiva prima; poi emette `context-menu(index_o_-1, x, y)`. Il
menu vero lo costruisce il consumatore.

## Drag and drop

Due ruoli distinti sullo stesso widget.

### Riposizionamento icone (solo modalita Free, interno alla vista)

`GestureDrag` sugli `IconItem`: partendo da un item selezionato (o
selezionandolo al primo movimento se non lo era), il drag muove tutte le icone
selezionate. Durante il trascinamento `IconField` aggiorna le posizioni nel
`Fixed` in tempo reale con un offset comune. Al rilascio: clampa le posizioni
dentro `virtual_w/h`, aggiorna la mappa interna delle posizioni, emette
`move-icons`. Nessun `DragSource` GDK qui: e movimento locale, cosi non fa
conflitto col DnD tra finestre. Soglia di avvio drag di circa 4 px.

### DnD tra finestre / verso l'esterno (entrambe le modalita)

Logica riusata da `crates/lister/src/dnd.rs`, spostata in
`opusdm_iconview::dnd`:

- `DragSource` sul `Fixed`: `connect_prepare` legge `selected_paths()`; se vuoto
  ritorna `None`, altrimenti `ContentProvider::for_bytes("text/uri-list", ...)`
  con `paths_to_uri_list`. Azioni `COPY | MOVE`. In modalita Free, se il gesture
  di riposizionamento interno e gia attivo, il `DragSource` non parte (guardia
  con un flag).
- `DropTarget` (`STRING`, `COPY | MOVE`) sul `Fixed`: su drop fa `uris_to_paths`,
  calcola l'indice target (item sotto il puntatore, o `-1` se area vuota) ed
  emette `drop-uris(uris, target, shift)`. Il componente non lancia il job: e il
  consumatore che nel handler decide `Op::Copy`/`Op::Move` (in base a `shift`) e
  chiama `ToHub::RunJob`.

`crates/lister/src/dnd.rs` si riduce a un thin adapter `attach(ui, field)` che
collega i segnali `drop-uris` del `IconField` a `ToHub::RunJob`, riusando le
funzioni di parsing dal nuovo crate. I test di roundtrip uri-list migrano col
codice.

## Servizio thumbnail e cache (fase 2)

`thumbs.rs`. Dato un `path` + `ThumbSize`, restituisce una `gdk::Texture` in modo
asincrono senza mai bloccare il main loop.

### Standard freedesktop (`~/.cache/thumbnails/`)

- directory `normal/` (128 px) e `large/` (256 px); `ThumbSize` mappata sulla
  piu vicina, scaling finale lato `IconItem`;
- nome file = `md5(uri)` con `uri = "file://" + path_assoluto_percent_encoded`,
  estensione `.png`;
- validita: il PNG porta i tag `Thumb::URI` e `Thumb::MTime`; valido solo se
  `Thumb::MTime` == mtime attuale del sorgente, altrimenti si rigenera;
- niente scrittura in cache per file gia piccoli: si carica il sorgente diretto.

### Generatori (shell out su thread pool, mai sul main)

- immagini: nessun tool esterno, `gdk::Texture::from_file` + scale via
  `gdk_pixbuf`;
- PDF: `pdftoppm -png -singlefile -scale-to <px> <file> <tmp>`; fallback
  `pdftocairo`;
- video: `ffmpegthumbnailer -i <file> -o <tmp> -s <px>`; fallback
  `ffmpeg -ss 00:00:01 -i <file> -frames:v 1 -vf scale=<px>:-1 <tmp>`;
- il tool scelto viene rilevato una volta (`which`) e memoizzato; se nessuno e
  disponibile per un tipo, quel tipo resta al glyph, senza errore visibile;
- timeout dei generatori: `Command` con kill dopo N secondi, cosi un file
  malformato non blocca un worker per sempre.

### Flusso asincrono

1. `IconItem` parte col glyph di `glyphs.rs`.
2. `IconField` accoda una richiesta `(index, path, size)` a un `WorkerPool` a
   dimensione fissa (un thread per core, cap 4-6; pattern di riferimento
   `crates/fsops/src/pool.rs`).
3. Il worker: cache hit valida -> decodifica PNG; altrimenti genera, scrive in
   cache atomica (`tmp` + `rename`), ritorna i bytes.
4. Il risultato torna sul main via `async_channel`; `IconField` verifica che
   l'`index` corrisponda ancora allo stesso `path` (la directory puo essere
   cambiata) e sostituisce l'immagine dell'`IconItem` con un cross-fade breve.
5. Richieste per item usciti dal viewport o non piu presenti vengono scartate al
   ritorno.

Priorita agli item visibili tramite reorder della coda quando `set_entries`
cambia. Nessun inotify sulla cache.

## Persistenza `.opusdm-layout` (fase 3)

`layout_file.rs`, puro e testabile.

### Formato (JSON, un file per directory, nascosto)

```json
{
  "version": 1,
  "mode": "free",
  "icon_size": 96,
  "positions": {
    "Report.pdf": { "x": 240, "y": 120 },
    "photos":     { "x": 40,  "y": 40 }
  }
}
```

- chiave = **basename**, non path: il file resta valido se si sposta la cartella;
- `mode` e `icon_size` per-directory sono opzionali: il consumatore li applica se
  presenti, altrimenti usa i suoi default. Sul desktop `mode` e sempre `free`;
- voci `positions` orfane (file non piu esistenti) scartate alla lettura e
  ripulite al primo salvataggio.

### API

```rust
pub struct DirLayout {
    pub mode: Option<LayoutMode>,
    pub icon_size: Option<i32>,
    pub positions: HashMap<String, (f64, f64)>,
}

pub fn load(dir: &Path) -> DirLayout; // file mancante/corrotto -> default, mai errore propagato
pub fn save(dir: &Path, layout: &DirLayout) -> io::Result<()>; // scrittura atomica tmp+rename
```

### Cablaggio nel widget

`IconField::set_layout_dir(Some(dir))` fa `load`, popola la mappa di
`FreeArrange`, applica `mode`/`icon_size` se presenti. Sul segnale interno di
fine drag, se `layout_dir` e settato, il widget fa `save` in debounce (circa
500 ms) con l'intera mappa corrente. `set_layout_dir(None)` = modalita effimera
(nessuna lettura/scrittura), usata dal Lister in griglia e dai test.

Scritture solo su fine-drag e su cambio `icon_size`/`mode` dal desktop, mai su
semplice navigazione. Il file viene creato solo quando c'e qualcosa da salvare.

Concorrenza: due Lister sulla stessa cartella, ultimo che salva vince; `load`
viene rifatto quando la cartella riacquista rilevanza. `.opusdm-layout` va
filtrato dalla vista come i dotfile.

## Gestione degli errori

| Situazione | Comportamento |
|------------|---------------|
| `read_entries` fallisce sulla dir | ritorna `io::Result` al consumatore; il Lister mostra il messaggio come oggi per `read_dir_rows`, l'hub logga su `eprintln!` e lascia l'area desktop vuota |
| singolo entry illeggibile | saltato in `read_entries` con `eprintln!` una riga, gli altri passano |
| `.opusdm-layout` mancante o corrotto | `load` ritorna `DirLayout::default()`, nessun errore propagato; il primo `save` lo riscrive pulito |
| `save` di `.opusdm-layout` fallisce | `io::Result` loggato una volta; le posizioni restano in memoria per la sessione |
| tool thumbnail assente | rilevato una volta, memoizzato; quel tipo resta al glyph |
| generatore thumbnail fallisce/timeout | il worker ritorna `None`, l'`IconItem` resta al glyph; `eprintln!` una riga col path |
| PNG di cache corrotto | trattato come cache miss, si rigenera |
| risultato thumbnail torna dopo cambio directory | scartato dal check `index -> path` |

## Integrazione nei consumatori

### Codice condiviso spostato nel nuovo crate

- `crates/lister/src/dnd.rs` -> `opusdm_iconview::dnd` (parsing uri-list + test).
  Il Lister tiene un `dnd.rs` ridotto ad adapter.
- `crates/lister/src/glyphs.rs` -> `opusdm_iconview::glyphs`. Serve alla vista a
  icone e alla decorazione della colonna Name del `ColumnView`. `Row` resta nel
  Lister; `glyph_for` viene generalizzato a
  `glyph_for(kind: IconKind, mode: u32, name: &str)`.
- nuovo `opusdm_iconview::scan::read_entries`, usato direttamente dall'hub. Il
  Lister non lo usa: mappa i `Row` che gia legge con `model::read_dir_rows` in
  `IconEntry`, senza rileggere.

### Lister (`crates/lister`)

- `Ui` guadagna `icon_field: IconField` e `view_mode: Cell<ViewMode>`
  (`Details` | `Icons`). Un `gtk::Stack` in `window.rs` contiene lo
  `ScrolledWindow` del `ColumnView` e il `IconField`; il toggle sta nella button
  row.
- `navigate_to` / `reload`: dopo aver ottenuto i `Vec<Row>` ordinati con
  `dir_first_cmp`, se `view_mode == Icons` costruisce i `IconEntry` e chiama
  `icon_field.set_entries`; il `ColumnView` viene aggiornato solo quando e la
  vista attiva. Cambio di vista -> rebuild della vista che diventa visibile.
- Segnali del `IconField` -> handler esistenti: `activate` riusa la logica di
  apertura; `rename-requested` -> `actions.rs` rename; `context-menu` ->
  `context_menu.rs`; `drop-uris` -> `ToHub::RunJob` via l'adapter `dnd.rs`;
  `zoom-requested` -> cicla tra size predefinite e richiama `set_icon_size`.
- Modalita `Grid`, `set_layout_dir(None)`: il Lister in griglia non scrive
  `.opusdm-layout`.
- Sort in icon mode: nessun header; si riusa l'infrastruttura di
  `columns.rs`/tasti per scegliere colonna e verso, poi si ripassa la lista
  riordinata a `set_entries`.
- `watcher.rs`: aggiungere `.opusdm-layout` ai nomi filtrati dalla vista.

### Hub (`crates/hub`)

- `toolbar.rs::build_desktop_area()`: la `Label` "OpusDM" centrata viene
  sostituita da un `IconField` in modalita `Free` come child principale
  dell'`Overlay`; il box del menu contestuale resta come layer sopra.
- nuovo `crates/hub/src/desktop_files.rs`: risolve la dir desktop
  (`$XDG_DESKTOP_DIR` via parsing di `user-dirs.dirs` -> fallback `~/Desktop` ->
  `None`), fa `read_entries`, e possiede un piccolo watch inotify sulla sola dir
  desktop (versione ridotta della logica di `watcher.rs`, solo `libc`). Su
  evento -> `read_entries` + `icon_field.set_entries`.
- `icon_field.set_layout_dir(Some(desktop_dir))`: posizioni lette/salvate da
  `<desktop>/.opusdm-layout` (fase 3).
- Segnali: `activate` -> `gio::AppInfo::launch_default_for_uri` per i file, nuovo
  Lister per le directory; `context-menu` -> menu contestuale del desktop
  esteso con azioni per-file quando l'indice non e `-1`; `drop-uris` ->
  `ToHub::RunJob` con `dest_dir = desktop_dir`; `move-icons` -> gestito
  internamente dal widget.
- Se la dir desktop e `None`: `IconField` non montato, si tiene l'area vuota
  attuale col solo menu contestuale.

Estrazione del watcher completo in un modulo condiviso: possibile follow-up,
fuori scope ora.

## Testing

Pattern del progetto: i test che richiedono un display si auto-skippano.

### Unit test puri (nessun GTK)

- `arrange.rs`: colonne per viewport, wrap, minimo 1 colonna, crescita di
  `virtual_h`, ordine preservato (Grid); posizioni note rispettate, slot liberi
  non sovrapposti, `virtual_w/h` = bounding box + margine, mappa vuota =>
  comportamento a griglia, navigazione direzionale (Free).
- `dnd.rs`: roundtrip uri-list, skip commenti e non-`file:`, spazi => `%20`.
- `layout_file.rs`: round-trip serialize/deserialize; file mancante => default;
  JSON corrotto => default; voci orfane scartate; `save` atomico; chiave =
  basename.
- `glyphs.rs`: test esistenti migrati + nuova firma.
- `scan.rs`: dir/file/symlink buono/rotto/dotfile => `kind` ed `ext` corretti,
  hidden filtrati, `.opusdm-layout` escluso.
- `thumbs.rs`: costruzione path di cache (`md5(uri)` + `normal/large`),
  validazione `Thumb::MTime` (match => valido, mismatch => stale) senza invocare
  i tool; rilevamento tool memoizzato mockabile via `enum ToolChoice` iniettata.

### Test GTK (auto-skip senza `gtk::init`)

- `set_entries` crea N `IconItem`; una lista diversa aggiorna senza leak
  (conteggio figli del `Fixed`);
- selezione: click esclusivo, `Ctrl`-toggle, `Shift`-range producono l'insieme
  atteso e il numero giusto di `selection-changed`;
- `set_mode(Grid->Free)` rifa il placement; `set_icon_size` cambia le `ItemBox`
  e ri-arrangia;
- `activate` emesso su Invio/doppio click con l'indice giusto;
- `rename-requested` emesso con `(index, nuovo_nome)` a fine editing, non emesso
  se il testo non cambia o su `Esc`.

### Fuori da CI / manuale

Generazione reale di thumbnail PDF/video, DnD tra due finestre vere, persistenza
vista riavviando l'hub.

## Fasi di implementazione

### Fase 1 - Walking skeleton

1. Scaffold `crates/iconview` + voce nei `members` del workspace.
2. `glyphs.rs` e `dnd.rs` spostati dal Lister, con adapter e re-import; test
   migrati e verdi.
3. `arrange.rs` con `GridArrange` + `FreeArrange` (mappa vuota) e unit test.
4. `item.rs` (`IconItem`: glyph + etichetta, CSS class) e `field.rs`
   (`IconField` su `Fixed` in `ScrolledWindow`,
   `set_entries`/`set_mode`/`set_icon_size`).
5. Interazione: selezione (click, Ctrl, Shift), rubber-band, tastiera,
   `activate`, `context-menu`, editing inline + `rename-requested`,
   `zoom-requested`.
6. DnD tra finestre: `DragSource`/`DropTarget` con `drop-uris`; drag di
   riposizionamento interno in Free che aggiorna il `Fixed` ed emette
   `move-icons` (senza persistere).
7. `scan.rs` + `read_entries`.
8. Integrazione Lister: `Stack` Details/Icons, toggle, wiring dei segnali agli
   handler esistenti.
9. Integrazione hub: `IconField` in Free nell'overlay del desktop,
   `desktop_files.rs` (risoluzione dir + watch inotify ridotto), wiring.
10. `CHANGES.md` aggiornato.

Esito: icone con glyph, selezione completa, apertura, rename, DnD funzionante in
Lister e sul desktop; le icone del desktop si trascinano ma non ricordano la
posizione dopo un riavvio.

### Fase 2 - Thumbnail

11. `thumbs.rs`: `WorkerPool` (riferimento `fsops/src/pool.rs`),
    path/validazione cache freedesktop, generatori immagine/PDF/video con
    rilevamento tool e timeout.
12. `IconField` accoda richieste per gli item visibili, sostituisce l'immagine
    al ritorno con check `index -> path`, cross-fade.
13. Unit test cache/validazione; test manuale generazione reale.
14. `CHANGES.md`.

### Fase 3 - Posizione libera persistente

15. `layout_file.rs` (`load`/`save` atomici, chiave basename, pulizia orfani) +
    unit test.
16. `IconField::set_layout_dir`: `load` popola `FreeArrange`, applica
    `mode`/`icon_size`; `save` in debounce su fine-drag e su cambio size/mode.
17. Hub: `set_layout_dir(Some(desktop_dir))`; `.opusdm-layout` filtrato dalla
    vista e gestito dal watch.
18. `CHANGES.md`.

Ogni fase e un commit-set che compila e passa i test. Le fasi 2 e 3 sono
indipendenti tra loro.

## Fuori scope

- virtualizzazione della vista (migrazione al `LayoutManager` custom
  dell'approccio "A");
- "Align to grid" / snap opzionale in modalita Free;
- estrazione del watcher inotify completo in un modulo condiviso;
- inotify sulla cache thumbnail;
- override manuale dell'ordine nella griglia del Lister (la griglia segue sempre
  il sort attivo).
