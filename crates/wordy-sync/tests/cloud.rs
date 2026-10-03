//! The cloud client against a real server. Runs only when a Wordy site is
//! up locally (`cd site && pnpm build && pnpm wrangler dev --port 8787`),
//! or wherever `WORDY_TEST_SERVER` points; otherwise it is skipped.
//! Approval needs a passkey in a browser; `full_link_flow` gets one from
//! `site/scripts/e2e-auth.mjs --approve` (headless Chromium with a virtual
//! authenticator) when `node` and `chromium` are around, and is skipped
//! otherwise.

use std::time::Duration;

use std::sync::mpsc;

use wordy_sync::cloud::{self, Client, Poll, RoomEvent, RoomHandle, RoomOptions};
use wordy_sync::loro::{ExportMode, LoroDoc};
use wordy_sync::SyncConfig;

fn server() -> Option<String> {
    let url = std::env::var("WORDY_TEST_SERVER").unwrap_or_else(|_| "http://localhost:8787".into());
    let probe = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(probe);
    match agent.get(format!("{url}/api/devices")).call() {
        Ok(r) if r.status().as_u16() == 401 => Some(url),
        _ => {
            eprintln!("no Wordy server at {url}; skipping");
            None
        }
    }
}

#[test]
fn device_flow_up_to_approval() {
    let Some(url) = server() else { return };
    let client = Client::new(&url);

    let link = client.start_link().unwrap();
    assert!(!link.user_code.is_empty());
    assert!(!link.device_code.is_empty());
    assert!(link.verify_url.starts_with(&url), "{}", link.verify_url);
    assert!(link.verify_url.contains(&link.user_code));
    assert!(link.interval >= Duration::from_secs(1));

    // Nobody has approved it.
    assert_eq!(client.poll_token(&link).unwrap(), Poll::Pending);

    // A cancelled wait returns straight away.
    let err = client.wait_for_token(&link, || false).unwrap_err();
    assert_eq!(err.to_string(), "cancelled");

    // A made-up device code is an error, not Pending.
    let bogus = cloud::DeviceLink {
        device_code: "not-a-real-code".into(),
        ..link.clone()
    };
    assert!(client.poll_token(&bogus).is_err());
}

#[test]
fn bad_token_paths() {
    let Some(url) = server() else { return };
    let client = Client::new(&url);
    let err = client.account("nope").unwrap_err();
    assert!(cloud::is_unauthorized(&err), "{err:#}");
    assert!(client.register_device("nope", "box", "linux", "0.0.0").is_err());
    assert!(cloud::is_unauthorized(&client.revoke_device("nope", "x").unwrap_err()));
    // Signing out with a dead token is fine.
    client.unlink("nope").unwrap();
}

#[test]
fn unreachable_server_is_an_error() {
    let client = Client::new("http://127.0.0.1:9/");
    assert_eq!(client.server(), "http://127.0.0.1:9");
    assert!(client.start_link().is_err());
}

