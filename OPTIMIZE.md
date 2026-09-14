# Optimization TODO

> Regenerated after the previous pass. Remaining items only.

## High

- [ ] **Icon view is one GTK widget per file** — `IconField` keeps a `Vec<IconItem>` of widgets inside a `gtk::Fixed`; large directories will exhaust widget resources and layout time. For directories beyond a threshold, switch to a `ListView`/grid with factories or a single custom-drawn widget.
  - File(s): `crates/iconview/src/field.rs`, `crates/iconview/src/item.rs`

## Low / Nice to have

- [x] **Preview kind detection reads on the UI thread** — `load_file` does synchronous I/O directly on the GTK thread: `detect_file_kind`'s fallback probe reads 512 bytes, `render_text` reads up to 256 KB, `render_image` decodes via `gdk::Texture::from_file`, and `render_archive` shells out to `zipinfo`/`tar`/`7z`. Fine for local disk, but it stalls on network mounts. Move the pipeline off-thread (or make it lazy) so a slow mount doesn't freeze the window.
  - File(s): `crates/lister/src/preview.rs`
