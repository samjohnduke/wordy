//! Two copies of a project edit the same scene offline, sync, and converge.

use std::net::SocketAddr;
use std::sync::{mpsc, Arc};

use wordy_doc::project::Project;
use wordy_doc::{NodeKind, Space};
use wordy_sync::{LocalState, Server, ServerEvent, SyncOutcome};

fn state(p: &Project, dir: &std::path::Path, name: &str, code: &str, words: &[&str]) -> LocalState {
    LocalState {
        project_id: p.id(),
        peer_name: name.into(),
        pairing_code: code.into(),
        snapshot: p.export_snapshot().unwrap(),
        assets_dir: dir.join("assets"),
        dictionary: words.iter().map(|w| w.to_string()).collect(),
    }
}

fn sync_pair(
    state_a: LocalState,
    state_b: LocalState,
) -> (anyhow::Result<SyncOutcome>, anyhow::Result<SyncOutcome>) {
    let (tx, rx) = mpsc::channel();
    let state_b = Arc::new(state_b);
    let server = Server::start(0, move |ev| match ev {
        ServerEvent::NeedState { reply, .. } => {
            reply.send(Ok((*state_b).clone())).unwrap();
        }
        ServerEvent::Finished { result, .. } => tx.send(result).unwrap(),
    })
    .unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", server.port()).parse().unwrap();
    let out_a = wordy_sync::sync_with(addr, &state_a);
    let out_b = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("server finished");
    (out_a, out_b)
}

#[test]
fn concurrent_edits_converge_and_assets_and_words_cross() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("A");
    let dir_b = tmp.path().join("B");
    let a = Project::create(&dir_a, "Novel").unwrap();
    let scene = a
        .create_node(a.root(Space::Manuscript), NodeKind::Scene, "One")
        .unwrap();
    a.node(scene)
        .unwrap()
        .body()
        .unwrap()
        .insert(0, "It was a dark night.")
        .unwrap();
    a.save().unwrap();

    // Copy the folder to the other machine.
    std::fs::create_dir_all(dir_b.join("assets")).unwrap();
    std::fs::copy(
        wordy_doc::storage::snapshot_path(&dir_a),
        wordy_doc::storage::snapshot_path(&dir_b),
    )
    .unwrap();
    let b = Project::open(&dir_b).unwrap();
    assert_eq!(a.id(), b.id());
    assert!(!a.id().is_empty());

    // Offline edits on both sides of the same scene, plus one asset each.
    a.node(scene)
        .unwrap()
        .body()
        .unwrap()
        .insert(20, " The wind howled.")
        .unwrap();
    b.node(scene)
        .unwrap()
        .body()
        .unwrap()
        .insert(0, "Suddenly, ")
        .unwrap();
    a.save().unwrap();
    b.save().unwrap();
    std::fs::write(dir_a.join("assets/from-a.png"), b"AAA").unwrap();
    std::fs::write(dir_b.join("assets/from-b.png"), b"BBB").unwrap();

    let sa = state(&a, &dir_a, "A", "4242", &["Elendil", "shared"]);
    let sb = state(&b, &dir_b, "B", "4242", &["shared", "Zorbo"]);
    let (out_a, out_b) = sync_pair(sa, sb);
    let out_a = out_a.unwrap();
    let out_b = out_b.unwrap();

    assert_eq!(out_a.peer, "B");
    assert_eq!(out_b.peer, "A");
    assert!(out_a.ops_in > 0 && out_a.ops_out > 0);
    assert_eq!(out_a.ops_in, out_b.ops_out);
    assert_eq!(out_a.ops_out, out_b.ops_in);
    assert_eq!(out_a.files_in, vec!["from-b.png"]);
    assert_eq!(out_a.files_out, vec!["from-a.png"]);
    assert_eq!(out_b.files_in, vec!["from-a.png"]);
    assert_eq!(out_a.new_words, vec!["Zorbo"]);
    assert_eq!(out_b.new_words, vec!["Elendil"]);
    assert_eq!(
        std::fs::read(dir_a.join("assets/from-b.png")).unwrap(),
        b"BBB"
    );
    assert_eq!(
        std::fs::read(dir_b.join("assets/from-a.png")).unwrap(),
        b"AAA"
    );

    // Apply on each side, as the UI thread would.
    a.import_bytes(&out_a.incoming_updates).unwrap();
    b.import_bytes(&out_b.incoming_updates).unwrap();
    let ta = a.node(scene).unwrap().plain_text();
    let tb = b.node(scene).unwrap().plain_text();
    assert_eq!(ta, tb);
    assert_eq!(ta, "Suddenly, It was a dark night. The wind howled.");

    // A second sync has nothing to move.
    let (out_a, out_b) = sync_pair(
        state(&a, &dir_a, "A", "4242", &[]),
        state(&b, &dir_b, "B", "4242", &[]),
    );
    let (out_a, out_b) = (out_a.unwrap(), out_b.unwrap());
    assert_eq!((out_a.ops_in, out_a.ops_out), (0, 0));
    assert!(out_a.incoming_updates.is_empty() && out_b.incoming_updates.is_empty());
    assert!(out_a.files_in.is_empty() && out_b.files_in.is_empty());
}

#[test]
fn wrong_code_and_wrong_project_are_refused() {
    let a = Project::new_in_memory("A").unwrap();
    let b = Project::new_in_memory("B").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut sa = state(&a, tmp.path(), "A", "1111", &[]);
    let mut sb = state(&a, tmp.path(), "B", "2222", &[]);
    let (out_a, out_b) = sync_pair(sa.clone(), sb.clone());
    assert!(out_a.unwrap_err().to_string().contains("pairing code"));
    assert!(out_b.unwrap_err().to_string().contains("pairing code"));

    sb.pairing_code = "1111".into();
    sb.project_id = b.id();
    sa.pairing_code = "1111".into();
    let (out_a, out_b) = sync_pair(sa, sb);
    assert!(out_a.unwrap_err().to_string().contains("different project"));
    assert!(out_b.unwrap_err().to_string().contains("different project"));
}
