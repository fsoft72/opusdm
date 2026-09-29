//! Acceptance tests for `JobOptions`, `CopyAs`, `MoveAs` and `Duplicate`.

use filetime::{set_file_mtime, FileTime};
use opusdm_fsops::{run_job, CancelToken, JobSpec, Progress};
use opusdm_ipc::{JobOptions, Op, Resolution};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

/// Run `spec` to completion, answering conflicts with `Overwrite`, and
/// return `(copied, skipped, errors)`.
fn run(spec: JobSpec) -> (u64, u64, Vec<(PathBuf, String)>) {
    let (tx, rx) = channel::<Progress>();
    let h = std::thread::spawn(move || run_job(spec, tx, CancelToken::new()));
    let mut out = None;
    while let Ok(ev) = rx.recv() {
        match ev {
            Progress::Conflict { reply, .. } => {
                let _ = reply.send(Resolution::Overwrite);
            }
            Progress::Done { copied, skipped, errors, .. } => out = Some((copied, skipped, errors)),
            _ => {}
        }
    }
    h.join().unwrap();
    out.expect("job sent Done")
}

fn spec(op: Op, srcs: Vec<PathBuf>, dest: &Path, options: JobOptions) -> JobSpec {
    JobSpec { op, srcs, dest_dir: dest.to_path_buf(), options }
}

fn write_at(path: &Path, body: &str, mtime: i64) {
    fs::write(path, body).unwrap();
    set_file_mtime(path, FileTime::from_unix_time(mtime, 0)).unwrap();
}

#[test]
fn newer_copies_only_the_newer_file() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    write_at(&src.join("new.txt"), "fresh", 2_000_000);
    write_at(&dst.join("new.txt"), "stale", 1_000_000);
    write_at(&src.join("old.txt"), "stale", 1_000_000);
    write_at(&dst.join("old.txt"), "fresh", 2_000_000);
    write_at(&src.join("missing.txt"), "added", 1_000_000);

    let options = JobOptions { newer: true, ..JobOptions::default() };
    let srcs = vec![src.join("new.txt"), src.join("old.txt"), src.join("missing.txt")];
    let (copied, skipped, errors) = run(spec(Op::Copy, srcs, &dst, options));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!((copied, skipped), (2, 1));
    assert_eq!(fs::read_to_string(dst.join("new.txt")).unwrap(), "fresh");
    assert_eq!(fs::read_to_string(dst.join("old.txt")).unwrap(), "fresh");
    assert_eq!(fs::read_to_string(dst.join("missing.txt")).unwrap(), "added");
}

#[test]
fn update_copies_only_names_that_exist_at_the_destination() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A2").unwrap();
    fs::write(src.join("b.txt"), "B2").unwrap();
    fs::write(dst.join("a.txt"), "A1").unwrap();

    let options = JobOptions { update: true, ..JobOptions::default() };
    let (copied, skipped, _) =
        run(spec(Op::Copy, vec![src.join("a.txt"), src.join("b.txt")], &dst, options));

    assert_eq!((copied, skipped), (1, 1));
    assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "A2");
    assert!(!dst.join("b.txt").exists());
}

#[test]
fn filtered_recursive_copy_skips_other_files_and_empty_dirs() {
    let td = tempfile::tempdir().unwrap();
    let (root, dst) = (td.path().join("proj"), td.path().join("d"));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
    fs::write(root.join("src/notes.txt"), "n").unwrap();
    fs::write(root.join("docs/readme.md"), "r").unwrap();

    let options = JobOptions { include_glob: Some("*.rs".into()), ..JobOptions::default() };
    let (copied, _, errors) = run(spec(Op::Copy, vec![root], &dst, options));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(copied, 1);
    assert!(dst.join("proj/src/main.rs").is_file());
    assert!(!dst.join("proj/src/notes.txt").exists());
    assert!(!dst.join("proj/docs").exists());
}

#[test]
fn filtered_move_deletes_only_what_it_copied() {
    let td = tempfile::tempdir().unwrap();
    let (root, dst) = (td.path().join("proj"), td.path().join("d"));
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(root.join("sub/a.rs"), "a").unwrap();
    fs::write(root.join("sub/keep.txt"), "k").unwrap();
    fs::write(root.join("b.rs"), "b").unwrap();

    let options = JobOptions { include_glob: Some("*.rs".into()), ..JobOptions::default() };
    let (copied, _, errors) = run(spec(Op::Move, vec![root.clone()], &dst, options));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(copied, 2);
    assert!(dst.join("proj/sub/a.rs").is_file() && dst.join("proj/b.rs").is_file());
    assert!(!root.join("b.rs").exists() && !root.join("sub/a.rs").exists());
    assert!(root.join("sub/keep.txt").is_file(), "unmatched file must survive");
}

