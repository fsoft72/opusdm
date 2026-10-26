# OpusDM - Guida al tour delle funzionalità

Questa guida è un percorso a step per provare, una dopo l'altra, tutte le
funzionalità di OpusDM. Ogni step dice dove cliccare, quali tasti premere e che
cosa dovresti vedere. Si può seguire dall'inizio alla fine in circa 60-90
minuti, oppure saltare direttamente alla parte che interessa.

Convenzioni usate nel documento:

- `Ctrl+P` significa "tieni premuto Ctrl e premi P".
- **Grassetto** indica il testo di un pulsante, di una voce di menu o di un campo.
- "Palette" è la **Command Palette**, che si apre con `Ctrl+P` (o `Ctrl+Shift+P`).
  Per eseguire un comando si apre la palette, si scrive una parte del nome, si
  sceglie con le frecce `Su` / `Giù` e si preme `Invio`.
- "Lister" è una finestra file di OpusDM, "hub" è il processo in background che
  gestisce desktop, pannello e operazioni sui file.

Indice:

1. [Preparazione](#1-preparazione)
2. [Primo avvio: desktop, pannello, dock](#2-primo-avvio-desktop-pannello-dock)
3. [Il Lister e la navigazione](#3-il-lister-e-la-navigazione)
4. [SOURCE e DEST](#4-source-e-dest)
5. [Selezione dei file](#5-selezione-dei-file)
6. [Operazioni sui file](#6-operazioni-sui-file)
7. [Operazioni avanzate sui file](#7-operazioni-avanzate-sui-file)
8. [Filtro, ricerca e Quick Look](#8-filtro-ricerca-e-quick-look)
9. [Viewer e slideshow](#9-viewer-e-slideshow)
10. [Terminale integrato](#10-terminale-integrato)
11. [Archivi virtuali e server remoti](#11-archivi-virtuali-e-server-remoti)
12. [Strumenti: checksum, dimensioni, confronto cartelle](#12-strumenti-checksum-dimensioni-confronto-cartelle)
13. [Formato del Lister, cache e cronologia](#13-formato-del-lister-cache-e-cronologia)
14. [Drag and drop e clipboard](#14-drag-and-drop-e-clipboard)
15. [Sistema di Function](#15-sistema-di-function)
16. [Hotkey, menu utente ed eventi script](#16-hotkey-menu-utente-ed-eventi-script)
17. [Desktop: Leave Out, gruppi, Hide/Reveal, sfondo](#17-desktop-leave-out-gruppi-hidereveal-sfondo)
18. [Button Banks](#18-button-banks)
19. [Impostazioni e temi](#19-impostazioni-e-temi)
20. [Chiusura e funzioni non ancora disponibili](#20-chiusura-e-funzioni-non-ancora-disponibili)
21. [Riepilogo tasti](#21-riepilogo-tasti)

---

## 1. Preparazione

### Step 1.1 - Avvia OpusDM

OpusDM funziona su X11 e su Wayland con gli stessi binari: su Wayland il
compositor di riferimento è labwc (vedi `docs/WAYLAND-PORT.md` per cosa funziona
dove). Per provarlo senza toccare la tua sessione
reale c'è uno script che apre un server X annidato (Xephyr) con un window
manager (openbox):

1. Apri un terminale nella cartella del progetto (`opusdm`).
2. Lancia `./run-x11.sh` (usa `./run-x11.sh :2 1920x1080` per scegliere display
   e risoluzione).
3. Lo script compila il progetto in release, apre la finestra Xephyr e avvia
   `opusdm-hub`, che a sua volta apre il primo Lister.

Per provarlo su Wayland lancia invece `./run-wayland.sh` (apre un labwc
annidato; richiede i pacchetti `labwc`, `wlr-randr` e `libgtk4-layer-shell-dev`).

In alternativa, dentro una sessione X11 o labwc normale, avvia direttamente
`target/release/opusdm-hub`.

Dovresti vedere: uno sfondo desktop con delle icone, un pannello in alto, un
dock di lanciatori e una finestra Lister.

### Step 1.2 - Crea una cartella di prova

Per provare le operazioni sui file senza rischi, crea dei file di esempio. In un
terminale normale esegui:

```bash
mkdir -p ~/opusdm-tour/{sorgente,destinazione,archivi,immagini}
cd ~/opusdm-tour/sorgente
for i in 01 02 03 04 05; do echo "Questo è il file $i" > "documento_$i.txt"; done
echo "TODO: provare la ricerca" > note.txt
echo "int main(void) { return 0; }" > prova.c
dd if=/dev/urandom of=grande.bin bs=1M count=12 2>/dev/null
mkdir -p cartella_a/sotto cartella_b
echo "dentro" > cartella_a/sotto/profondo.txt
echo "uno" > cartella_b/uno.txt
cd ~/opusdm-tour
zip -qr archivi/esempio.zip sorgente
```

Se vuoi anche delle immagini per lo slideshow, copia 4 o 5 file `.png` o `.jpg`
qualsiasi in `~/opusdm-tour/immagini`.

Tutti gli step seguenti usano questa cartella `~/opusdm-tour`.

---

## 2. Primo avvio: desktop, pannello, dock

### Step 2.1 - Il desktop

Il desktop mostra le icone della cartella Desktop dell'utente
(`$XDG_DESKTOP_DIR`, di solito `~/Desktop`).

1. **Doppio click** su una cartella del desktop: si apre in un nuovo Lister.
2. **Click destro** su un punto vuoto del desktop: appare il menu del desktop.
   Contiene, tra le altre, le voci **New Lister**, **Paste**, **New Folder...**,
   **New File...**, **New Group...**, **Button Banks...**, **Hide All**,
   **Open Terminal Here**, **Settings**, **About**, **Quit** e un sottomenu
   **Desktop** (espandilo con un click) che contiene **Rearrange icons**,
   **Edit Hotkeys** (apre direttamente l'editor delle hotkey),
   **Manage Functions** (apre direttamente l'elenco delle function),
   **Edit File Types** (apre direttamente l'editor dei tipi di file), **Edit Theme** (apre direttamente il Theme Editor) e **Settings**.
3. Scegli **New Folder...**, scrivi `prova` e conferma con **OK**: appare una
   nuova cartella sul desktop. Poi **click destro** sulla sua icona: il menu
   dell'icona offre **Open**, **Cut**, **Copy**, **Rename...**, **Delete...**.
   Elimina la cartella con **Delete...** per pulire.
4. **Trascina** un'icona in un altro punto del desktop e poi scegli
   **Desktop > Rearrange icons**: le icone vengono rimesse in ordine
   (cartelle prima, poi file).

### Step 2.2 - Il pannello

In alto c'è il pannello (la posizione si cambia dalle Impostazioni, step 19).
Da sinistra a destra trovi:

- il **menu applicazioni** (la pallina): si apre con un click e mostra le tue
  cartelle di applicazioni, più **About OpusDM** e **Logout**;
- il **pager** dei desktop virtuali e la **lista delle finestre aperte**;
- l'area di notifica (**tray**), il monitor di sistema (CPU, memoria, rete) e
  l'orologio.

Prova ad aprire il menu applicazioni e a lanciare un programma.

Un click sull'orologio apre il **calendario** del mese (le frecce cambiano
mese e anno; i giorni con appuntamenti sono sottolineati). Clicca un giorno:
si apre la finestra del giorno con i suoi appuntamenti e un modulo con
**Text**, **Time** (`HH:MM`) e **Alarm** (No alarm, 5, 10, 20, 30 mins,
1 hour). **Add** lo salva; un click su un appuntamento della lista lo
riapre per cambiarlo (**Save**) o eliminarlo (**Delete**); **New** svuota il
modulo. L'allarme arriva come notifica, anche con Non disturbare attivo.

### Step 2.3 - Il dock

Il dock è una riga di icone lanciatore (di default sul bordo inferiore).

1. **Click** su un'icona: l'applicazione parte (il cursore mostra un'attesa
   mentre si avvia) e un tooltip mostra il nome passando sopra col mouse.
2. **Trascina** un'icona per riordinarla all'interno del dock.
3. **Click destro** su un'icona: compaiono le azioni dell'app e
   **Remove from Dock**.
4. **Click destro** sullo sfondo del dock: **Add Application...** per aggiungere
   un'applicazione.
5. Trascinare un'icona fuori dal dock la rimuove.

Nota: il dock non riserva spazio sullo schermo, quindi le finestre massimizzate
lo coprono: si vede solo sopra il desktop.

---

## 3. Il Lister e la navigazione

### Step 3.1 - Conosci la finestra

Il Lister è la finestra file. Dall'alto in basso:

- **barra di stato**: i contatori `D` (directory), `F` (file) e `B` (byte), il
  **badge del ruolo** (`SOURCE`, `DEST` o `NONE`) e il pulsante di pin;
- **riga dei pulsanti**: Copy, Move, Delete, Make Directory, New File, Rename, il
  tasto occhio per i file nascosti, `👁` (Quick Look), `>_` (terminale) e il menu
  **Menu** (menu utente);
- **elenco file** con le colonne **Name**, **Size**, **Date**, **User**,
  **Group**, **Access**;
- in basso: pulsante **Home**, pulsante **Parent**, i pulsanti **indietro** /
  **avanti**, il menu **History** e la **barra del percorso**.

Passa il mouse su ogni pulsante: il tooltip dice cosa fa e quale tasto lo
richiama.

### Step 3.2 - Naviga

1. Nella barra del percorso (in basso) cancella il testo, scrivi
   `~/opusdm-tour/sorgente` e premi `Invio`. L'elenco mostra i file di prova.
2. **Doppio click** su `cartella_a` (oppure selezionala con le frecce e premi
   `Invio`): entri nella cartella. Il contatore in alto si aggiorna.
3. Premi `Backspace` (o clicca il pulsante **Parent**): torni alla cartella
   superiore.
4. Premi `~` (o clicca **Home**): vai alla tua home.
5. Torna a `~/opusdm-tour/sorgente` scrivendo di nuovo il percorso.

### Step 3.3 - Ordinamento e colonne

1. **Clicca l'intestazione** della colonna **Size**: l'elenco si ordina per
   dimensione crescente. Clicca di nuovo: ordine decrescente. Le directory
   restano sempre in blocco, solo i file si riordinano.
2. Fai lo stesso con **Date** e **Name**.
3. Leggi la colonna **Access**: mostra i permessi nella forma `rwxr-xr-x`.

### Step 3.4 - File nascosti

1. Premi `Ctrl+H` (oppure clicca il pulsante a forma di occhio nella riga dei
   pulsanti): compaiono i file che iniziano con `.`.
2. Premi di nuovo `Ctrl+H` per nasconderli.

### Step 3.5 - Ricarica e aggiornamento automatico

1. Apri un terminale e crea un file: `touch ~/opusdm-tour/sorgente/nuovo.txt`.
2. Nel Lister, senza premere nulla, il file compare entro pochi centesimi di
   secondo (OpusDM osserva la cartella con inotify).
3. `Ctrl+R` rilegge comunque la cartella dal disco.
4. Cancella il file di prova: `rm ~/opusdm-tour/sorgente/nuovo.txt`.

### Step 3.6 - Type-to-find

1. Con il focus sull'elenco (clicca su una riga), scrivi `gra`.
2. Il cursore salta a `grande.bin` e la barra di stato mostra `Find: gra`.
3. `Backspace` cancella un carattere, `Esc` chiude la ricerca. Si chiude anche
   da sola dopo 1,5 secondi di inattività.

Nota: in OpusDM non ci sono comandi a lettera singola, per questo scrivere sopra
l'elenco avvia sempre la ricerca per prefisso.

### Step 3.7 - Radice e cartella superiore in un nuovo Lister

1. `Ctrl+P` > **Go to Root**: il Lister va alla radice (`/` per una cartella
   normale, la cima dell'archivio dentro un archivio, la `/` del server per una
   cartella remota).
2. `Ctrl+P` > **Open Parent in NEW Lister** apre la cartella superiore in una
   nuova finestra, **Open Root in NEW Lister** apre la radice.
3. Dentro un archivio `Backspace` sale correttamente: cartella interna, cima
   dell'archivio, poi la cartella che contiene l'archivio.

### Step 3.8 - Segnalibri (bookmark)

Un segnalibro dà un nome a una cartella, come un "assign" dell'Amiga.

1. Vai in `~/opusdm-tour/sorgente`, poi `Ctrl+P` > **Add Bookmark for This
   Folder...** e scrivi `tour` (lettere, cifre, `_` e `-`; non `ftp`, `ftps`,
   `sftp`).
2. Vai in un'altra cartella. Nella barra del percorso scrivi `tour:` e premi
   `Invio`: torni in `sorgente`. `tour:cartella_a` va in una sottocartella
   (`..` viene rifiutato).
3. `Ctrl+P` > **Bookmarks...** elenca, aggiunge, modifica, elimina i segnalibri
   e permette di andare in uno di essi.

---

## 4. SOURCE e DEST

OpusDM non ha un pannello doppio fisso: puoi aprire quanti Lister vuoi e ognuno
può essere **SOURCE** (da dove si leggono i file) o **DEST** (dove si scrive).

### Step 4.1 - Apri un secondo Lister

1. **Click destro** sul desktop e scegli **New Lister**.
2. Nel nuovo Lister vai a `~/opusdm-tour/destinazione` (barra del percorso, poi
   `Invio`).
3. Affianca le due finestre trascinandole per la barra del titolo.

### Step 4.2 - Osserva i ruoli

1. Clicca nel primo Lister: il suo badge diventa `SOURCE`.
2. Clicca nel secondo: diventa `SOURCE` lui e quello di prima diventa `DEST`
   (il ruolo segue il focus).
3. Premi `Tab`: il focus salta al Lister con il ruolo opposto.
4. **Clicca il badge** del ruolo: cicla tra `SOURCE`, `DEST` e `NONE`.
5. Con la palette (`Ctrl+P`) puoi anche scegliere **Set Role: SOURCE**,
   **Set Role: DEST**, **Set Role: NONE**.

### Step 4.3 - Blocca un ruolo (pin)

1. Imposta il Lister `destinazione` come `DEST`.
2. Clicca il **pulsante di pin** accanto al badge (oppure palette >
   **Toggle Role Pin / Lock**).
3. Ora cliccare altre finestre non cambia più il ruolo di quel Lister.
4. Se blocchi più Lister come `DEST`, **Copy** copia in tutti
   contemporaneamente (fan-out su più destinazioni).
5. Sblocca il pin prima di proseguire.

---

## 5. Selezione dei file

Lavora nel Lister `sorgente` (SOURCE).

### Step 5.1 - Selezioni di base

1. Clicca su `documento_01.txt`: è selezionato.
2. `Ctrl+click` su `documento_03.txt`: la selezione ne contiene due.
3. `Shift+click` su `documento_05.txt`: selezione di un intervallo.
4. `Ins` seleziona o deseleziona la riga sotto il cursore e scende di una riga.
5. `Ctrl+A` seleziona tutto.
6. `Esc` deseleziona tutto.
7. `Ctrl+Shift+A` (**Reselect**) ripristina l'ultima selezione cancellata (da
   `Esc` o da una operazione come Copy).
8. `*` inverte la selezione.

### Step 5.2 - Selezione per pattern

1. Premi `+`: si apre una richiesta. Scrivi `*.txt` e conferma: tutti i file
   `.txt` vengono selezionati.
2. Premi `-` e scrivi `documento_0[1-3]*` oppure `documento_01*`: i file
   corrispondenti vengono deselezionati.
3. Equivalenti da palette: **Select by Pattern**, **Deselect by Pattern**,
   **Select All**, **Deselect All**, **Invert Selection**.

### Step 5.3 - Advanced Select

1. `Ctrl+P`, scrivi `advanced` e premi `Invio`: si apre **Advanced Select**.
2. Compila **Name** con `*.bin`, **Size at least** con `1M`, **Type** con "files
   only".
3. Clicca **Select**: viene selezionato solo `grande.bin`.
4. Prova altri criteri: **Modified after** con `7d` (ultimi 7 giorni) oppure con
   una data come `2026-01-01`. **Deselect** rimuove dalla selezione ciò che
   corrisponde.
5. Un valore sbagliato viene segnalato accanto al campo e la finestra resta
   aperta.

---

## 6. Operazioni sui file

Imposta `sorgente` come SOURCE e `destinazione` come DEST.

### Step 6.1 - Copia (F5)

1. Seleziona `documento_01.txt` e `documento_02.txt`.
2. Premi `F5` (oppure clicca il pulsante **Copy** nella riga dei pulsanti).
3. In alto compare il **progresso del job** con la barra e un pulsante
   **Abort** per annullare.
4. Nel Lister `destinazione` appaiono i due file.

Se un file esiste già a destinazione, ripeti la copia: OpusDM chiede come
risolvere il conflitto (sovrascrivere o saltare).

### Step 6.2 - Sposta (F6)

1. Seleziona `note.txt` e premi `F6` (o il pulsante **Move**): il file passa da
   `sorgente` a `destinazione`.
2. Riportalo indietro: nel Lister `destinazione` selezionalo e, dopo aver
   scambiato i ruoli (clic sul Lister giusto), premi di nuovo `F6`.

### Step 6.3 - Crea cartelle e file

1. In `sorgente` premi `Ctrl+N`, scrivi `nuova_cartella` e conferma.
2. Premi `Ctrl+Shift+N`, scrivi `vuoto.txt` e conferma: si crea un file vuoto.

### Step 6.4 - Rinomina (F2)

1. Seleziona un solo file, ad esempio `vuoto.txt`, e premi `F2`.
2. Il campo del nome ha preselezionato solo il nome, senza l'estensione:
   scrivendo si sostituisce `vuoto` e `.txt` resta. (Cartelle e file che
   iniziano con `.` sono selezionati per intero.)
3. Scrivi `vuoto_rinominato` e conferma.

### Step 6.5 - Rinomina in blocco (Batch Renamer)

1. Seleziona `documento_01.txt`... `documento_05.txt` (`+`, pattern
   `documento_*`).
2. Premi `F2`: con più file selezionati si apre il **Batch Renamer**.
3. Prova le sezioni, osservando la tabella di anteprima
   `Nome originale -> Nuovo nome`:
   - **Search & Replace**: cerca `documento`, sostituisci con `doc`. Attiva la
     modalità regex per usare i gruppi `$1`, `$2`.
   - **Prefix / Suffix**: aggiungi del testo prima o dopo il nome (l'estensione
     viene preservata).
   - **Numerazione**: numeri con zeri iniziali (`_001`, `_002`), con inizio,
     passo e cifre configurabili.
   - **Case**: `lowercase`, `UPPERCASE`, `Title Case`, `snake_case`,
     `kebab-case`.
4. I nomi duplicati o in conflitto sono evidenziati in rosso grassetto con un
   avviso, e non si può applicare finché non li risolvi.
5. Conferma per rinominare: il Lister si aggiorna subito. (Puoi annullare con
   Cancel se non vuoi tenere i nuovi nomi.)

### Step 6.6 - Elimina (Del)

1. Seleziona `nuova_cartella` e premi `Del` (o il pulsante **Delete**).
2. Compare una richiesta di conferma: la cancellazione è definitiva (non c'è
   cestino). Clicca **Delete**.

---

## 7. Operazioni avanzate sui file

Tutti questi comandi sono nella palette (`Ctrl+P`, poi scrivi un pezzo del
nome). Usa SOURCE `sorgente` e DEST `destinazione`.

### Step 7.1 - Copy As / Move As

1. Seleziona `prova.c`.
2. `Ctrl+P` > **Copy As...**.
3. Nel campo del nome scrivi `*.bak`: `*` sta per il nome originale, quindi la
   copia si chiamerà `prova.c.bak`. L'anteprima in tempo reale mostra il
   risultato.
4. Conferma con **OK**. **Move As...** funziona allo stesso modo ma sposta.
5. Scorciatoia: **click destro** sui pulsanti **Copy** / **Move** della riga dei
   pulsanti apre **Copy As...** / **Move As...**.
6. Con **più elementi** selezionati, **Copy As...** e **Move As...** aprono la
   finestra delle regole del Batch Renamer (cerca/sostituisci, prefisso,
   suffisso, case, numerazione, anteprima in tempo reale) e usano i nomi
   mostrati nell'anteprima.

### Step 7.2 - Duplicate

1. Seleziona `prova.c` e usa **Duplicate Selected Items**.
2. Nella stessa cartella nasce `prova.c.copy`; ripetendo il comando, `.copy2` e
   così via.


### Step 7.3 - Copy (newer only)

1. Modifica `documento_02.txt` dalla shell (`echo altro >> documento_02.txt`).
2. Seleziona `documento_01.txt` e `documento_02.txt` e usa
   **Copy (newer only, Source -> Dest)**.
3. Viene copiato solo ciò che a destinazione manca o è più vecchio.

### Step 7.4 - Change Permissions

1. Seleziona `prova.c`, poi **Change Permissions...**.
2. La griglia rwx per proprietario, gruppo e altri è sincronizzata con il campo
   ottale. Scrivi `755` o clicca le caselle.
3. **Apply recursively** applica anche alle directory (che mantengono il bit
   `x` dove c'è `r`).
4. Clicca **Apply** e controlla la colonna **Access**.

### Step 7.5 - Set Timestamp

1. Seleziona due file e usa **Set Timestamp...**.
2. Inserisci `2020-01-01 12:00:00` oppure clicca **Now**.
3. Clicca **Apply**: la colonna **Date** cambia.

### Step 7.6 - Make Link

1. Seleziona `prova.c` e usa **Make Link... (hard or symbolic, Source -> Dest)**.
2. Scegli il tipo (simbolico o hard) e il nome (`*` = nome originale).
3. Clicca **Create**: nel Lister DEST compare il link (i simbolici sono marcati
   con `@` se attivi gli "type indicators", step 19).

### Step 7.7 - Split e Join

1. Seleziona `grande.bin` e usa **Split File...**.
2. Imposta una dimensione, ad esempio `4M`, e clicca **Split**: si creano
   `grande.bin.001`, `.002`, `.003` nel DEST.
3. Nel DEST seleziona le tre parti e usa **Join Files...**. Controlla il nome
   proposto e clicca **Join**: ottieni un file unico.
4. Verifica dal terminale con `cmp` che sia identico all'originale.

### Step 7.8 - Check Fit

1. Seleziona `grande.bin` e usa
   **Check Fit (Source -> Dest free space)...**.
2. Una finestra dice se la selezione entra nello spazio libero del DEST
   (oppure che percentuale ci sta), con entrambe le dimensioni.

### Step 7.9 - Recursive filter (copia filtrata)

1. Apri le Impostazioni (step 19), sezione **BEHAVIOUR** della scheda **Lister**,
   e attiva il **recursive filter**. Salva con **Apply**.
2. Seleziona `cartella_a` e premi `F5`.
3. OpusDM chiede un pattern di file: scrivi `*.txt` per copiare solo quelli
   (le directory vengono percorse, ma quelle senza file corrispondenti non
   vengono create). Vuoto = tutto, **Cancel** = annulla il job.
4. Disattiva di nuovo l'opzione quando hai finito.

### Step 7.10 - Cifratura e decifratura

1. Seleziona `documento_01.txt` e `documento_02.txt` nel SOURCE.
2. `Ctrl+P` > **Encrypt...**, scrivi una password e ripetila (il campo di
   conferma c'è solo quando si cifra), poi conferma.
3. Nel DEST (o nella stessa cartella se non c'è un DEST) nascono
   `documento_01.txt.enc` e `documento_02.txt.enc`. Le cartelle mantengono la
   loro struttura. I file esistenti non vengono mai sovrascritti.
4. Seleziona un `.enc` e usa **Decrypt...** con la password giusta: ottieni il
   file originale (un file che non finisce in `.enc` diventa `nome.dec`). Con la
   password sbagliata, o con un file manomesso o troncato, il file fallisce e
   non lascia nulla di parziale, mentre gli altri proseguono.
5. Le stesse voci sono anche nel **click destro** su un file: **Encrypt...** e,
   sui `.enc`, **Decrypt...**. Agiscono sulla selezione se il file cliccato ne
   fa parte, altrimenti solo su quel file.

Attenzione: **non esiste il recupero della password**. Il formato è
XChaCha20-Poly1305 con chiave derivata da Argon2id.

---

## 8. Filtro, ricerca e Quick Look

### Step 8.1 - Filtro rapido (fuzzy)

1. Premi `/` o `Ctrl+F`: appare la barra del filtro.
2. Scrivi `dcm`: restano solo i nomi che contengono quelle lettere in ordine
   (match fuzzy, es. `documento_01.txt`). Un contatore mostra `12/150`.
3. Prova con un glob: `*.txt`, oppure `documento_0?.*`.
4. `Invio` seleziona il primo risultato, `Esc` chiude il filtro e mostra di
   nuovo tutto.

### Step 8.2 - Quick Look (F3 o Spazio)

1. Seleziona `documento_01.txt` e premi `Spazio` (o `F3`, o il pulsante `👁`).
2. Si apre l'anteprima: testo con numeri di riga, conteggio righe e byte.
3. **Lascia la finestra aperta** e muovi il cursore con le frecce nel Lister:
   l'anteprima segue la selezione.
4. Prova con altri tipi:
   - un'immagine: mostra l'immagine e le dimensioni in pixel;
   - `grande.bin`: dump esadecimale a 16 byte per riga con colonna ASCII;
   - `archivi/esempio.zip`: elenco delle voci dell'archivio;
   - una cartella: contenuto della cartella.
5. Premi di nuovo `F3` (o `Esc`) per chiudere.

### Step 8.3 - Find Files (ricerca per nome)

1. `Ctrl+P` > **Find Files...**: si apre la finestra **Find**.
2. In **Name** scrivi `*.txt`. **Folder** parte dalla cartella del Lister:
   lascialo così.
3. Clicca **Search**: i risultati compaiono mentre la ricerca procede. **Stop**
   la interrompe. In fondo trovi quanti elementi sono stati scansionati.
4. Seleziona un risultato e clicca **Show in folder**: il Lister va nella
   cartella che lo contiene e lo seleziona.
5. Opzioni utili: **Include hidden** include i file nascosti, `!pattern` esclude.

### Step 8.4 - Search in Files (ricerca nel contenuto)

1. `Ctrl+P` > **Search in Files...**: stessa finestra, ma il cursore parte nel
   campo **Contains**.
2. Scrivi `TODO` in **Contains** e `*.txt, *.c` in **Name**, poi **Search**.
3. I risultati sono nella forma `percorso:RIGA  testo`.
4. **Doppio click** su un risultato (o selezionalo e clicca **View**): si apre il
   Viewer posizionato sulla prima occorrenza.

---

## 9. Viewer e slideshow

A differenza di Quick Look (leggero e che segue la selezione), il Viewer serve
per leggere davvero uno o più file.

### Step 9.1 - Read (SmartRead)

1. Seleziona `documento_01.txt`, `documento_02.txt` e `prova.c`.
2. `Ctrl+P` > **Read (SmartRead)**: il Viewer sceglie da solo la modalità
   (testo, ANSI, hex). Le immagini selezionate si aprono invece in
   `opusdm-viewer`.
3. Tasti nel Viewer:
   - `n` / `p`: file successivo / precedente (con giro completo);
   - `t`, `a`, `x`, `s`: passa a modalità Testo / ANSI / Hex / Smart;
   - `Ctrl+F`: apre la barra di ricerca;
   - `Esc`: chiude la ricerca e poi la finestra.
4. Ricerca: scrivi del testo, ogni occorrenza si evidenzia e un contatore mostra
   `3/17`. `Invio` o `F3` vanno al successivo, `Shift+Invio` o `Shift+F3` al
   precedente.
5. Altre voci nella palette: **Read as Text**, **Read as ANSI**, **Read as Hex**
   per forzare la modalità.

Per provare la modalità ANSI, crea un file di prova con colori:

```bash
printf '\033[1;31mROSSO\033[0m \033[32mverde\033[0m\n' > ~/opusdm-tour/sorgente/colori.ans
```

Selezionalo e usa **Read (SmartRead)**: si apre in modalità ANSI con i colori.

### Step 9.2 - Show (slideshow di immagini)

1. Vai in `~/opusdm-tour/immagini` e seleziona più immagini (`Ctrl+A`).
2. `Ctrl+P` > **Show (image slideshow)**: le immagini selezionate si aprono in
   `opusdm-viewer`, l'unico visualizzatore di immagini di OpusDM.
3. `Freccia destra` / `Freccia sinistra` scorrono le immagini.
4. `Delete` sposta l'immagine corrente nel cestino (chiede conferma prima di
   una cancellazione definitiva).

---

## 10. Terminale integrato

### Step 10.1 - Il cassetto del terminale

1. Premi `F4` (oppure `` ` ``, oppure clicca `>_`): in basso scorre un terminale
   a cassetto.
2. Scrivi `ls -la` e premi `Invio`: il comando gira nella cartella del Lister.
3. Frecce `Su` / `Giù` richiamano la cronologia comandi.
4. Scrivi `cd cartella_a`: **anche il Lister entra in `cartella_a`**.
5. Naviga con il Lister in un'altra cartella: il prompt del terminale si
   aggiorna alla nuova cartella.
6. Il pulsante **External** nell'intestazione del cassetto apre
   `opusdm-terminal` nella cartella corrente.
7. `F4` (o `Esc`) richiude il cassetto.

Anche il menu del desktop ha **Open Terminal Here**: apre `opusdm-terminal`
nella cartella del Desktop (se non parte, prova `$TERMINAL` e poi alcuni
terminali comuni).

---

## 11. Archivi virtuali e server remoti

### Step 11.1 - Entra in un archivio

1. Vai a `~/opusdm-tour/archivi`.
2. **Doppio click** su `esempio.zip`: ti ritrovi "dentro" l'archivio, come se
   fosse una cartella. Il percorso mostra `...esempio.zip#sorgente`.
3. Naviga nelle sottocartelle dell'archivio.
4. `Backspace` sale di un livello e, dall'alto, esce dall'archivio.

Formati supportati: `.zip`, `.tar`, `.tar.gz`, `.tar.bz2`, `.tar.xz`,
`.tar.zst`, `.7z`, `.rar`, `.iso`, `.cpio`.

### Step 11.2 - Estrazione selettiva

1. Imposta come DEST il Lister `destinazione`.
2. Dentro l'archivio seleziona un paio di file.
3. Premi `F5` (**Copy**): OpusDM li estrae in un'area temporanea e li trasferisce
   nel DEST tramite l'hub.

Nota: le cartelle dentro un archivio non vengono osservate da inotify (non c'è
nulla su disco da osservare).

### Step 11.3 - Naviga un server FTP, FTPS o SFTP

Un server remoto si sfoglia come una cartella. Ti serve un server raggiungibile
(per SFTP basta una macchina con `sshd`, anche `localhost`).

1. Nella barra del percorso scrivi `sftp://utente@host/percorso` (oppure
   `ftp://...`, `ftps://...`) e premi `Invio`.
2. La password viene chiesta quando serve e resta solo in memoria. SFTP prova
   prima l'ssh-agent e le chiavi `~/.ssh/id_*`. FTP senza utente entra come
   anonimo.
3. Al primo collegamento SFTP compare l'impronta SHA-256 della chiave dell'host:
   confermala per proseguire. Se in futuro la chiave cambia, il collegamento
   viene rifiutato.
4. `Ctrl+P` > **FTP / SFTP Address Book...** apre la rubrica: nome, protocollo,
   host, porta, utente, cartella iniziale, modalità passiva (FTP) e verifica del
   certificato (FTPS). **Connect** apre il sito.
5. `Backspace`, il pulsante **Parent** e il doppio click funzionano come nelle
   cartelle locali. Il doppio click su un file remoto lo scarica e lo apre con
   l'applicazione predefinita.
6. **Copy**, **Move** e **Delete** funzionano con una cartella remota come
   SOURCE, come DEST o entrambe (due server). Da remoto a locale scarica, da
   locale a remoto carica, da remoto a remoto passa da una cartella temporanea.
   Una finestrella mostra il file in trasferimento e ha **Cancel**. Se i
   bersagli esistono già scegli tra **Overwrite**, **Skip existing** e
   **Cancel**. Un Move cancella i sorgenti solo a copia riuscita.
7. **New Folder** e **New File** funzionano anche in una cartella remota.
8. `Ctrl+P` > **FTP Quit** chiude tutte le connessioni.

Non disponibili su cartelle remote: Rename, Duplicate, permessi, timestamp,
link, Join e Split. Le cartelle remote non vengono messe in cache né osservate.

---

## 12. Strumenti: checksum, dimensioni, confronto cartelle

### Step 12.1 - Calcola dimensioni delle cartelle

1. Seleziona `cartella_a` e premi `Spazio` (oppure palette >
   **Calculate Directory Sizes**).
2. Il calcolo gira in background: la colonna **Size** della cartella si riempie
   con il totale e la dimensione complessiva compare nella barra di stato.
3. Il risultato viene ricordato: chiedendo di nuovo, le dimensioni note
   compaiono subito e vengono calcolate solo le cartelle nuove. Restano anche
   dopo un ricaricamento.
4. `Ctrl+P` > **Calculate Directory Sizes (force rescan)** dimentica le
   dimensioni ricordate delle cartelle scelte e le rimisura. La memoria si
   svuota anche dopo ogni Copy / Move / Delete e con **Free Caches**.

### Step 12.2 - Checksum

1. Seleziona uno o più file.
2. `Ctrl+P` > **Calculate Checksums (SHA-256 / MD5)...**.
3. I calcoli avvengono in background. Clicca **Copy** accanto a un hash per
   copiarlo negli appunti.

### Step 12.3 - Confronto e sincronizzazione di cartelle

1. Tieni aperta `sorgente` in un Lister (SOURCE) e `destinazione` nell'altro.
2. `Ctrl+P` > **Compare / Sync Directories (Visual Diff)...**.
3. Indica la cartella da confrontare e clicca **Compare**.
4. Il risultato divide gli elementi in: solo in A, solo in B, più recente in A,
   più recente in B, dimensione diversa, identici.
5. Clicca **Select Differences in Lister** per selezionare nel Lister tutto ciò
   che è nuovo o modificato. Poi puoi copiare con `F5`.

### Step 12.4 - Anteprima dei font

1. `Ctrl+P` > **View Font...**.
2. Scegli una famiglia dall'elenco (c'è una casella di ricerca), imposta la
   dimensione e **Bold** / **Italic**.
3. Modifica il testo di esempio per provare le tue parole. La finestra non è
   modale: può restare aperta mentre lavori.

---

## 13. Formato del Lister, cache e cronologia

### Step 13.1 - Lister Format

1. `Ctrl+P` > **Lister Format...**.
2. **Columns**: spunta le colonne da mostrare e spostale con **Up** / **Down**
   (Name si può spostare ma non togliere).
3. **Sort by**, **Reverse** e **Separation** (directory prima, dopo o mescolate)
   scelgono l'ordine.
4. **Show only** e **Hide** sono liste di pattern per i file (`*.rs,!test*`);
   **Hide** vince su **Show**.
5. **Free space gauge** aggiunge una barra di spazio usato alla barra di stato.
6. **Use** applica il formato finché resti in questa cartella. **Save (this
   directory)**, **Save (with subdirectories)** e **Save as default** lo
   memorizzano per sempre. **Remove saved format** lo elimina, **Reset** torna a
   quello di fabbrica.
7. Prova: nascondi le colonne **User** e **Group**, premi **Save (this
   directory)**, esci e rientra nella cartella: il formato si riapplica.

### Step 13.2 - Pattern Show / Hide persistenti

1. `Ctrl+P` > **Set Hide Pattern...** e scrivi `*.bin`: `grande.bin`
   scompare dall'elenco.
2. **Set Show Pattern...** limita l'elenco ai soli file corrispondenti. Una
   risposta vuota cancella il pattern.

### Step 13.3 - Cronologia indietro/avanti

1. Visita tre cartelle diverse.
2. `Alt+Sinistra` e `Alt+Destra` (o i pulsanti **indietro** / **avanti**) si
   muovono nella cronologia.
3. Il menu **History** accanto alla barra del percorso elenca le cartelle
   visitate (la più recente in alto, quella corrente in grassetto).

### Step 13.4 - Cache delle cartelle

1. Rientrare in una cartella già visitata la mostra subito, mentre il disco
   viene riletto in background.
2. `Ctrl+P` > **Cache List...**: elenca le cartelle in cache di tutti i Lister.
   **Doppio click** porta in primo piano il Lister che la contiene; **Open here**
   la apre nel Lister corrente; **Free all caches** svuota tutto.
3. **Free Caches** è disponibile anche come comando a sé nella palette.

### Step 13.5 - Formato della barra di stato

Apri le Impostazioni (step 19), scheda **Lister**, sezione **BEHAVIOUR**, campo
**Status bar format**. Prova ad esempio `%f file, %b byte  sel: %s  %p`. Codici:
`%d %f %b` totali, `%D %F %B` della selezione, `%s` numero di selezionati, `%p`
percorso, `%%` per il segno di percento. Vuoto = formato predefinito.

---

## 14. Drag and drop e clipboard

### Step 14.1 - Trascina tra Lister

1. Affianca due Lister.
2. **Trascina** un file da uno all'altro: viene **copiato**.
3. Tieni premuto `Shift` mentre rilasci: viene **spostato**.
4. Puoi trascinare file anche da e verso altre applicazioni (editor, file
   manager) e verso il desktop.

### Step 14.2 - Clipboard

1. `Ctrl+P` > **Copy to Clipboard**, **Cut to Clipboard** e **Paste from
   Clipboard** lavorano sugli appunti di sistema, compatibili con altri file
   manager.
2. Sul desktop, il menu ha **Paste** per incollare nella cartella Desktop.

### Step 14.3 - Menu contestuale di un file

**Click destro** su un file in un Lister. Compaiono: **Copy (Source -> Dest)**,
**Move (Source -> Dest)**, **Rename...**, **Delete...**, **Quick Look
(Preview)**, **Checksums & Dimensioni cartelle**, **Encrypt...** (e **Decrypt...** sui
`.enc`), **Leave Out on Desktop** e, sulle immagini, **Set as Desktop
Background**. Se per quel file esiste un file type (step 16.5) compaiono anche
le sue voci **Open With**.

---

## 15. Sistema di Function

Una Function è una sequenza di passi con un nome. Può contenere comandi interni
di OpusDM (Copy, Rename, Hide...) e programmi esterni. Una volta salvata si può
lanciare dalla palette, da una hotkey, dal menu utente, da un bottone di una
Button Bank o da un evento.

### Step 15.1 - Crea una Function

1. `Ctrl+P` > **Manage Functions...** (oppure, dal menu del desktop,
   **Desktop > Manage Functions**): si apre l'elenco delle function, in una
   finestra indipendente dal Lister. Il pulsante **Test** dell'editor usa
   come SOURCE il Lister che ha il ruolo SOURCE in quel momento; se nessun
   Lister è SOURCE, il test non parte e un avviso chiede di selezionarne uno.
2. Clicca **New Function...**: si apre l'editor.
3. Nel campo in alto scrivi il nome `Copia e notifica`.
4. Clicca **Add Internal Step...**, scegli **Copy** dal menu a tendina e conferma.
   Il passo compare nell'elenco (le opzioni **Only files that already exist at
   the destination**, **Only files missing or older at the destination** e il
   filtro per pattern sono facoltative).
5. Clicca **Add External Step...** e, come comando, scrivi:
   `notify-send "Fatto" "{o}"`.
6. Clicca **Save**.

Codici segnaposto utilizzabili nei parametri:

| Codice | Significato |
| :--- | :--- |
| `{f}` | elementi selezionati nel SOURCE, con percorso completo |
| `{o}` | elementi selezionati nel SOURCE, solo il nome |
| `{df}` | selezione del DEST, con percorso completo |
| `{do}` | selezione del DEST, solo il nome |
| `{df!}` | come `{df}`, ma vuoto invece di errore se manca |

### Step 15.2 - Prova la Function

1. Nel Lister SOURCE seleziona un file e mettine un altro come DEST.
2. Nell'editor clicca **Test**: il log in basso mostra l'esito di ogni passo.
3. Oppure `Ctrl+P`, scrivi `Copia e notifica` ed esegui la function dalla
   palette: compare tra i comandi. Dovresti vedere la copia e la notifica.

### Step 15.3 - I comandi interni disponibili

Nel menu a tendina di **Add Internal Step...** trovi: **Copy**, **Move**,
**Delete**, **Make Directory**, **New File**, **Rename**, **Copy As**,
**Move As**, **Duplicate**, **Change Directory**, **Set Background**,
**Leave Out**, **Put Away**, **Hide**, **Reveal**, **Toggle Hide**,
**Open Bank**, **Close Bank**, **Toggle Bank**, **Hide Bank**, **Confirm** e
**Run**.

Prova una function con due passi, **Change Directory** (`~/opusdm-tour`) e poi
**Make Directory** (`creata_da_function`): la cartella viene creata proprio lì,
perché Change Directory sposta anche la directory di lavoro dei passi successivi.

Per modificare o eliminare una function, apri **Manage Functions...** e
seleziona la riga, poi **Edit...** o **Delete**.

### Step 15.4 - Confirm: chiedere prima di procedere

1. In una nuova function aggiungi un passo interno **Confirm** con il messaggio
   `Cancello {o}?` (i codici `{f}`, `{o}`... funzionano).
2. Aggiungi dopo un passo **Delete**, salva e lancia la function su un file di
   prova.
3. Compare una richiesta sì/no. Con **No**, o se il Lister si chiude, il passo
   fallisce e la function si ferma. Una function lanciata senza nessun Lister
   a cui chiedere (evento script, hotkey globale senza Lister) si ferma sempre
   al Confirm.

### Step 15.5 - Run: lanciare un programma

1. Aggiungi un passo interno **Run** con `ls -l {f}` come **command** (le
   virgolette raggruppano le parole; gira nella cartella del SOURCE).
2. Di default la function **non aspetta**: il passo finisce appena il programma
   è partito e l'output viene scartato.
3. **Wait for it to finish** fa aspettare il passo; un'uscita diversa da zero
   ferma la function e l'output va nel log, come per un passo External.
4. **Show its output in a window** apre una finestra di testo sul Lister che ha
   lanciato la function, riempita man mano e lasciata aperta alla fine con il
   codice di uscita. Funziona con o senza Wait. Una function lanciata da un
   evento script non ha Lister, quindi niente finestra.

### Step 15.6 - Change Directory: le nuove opzioni

Il passo **Change Directory** ha tre parametri facoltativi:

- **Open in a NEW Lister** apre la cartella in una nuova finestra invece di
  spostare quella SOURCE;
- **The path may be a file** apre la cartella del file e lo seleziona;
- **View** (`details` o `icons`) cambia la vista del Lister SOURCE (le ultime
  due non valgono per un Lister nuovo).

I passi successivi girano comunque dalla nuova cartella.

### Step 15.7 - Conflitti e indicatore di attività

1. Se un passo **Copy** o **Move** trova un nome già presente a destinazione,
   il Lister che ha avviato la function mostra la solita richiesta
   **Overwrite / Skip / Overwrite All / Skip All / Cancel** e il passo prosegue
   con la risposta. Senza un Lister a cui chiedere (evento script, hook di
   Shutdown) l'elemento esistente viene saltato. **Delete** non va mai in
   conflitto.
2. Finché una function avviata da un Lister (palette, hotkey, menu utente,
   azione di file type, **Test** dell'editor) è in esecuzione, in basso nel
   Lister compare uno spinner con "Running a function..." (o "Running N
   functions..."). Sparisce a fine o in caso di errore. Le function avviate da
   Button Bank o da eventi script non lo mostrano.

### Step 15.8 - Riga di comando interna

1. `Ctrl+P` > **Command Line...**.
2. `!ls -l {f}` esegue un comando di shell nella cartella SOURCE.
3. Un comando interno con argomenti: `MakeDir backup`, `cd ~/opusdm-tour`,
   `CopyAs new_name=*.bak`, `Copy newer`. I nomi non distinguono maiuscole e
   ignorano spazi, `_` e `-` (`make-directory` funziona). `cd`, `md`, `mkdir`,
   `mv` sono nomi brevi. Una parola senza `nome=` riempie il primo parametro
   libero, una casella si attiva col suo nome (`newer`) o con `newer=yes`, le
   virgolette raggruppano le parole.
4. Il nome di una function salvata la lancia.
5. Gli errori (comando sconosciuto, parametro mancante o in più) compaiono nella
   richiesta e l'ultima riga viene riproposta la volta dopo.

---

## 16. Hotkey, menu utente ed eventi script

Per questi step ti serve almeno una function salvata (step 15).

### Step 16.1 - Assegna una hotkey

1. `Ctrl+P` > **Edit Hotkeys...** (oppure, dal menu del desktop,
   **Desktop > Edit Hotkeys**): si apre in una finestra a parte.
2. Clicca **Add Hotkey**.
3. Clicca il pulsante del tasto nella riga, e quando compare "Press the key
   combination now" premi, ad esempio, `Ctrl+Alt+U` (`Esc` annulla).
4. Scegli la function dall'elenco.
5. Se vuoi che funzioni da qualunque finestra, spunta **Global** (richiede
   almeno Ctrl, Alt o Super e una sessione X11).
6. Clicca **Save**.
7. Premi `Ctrl+Alt+U` nel Lister: la function parte.

Regole: due hotkey non possono usare lo stesso tasto (la riga mostra un errore
e **Save** resta disabilitato); una hotkey utente vince sui tasti predefiniti;
mentre scrivi in un campo di testo, una hotkey scatta solo se ha Ctrl, Alt o
Super oppure è un tasto funzione.

### Step 16.2 - Find Key

1. `Ctrl+P` > **Find Key...**.
2. Premi una combinazione, ad esempio `F5`: la finestra dice cosa fa (un comando
   interno, una function oppure niente).

### Step 16.3 - Menu utente

1. Nella riga dei pulsanti del Lister clicca **Menu**, poi **Edit User Menu...**
   in fondo (oppure palette > **Edit User Menu...**).
2. Usa **+ Entry** per aggiungere una voce collegata a una function,
   **+ Submenu** per un sottomenu, **+ Separator** per un separatore. Puoi
   rinominare e riordinare le voci.
3. Clicca **Save**: il menu **Menu** si aggiorna subito in tutti i Lister aperti,
   senza riavvio.
4. Clicca **Menu** e scegli la tua voce per eseguire la function.

Una voce il cui function è stata cancellata sparisce dal menu (l'editor la
segnala).

### Step 16.4 - Eventi script

1. `Ctrl+P` > **Edit Script Events...**.
2. Clicca **Add Binding**, scegli un evento e una function. Gli eventi sono:
   **Startup**, **Shutdown**, **Lister opened**, **Lister closed** e il doppio
   click sullo sfondo del desktop con tasto primario, centrale e destro.
3. Clicca **Save**.
4. Prova: collega a `Lister opened` una function con un passo **Change
   Directory**, poi apri un nuovo Lister (menu desktop > **New Lister**): la
   function parte da sola.

Per evitare cicli, un evento viene ignorato mentre la sua esecuzione precedente
non è finita e non scatta due volte nell'arco di 2 secondi.

### Step 16.5 - File type: cosa fa il doppio click

Un file type decide cosa succede al doppio click su un file e quali voci **Open
With** compaiono nel click destro. Serve una function salvata (step 15).

1. `Ctrl+P` > **Edit File Types...**: si apre l'elenco (salvato in
   `~/.config/opusdm/filetypes.json`). Vince il primo tipo, dall'alto, la cui
   regola corrisponde al file; le frecce riordinano.
2. Crea un nuovo tipo. In **Match** aggiungi una o più condizioni, combinate con
   "all of" o "any of": pattern dei nomi, byte a un offset (esadecimale, es.
   `89 50 4E 47`), testo nei primi N byte, intervallo di dimensione (`10K`,
   `4M`), intervallo di data (`7d`, `2026-01-01`). Il contenuto viene letto dai
   primi 64 KiB.
3. In **Actions** scegli la function da lanciare al **doppio click** e quelle da
   mostrare in **Open With**. La function lavora sul file cliccato, che diventa
   l'unico selezionato.
4. **Save** resta disabilitato finché un tipo ha un problema (indicato sotto il
   modulo). Le modifiche valgono subito per tutti i Lister.
5. Scorciatoia: seleziona un file e usa **Filetype Creator (from selected
   file)...**: l'editor si apre con un tipo già derivato dal file (`*.ext` se ha
   un'estensione, i primi byte se è binario senza estensione, altrimenti il nome
   esatto).
6. Prova con un doppio click su un file che corrisponde: parte la function.
   Cartelle e archivi si comportano come sempre, un file senza tipo si apre con
   l'applicazione predefinita.

L'azione **drop** viene salvata ma non è ancora collegata al drag and drop.

---

## 17. Desktop: Leave Out, gruppi, Hide/Reveal, sfondo

### Step 17.1 - Leave Out (icone permanenti sul desktop)

1. In un Lister **click destro** su `prova.c` > **Leave Out on Desktop**.
2. Sul desktop compare l'icona con una **freccia** nell'angolo. Se il file cliccato fa parte di una selezione, vengono lasciati fuori tutti gli elementi
   selezionati.
3. **Trascina** l'icona dove preferisci: la posizione viene ricordata anche
   dopo un riavvio.
4. **Click destro** sull'icona: **Open**, **Put Away** (rimuove solo l'icona,
   il file non viene toccato) e **Copy**.
5. **Doppio click**: una cartella si apre in un nuovo Lister, un file con
   l'applicazione predefinita.

Le icone Leave Out compaiono solo sul monitor principale.

### Step 17.2 - Program Groups

1. **Click destro** sul desktop > **New Group...**, scrivi un nome (`Strumenti`)
   e conferma. Il gruppo appare come icona cartella con un piccolo badge.
2. **Trascina** sull'icona del gruppo un'applicazione o dei file da un Lister
   (o dal desktop): vengono aggiunti. Un elemento già presente viene saltato.
3. **Doppio click** sull'icona del gruppo: si apre la finestra del gruppo.
4. **Doppio click** su un membro per avviarlo: cartella in un Lister, file
   `.desktop` come applicazione, binario eseguibile o script `#!` eseguito, altro
   con l'applicazione predefinita.
5. **Click destro** su un membro > **Remove from Group**.
6. **Click destro** sull'icona del gruppo: **Open**, **Rename Group...**,
   **Delete Group...** (eliminare un gruppo non tocca mai i file che contiene).

### Step 17.3 - Hide All e Reveal

1. **Click destro** sul desktop > **Hide All**: tutti i Lister vengono ridotti a
   icona e pannello, dock e gruppi si nascondono. Rimane solo il desktop.
2. **Click destro** sul desktop > **Reveal** (la stessa voce cambia nome): tutto
   torna com'era. Un Lister che avevi già minimizzato tu resta minimizzato, e il
   SOURCE torna in primo piano.
3. Aprire un nuovo Lister mentre è tutto nascosto (**New Lister**) riporta tutto in
   vista.
4. Da tastiera: crea una function con il passo **Toggle Hide**, assegnale una
   hotkey **Global** (step 16.1) e prova a premerla da un'altra applicazione.

OpusDM parte sempre in stato "visibile".

### Step 17.4 - Imposta uno sfondo

1. In un Lister **click destro** su un'immagine > **Set as Desktop Background**:
   lo sfondo cambia subito.
2. Per farlo da function: crea una function con il passo **Set Background**, con
   `{f}` come **Image file** e, facoltativo, `fill`, `fit`, `center` o `tile`
   come **Fit**.

### Step 17.5 - Applet sul desktop

Gli applet sono piccoli strumenti in JavaScript che stanno sul desktop
(solo sessione X11). Ognuno è una cartella in `~/.config/opusdm/applets/<id>/`
con `applet.json` (nome, finestra, impostazioni) e `main.js`.

1. Copia un applet di esempio dal repository:
   `cp -r applets/weather-clock ~/.config/opusdm/applets/`.
2. **Click destro** sul desktop > **Edit Applets** (l'ultima voce del menu):
   si apre l'**Applet Browser** (se era già aperto premi **Refresh**). Accendi
   l'interruttore di
   **Weather Clock**: l'orologio appare sul desktop. Lo stesso browser si apre
   da **Settings > Advanced > Edit Applets**.
3. **Alt + trascinamento** con il tasto sinistro sposta l'applet; la posizione
   viene ricordata.
4. Nel browser accendi **Edit mode**: ogni applet mostra un bordo tratteggiato,
   una maniglia per spostarlo e un ingranaggio che apre le sue impostazioni
   (cambia **City** e premi **Apply**). Spegni **Edit mode** per nasconderli.
5. Chiudere la finestra di un applet (Alt+F4) lo disattiva; riaccendilo dal
   browser. Dopo aver modificato `main.js` premi **Reload**.

La guida completa (manifest, API `opus`, widget e canvas) è in
`docs/usage.md`, sezione 3.41.

---

## 18. Button Banks

Una bank è una piccola finestra fluttuante di pulsanti. Ogni pulsante ha
un'etichetta e/o un'icona e fino a tre function: una per il click sinistro, una
per il destro e una per il centrale.

### Step 18.1 - Crea una bank

1. **Click destro** sul desktop > **Button Banks...** (oppure `Ctrl+P` >
   **Button Banks...**).
2. Nel gestore clicca **New...**: si apre l'editor della bank.
3. A sinistra c'è l'anteprima dei pulsanti. Clicca **Add** per creare un pulsante
   e selezionalo.
4. Nel modulo imposta: **etichetta**, **icona** (un file immagine, facoltativo) e
   le tre function (sinistro, destro, centrale).
5. Aggiungi altri due pulsanti con **Add** (o **Insert** per inserire prima della
   selezione). **Delete**, **Copy**, **Paste**, **Up** e **Down** lavorano sul
   pulsante selezionato.
6. Nelle impostazioni della bank scegli il **nome**, l'**orientamento**
   (Horizontal riempie per righe, Vertical per colonne), il numero di
   **colonne**, **Borderless** e **Visible on all desktops** (la bank resta
   visibile su tutti i desktop virtuali, invece che solo su quello dove è
   stata aperta).
7. **Save** è disabilitato finché qualcosa non è valido: il motivo è mostrato
   sotto il modulo.

### Step 18.2 - Aprila e usala

1. Nel gestore seleziona la bank e clicca **Open**: compare la finestra dei
   pulsanti (non appare nella lista finestre né nel pager).
2. Clicca un pulsante col tasto sinistro, poi col destro e col centrale: ognuno
   esegue la sua function. Un pulsante le cui function sono state tutte
   cancellate è disattivato e mostra un tooltip.
3. Trascina la finestra: la posizione viene salvata alla chiusura.
4. **Close Bank** nel gestore la chiude.

### Step 18.3 - Comandi da function

Una function può usare i passi **Open Bank**, **Close Bank**, **Toggle Bank** e
**Hide Bank** (che nasconde la finestra ma lascia la bank "aperta"), scegliendo la
bank da un menu a tendina. Così una hotkey, una voce del menu utente o un evento
può comandare una bank.

### Step 18.4 - Ripristino

1. Lascia una bank aperta, poi esci da OpusDM (step 20) e riavvialo.
2. La bank si riapre nella stessa posizione. **Hide All** (step 17.3) nasconde
   anche le bank.

---

## 19. Impostazioni e temi

### Step 19.1 - Apri le Impostazioni

**Click destro** sul desktop > **Desktop > Settings** (oppure **Settings** nel
menu del desktop). Si apre **OpusDM Settings** con quattro schede: **Desktop**,
**Dock**, **Menu**, **Lister**.

I cambiamenti si applicano subito a tutti i Lister aperti quando clicchi
**Apply**.

### Step 19.2 - Scheda Desktop

Le Impostazioni contengono solo le tue preferenze. L'aspetto (colori, font,
immagini dei Lister, icone) appartiene al tema e si modifica solo nel **Theme
Editor** (step 19.6).

1. **LIGHT / DARK**: la modalità chiara o scura. È una tua preferenza e resta
   uguale quando cambi tema.
2. **THEME**: scegli il tema attivo. Sotto ci sono **Edit themes...**,
   **Import theme...** ed **Export theme...** (step 19.6).
3. **BACKGROUND**: scegli **Solid colour**, **Gradient** (due colori e un angolo)
   o **Image** (scegli un file con **Browse...** e un fit: **Fill**, **Fit**,
   **Center**, **Tile**). **From theme...** mostra le immagini incluse nel tema
   selezionato: clicca una miniatura per usarla. Lo sfondo resta uguale quando
   cambi tema.
4. **ICON GRID**: dimensione icone, spaziatura e gap con i pulsanti `S` / `M` /
   `L` o con il campo numerico.
5. **DESKTOP FOLDER**: scegli una cartella diversa per le icone del desktop.
   **Reset to default** torna a quella predefinita.
6. **PANEL**: posizione del pannello (**Top** o **Bottom**).
7. Clicca **Apply** per vedere subito il risultato.

### Step 19.3 - Scheda Dock

1. Scegli il bordo del dock (solo i bordi che il pannello non usa).
2. Scegli la dimensione delle icone (32, 48 o 64).
3. Nell'elenco delle app fissate usa i pulsanti su/giù/rimuovi/aggiungi.

### Step 19.4 - Scheda Menu

Qui costruisci il menu applicazioni del pannello: aggiungi cartelle e
applicazioni (**Add Application**), riordinale e rinominale. Il menu del pannello
si aggiorna subito.

### Step 19.5 - Scheda Lister (comportamento)

**BEHAVIOUR**: **auto-refresh (inotify)**, **Status bar format**, **Directory
cache size** e il **recursive filter** (step 7.9). I colori dei Lister sono nel
tema: vedi lo step 19.6.

### Step 19.6 - Temi completi e Theme Editor

Un tema ora può portare con sé, oltre ai colori, anche **font** e **immagini**
(sfondo del desktop, sfondo dei Lister e delle finestre di richiesta) e **icone**.
Un tema è una cartella in `~/.config/opusdm/themes/<nome>/`.

Il tema è l'unica fonte dell'aspetto: colori dei Lister, appearance, font,
immagini dei Lister e delle finestre di richiesta, icone, suoni e gli sfondi
inclusi. Quando scegli un tema e premi **Apply** ottieni esattamente quel tema;
le parti che il tema non imposta tornano ai valori predefiniti. Le tue
preferenze (chiaro/scuro, sfondo, griglia icone, comportamento) non cambiano.

Se prima di questa versione avevi modificato colori o appearance nelle
Impostazioni, alla prima apertura delle Impostazioni questi valori diventano un
nuovo tema **My Theme**, già selezionato: non perdi nulla.

1. Nella scheda **Desktop**, sezione **THEME**, ci sono **Edit themes...**,
   **Import theme...** ed **Export theme...**.
2. **Export theme...**: impacchetta il tema selezionato in un archivio che puoi
   passare ad altri; **Import theme...** lo importa (i file che non sono font
   `.ttf`/`.otf` in `fonts/` o immagini in `backgrounds/` e `wallpapers/`, e i
   link simbolici, vengono scartati).
3. **Edit themes...** apre il **Theme Editor**:
   - a sinistra l'elenco dei temi con **New**, **Duplicate**, **Delete**;
   - in basso **Save**, **Save as copy...**, **Revert** (torna alla versione
     salvata) e **Save & Apply** (salva e lo rende il tema attivo);
   - schede **General** (nome, autore, versione), **Colours** (preset, colori e
     Appearance: **Type indicators**, **Monospace file list**, colori estesi a
     tutta la finestra), **Fonts** (font dell'interfaccia e dell'elenco file),
     **Pictures** (gli sfondi inclusi nel tema in **WALLPAPERS**, poi le immagini
     del Lister e delle finestre di richiesta), **Icons** e **Sounds**;
   - ogni sezione ha la casella **Set by this theme**: se è spenta, quella parte
     usa il valore predefinito.
4. Il tema predefinito **Breeze Dark** non si può modificare: usa **Duplicate**
   o **Save as copy...** e modifica la copia.
5. Prova: duplica un tema, in **Colours** scegli il preset `Amiga Workbench 1.3`,
   in **Pictures** aggiungi un'immagine a **WALLPAPERS**, premi **Save & Apply**
   e guarda i Lister cambiare. Poi, nelle Impostazioni, **BACKGROUND** >
   **Image** > **From theme...** e scegli l'immagine appena aggiunta. Quando un
   Lister ha un'immagine, le righe dei file sono disegnate leggermente
   trasparenti per restare leggibili.

Note: i suoni vengono salvati nel tema ma non ancora riprodotti. Le immagini
delle finestre di richiesta si vedono solo nelle richieste mostrate da Lister e
Impostazioni, non in pannello, dock e Button Bank. Questa parte non è ancora
stata verificata a mano in tutti i dettagli: se qualcosa non torna, è utile
segnalarlo.

---

## 20. Chiusura e funzioni non ancora disponibili

### Step 20.1 - Esci

1. **Click destro** sul desktop > **Quit**.
2. Conferma con **Quit** nella richiesta "Quit OpusDM?": si chiudono tutti i
   Lister e il desktop. Chiudere la finestra dello sfondo dal window manager
   mostra la stessa conferma. Gli eventi **Shutdown** hanno fino a 10 secondi
   per terminare.

### Step 20.2 - Cosa non troverai (ancora)

Per non cercare funzioni che non esistono, queste sono segnate come da fare:

- commenti sui file;
- riproduzione audio (`Play`), `beep` / `flash`, eventi sonori e riproduzione
  dei suoni dei temi;
- modalità icone nei Lister, vista dei dispositivi (`devices://`), formato del
  titolo del pannello;
- argomenti facoltativi `{x!}` nelle function (da verificare) e, per FTP, la
  rinomina remota, il keep-alive e i comandi grezzi;
- nel Theme Editor: modifica del comportamento e delle metriche del desktop,
  scelta di `preview.png`;
- nelle Button Banks: apertura sotto il mouse, variante Start Menu, toolbar del
  Lister come bank, riordino con drag and drop dei pulsanti;
- il cambio di proprietario dei file (chown): sono supportati solo i permessi.

Scelte definitive: Lister doppio in una sola finestra, tiling e cascade,
salvataggio del layout delle finestre e stampa non verranno realizzati.

---

## 21. Riepilogo tasti

Questi tasti funzionano quando il focus è sull'elenco file. Mentre scrivi nella
barra del filtro, del percorso o nel terminale, restano attivi solo `Esc`,
`F1`-`F12` e le combinazioni con `Ctrl`.

| Tasto | Azione |
| :--- | :--- |
| `Invio` | Entra nella cartella, nell'archivio, o apre il file |
| `Backspace` | Cartella superiore |
| `~` | Home |
| `Ins` | Seleziona/deseleziona la riga e scende |
| `Tab` | Salta al Lister con ruolo opposto (SOURCE / DEST) |
| `Alt+Sinistra` / `Alt+Destra` | Indietro / avanti nella cronologia |
| *scrivi un nome* | Type-to-find |
| `/` o `Ctrl+F` | Barra del filtro |
| `Esc` | Chiude filtro o terminale, oppure deseleziona |
| `Ctrl+P` / `Ctrl+Shift+P` | Command Palette |
| `F3` o `Spazio` | Quick Look (su una cartella, `Spazio` calcola la dimensione) |
| `F4` o `` ` `` | Terminale a cassetto |
| `F5` | Copy da SOURCE a DEST |
| `F6` | Move da SOURCE a DEST |
| `Del` | Elimina |
| `F2` | Rinomina (un file) o Batch Renamer (più file) |
| `Ctrl+N` | Nuova cartella |
| `Ctrl+Shift+N` | Nuovo file |
| `Ctrl+A` | Seleziona tutto |
| `Ctrl+Shift+A` | Reselect |
| `*` | Inverti la selezione |
| `+` / `-` | Seleziona / deseleziona per pattern |
| `Ctrl+H` | File nascosti |
| `Ctrl+R` | Ricarica |

Buon divertimento con OpusDM.
