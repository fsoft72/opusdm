//! Copy, Move, Delete and New Folder / File on remote locations (F-39). The
//! hub hands such jobs back as `ToLister::RemoteTransfer`; this module plans
//! them (pure, tested), connects, asks about existing targets, and runs the
//! transfer on a worker thread behind a progress window with Cancel.

use crate::remote::{self, url_of};
use crate::remote_ui::SharedFs;
use crate::window::Ui;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use opusdm_ftp::{RemoteError, RemoteItem, RemoteUrl};
use opusdm_ipc::{Op, ToHub};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// What a remote job comes down to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Remote sources into a local folder.
    Download { src: RemoteUrl, paths: Vec<String>, dest: PathBuf, delete_source: bool },
    /// Local sources into a remote folder.
    Upload { locals: Vec<PathBuf>, dest: RemoteUrl, delete_source: bool },
    /// Remote sources into a remote folder, through a scratch folder.
    Relay { src: RemoteUrl, paths: Vec<String>, dest: RemoteUrl, delete_source: bool },
    /// Delete remote sources.
    Remove { src: RemoteUrl, paths: Vec<String> },
    /// A new folder or empty file inside a remote folder.
    Create { dest: RemoteUrl, name: String, file: bool },
}

/// Work out what `op` on `srcs` into `dest_dir` means, or why it cannot be done.
pub fn plan(op: &Op, srcs: &[PathBuf], dest_dir: &Path) -> Result<Plan, String> {
    let dest = url_of(dest_dir);
    let remote_srcs: Vec<Option<RemoteUrl>> = srcs.iter().map(|s| url_of(s)).collect();
    let any_remote = remote_srcs.iter().any(Option::is_some);
    if any_remote && remote_srcs.iter().any(Option::is_none) {
        return Err("cannot mix remote and local sources in one job".to_string());
    }
    let src = match remote_srcs.first() {
        Some(Some(first)) => {
            let key = first.session_key();
            if remote_srcs.iter().flatten().any(|u| u.session_key() != key) {
                return Err("the sources must all be on the same server".to_string());
            }
            Some(first.clone())
        }
        _ => None,
    };
    let paths = || remote_srcs.iter().flatten().map(|u| u.path.clone()).collect::<Vec<_>>();
    match (op, src, dest) {
        (Op::Copy | Op::Move, Some(src), None) => {
            Ok(Plan::Download { src, paths: paths(), dest: dest_dir.to_path_buf(), delete_source: *op == Op::Move })
        }
        (Op::Copy | Op::Move, None, Some(dest)) => Ok(Plan::Upload { locals: srcs.to_vec(), dest, delete_source: *op == Op::Move }),
        (Op::Copy | Op::Move, Some(src), Some(dest)) => Ok(Plan::Relay { src, paths: paths(), dest, delete_source: *op == Op::Move }),
        (Op::Delete, Some(src), _) => Ok(Plan::Remove { src, paths: paths() }),
        (Op::MakeDir { name }, None, Some(dest)) => Ok(Plan::Create { dest, name: name.clone(), file: false }),
        (Op::NewFile { name }, None, Some(dest)) => Ok(Plan::Create { dest, name: name.clone(), file: true }),
        _ => Err("this operation is not available on remote locations (Copy, Move, Delete and New Folder / File are)".to_string()),
    }
}

/// What the first stage found out.
#[derive(Debug)]
struct Prepared {
    /// Remote sources, resolved.
    items: Vec<RemoteItem>,
    /// Names that already exist at the destination.
    existing: Vec<String>,
}

/// Keep the entries whose name is not in `skip`.
fn without<T>(all: Vec<T>, name_of: impl Fn(&T) -> String, skip: &[String]) -> Vec<T> {
    all.into_iter().filter(|x| !skip.contains(&name_of(x))).collect()
}

/// Run `f` on the locked connection `shared`.
fn with_fs<T>(shared: &Option<SharedFs>, f: impl FnOnce(&mut dyn opusdm_ftp::RemoteFs) -> Result<T, RemoteError>) -> Result<T, RemoteError> {
    let shared = shared.as_ref().ok_or_else(|| RemoteError::Failed("not connected".to_string()))?;
    let mut guard = shared.lock().map_err(|_| RemoteError::Failed("the connection is unusable".to_string()))?;
    f(guard.as_mut())
}

