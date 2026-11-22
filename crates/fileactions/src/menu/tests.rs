use super::*;
use opusdm_ipc::filetypes::{Actions, MatchRule};
use std::cell::RefCell;
use std::path::Path;

/// A recorded item: its title and its action.
type RecordedItem = (String, Box<dyn Fn()>);

/// A sink that records the titles (and `---` for a separator) and keeps the
/// actions so a test can click one.
#[derive(Default)]
struct Recorder {
    titles: RefCell<Vec<String>>,
    actions: RefCell<Vec<RecordedItem>>,
}

impl MenuSink for Recorder {
    fn item(&self, title: &str, action: Box<dyn Fn()>) {
        self.titles.borrow_mut().push(title.to_string());
        self.actions.borrow_mut().push((title.to_string(), action));
    }

    fn separator(&self) {
        self.titles.borrow_mut().push("---".to_string());
    }
}

impl Recorder {
    /// Click the item titled `title`.
    fn click(&self, title: &str) {
        let actions = self.actions.borrow();
        let (_, action) = actions.iter().find(|(t, _)| t == title).unwrap_or_else(|| panic!("no item {title}"));
        action();
    }
}

/// A host that answers from fixed values and records what it was asked.
#[derive(Default)]
struct FakeHost {
    selected: Vec<PathBuf>,
    dest: Option<String>,
    file_type: Option<FileType>,
    functions: Vec<Function>,
    bottom: Option<&'static str>,
    calls: RefCell<Vec<String>>,
}

