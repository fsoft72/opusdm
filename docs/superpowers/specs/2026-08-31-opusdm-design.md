# OpusDM - Design

Desktop Environment minimale per Linux/X11, scritto in Rust.
Wayland: fuori scope per la v1 (futuro: valutare smithay).

## Architettura

Due processi indipendenti che comunicano **solo** via EWMH sulla root window:

```
opusdm/
├── wm/      → binario "opus-wm": WM floating in Rust + x11rb
├── panel/   → binario "opus-panel": app GTK4 (backend X11)
└── session/ → script "opus-session": lancia tutto dentro Xephyr
```

Nessuna dipendenza diretta tra i due crate: il pannello funzionerebbe sotto
qualsiasi WM EWMH-compliant, e il WM funziona senza pannello.

## opus-wm (WM floating)

Event loop X11 (x11rb) che gestisce:

- **MapRequest**: nuova finestra mappata a cascata (offset progressivo
  da top-left), riceve il focus
- **Focus**: click-to-focus
- **Drag**: Alt+tasto sinistro → muove finestra; Alt+tasto destro →
  ridimensiona (dall'angolo basso-destra)
- **Chiusura**: Alt+F4 → `WM_DELETE_WINDOW` se la finestra lo supporta
  (protocollo WM_PROTOCOLS), altrimenti `kill_client`
- **EWMH pubblicato**: `_NET_SUPPORTED`, `_NET_SUPPORTING_WM_CHECK`,
  `_NET_WM_NAME`, `_NET_CLIENT_LIST`, `_NET_ACTIVE_WINDOW`
- **EWMH gestito in ingresso**: `_NET_ACTIVE_WINDOW` client message
  (richiesta di attivazione dal pannello) → raise + focus
- **Dock**: finestre con `_NET_WM_WINDOW_TYPE_DOCK` escluse da focus,
  move/resize, e dalla tasklist; sempre sopra

Fuori scope v1: tiling, workspace, decorazioni, compositing, keybinding
configurabili, dock multipli.

## opus-panel (GTK4)

Barra orizzontale **in alto**, full-width. Finestra dichiarata
`_NET_WM_WINDOW_TYPE_DOCK`.

- **Taskbar**: un bottone per finestra in `_NET_CLIENT_LIST` (esclusi
  i dock). Titolo da `_NET_WM_NAME`/`WM_NAME`. Click → client message
  `_NET_ACTIVE_WINDOW`. Finestra attiva evidenziata.
- **Orologio**: a destra, formato HH:MM, aggiornato ogni secondo.

Aggiornamento taskbar: connessione x11rb dedicata del pannello, con
`glib::unix_fd_add` sul fd X11; su evento PropertyNotify della root
(window `_NET_CLIENT_LIST` / `_NET_ACTIVE_WINDOW`) si rileggono le
proprieta' e si aggiorna la UI.

## Sessione / Testing

`session/opus-session`:

```sh
Xephyr -screen 1280x800 :1 &
sleep 1
DISPLAY=:1 opus-wm &
sleep 1
DISPLAY=:1 opus-panel &
DISPLAY=:1 xterm &
DISPLAY=:1 xterm &
```

- `cargo test` per la logica pura del WM (es. calcolo posizione a cascata,
  filtro dock dalla client list)
- Verifica visiva/integrazione: manuale in Xephyr

## Dipendenze

- wm: `x11rb` (feature `xfixes` se serve)
- panel: `gtk4`, `gdk4-x11`, `x11rb`

## Criteri di successo v1

1. `opus-session` apre Xephyr con pannello in alto e due xterm a cascata
2. Click su finestra → focus; Alt+drag muove; Alt+drag destro ridimensiona
3. Alt+F4 chiude la finestra attiva
4. Taskbar mostra le finestre aperte, click su bottone attiva la finestra
5. Orologio aggiornato