#[test]
fn config_roundtrip_is_private() {
    let dir = tempfile::tempdir().unwrap();
    // The config dir comes from the environment; keep this test alone in
    // its process-wide setting by using a unique folder.
    std::env::set_var("WORDY_CONFIG_DIR", dir.path());
    let mut c = SyncConfig::default();
    assert_eq!(c.cloud_server(), cloud::DEFAULT_SERVER);
    c.cloud_server = "http://localhost:8787/".into();
    assert_eq!(c.cloud_server(), "http://localhost:8787");
    c.cloud = Some(cloud::CloudAccount {
        token: "secret".into(),
        user_id: "u1".into(),
        email: "a@b.c".into(),
        device_id: "d1".into(),
        device_name: "box".into(),
    });
    c.save().unwrap();
    let back = SyncConfig::load();
    assert_eq!(back, c);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(wordy_sync::config::config_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

/// The app's whole flow: code, approval in a (headless) browser,
/// registration, account listing, unlink.
/// Link a fresh account through the browser script, or None when `node`
/// or Chromium is missing.
fn link_with_browser(url: &str, device_name: &str) -> Option<cloud::CloudAccount> {
    let site = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site");
    let chrome = std::env::var("CHROME").unwrap_or_else(|_| "chromium".into());
    let have = |bin: &str| {
        std::process::Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !have("node") || !have(&chrome) {
        eprintln!("node or {chrome} missing; skipping the browser approval");
        return None;
    }
    // The script signs up by email and takes the newest mail wrangler wrote,
    // so two sign-ups at once would read each other's links: one at a time.
    static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
    let client = Client::new(url);
    let base = url.to_string();
    let acct = client
        .link(
            device_name,
            "0.0.0-test",
            move |link| {
                assert!(link.verify_url.contains(&link.user_code));
                let out = std::process::Command::new("node")
                    .args(["scripts/e2e-auth.mjs", "--approve", &link.user_code])
                    .current_dir(&site)
                    .env("E2E_BASE", &base)
                    .output()
                    .expect("run node");
                assert!(
                    out.status.success(),
                    "approve script failed:\n{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
            },
            || true,
        )
        .unwrap();
    Some(acct)
}

/// Link a second device to the same account: an already-linked device's
/// bearer session may approve a code (what the website does with cookies).
fn link_sibling(url: &str, first: &cloud::CloudAccount, device_name: &str) -> cloud::CloudAccount {
    let client = Client::new(url);
    let agent = ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build());
    let auth = format!("Bearer {}", first.token);
    client
        .link(
            device_name,
            "0.0.0-test",
            |link| {
                let r = agent
                    .get(format!("{url}/api/auth/device?user_code={}", link.user_code))
                    .header("Authorization", &auth)
                    .call()
                    .unwrap();
                assert_eq!(r.status().as_u16(), 200, "device verify");
                let r = agent
                    .post(format!("{url}/api/auth/device/approve"))
                    .header("Authorization", &auth)
                    .send_json(serde_json::json!({ "userCode": link.user_code }))
                    .unwrap();
                assert_eq!(r.status().as_u16(), 200, "device approve");
            },
            || true,
        )
        .unwrap()
}

#[test]
fn full_link_flow() {
    let Some(url) = server() else { return };
    let Some(acct) = link_with_browser(&url, "Test box") else {
        return;
    };
    let client = Client::new(&url);
    assert!(!acct.token.is_empty());
    assert!(acct.email.ends_with("@example.test"), "{}", acct.email);
    assert_eq!(acct.device_name, "Test box");

    let a = client.account(&acct.token).unwrap();
    assert_eq!(a.user.email, acct.email);
    let me = a.devices.iter().find(|d| d.current).expect("current device listed");
    assert_eq!(me.id, acct.device_id);
    assert_eq!(me.platform, cloud::platform());
    assert_eq!(me.app_version, "0.0.0-test");

    // Registering again under the same session updates, not duplicates.
    let (_, d2) = client
        .register_device(&acct.token, "Test box 2", "linux", "0.0.1")
        .unwrap();
    assert_eq!(d2.id, acct.device_id);
    assert_eq!(client.account(&acct.token).unwrap().devices.len(), 1);

    client.unlink(&acct.token).unwrap();
    assert!(cloud::is_unauthorized(&client.account(&acct.token).unwrap_err()));
}

/// Events from one room, with a helper that waits for a matching one
/// (anything else in between is dropped).
struct Events(mpsc::Receiver<RoomEvent>);

impl Events {
    fn wait<T>(&self, what: &str, mut pick: impl FnMut(RoomEvent) -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let ev = self
                .0
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            eprintln!("  event: {}", describe(&ev));
            if let RoomEvent::Disconnected { reason, .. } = &ev {
                panic!("disconnected while waiting for {what}: {reason}");
            }
            if let Some(t) = pick(ev) {
                return t;
            }
        }
    }

    fn wait_updates(&self) -> Vec<(u64, Vec<u8>)> {
        self.wait("updates", |ev| match ev {
            RoomEvent::Updates(u) => Some(u),
            _ => None,
        })
    }

    fn wait_pushed(&self) -> u64 {
        self.wait("pushed", |ev| match ev {
            RoomEvent::Pushed(seq) => Some(seq),
            _ => None,
        })
    }

    fn wait_synced(&self) -> (u64, bool) {
        self.wait("synced", |ev| match ev {
            RoomEvent::Synced { head, reset, .. } => Some((head, reset)),
            _ => None,
        })
    }

    /// Nothing arrives for a while (except presence chatter).
    fn quiet(&self, for_: Duration) {
        let deadline = std::time::Instant::now() + for_;
        while let Ok(ev) = self
            .0
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            match ev {
                RoomEvent::Presence(_) => {}
                other => panic!("unexpected event: {}", describe(&other)),
            }
        }
    }
}

fn describe(ev: &RoomEvent) -> String {
    match ev {
        RoomEvent::Updates(u) => format!(
            "Updates({:?})",
            u.iter().map(|(s, b)| (*s, b.len())).collect::<Vec<_>>()
        ),
        other => format!("{other:?}"),
    }
}

fn room(
    url: &str,
    acct: &cloud::CloudAccount,
    project_id: &str,
    dir: &std::path::Path,
    since: u64,
    words: &[&str],
) -> (RoomHandle, Events) {
    let (tx, rx) = mpsc::channel();
    let handle = RoomHandle::start(
        RoomOptions {
            server: url.to_string(),
            token: acct.token.clone(),
            project_id: project_id.to_string(),
            project_name: "Room test".into(),
            assets_dir: dir.join("assets"),
            since,
            dictionary: words.iter().map(|w| w.to_string()).collect(),
        },
        move |ev| {
            let _ = tx.send(ev);
        },
    );
    (handle, Events(rx))
}

fn text(doc: &LoroDoc) -> String {
    doc.get_text("body").to_string()
}

/// Two devices of one account share a project room: snapshot, live
/// updates both ways, dictionary, assets, a large blob over HTTP,
/// compaction and replay after a reconnect.
#[test]
fn room_syncs_two_copies() {
    let Some(url) = server() else { return };
    let Some(a) = link_with_browser(&url, "Room A") else {
        return;
    };
    let b = link_sibling(&url, &a, "Room B");
    let project_id = ulid::Ulid::new().to_string();
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();

    // A starts with some text and claims the room.
    let doc_a = LoroDoc::new();
    doc_a.get_text("body").insert(0, "Hello").unwrap();
    doc_a.commit();
    let (room_a, ev_a) = room(&url, &a, &project_id, dir_a.path(), 0, &["Gondor"]);
    let role = ev_a.wait("connected", |ev| match ev {
        RoomEvent::Connected { role } => Some(role),
        _ => None,
    });
    assert_eq!(role, "owner");
    assert_eq!(ev_a.wait_synced(), (0, false));
    room_a.push(doc_a.export(ExportMode::Snapshot).unwrap());
    assert_eq!(ev_a.wait_pushed(), 1);
    let mut vv_a = doc_a.oplog_vv();

    // A has an attachment; a rescan uploads it.
    std::fs::create_dir_all(dir_a.path().join("assets/img")).unwrap();
    std::fs::write(dir_a.path().join("assets/img/pic.png"), b"not really a png").unwrap();
    room_a.rescan_assets();
    ev_a.wait("asset upload", |ev| match ev {
        RoomEvent::Assets { uploaded, .. } if uploaded == ["img/pic.png"] => Some(()),
        _ => None,
    });

    // B joins empty: gets the snapshot, the dictionary and the file.
    let doc_b = LoroDoc::new();
    let (room_b, ev_b) = room(&url, &b, &project_id, dir_b.path(), 0, &[]);
    ev_b.wait("dictionary", |ev| match ev {
        RoomEvent::Dictionary(w) if w == ["Gondor"] => Some(()),
        _ => None,
    });
    let batch = ev_b.wait_updates();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].0, 1);
    doc_b.import(&batch[0].1).unwrap();
    assert_eq!(text(&doc_b), "Hello");
    assert_eq!(ev_b.wait_synced(), (1, false));
    ev_b.wait("asset download", |ev| match ev {
        RoomEvent::Assets { downloaded, .. } if downloaded == ["img/pic.png"] => Some(()),
        _ => None,
    });
    assert_eq!(
        std::fs::read(dir_b.path().join("assets/img/pic.png")).unwrap(),
        b"not really a png"
    );
    ev_a.wait("presence with both", |ev| match ev {
        RoomEvent::Presence(names) if names.len() == 2 => Some(()),
        _ => None,
    });

    // B edits; A sees it live and its own echo is skipped.
    let vv_b = doc_b.oplog_vv();
    doc_b.get_text("body").insert(5, " world").unwrap();
    doc_b.commit();
    room_b.push(doc_b.export(ExportMode::updates(&vv_b)).unwrap());
    assert_eq!(ev_b.wait_pushed(), 2);
    let live = ev_a.wait_updates();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].0, 2);
    doc_a.import(&live[0].1).unwrap();
    assert_eq!(text(&doc_a), "Hello world");
    ev_b.quiet(Duration::from_millis(500));

    // Words travel too.
    room_b.add_words(vec!["Rohan".into()]);
    ev_a.wait("word", |ev| match ev {
        RoomEvent::Dictionary(w) if w == ["Rohan"] => Some(()),
        _ => None,
    });

    // A big edit goes over HTTP; B fetches it the same way.
    let big = "lorem ipsum ".repeat(60_000);
    doc_a.get_text("body").insert(0, &big).unwrap();
    doc_a.commit();
    let blob = doc_a.export(ExportMode::updates(&vv_a)).unwrap();
    assert!(blob.len() > 512 * 1024, "{} bytes", blob.len());
    vv_a = doc_a.oplog_vv();
    room_a.push(blob);
    assert_eq!(ev_a.wait_pushed(), 3);
    let fetched = ev_b.wait_updates();
    assert_eq!(fetched[0].0, 3);
    doc_b.import(&fetched[0].1).unwrap();
    assert_eq!(text(&doc_b), text(&doc_a));

    // A compacts: its snapshot becomes the base. B is told and imports it
    // (a no-op for it).
    room_a.upload_snapshot(doc_a.export(ExportMode::Snapshot).unwrap(), 3);
    assert_eq!(ev_a.wait_pushed(), 4);
    let snap = ev_b.wait_updates();
    assert_eq!(snap[0].0, 4);
    doc_b.import(&snap[0].1).unwrap();
    assert_eq!(text(&doc_b), text(&doc_a));
    assert_eq!(doc_b.oplog_vv(), vv_a);

    // B reconnects from before the base: the replay starts at the snapshot.
    drop(room_b);
    let (room_b2, ev_b2) = room(&url, &b, &project_id, dir_b.path(), 1, &[]);
    let replay = ev_b2.wait_updates();
    assert_eq!(replay.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![4]);
    assert_eq!(ev_b2.wait_synced(), (4, false));
    drop(room_b2);

    // Up to date: nothing replayed. Ahead of the room: a reset.
    let (_room_b3, ev_b3) = room(&url, &b, &project_id, dir_b.path(), 4, &[]);
    assert_eq!(ev_b3.wait_synced(), (4, false));
    let (_room_b4, ev_b4) = room(&url, &b, &project_id, dir_b.path(), 40, &[]);
    assert_eq!(ev_b4.wait_synced(), (4, true));

    // A stranger's token is refused for good.
    let Some(stranger) = link_with_browser(&url, "Stranger") else {
        return;
    };
    let (_room_s, ev_s) = room(&url, &stranger, &project_id, dir_b.path(), 0, &[]);
    let fatal = ev_s.0.recv_timeout(Duration::from_secs(30)).expect("stranger event");
    assert!(
        matches!(&fatal, RoomEvent::Disconnected { fatal: true, .. }),
        "{}",
        describe(&fatal)
    );
}

