# libarchive backend for archive browsing and extraction

## Goal

Replace the external `zipinfo` / `unzip` / `tar` / `7z` commands used to list
and extract archives with libarchive, behind one small safe crate. One reader
for every format, and the formats libarchive knows (RAR read, ISO, cpio, ...)
come for free.

In scope: listing entries, extracting one entry to a stream, extracting a
selection or a whole archive into a folder.

Out of scope: creating archives (`fsops/src/compress.rs` keeps its commands),
encrypted archives (reported as an error, no password prompt).

## Crate `opusdm-archive` (`crates/archive`)

The only crate with `unsafe`. Depends on `libarchive3-sys` and `thiserror`;
it does not depend on `opusdm-ipc` or `opusdm-fsops` (cancellation is a
callback, not a `CancelToken`).

```rust
pub struct Entry { pub path: String, pub kind: EntryKind, pub size: u64, pub mode: u32, pub mtime: Option<i64> }
pub enum EntryKind { File, Dir, Symlink, Other }

pub fn is_archive_name(name: &str) -> bool;
pub fn list(archive: &Path) -> Result<Vec<Entry>>;
pub fn extract_entry(archive: &Path, name: &str, out: &mut dyn Write) -> Result<()>;
pub fn extract(archive: &Path, dest: &Path, only: Option<&[String]>, cancel: &dyn Fn() -> bool) -> Result<Extracted>;
```

### Behaviour

- Format and compression filter are detected from the content
  (`support_format_all`, `support_filter_all`). `is_archive_name` is only the
  suffix whitelist the UI uses to decide what to show as an archive without
  opening every file; it replaces the two duplicated lists (`lister`
  `is_archive_file`, `fsops` `_kind_of`).
- Entry names are normalised (leading `./` and trailing `/` removed) and
  matched exactly. `[`, `*`, `?` and a leading `-` have no special meaning.
- `only` selecting a directory also extracts everything under it.
- Extraction goes through `archive_write_disk` with `SECURE_NODOTDOT`,
  `SECURE_SYMLINKS`, `NO_OVERWRITE` and no owner restore. Existing files are
  never overwritten (same as today's `unzip -n`, `tar --keep-old-files`,
  `7z -aos`); they are counted in `Extracted::skipped`.
- `cancel` is polled between data blocks, so a big file stops mid-way.
- A selected name that is not in the archive is an error (`NotFound`).
- Errors: `Open`, `Read(String)` (libarchive's own message), `Unsupported`,
  `Encrypted`, `UnsafePath`, `NotFound`, `Cancelled`, `Io`.

## Integration

- `lister/src/archive.rs`: `read_archive_entries`, `extract_single_file_temp`
  and `extract_items_to_dir` call the crate. Removed: `ArchiveKind`,
  `read_zip/tar/7z_entries`, `_unzip_pattern`, `_check_status` and every
  `Command`. Kept: path parsing, `ListingCache`, scratch and preview folder
  bookkeeping, `read_archive_rows`. `ArchiveItem` is built from `Entry`.
- `fsops/src/extract.rs`: `run_extract` calls `opusdm_archive::extract` with
  the job's `CancelToken`; the process polling goes away. The `Progress`
  contract towards the UI is unchanged. `tools::seven_z_program` stays (used
  by `compress.rs`).

## Tests

Fixtures are built in the tests with the `tar` and `zip` crates (already in
the workspace), so tests no longer skip silently when a tool is missing. A
small 7z file is committed under `crates/archive/tests/fixtures`.

Cases: nested entries, names such as `-dash.txt` and `st[ar]*.txt`, `../evil`
traversal, an existing file kept, cancel in the middle, `tar.gz`, `tar.zst`
(when the filter exists), 7z, directory selection, missing entry, a file that
is not an archive. Existing lister and fsops tests must keep passing.

## Build requirement

`libarchive-dev` (pkg-config `libarchive`) is needed to build. Documented in
`CHANGES.md` and `docs/usage.md`; the runtime needs `libarchive13`.

## Rollout

Work on branch `archive`, one commit per step, each compiling and tested:
1. crate with `list` and `is_archive_name`
2. `extract_entry` and `extract`
3. lister switches to the crate
4. fsops switches to the crate
5. dead code removed, `CHANGES.md` and docs updated
