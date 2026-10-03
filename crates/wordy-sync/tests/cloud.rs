//! The cloud client against a real server. Runs only when a Wordy site is
//! up locally (`cd site && pnpm build && pnpm wrangler dev --port 8787`),
//! or wherever `WORDY_TEST_SERVER` points; otherwise it is skipped.
//! Approval needs a passkey in a browser; `full_link_flow` gets one from
//! `site/scripts/e2e-auth.mjs --approve` (headless Chromium with a virtual
//! authenticator) when `node` and `chromium` are around, and is skipped
//! otherwise.

use std::time::Duration;

use wordy_sync::cloud::{self, Client, Poll};
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
#[test]
fn full_link_flow() {
    let Some(url) = server() else { return };
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
        return;
    }
    let client = Client::new(&url);
    let base = url.clone();
    let acct = client
        .link(
            "Test box",
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
