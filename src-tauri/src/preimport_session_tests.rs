use super::*;
use std::{fs, sync::atomic::AtomicU64};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "preimport-session-owned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(w: &Workspace) -> (Session, PathBuf, PathBuf) {
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("One.wav"), b"abc").unwrap();
    let library = Library::create(&w.0.join("Library")).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let session = Session::default();
    session.set_root(root.clone()).unwrap();
    (session, root, incoming)
}
#[test]
fn preimport_owner_exists_before_prepare_and_cancelled_unfinished_id_cannot_be_reused() {
    let w = Workspace::new();
    let (session, root, incoming) = fixture(&w);
    let (_, gen, owner) = session.begin_preimport(&root, "pending").unwrap();
    let prepared = prepare_import_evidence(&root, &incoming, false, &owner, |_| {}).unwrap();
    session.cancel_prepared_import(Some("pending")).unwrap();
    assert!(session.begin_preimport(&root, "pending").is_err());
    assert!(session
        .attach_preimport(&root, gen, "pending", &owner, prepared, false)
        .is_err());
    assert!(owner.cancellation_flag().load(Ordering::Relaxed));
    session.finish_preimport("pending", &owner).unwrap();
    assert!(!root.join("packages").exists());
}
#[test]
fn preimport_same_path_reopen_and_root_a_b_a_reject_original_owner_and_late_cleanup() {
    let w = Workspace::new();
    let (session, root, incoming) = fixture(&w);
    let (_, gen, old) = session.begin_preimport(&root, "old").unwrap();
    let plan = prepare_import_evidence(&root, &incoming, false, &old, |_| {}).unwrap();
    session.set_root(root.clone()).unwrap();
    assert!(session
        .attach_preimport(&root, gen, "old", &old, plan, false)
        .is_err());
    session.set_root(w.0.join("B")).unwrap();
    session.set_root(root.clone()).unwrap();
    let (_, newgen, new) = session.begin_preimport(&root, "new").unwrap();
    assert_ne!(gen, newgen);
    session.finish_preimport("old", &old).unwrap();
    assert!(session
        .prepared_import
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|j| Arc::ptr_eq(&j.control, &new)));
    session.cancel_prepared_import(Some("old")).unwrap();
    assert!(!new.cancellation_flag().load(Ordering::Relaxed));
}
#[test]
fn preimport_ready_plan_is_single_use_and_root_switch_during_copy_preserves_catalog() {
    let w = Workspace::new();
    let (session, root, incoming) = fixture(&w);
    let before = fs::read(root.join("catalog.sqlite3")).unwrap();
    let (_, gen, owner) = session.begin_preimport(&root, "copy").unwrap();
    let plan = prepare_import_evidence(&root, &incoming, false, &owner, |_| {}).unwrap();
    let reply = session
        .attach_preimport(&root, gen, "copy", &owner, plan, false)
        .unwrap();
    assert_eq!(reply.binding, "verified");
    assert!(reply.zip_estimate.is_none());
    let (_, generation, control, plan, authorized) = session.take_preimport("copy", false).unwrap();
    assert!(session.take_preimport("copy", false).is_err());
    let mut stopped = false;
    let result = consume_import_evidence(
        &root,
        plan,
        authorized,
        &control,
        &mut |p| {
            if p.completed_bytes > 0 && !stopped {
                stopped = true;
                session.set_root(w.0.join("B")).unwrap();
            }
        },
        &mut |operation| {
            session.with_preimport_root(&root, generation, "copy", &control, operation)
        },
    );
    assert!(result.is_err());
    assert_eq!(fs::read(root.join("catalog.sqlite3")).unwrap(), before);
    assert!(fs::read_dir(root.join("packages"))
        .unwrap()
        .next()
        .is_none());
}
#[test]
fn preimport_unverified_requires_native_authorization_and_stop_during_estimate_cannot_attach() {
    let w = Workspace::new();
    let (session, root, _) = fixture(&w);
    let zip = w.0.join("Broken.zip");
    fs::write(&zip, b"broken").unwrap();
    let (_, generation, owner) = session.begin_preimport(&root, "zip").unwrap();
    let plan = prepare_import_evidence(&root, &zip, true, &owner, |_| {}).unwrap();
    session
        .attach_preimport(&root, generation, "zip", &owner, plan, false)
        .unwrap();
    assert!(session.take_preimport("zip", false).is_err());
    let (_, gen, control, plan, _) = session.take_preimport("zip", true).unwrap();
    assert!(session.take_preimport("zip", false).is_err());
    session.cancel_prepared_import(Some("zip")).unwrap();
    assert!(session
        .attach_preimport(&root, gen, "zip", &control, plan, true)
        .is_err());
    assert!(!root.join("packages").exists());
}
