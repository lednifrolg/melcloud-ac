//! End to end: the real `klima` binary against a mock MELCloud.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, ResponseTemplate};

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread")]
async fn login_stores_the_session_but_never_the_password() {
    let klima = Klima::new().await;
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin3"))
        .and(header(
            "user-agent",
            concat!("klima/", env!("CARGO_PKG_VERSION")),
        ))
        .and(body_partial_json(
            json!({ "Email": EMAIL, "Password": PASSWORD, "Persist": true }),
        ))
        .respond_with(json_response(&login_response(SignedDuration::from_hours(
            24 * 30,
        ))))
        .expect(1)
        .mount(&klima.server)
        .await;

    let output = klima.login().await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Logged in as user@example.com"),
        "{}",
        stdout(&output)
    );
    assert!(stdout(&output).contains("(30 days)"), "{}", stdout(&output));
    let session = klima.file("state/session.json");
    assert_eq!(
        fs::metadata(&session).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for entry in walk(klima.home.path()) {
        let content = fs::read_to_string(&entry).unwrap_or_default();
        assert!(
            !content.contains(PASSWORD),
            "password stored in {}",
            entry.display()
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[tokio::test(flavor = "multi_thread")]
async fn files_follow_the_xdg_directories() {
    let klima = Klima::new().await;
    Mock::given(path("/Login/ClientLogin3"))
        .respond_with(json_response(&login_response(SignedDuration::from_hours(
            1,
        ))))
        .mount(&klima.server)
        .await;
    let home = klima.home.path();

    let mut child = Command::new(env!("CARGO_BIN_EXE_klima"))
        .args(["login", "--email", EMAIL, "--password-stdin"])
        .env_remove("KLIMA_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env("HOME", home.join("home"))
        .env("XDG_STATE_HOME", home.join("xdg-state"))
        .env("KLIMA_API_URL", klima.server.uri())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{PASSWORD}\n").as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(home.join("xdg-state/klima/session.json").exists());
    assert!(home.join("xdg-state/klima/ledger.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn status_without_login_asks_to_log_in() {
    let klima = Klima::new().await;
    let output = klima.run(&["status"]).await;
    assert_eq!(code(&output), 4);
    assert!(
        stderr(&output).contains("klima login"),
        "{}",
        stderr(&output)
    );
    assert_eq!(klima.server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reads_the_unit_once_and_then_uses_the_cache() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    let first = klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;
    let second = klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;

    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let text = stdout(&first);
    assert!(
        text.contains("Living room · EXAMPLE-35   ● on · cool"),
        "{text}"
    );
    assert!(text.contains("22.0 °C target"), "{text}");
    assert!(text.contains("24.5 °C room"), "{text}");
    assert!(text.contains("2 °C outside"), "{text}");
    assert!(text.contains("fan ▁▂▄▆█ 2/5"), "{text}");
    assert!(!text.contains("cached"), "{text}");
    assert_eq!(code(&second), 0);
    assert!(stdout(&second).contains("cached"), "{}", stdout(&second));
    assert_eq!(klima.requests_to("/User/ListDevices").await, 1);
    assert_eq!(klima.requests_to("/Device/Get").await, 1);
    let history = fs::read_to_string(klima.file("state/history.jsonl")).unwrap();
    assert_eq!(history.lines().count(), 1, "{history}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_status_right_after_a_read_does_not_ask_again() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;
    let output = klima
        .run(&[&LIVING_ROOM[..], &["status", "--fresh"]].concat())
        .await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        !stdout(&output).contains("not refreshed"),
        "{}",
        stdout(&output)
    );
    assert_eq!(klima.requests_to("/Device/Get").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn several_devices_need_a_choice() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    let output = klima.run(&["status"]).await;

    assert_eq!(code(&output), 2);
    assert!(
        stderr(&output).contains("\"Living room\", \"Bedroom\""),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn throttling_stops_every_request_until_unblocked() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    Mock::given(path("/User/ListDevices"))
        .respond_with(ResponseTemplate::new(429))
        .up_to_n_times(1)
        .mount(&klima.server)
        .await;

    let throttled = klima.run(&["devices"]).await;
    let still_blocked = klima.run(&["devices"]).await;

    assert_eq!(code(&throttled), 3);
    assert!(
        stderr(&throttled).contains("HTTP 429"),
        "{}",
        stderr(&throttled)
    );
    assert_eq!(code(&still_blocked), 3);
    assert!(
        stderr(&still_blocked).contains("throttled"),
        "{}",
        stderr(&still_blocked)
    );
    assert_eq!(klima.requests_to("/User/ListDevices").await, 1);

    assert_eq!(code(&klima.run(&["unblock"]).await), 0);
    klima.mock_account().await;
    let after = klima.run(&["devices"]).await;
    assert_eq!(code(&after), 0, "{}", stderr(&after));
    assert_eq!(klima.requests_to("/User/ListDevices").await, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_session_is_forgotten() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    Mock::given(path("/User/ListDevices"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&klima.server)
        .await;

    let output = klima.run(&["devices"]).await;

    assert_eq!(code(&output), 4);
    assert!(
        stderr(&output).contains("session expired"),
        "{}",
        stderr(&output)
    );
    assert!(!klima.file("state/session.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_session_is_not_used() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_mins(-1)).await;

    let output = klima.run(&["devices"]).await;

    assert_eq!(code(&output), 4);
    assert_eq!(klima.requests_to("/User/ListDevices").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_lockout_is_respected_without_asking_again() {
    let klima = Klima::new().await;
    let locked = json!({ "ErrorId": 6, "LoginMinutes": 15, "LoginAttempts": 5, "LoginData": null });
    Mock::given(path("/Login/ClientLogin3"))
        .respond_with(json_response(&locked.to_string()))
        .mount(&klima.server)
        .await;

    let first = klima.login().await;
    let second = klima.login().await;

    assert_eq!(code(&first), 3);
    assert!(
        stderr(&first).contains("locked logins"),
        "{}",
        stderr(&first)
    );
    assert_eq!(code(&second), 3);
    assert_eq!(klima.requests_to("/Login/ClientLogin3").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_says_so() {
    let klima = Klima::new().await;
    let rejected =
        json!({ "ErrorId": 1, "LoginMinutes": 0, "LoginAttempts": 1, "LoginData": null });
    Mock::given(path("/Login/ClientLogin3"))
        .respond_with(json_response(&rejected.to_string()))
        .mount(&klima.server)
        .await;

    let output = klima.login().await;

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("wrong email or password"),
        "{}",
        stderr(&output)
    );
    assert!(!klima.file("state/session.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn status_json_is_machine_readable() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    let output = klima
        .run(&[&LIVING_ROOM[..], &["status", "--json"]].concat())
        .await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["device"],
        json!({ "id": 1_000_001, "name": "Living room" })
    );
    assert_eq!(status["power"], json!(true));
    assert_eq!(status["mode"], json!("cool"));
    assert_eq!(status["target_temperature"], json!(22.0));
    assert_eq!(status["outside_temperature"], json!(2.0));
    assert_eq!(status["fan_speed"], json!("2"));
    assert_eq!(status["source"], json!("server"));
    assert_eq!(status["stale"], json!(false));
}

#[tokio::test(flavor = "multi_thread")]
async fn state_of_another_device_is_rejected_and_not_cached() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    // The mock answers every unit with the living room's state.
    let output = klima.run(&["--device", "Bedroom", "status"]).await;

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("state of device 1000001 instead of 1000002"),
        "{}",
        stderr(&output)
    );
    assert!(!klima.file("cache/state-1000002.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_lists_the_account() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    let table = klima.run(&["devices"]).await;
    let list = klima.run(&["devices", "--json"]).await;

    assert_eq!(code(&table), 0, "{}", stderr(&table));
    let text = stdout(&table);
    assert!(text.contains("NAME"), "{text}");
    assert!(text.contains("Living room  EXAMPLE-35  owner"), "{text}");
    assert!(text.contains("Bedroom      EXAMPLE-25  guest"), "{text}");
    let devices: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(
        devices[1]["capabilities"]["modes"],
        json!(["heat", "cool", "fan"])
    );
    assert_eq!(klima.requests_to("/User/ListDevices").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn old_state_is_shown_with_the_reason_it_was_not_refreshed() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;
    klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;

    // Twenty minutes later: the cached state is past its lifetime, the last read is long
    // forgotten, and the server fails.
    fs::remove_file(klima.file("state/ledger.json")).unwrap();
    let cache = klima.file("cache/state-1000001.json");
    let mut cached: Value = serde_json::from_str(&fs::read_to_string(&cache).unwrap()).unwrap();
    cached["fetched_at"] = json!((Timestamp::now() - SignedDuration::from_mins(20)).to_string());
    fs::write(&cache, cached.to_string()).unwrap();
    klima.server.reset().await;
    Mock::given(path("/Device/Get"))
        .respond_with(ResponseTemplate::new(500).insert_header("X-Mitsubishi-ErrorId", "e-1"))
        .mount(&klima.server)
        .await;

    let output = klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;

    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    assert!(text.contains("22.0 °C target"), "{text}");
    assert!(
        text.contains("! not refreshed: MELCloud answered HTTP 500 (error ID e-1)"),
        "{text}"
    );
    assert!(text.contains("updated 20 min ago · cached"), "{text}");
    assert!(stderr(&output).contains("HTTP 500"), "{}", stderr(&output));
}
