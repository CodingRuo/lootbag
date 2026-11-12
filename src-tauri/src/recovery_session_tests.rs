use super::*;
use std::{fs, path::PathBuf};
struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "game-assets-recovery-session-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn recovery_session_registers_before_work_retains_cancelled_pending_owner_and_finishes_by_arc() {
    let session = Session::default();
    let (_, generation, old) = session.begin_recovery(None, "owned-one").unwrap();
    assert_eq!(generation, 0);
    session.cancel_recovery(Some("owned-one")).unwrap();
    assert!(old.cancellation_flag().load(Ordering::Relaxed));
    assert!(session.begin_recovery(None, "owned-one").is_err());
    let (_, _, new) = session.begin_recovery(None, "owned-two").unwrap();
    session.finish_recovery("owned-one", &old).unwrap();
    assert!(session
        .recovery
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|j| Arc::ptr_eq(&j.control, &new)));
    session.finish_recovery("owned-two", &new).unwrap();
    assert!(session.recovery.lock().unwrap().is_none());
}
#[test]
fn recovery_session_null_root_same_path_reopen_and_a_b_a_revoke_old_generation() {
    let session = Session::default();
    let w = Workspace::new();
    let a = w.0.join("A");
    let b = w.0.join("B");
    let (_, old_generation, old) = session.begin_recovery(None, "owned-null").unwrap();
    session.set_root(a.clone()).unwrap();
    assert!(old.cancellation_flag().load(Ordering::Relaxed));
    assert!(session.root_generation.load(Ordering::Relaxed) > old_generation);
    session.finish_recovery("owned-null", &old).unwrap();
    let (_, g, first) = session.begin_recovery(Some(&a), "owned-a").unwrap();
    session.set_root(b).unwrap();
    session.set_root(a.clone()).unwrap();
    assert!(first.cancellation_flag().load(Ordering::Relaxed));
    assert!(session.root_generation.load(Ordering::Relaxed) > g);
    let (_, g2, reopened) = session.begin_recovery(Some(&a), "owned-reopen").unwrap();
    session.set_root(a).unwrap();
    assert!(reopened.cancellation_flag().load(Ordering::Relaxed));
    assert!(session.root_generation.load(Ordering::Relaxed) > g2);
}
#[test]
fn recovery_session_real_ready_plan_is_single_use_and_source_writer_gate_is_explicit() {
    let w = Workspace::new();
    let incoming = w.0.join("Incoming");
    fs::create_dir(&incoming).unwrap();
    fs::write(incoming.join("Owned.wav"), b"owned-sound").unwrap();
    let mut library = Library::create(&w.0.join("Library")).unwrap();
    library.import_folder(&incoming).unwrap();
    let root = library.root().to_owned();
    drop(library);
    let output = w.0.join("Backup");
    fs::create_dir(&output).unwrap();
    let session = Session::default();
    session.set_root(root.clone()).unwrap();
    {
        let _gate = session.import_gate.lock().unwrap();
        assert!(session.begin_recovery(Some(&root), "owned-busy").is_err());
        assert!(session.recovery.lock().unwrap().is_none());
    }
    let (captured, generation, control) =
        session.begin_recovery(Some(&root), "owned-ready").unwrap();
    let plan = prepare_recovery(
        captured.as_deref(),
        RecoveryRequest::Backup {
            destination_parent: output.to_str().unwrap().into(),
        },
        &control,
        "owned-ready",
        |_| {},
    )
    .unwrap();
    let destination = plan.destination.clone();
    session
        .attach_recovery(
            captured.as_deref(),
            generation,
            "owned-ready",
            &control,
            plan,
        )
        .unwrap();
    let (captured, generation, control, plan) = session.take_recovery("owned-ready").unwrap();
    assert!(session.take_recovery("owned-ready").is_err());
    let mut publisher = |op: &mut dyn FnMut() -> Result<Option<String>>| {
        session.with_recovery_root(captured.as_deref(), generation, "owned-ready", &control, op)
    };
    let result = run_recovery(plan, &control, "owned-ready", |_| {}, &mut publisher);
    assert!(
        matches!(result, RecoveryOutcome::Published { .. }),
        "{result:?}"
    );
    assert!(destination.join("manifest.json").exists());
    assert_eq!(
        session.root.lock().unwrap().as_deref(),
        Some(root.as_path())
    );
    session.finish_recovery("owned-ready", &control).unwrap();
    assert!(session.recovery.lock().unwrap().is_none());
}
#[test]
fn recovery_session_cancelled_ready_is_disposed_and_late_attach_cannot_revive() {
    let w = Workspace::new();
    let root = Library::create(&w.0.join("Library"))
        .unwrap()
        .root()
        .to_owned();
    let output = w.0.join("Backup");
    fs::create_dir(&output).unwrap();
    let session = Session::default();
    session.set_root(root.clone()).unwrap();
    let (captured, generation, control) =
        session.begin_recovery(Some(&root), "owned-late").unwrap();
    let plan = prepare_recovery(
        captured.as_deref(),
        RecoveryRequest::Backup {
            destination_parent: output.to_str().unwrap().into(),
        },
        &control,
        "owned-late",
        |_| {},
    )
    .unwrap();
    session.cancel_recovery(Some("owned-late")).unwrap();
    assert!(session
        .attach_recovery(
            captured.as_deref(),
            generation,
            "owned-late",
            &control,
            plan
        )
        .is_err());
    session.finish_recovery("owned-late", &control).unwrap();
    let (captured, generation, control) = session
        .begin_recovery(Some(&root), "owned-ready-stop")
        .unwrap();
    let plan = prepare_recovery(
        captured.as_deref(),
        RecoveryRequest::Backup {
            destination_parent: output.to_str().unwrap().into(),
        },
        &control,
        "owned-ready-stop",
        |_| {},
    )
    .unwrap();
    session
        .attach_recovery(
            captured.as_deref(),
            generation,
            "owned-ready-stop",
            &control,
            plan,
        )
        .unwrap();
    session.cancel_recovery(Some("owned-ready-stop")).unwrap();
    assert!(session.recovery.lock().unwrap().is_none());
    assert!(session.take_recovery("owned-ready-stop").is_err());
    assert_eq!(fs::read_dir(output).unwrap().count(), 0);
}
#[test]
fn recovery_session_restores_from_null_root_and_leaves_active_library_unopened() {
    let w = Workspace::new();
    let root = Library::create(&w.0.join("Library"))
        .unwrap()
        .root()
        .to_owned();
    let output = w.0.join("Backup");
    fs::create_dir(&output).unwrap();
    let control = ImportControl::default();
    let plan = prepare_recovery(
        Some(&root),
        RecoveryRequest::Backup {
            destination_parent: output.to_str().unwrap().into(),
        },
        &control,
        "owned-artifact",
        |_| {},
    )
    .unwrap();
    let artifact = plan.destination.clone();
    let mut direct = |op: &mut dyn FnMut() -> Result<Option<String>>| op();
    assert!(matches!(
        run_recovery(plan, &control, "owned-artifact", |_| {}, &mut direct),
        RecoveryOutcome::Published { .. }
    ));
    let destination = w.0.join("Restores");
    fs::create_dir(&destination).unwrap();
    let session = Session::default();
    let (captured, generation, control) =
        session.begin_recovery(None, "owned-null-restore").unwrap();
    let plan = prepare_recovery(
        None,
        RecoveryRequest::Restore {
            artifact_root: artifact.to_str().unwrap().into(),
            destination_parent: destination.to_str().unwrap().into(),
        },
        &control,
        "owned-null-restore",
        |_| {},
    )
    .unwrap();
    let final_path = plan.destination.clone();
    session
        .attach_recovery(None, generation, "owned-null-restore", &control, plan)
        .unwrap();
    let (_, generation, control, plan) = session.take_recovery("owned-null-restore").unwrap();
    let mut publisher = |op: &mut dyn FnMut() -> Result<Option<String>>| {
        session.with_recovery_root(
            captured.as_deref(),
            generation,
            "owned-null-restore",
            &control,
            op,
        )
    };
    let result = run_recovery(plan, &control, "owned-null-restore", |_| {}, &mut publisher);
    assert!(
        matches!(result, RecoveryOutcome::Published { .. }),
        "{result:?}"
    );
    assert!(final_path.join("catalog.sqlite3").exists());
    assert!(session.root.lock().unwrap().is_none());
    session
        .finish_recovery("owned-null-restore", &control)
        .unwrap();
}