#[test]
fn newer_move_keeps_skipped_sources() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    write_at(&src.join("old.txt"), "old", 1_000_000);
    write_at(&dst.join("old.txt"), "newer", 2_000_000);
    write_at(&src.join("fresh.txt"), "fresh", 2_000_000);

    let options = JobOptions { newer: true, ..JobOptions::default() };
    let srcs = vec![src.join("old.txt"), src.join("fresh.txt")];
    run(spec(Op::Move, srcs, &dst, options));

    assert!(src.join("old.txt").exists(), "skipped source must not be deleted");
    assert!(!src.join("fresh.txt").exists());
    assert_eq!(fs::read_to_string(dst.join("old.txt")).unwrap(), "newer");
}

#[test]
fn duplicate_makes_distinct_copy_names() {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    for n in ["a", "b", "c", "a.copy"] {
        fs::write(d.join(n), n).unwrap();
    }
    let srcs = vec![d.join("a"), d.join("b"), d.join("c"), d.join("a.copy")];
    let (copied, _, errors) = run(spec(Op::Duplicate, srcs, d, JobOptions::default()));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(copied, 4);
    assert_eq!(fs::read_to_string(d.join("a.copy2")).unwrap(), "a");
    assert_eq!(fs::read_to_string(d.join("b.copy")).unwrap(), "b");
    assert_eq!(fs::read_to_string(d.join("c.copy")).unwrap(), "c");
    assert_eq!(fs::read_to_string(d.join("a.copy.copy")).unwrap(), "a.copy");
}

#[test]
fn copy_as_renames_each_source() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(src.join("dir")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A").unwrap();
    fs::write(src.join("dir/x"), "X").unwrap();

    let op = Op::CopyAs { new_names: vec!["z.txt".into(), "dir2".into()] };
    let (copied, _, errors) =
        run(spec(op, vec![src.join("a.txt"), src.join("dir")], &dst, JobOptions::default()));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(copied, 2);
    assert_eq!(fs::read_to_string(dst.join("z.txt")).unwrap(), "A");
    assert_eq!(fs::read_to_string(dst.join("dir2/x")).unwrap(), "X");
    assert!(src.join("a.txt").exists());
}

#[test]
fn move_as_renames_and_removes_the_source() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A").unwrap();

    let op = Op::MoveAs { new_names: vec!["b.txt".into()] };
    let (_, _, errors) = run(spec(op, vec![src.join("a.txt")], &dst, JobOptions::default()));

    assert!(errors.is_empty(), "{errors:?}");
    assert!(!src.join("a.txt").exists());
    assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "A");
}

#[test]
fn bad_name_lists_fail_before_touching_files() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A").unwrap();
    fs::write(src.join("b.txt"), "B").unwrap();
    let srcs = vec![src.join("a.txt"), src.join("b.txt")];

    for names in [
        vec!["only-one".to_string()],
        vec!["ok".to_string(), "x/y".to_string()],
        vec!["dup".to_string(), "dup".to_string()],
    ] {
        let op = Op::CopyAs { new_names: names };
        let (copied, _, errors) = run(spec(op, srcs.clone(), &dst, JobOptions::default()));
        assert_eq!(copied, 0);
        assert_eq!(errors.len(), 1);
    }
    assert_eq!(fs::read_dir(&dst).unwrap().count(), 0);
}

#[test]
fn empty_glob_is_rejected_before_touching_files() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A").unwrap();

    let options = JobOptions { include_glob: Some(" ; ".into()), ..JobOptions::default() };
    let (copied, _, errors) = run(spec(Op::Delete, vec![src.join("a.txt")], &dst, options));

    assert_eq!(copied, 0);
    assert_eq!(errors.len(), 1);
    assert!(src.join("a.txt").exists());
}

#[test]
fn filtered_delete_removes_only_matching_files() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("p");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.tmp"), "1").unwrap();
    fs::write(root.join("sub/b.tmp"), "2").unwrap();
    fs::write(root.join("sub/keep.rs"), "3").unwrap();

    let options = JobOptions { include_glob: Some("*.tmp".into()), ..JobOptions::default() };
    let (removed, _, errors) = run(spec(Op::Delete, vec![root.clone()], td.path(), options));

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(removed, 2);
    assert!(!root.join("a.tmp").exists() && !root.join("sub/b.tmp").exists());
    assert!(root.join("sub/keep.rs").is_file());
}

#[test]
fn move_when_same_turns_copy_into_a_move_on_one_device() {
    let td = tempfile::tempdir().unwrap();
    let (src, dst) = (td.path().join("s"), td.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("a.txt"), "A").unwrap();
    fs::write(src.join("b.txt"), "B").unwrap();

    let plain = JobOptions::default();
    run(spec(Op::Copy, vec![src.join("a.txt")], &dst, plain));
    assert!(src.join("a.txt").exists(), "plain Copy keeps the source");

    let options = JobOptions { move_when_same: true, ..JobOptions::default() };
    run(spec(Op::Copy, vec![src.join("b.txt")], &dst, options));
    assert!(!src.join("b.txt").exists(), "same device: Copy became a Move");
    assert!(dst.join("b.txt").exists());
}

#[test]
fn same_device_is_false_for_missing_paths() {
    assert!(!opusdm_fsops::same_device(&[], Path::new("/tmp")));
    assert!(!opusdm_fsops::same_device(&["/definitely/not/here".into()], Path::new("/tmp")));
}