/// A machine without a copy of a project gets one from its room: the
/// folder opens as a project with the same id, name, text and assets, and
/// `cloud.json` says sync is on from the replay's end.
#[test]
fn fetch_project_copies_a_room() {
    let Some(url) = server() else { return };
    let Some(a) = link_with_browser(&url, "Fetch A") else {
        return;
    };
    let b = link_sibling(&url, &a, "Fetch B");
    let tmp = tempfile::tempdir().unwrap();

    // A has a real project with text and an attachment, and claims the room.
    let dir_a = tmp.path().join("Fetch test");
    let project_a = wordy_doc::Project::create(&dir_a, "Fetch test").unwrap();
    let id = project_a.id();
    assert!(!id.is_empty());
    let root = project_a.root(wordy_doc::Space::Manuscript);
    let scene = project_a.create_node(root, wordy_doc::NodeKind::Scene, "One").unwrap();
    project_a
        .node(scene)
        .unwrap()
        .body()
        .unwrap()
        .insert(0, "Hello from A")
        .unwrap();
    project_a.commit_meta();
    std::fs::create_dir_all(dir_a.join("assets/img")).unwrap();
    std::fs::write(dir_a.join("assets/img/pic.png"), b"not really a png").unwrap();
    let (room_a, ev_a) = room(&url, &a, &id, &dir_a, 0, &[]);
    assert_eq!(ev_a.wait_synced(), (0, false));
    // The file was there before connecting, so the first reconcile sends it.
    ev_a.wait("asset upload", |ev| match ev {
        RoomEvent::Assets { uploaded, .. } if uploaded == ["img/pic.png"] => Some(()),
        _ => None,
    });
    room_a.push(project_a.doc.export(ExportMode::Snapshot).unwrap());
    assert_eq!(ev_a.wait_pushed(), 1);

    // The account lists it, under the name the room helper sent with hello.
    let listed = Client::new(&url).list_projects(&b.token).unwrap();
    let mine = listed.iter().find(|p| p.id == id).expect("project listed");
    assert_eq!(
        (mine.name.as_str(), mine.role.as_str(), mine.owner),
        ("Room test", "owner", true)
    );

    // B fetches a copy.
    let dir_b = tmp.path().join("copy");
    cloud::fetch_project(&url, &b.token, &id, "Fetch test", &dir_b).unwrap();
    let project_b = wordy_doc::Project::open(&dir_b).unwrap();
    assert_eq!(project_b.id(), id);
    assert_eq!(project_b.name(), "Fetch test");
    assert_eq!(project_b.node(scene).unwrap().plain_text(), "Hello from A");
    assert_eq!(
        std::fs::read(dir_b.join("assets/img/pic.png")).unwrap(),
        b"not really a png"
    );
    let state = cloud::CloudState::load(&dir_b);
    assert!(state.enabled);
    assert_eq!(state.last_seq, 1);
    assert_eq!(state.vv(), project_b.doc.oplog_vv());

    // A folder with something in it is left alone.
    let err = cloud::fetch_project(&url, &b.token, &id, "Fetch test", &dir_b).unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    drop(room_a);
}