/// Stage one, on a worker thread: look the sources up and find the names
/// that would be overwritten.
fn prepare(plan: &Plan, src: &Option<SharedFs>, dst: &Option<SharedFs>) -> Result<Prepared, RemoteError> {
    let resolve = |paths: &[String]| with_fs(src, |fs| opusdm_ftp::resolve_items(fs, paths));
    match plan {
        Plan::Download { paths, dest, .. } => {
            let items = resolve(paths)?;
            let existing = items.iter().filter(|i| dest.join(&i.name).exists()).map(|i| i.name.clone()).collect();
            Ok(Prepared { items, existing })
        }
        Plan::Upload { locals, dest, .. } => {
            let names: Vec<String> = locals.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
            let existing = with_fs(dst, |fs| opusdm_ftp::existing_remote(fs, &dest.path, &names))?;
            Ok(Prepared { items: Vec::new(), existing })
        }
        Plan::Relay { paths, dest, .. } => {
            let items = resolve(paths)?;
            let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
            let existing = with_fs(dst, |fs| opusdm_ftp::existing_remote(fs, &dest.path, &names))?;
            Ok(Prepared { items, existing })
        }
        Plan::Remove { paths, .. } => Ok(Prepared { items: resolve(paths)?, existing: Vec::new() }),
        Plan::Create { .. } => Ok(Prepared { items: Vec::new(), existing: Vec::new() }),
    }
}

/// Stage two, on a worker thread: the transfer itself. `skip` lists the
/// existing names the user chose not to overwrite. A Move deletes its sources
/// only after the copy part succeeded.
fn execute(
    plan: &Plan,
    prepared: Prepared,
    skip: &[String],
    src: &Option<SharedFs>,
    dst: &Option<SharedFs>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<(), RemoteError> {
    match plan {
        Plan::Download { dest, delete_source, .. } => {
            let items = without(prepared.items, |i| i.name.clone(), skip);
            with_fs(src, |fs| opusdm_ftp::download(fs, &items, dest, cancel, progress).map(|_| ()))?;
            if *delete_source {
                with_fs(src, |fs| opusdm_ftp::remove_tree(fs, &items, cancel, progress))?;
            }
            Ok(())
        }
        Plan::Upload { locals, dest, delete_source } => {
            let locals = without(locals.clone(), |p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), skip);
            with_fs(dst, |fs| opusdm_ftp::upload(fs, &locals, &dest.path, cancel, progress).map(|_| ()))?;
            if *delete_source {
                for local in &locals {
                    let removed = if local.is_dir() { std::fs::remove_dir_all(local) } else { std::fs::remove_file(local) };
                    removed.map_err(|e| RemoteError::Failed(format!("{}: {e}", local.display())))?;
                }
            }
            Ok(())
        }
        Plan::Relay { dest, delete_source, .. } => {
            let items = without(prepared.items, |i| i.name.clone(), skip);
            let scratch = remote::new_download_dir()?;
            let result = (|| {
                let local = with_fs(src, |fs| opusdm_ftp::download(fs, &items, &scratch, cancel, progress))?;
                with_fs(dst, |fs| opusdm_ftp::upload(fs, &local, &dest.path, cancel, progress).map(|_| ()))?;
                if *delete_source {
                    with_fs(src, |fs| opusdm_ftp::remove_tree(fs, &items, cancel, progress))?;
                }
                Ok(())
            })();
            let _ = std::fs::remove_dir_all(&scratch);
            result
        }
        Plan::Remove { .. } => with_fs(src, |fs| opusdm_ftp::remove_tree(fs, &prepared.items, cancel, progress)),
        Plan::Create { dest, name, file } => {
            let path = dest.join(name).path;
            with_fs(dst, |fs| if *file { fs.write_file(&path, &mut std::io::empty()).map(|_| ()) } else { fs.mkdir(&path) })
        }
    }
}

/// The remote servers a plan talks to: (source, destination).
fn servers(plan: &Plan) -> (Option<RemoteUrl>, Option<RemoteUrl>) {
    match plan {
        Plan::Download { src, .. } | Plan::Remove { src, .. } => (Some(src.clone()), None),
        Plan::Upload { dest, .. } | Plan::Create { dest, .. } => (None, Some(dest.clone())),
        Plan::Relay { src, dest, .. } => (Some(src.clone()), Some(dest.clone())),
    }
}

/// Messages from the worker to the window.
enum Msg {
    Prepared(Result<Prepared, RemoteError>),
    File(String),
    Done(Result<(), RemoteError>),
}