impl FakeHost {
    fn log(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
}

impl FileActionHost for FakeHost {
    fn parent_window(&self) -> gtk4::Window {
        panic!("the menu must not open a dialog while it is filled")
    }
    fn notify(&self, text: &str) {
        self.log(format!("notify {text}"));
    }
    fn folder(&self) -> PathBuf {
        PathBuf::from("/d")
    }
    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected.clone()
    }
    fn dest_check(&self) -> Result<(), String> {
        match &self.dest {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }
    fn submit_job(&self, op: Op, srcs: Vec<PathBuf>, dest_dir: Option<PathBuf>) {
        self.log(format!("job {op:?} {srcs:?} {dest_dir:?}"));
    }
    fn rename(&self, path: &Path, old_name: &str) {
        self.log(format!("rename {} {old_name}", path.display()));
    }
    fn confirm_delete(&self, message: &str, srcs: Vec<PathBuf>, permanent: bool) {
        self.log(format!("delete {message} {srcs:?} {permanent}"));
    }
    fn functions(&self) -> Vec<Function> {
        self.functions.clone()
    }
    fn file_type(&self, _target: &MenuTarget) -> Option<FileType> {
        self.file_type.clone()
    }
    fn run_on_target(&self, target: &MenuTarget, id: &FunctionId) {
        self.log(format!("run {id} on {}", target.name));
    }
    fn open_in_viewer(&self, paths: Vec<PathBuf>) {
        self.log(format!("viewer {paths:?}"));
    }
    fn set_background(&self, path: &Path) {
        self.log(format!("background {}", path.display()));
    }
    fn inside_archive(&self, path: &Path) -> bool {
        path.to_string_lossy().contains('#')
    }
    fn extra_items(&self, place: ExtraPlace, menu: &dyn MenuSink, _target: &MenuTarget) {
        match place {
            ExtraPlace::Top => menu.item("Top", Box::new(|| {})),
            ExtraPlace::Bottom => {
                if let Some(title) = self.bottom {
                    menu.item(title, Box::new(|| {}));
                }
            }
        }
    }
}

fn file(name: &str) -> MenuTarget {
    MenuTarget { path: Path::new("/d").join(name), name: name.to_string(), is_dir: false, size: 1, modified: 0, elsewhere: false }
}

fn fill(host: FakeHost, target: MenuTarget) -> (Rc<FakeHost>, Recorder) {
    let host = Rc::new(host);
    let dyn_host: Rc<dyn FileActionHost> = host.clone();
    let rec = Recorder::default();
    fill_entry_menu(&rec, &dyn_host, target);
    (host, rec)
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_plain_file_gets_the_lister_menu_in_the_lister_order() {
    let (_, rec) = fill(FakeHost { bottom: Some("Leave Out on Desktop"), ..Default::default() }, file("a.txt"));
    assert_eq!(
        *rec.titles.borrow(),
        strs(&[
            "Top",
            "Copy (Source -> Dest)",
            "Rename...",
            "Move (Source -> Dest)",
            "Delete...",
            "Change Permissions...",
            "Open with...",
            "Compress...",
            "Encrypt...",
            "---",
            "Open in Viewer",
            "Leave Out on Desktop",
        ])
    );
}

#[test]
fn archives_encrypted_files_and_pictures_get_their_own_items() {
    let (_, rec) = fill(FakeHost::default(), file("a.zip"));
    assert!(rec.titles.borrow().contains(&"Extract archive".to_string()));
    assert!(rec.titles.borrow().contains(&"Extract here".to_string()));
    let (_, rec) = fill(FakeHost::default(), file("a.zip#inner.zip"));
    assert!(!rec.titles.borrow().contains(&"Extract archive".to_string()), "no Extract inside an archive");
    let (_, rec) = fill(FakeHost::default(), file("a.txt.enc"));
    assert!(rec.titles.borrow().contains(&"Decrypt...".to_string()));
    let (_, rec) = fill(FakeHost::default(), file("a.png"));
    assert_eq!(rec.titles.borrow().last(), Some(&"Set as Desktop Background".to_string()));
    let mut dir = file("pics.png");
    dir.is_dir = true;
    let (_, rec) = fill(FakeHost::default(), dir);
    assert!(!rec.titles.borrow().contains(&"Set as Desktop Background".to_string()));
}

#[test]
fn an_entry_that_lives_elsewhere_has_no_rename_move_or_delete() {
    let mut target = file("a.txt");
    target.elsewhere = true;
    let (_, rec) = fill(FakeHost::default(), target);
    let titles = rec.titles.borrow();
    for gone in ["Rename...", "Move (Source -> Dest)", "Delete..."] {
        assert!(!titles.contains(&gone.to_string()), "{gone} offered on a left-out icon");
    }
    assert!(titles.contains(&"Copy (Source -> Dest)".to_string()));
}

#[test]
fn file_type_entries_come_after_a_separator() {
    let file_type = FileType {
        name: "T".into(),
        rule: MatchRule::Glob { pattern: "*".into() },
        actions: Actions { dblclick: None, drop: None, menu: vec!["exec:ls -l {f}".into()] },
    };
    let (host, rec) = fill(FakeHost { file_type: Some(file_type), ..Default::default() }, file("a.txt"));
    let titles = rec.titles.borrow().clone();
    let at = titles.iter().position(|t| t == "Change Permissions...").unwrap();
    assert_eq!(titles[at + 1], "---");
    assert_eq!(titles[at + 2], "Open With: ls -l {f}");
    rec.click("Open With: ls -l {f}");
    assert_eq!(*host.calls.borrow(), strs(&["run exec:ls -l {f} on a.txt"]));
}

#[test]
fn copy_needs_a_dest_and_goes_to_every_dest() {
    let (host, rec) = fill(FakeHost { dest: Some("no DEST Lister set".into()), ..Default::default() }, file("a.txt"));
    rec.click("Copy (Source -> Dest)");
    assert_eq!(*host.calls.borrow(), strs(&["notify no DEST Lister set"]));
    let (host, rec) = fill(FakeHost::default(), file("a.txt"));
    rec.click("Move (Source -> Dest)");
    assert_eq!(*host.calls.borrow(), strs(&["job Move [\"/d/a.txt\"] None"]));
}

#[test]
fn delete_asks_to_delete_the_clicked_entry_for_good() {
    let (host, rec) = fill(FakeHost::default(), file("a.txt"));
    rec.click("Delete...");
    assert_eq!(*host.calls.borrow(), strs(&["delete Delete \"a.txt\" permanently? [\"/d/a.txt\"] true"]));
}

#[test]
fn the_viewer_gets_the_selection_only_when_it_holds_the_clicked_entry() {
    let selected = vec![PathBuf::from("/d/a.txt"), PathBuf::from("/d/b.txt")];
    let (host, rec) = fill(FakeHost { selected: selected.clone(), ..Default::default() }, file("a.txt"));
    rec.click("Open in Viewer");
    let (other, rec_other) = fill(FakeHost { selected, ..Default::default() }, file("c.txt"));
    rec_other.click("Open in Viewer");
    assert_eq!(*host.calls.borrow(), strs(&["viewer [\"/d/a.txt\", \"/d/b.txt\"]"]));
    assert_eq!(*other.calls.borrow(), strs(&["viewer [\"/d/c.txt\"]"]));
}
