//! Delete a file: the trash first, a permanent delete only when the user agrees.

use std::path::Path;

/// What deleting can do; a fake one drives the tests.
pub trait FileRemover {
    /// Move the file to the trash.
    fn trash(&self, path: &Path) -> Result<(), String>;
    /// Delete the file for good.
    fn remove(&self, path: &Path) -> Result<(), String>;
}

/// Outcome of the first, gentle step.
#[derive(Debug, PartialEq, Eq)]
pub enum TrashStep {
    /// The file is in the trash.
    Trashed,
    /// There is no trash for it: the user must agree to a permanent delete. Holds the reason.
    AskPermanent(String),
}

/// Try the trash.
pub fn try_trash(remover: &dyn FileRemover, path: &Path) -> TrashStep {
    match remover.trash(path) {
        Ok(()) => TrashStep::Trashed,
        Err(reason) => TrashStep::AskPermanent(reason),
    }
}

/// Delete for good, after the user agreed.
pub fn remove_permanently(remover: &dyn FileRemover, path: &Path) -> Result<(), String> {
    remover.remove(path)
}

/// The real thing: `gio` trash and `remove_file`.
pub struct SystemRemover;

impl FileRemover for SystemRemover {
    fn trash(&self, path: &Path) -> Result<(), String> {
        use gtk4::gio::prelude::FileExt;
        gtk4::gio::File::for_path(path).trash(gtk4::gio::Cancellable::NONE).map_err(|e| e.message().to_string())
    }

    fn remove(&self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        trash_ok: bool,
        remove_ok: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FileRemover for Fake {
        fn trash(&self, _: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push("trash");
            if self.trash_ok { Ok(()) } else { Err("no trash".into()) }
        }

        fn remove(&self, _: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push("remove");
            if self.remove_ok { Ok(()) } else { Err("denied".into()) }
        }
    }

    fn fake(trash_ok: bool, remove_ok: bool) -> Fake {
        Fake { trash_ok, remove_ok, calls: RefCell::new(Vec::new()) }
    }

    #[test]
    fn trash_success_never_removes() {
        let f = fake(true, true);
        assert_eq!(try_trash(&f, Path::new("/x")), TrashStep::Trashed);
        assert_eq!(*f.calls.borrow(), ["trash"]);
    }

    #[test]
    fn trash_failure_asks_and_keeps_the_reason() {
        let f = fake(false, true);
        assert_eq!(try_trash(&f, Path::new("/x")), TrashStep::AskPermanent("no trash".into()));
        assert_eq!(*f.calls.borrow(), ["trash"]);
    }

    #[test]
    fn permanent_delete_reports_its_error() {
        assert!(remove_permanently(&fake(false, true), Path::new("/x")).is_ok());
        assert_eq!(remove_permanently(&fake(false, false), Path::new("/x")).unwrap_err(), "denied");
    }

    #[test]
    fn system_remover_deletes_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, b"x").unwrap();
        SystemRemover.remove(&file).unwrap();
        assert!(!file.exists());
        assert!(SystemRemover.remove(&file).is_err());
    }
}