impl Ui {
    /// Entry point for `ToLister::RemoteTransfer`.
    pub fn run_remote_transfer(self: &Rc<Self>, op: Op, srcs: Vec<PathBuf>, dest_dir: PathBuf) {
        let plan = match plan(&op, &srcs, &dest_dir) {
            Ok(p) => p,
            Err(msg) => {
                crate::actions::flash(self, &msg);
                return;
            }
        };
        // Which folders to refresh when it is over.
        let mut touched = vec![dest_dir.clone()];
        if matches!(op, Op::Move | Op::Delete) {
            touched.extend(srcs.iter().filter_map(|s| remote::parent_of(s)));
        }
        let (src_url, dst_url) = servers(&plan);
        self.ensure_pair(src_url, dst_url, move |ui, src, dst| ui.begin_transfer(plan, src, dst, touched));
    }

    /// Make sure the connections named by `src` and `dst` exist (connecting
    /// as needed), then call `then` with them.
    fn ensure_pair(
        self: &Rc<Self>,
        src: Option<RemoteUrl>,
        dst: Option<RemoteUrl>,
        then: impl FnOnce(&Rc<Ui>, Option<SharedFs>, Option<SharedFs>) + 'static,
    ) {
        let after_src = move |ui: &Rc<Ui>, src_fs: Option<SharedFs>| match dst {
            Some(url) => ui.ensure_remote(&url.clone(), move |ui| {
                let dst_fs = ui.remote_session(&url);
                then(ui, src_fs, dst_fs);
            }),
            None => then(ui, src_fs, None),
        };
        match src {
            Some(url) => self.ensure_remote(&url.clone(), move |ui| {
                let src_fs = ui.remote_session(&url);
                after_src(ui, src_fs);
            }),
            None => after_src(self, None),
        }
    }

    /// Stage one, the existing-target question, then stage two.
    fn begin_transfer(self: &Rc<Self>, plan: Plan, src: Option<SharedFs>, dst: Option<SharedFs>, touched: Vec<PathBuf>) {
        let (tx, rx) = async_channel::unbounded::<Msg>();
        let plan = Arc::new(plan);
        let (src_t, dst_t, plan_t, tx_t) = (src.clone(), dst.clone(), plan.clone(), tx.clone());
        std::thread::spawn(move || {
            let _ = tx_t.send_blocking(Msg::Prepared(prepare(&plan_t, &src_t, &dst_t)));
        });
        let window = TransferWindow::open(self);
        let ui = self.clone();
        let cancel = window.cancel.clone();
        glib::spawn_future_local(async move {
            let mut started = false;
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Msg::Prepared(Err(e)) => {
                        window.close();
                        crate::actions::flash(&ui, &format!("remote transfer failed: {e}"));
                        return;
                    }
                    Msg::Prepared(Ok(prepared)) if !started => {
                        started = true;
                        let (ui2, cancel, tx, plan, src, dst) = (ui.clone(), cancel.clone(), tx.clone(), plan.clone(), src.clone(), dst.clone());
                        ui.ask_overwrite(&window, prepared.existing.clone(), move |skip| {
                            let Some(skip) = skip else {
                                ui2.finish_transfer(Err(RemoteError::Failed("cancelled".to_string())), &[]);
                                return;
                            };
                            std::thread::spawn(move || {
                                let result = execute(&plan, prepared, &skip, &src, &dst, &cancel, &mut |p| {
                                    let _ = tx.send_blocking(Msg::File(p.to_string()));
                                });
                                let _ = tx.send_blocking(Msg::Done(result));
                            });
                        });
                    }
                    Msg::Prepared(Ok(_)) => {}
                    Msg::File(path) => window.set_text(&path),
                    Msg::Done(result) => {
                        window.close();
                        ui.finish_transfer(result, &touched);
                        return;
                    }
                }
            }
        });
    }

    /// Ask what to do with `existing` names: `Some(names to skip)` or `None`
    /// to cancel. No question when nothing exists.
    fn ask_overwrite(self: &Rc<Self>, window: &TransferWindow, existing: Vec<String>, then: impl FnOnce(Option<Vec<String>>) + 'static) {
        if existing.is_empty() {
            then(Some(Vec::new()));
            return;
        }
        let then = Rc::new(std::cell::RefCell::new(Some(then)));
        let detail = existing.iter().take(10).cloned().collect::<Vec<_>>().join("\n");
        let detail = if existing.len() > 10 { format!("{detail}\n... and {} more", existing.len() - 10) } else { detail };
        opusdm_ui::requester(
            &window.window,
            "Already exist",
            &format!("{} item(s) already exist at the destination.", existing.len()),
            Some(&detail),
            &[("Overwrite", 0u8), ("Skip existing", 1), ("Cancel", 2)],
            move |choice| {
                if let Some(then) = then.borrow_mut().take() {
                    then(match choice {
                        0 => Some(Vec::new()),
                        1 => Some(existing.clone()),
                        _ => None,
                    });
                }
            },
        );
    }

    /// Report the outcome and refresh everything that may have changed.
    fn finish_transfer(self: &Rc<Self>, result: Result<(), RemoteError>, touched: &[PathBuf]) {
        match result {
            Ok(()) => crate::actions::flash(self, "transfer complete"),
            Err(RemoteError::Failed(m)) if m == "cancelled" => crate::actions::flash(self, "transfer cancelled"),
            Err(e) => crate::actions::flash(self, &format!("transfer failed: {e}")),
        }
        for dir in touched {
            let _ = self.hub.client.send(&ToHub::Invalidate(dir.clone()));
        }
        self.reload();
    }
}

/// The small window shown while a remote transfer runs.
struct TransferWindow {
    window: gtk::Window,
    label: gtk::Label,
    cancel: Arc<AtomicBool>,
}

impl TransferWindow {
    fn open(ui: &Rc<Ui>) -> Rc<Self> {
        let shell = opusdm_ui::DialogShell::new(&ui.window, "Remote transfer").modal(false).size(420, 120);
        let label = gtk::Label::new(Some("Preparing..."));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        shell.content().append(&label);
        let bar = gtk::ProgressBar::new();
        bar.set_pulse_step(0.1);
        shell.content().append(&bar);
        let cancel_btn = shell.button("Cancel");
        let window = shell.present();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let cancel = cancel.clone();
            cancel_btn.connect_clicked(move |b| {
                cancel.store(true, Ordering::Relaxed);
                b.set_sensitive(false);
            });
        }
        {
            // Closing the window cancels too.
            let cancel = cancel.clone();
            window.connect_close_request(move |_| {
                cancel.store(true, Ordering::Relaxed);
                glib::Propagation::Proceed
            });
        }
        {
            let bar = bar.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                if bar.root().is_none() {
                    return glib::ControlFlow::Break;
                }
                bar.pulse();
                glib::ControlFlow::Continue
            });
        }
        Rc::new(TransferWindow { window, label, cancel })
    }

    fn set_text(&self, text: &str) {
        self.label.set_text(text);
    }

    fn close(&self) {
        self.window.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn plans_follow_where_the_sources_and_the_destination_are() {
        let down = plan(&Op::Move, &[p("sftp://u@h/a/x"), p("sftp://u@h/a/y")], Path::new("/local")).unwrap();
        assert_eq!(
            down,
            Plan::Download { src: RemoteUrl::parse("sftp://u@h/a/x").unwrap(), paths: vec!["/a/x".into(), "/a/y".into()], dest: p("/local"), delete_source: true }
        );
        assert!(matches!(plan(&Op::Copy, &[p("/l/a")], Path::new("ftp://h/in")).unwrap(), Plan::Upload { delete_source: false, .. }));
        assert!(matches!(plan(&Op::Copy, &[p("ftp://h/a")], Path::new("sftp://h2/in")).unwrap(), Plan::Relay { .. }));
        assert!(matches!(plan(&Op::Delete, &[p("ftp://h/a")], Path::new("ftp://h/")).unwrap(), Plan::Remove { .. }));
        assert!(matches!(
            plan(&Op::MakeDir { name: "n".into() }, &[], Path::new("ftp://h/")).unwrap(),
            Plan::Create { file: false, .. }
        ));
        assert!(matches!(plan(&Op::NewFile { name: "n".into() }, &[], Path::new("ftp://h/")).unwrap(), Plan::Create { file: true, .. }));
    }

    #[test]
    fn mixed_other_server_and_unsupported_jobs_are_refused() {
        assert!(plan(&Op::Copy, &[p("ftp://h/a"), p("/l/b")], Path::new("/d")).is_err());
        assert!(plan(&Op::Copy, &[p("ftp://h/a"), p("ftp://other/b")], Path::new("/d")).is_err());
        assert!(plan(&Op::Duplicate, &[p("ftp://h/a")], Path::new("ftp://h/")).is_err());
        assert!(plan(&Op::Chmod { mode: 0o644, recurse: false }, &[p("ftp://h/a")], Path::new("ftp://h/")).is_err());
    }

    #[test]
    fn skipping_removes_the_named_entries_only() {
        let kept = without(vec!["a".to_string(), "b".to_string(), "c".to_string()], |s| s.clone(), &["b".to_string()]);
        assert_eq!(kept, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn servers_name_the_connections_a_plan_needs() {
        let plan = plan(&Op::Copy, &[p("ftp://h/a")], Path::new("sftp://h2/in")).unwrap();
        let (src, dst) = servers(&plan);
        assert_eq!((src.unwrap().host, dst.unwrap().host), ("h".to_string(), "h2".to_string()));
    }

    use opusdm_ftp::{RemoteEntry, RemoteFs};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    /// An in-memory server: full path to content (`None` for a folder).
    #[derive(Default)]
    struct MemFs {
        nodes: BTreeMap<String, Option<Vec<u8>>>,
    }

    impl RemoteFs for MemFs {
        fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, RemoteError> {
            let prefix = format!("{}/", path.trim_end_matches('/'));
            Ok(self
                .nodes
                .iter()
                .filter_map(|(p, v)| {
                    let rest = p.strip_prefix(&prefix)?;
                    (!rest.contains('/')).then(|| RemoteEntry {
                        name: rest.to_string(),
                        is_dir: v.is_none(),
                        is_symlink: false,
                        size: v.as_ref().map_or(0, |d| d.len() as u64),
                        modified: 0,
                        mode: 0,
                    })
                })
                .collect())
        }
        fn read_file(&mut self, path: &str, out: &mut dyn Write) -> Result<u64, RemoteError> {
            match self.nodes.get(path) {
                Some(Some(d)) => {
                    out.write_all(d)?;
                    Ok(d.len() as u64)
                }
                _ => Err(RemoteError::Failed(format!("no file {path}"))),
            }
        }
        fn write_file(&mut self, path: &str, data: &mut dyn Read) -> Result<u64, RemoteError> {
            let mut buf = Vec::new();
            data.read_to_end(&mut buf)?;
            let n = buf.len() as u64;
            self.nodes.insert(path.to_string(), Some(buf));
            Ok(n)
        }
        fn mkdir(&mut self, path: &str) -> Result<(), RemoteError> {
            self.nodes.entry(path.to_string()).or_insert(None);
            Ok(())
        }
        fn remove_file(&mut self, path: &str) -> Result<(), RemoteError> {
            self.nodes.remove(path).map(|_| ()).ok_or_else(|| RemoteError::Failed("missing".into()))
        }
        fn remove_dir(&mut self, path: &str) -> Result<(), RemoteError> {
            self.nodes.remove(path).map(|_| ()).ok_or_else(|| RemoteError::Failed("missing".into()))
        }
        fn close(&mut self) {}
    }

    fn shared(files: &[(&str, Option<&str>)]) -> Option<SharedFs> {
        let mut fs = MemFs::default();
        for (p, c) in files {
            fs.nodes.insert(p.to_string(), c.map(|c| c.as_bytes().to_vec()));
        }
        let boxed: Box<dyn RemoteFs> = Box::new(fs);
        Some(Arc::new(Mutex::new(boxed)))
    }

    fn content(fs: &Option<SharedFs>, path: &str) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        with_fs(fs, |f| f.read_file(path, &mut out)).ok().map(|_| out)
    }

    fn run(plan: &Plan, skip: &[String], src: &Option<SharedFs>, dst: &Option<SharedFs>) -> (Prepared, Result<(), RemoteError>) {
        let prepared = prepare(plan, src, dst).unwrap();
        let again = Prepared { items: prepared.items.clone(), existing: prepared.existing.clone() };
        (prepared, execute(plan, again, skip, src, dst, &AtomicBool::new(false), &mut |_| {}))
    }

    #[test]
    fn upload_reports_existing_names_skips_them_and_move_deletes_only_what_was_sent() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("new.txt"), b"N").unwrap();
        std::fs::write(td.path().join("old.txt"), b"O").unwrap();
        let dst = shared(&[("/in", None), ("/in/old.txt", Some("keep"))]);
        let plan = Plan::Upload {
            locals: vec![td.path().join("new.txt"), td.path().join("old.txt")],
            dest: RemoteUrl::parse("sftp://h/in").unwrap(),
            delete_source: true,
        };
        let (prepared, result) = run(&plan, &["old.txt".to_string()], &None, &dst);
        assert_eq!(prepared.existing, vec!["old.txt".to_string()]);
        result.unwrap();
        assert_eq!(content(&dst, "/in/new.txt").unwrap(), b"N");
        assert_eq!(content(&dst, "/in/old.txt").unwrap(), b"keep");
        assert!(!td.path().join("new.txt").exists(), "a Move deletes what it sent");
        assert!(td.path().join("old.txt").exists(), "and keeps what it skipped");
    }

    #[test]
    fn download_move_deletes_the_remote_after_copying() {
        let td = tempfile::tempdir().unwrap();
        let src = shared(&[("/d", None), ("/d/a", Some("A")), ("/top", Some("T"))]);
        let plan = Plan::Download {
            src: RemoteUrl::parse("sftp://h/d").unwrap(),
            paths: vec!["/d".into(), "/top".into()],
            dest: td.path().to_path_buf(),
            delete_source: true,
        };
        let (prepared, result) = run(&plan, &[], &src, &None);
        assert!(prepared.existing.is_empty());
        result.unwrap();
        assert_eq!(std::fs::read(td.path().join("d/a")).unwrap(), b"A");
        assert_eq!(std::fs::read(td.path().join("top")).unwrap(), b"T");
        assert!(content(&src, "/d/a").is_none() && content(&src, "/top").is_none());
    }

    #[test]
    fn a_copy_leaves_the_remote_alone_and_flags_local_clashes() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("top"), b"mine").unwrap();
        let src = shared(&[("/top", Some("T"))]);
        let plan = Plan::Download { src: RemoteUrl::parse("sftp://h/").unwrap(), paths: vec!["/top".into()], dest: td.path().to_path_buf(), delete_source: false };
        let (prepared, result) = run(&plan, &["top".to_string()], &src, &None);
        assert_eq!(prepared.existing, vec!["top".to_string()]);
        result.unwrap();
        assert_eq!(std::fs::read(td.path().join("top")).unwrap(), b"mine");
        assert!(content(&src, "/top").is_some());
    }

    #[test]
    fn relay_copies_between_servers_and_move_clears_the_source() {
        let src = shared(&[("/a", None), ("/a/f", Some("F"))]);
        let dst = shared(&[("/in", None)]);
        let plan = Plan::Relay {
            src: RemoteUrl::parse("ftp://one/a").unwrap(),
            paths: vec!["/a".into()],
            dest: RemoteUrl::parse("sftp://two/in").unwrap(),
            delete_source: true,
        };
        run(&plan, &[], &src, &dst).1.unwrap();
        assert_eq!(content(&dst, "/in/a/f").unwrap(), b"F");
        assert!(content(&src, "/a/f").is_none());
    }

    #[test]
    fn remove_and_create_work_on_the_server() {
        let fs = shared(&[("/x", None), ("/x/f", Some("F")), ("/keep", Some("K"))]);
        let remove = Plan::Remove { src: RemoteUrl::parse("ftp://h/x").unwrap(), paths: vec!["/x".into()] };
        run(&remove, &[], &fs, &None).1.unwrap();
        assert!(content(&fs, "/x/f").is_none() && content(&fs, "/keep").is_some());

        let dir = Plan::Create { dest: RemoteUrl::parse("ftp://h/").unwrap(), name: "newdir".into(), file: false };
        run(&dir, &[], &None, &fs).1.unwrap();
        let file = Plan::Create { dest: RemoteUrl::parse("ftp://h/").unwrap(), name: "empty".into(), file: true };
        run(&file, &[], &None, &fs).1.unwrap();
        assert_eq!(content(&fs, "/empty").unwrap(), b"");
        assert!(with_fs(&fs, |f| f.list("/")).unwrap().iter().any(|e| e.name == "newdir" && e.is_dir));
    }

    #[test]
    fn a_cancelled_transfer_does_not_delete_the_source() {
        let td = tempfile::tempdir().unwrap();
        let src = shared(&[("/top", Some("T"))]);
        let plan = Plan::Download { src: RemoteUrl::parse("sftp://h/").unwrap(), paths: vec!["/top".into()], dest: td.path().to_path_buf(), delete_source: true };
        let prepared = prepare(&plan, &src, &None).unwrap();
        let result = execute(&plan, prepared, &[], &src, &None, &AtomicBool::new(true), &mut |_| {});
        assert!(result.is_err());
        assert!(content(&src, "/top").is_some());
    }
}
