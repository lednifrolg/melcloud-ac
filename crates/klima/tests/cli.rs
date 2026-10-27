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
    assert_eq!(stderr(&output), "", "the reason is not repeated");

    // Moments later the read is refused for spacing, but the state is still the old one.
    let again = klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;
    assert_eq!(code(&again), 3);
    let text = stdout(&again);
    assert!(
        text.contains("! not refreshed: the unit was asked moments ago; next read possible in"),
        "{text}"
    );
    assert_eq!(klima.requests_to("/Device/Get").await, 1);
}

/// Writes history lines for the living room, `minutes` ago each, with `power` and `mode`.
fn write_history(klima: &Klima, samples: &[(i64, bool, &str, f64)]) {
    let state = klima.file("state");
    fs::create_dir_all(&state).unwrap();
    let mut lines: Vec<String> = samples
        .iter()
        .map(|&(minutes, power, mode, target)| {
            let at = Timestamp::now() - SignedDuration::from_mins(minutes);
            json!({
                "at": at, "device": 1_000_001, "power": power, "mode": mode,
                "target": target, "room": 21.5, "outside": 3.0, "fan": "auto", "error_code": 8000
            })
            .to_string()
        })
        .collect();
    let other = json!({
        "at": Timestamp::now(), "device": 1_000_002, "power": true, "mode": "cool",
        "target": 18.0, "room": 30.0, "fan": "1", "error_code": 8000
    });
    lines.push(other.to_string());
    lines.push("not a sample".into());
    fs::write(state.join("history.jsonl"), lines.join("\n") + "\n").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn history_charts_the_day_and_lists_the_changes_without_reading_the_unit() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;
    write_history(
        &klima,
        &[
            (300, false, "heat", 22.0),
            (240, false, "heat", 22.0),
            (180, true, "heat", 22.0),
            (120, true, "heat", 23.5),
            (60, true, "cool", 23.5),
            (30, true, "cool", 23.5),
        ],
    );

    let output = klima.run(&[&LIVING_ROOM[..], &["history"]].concat()).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.starts_with("Living room · last 24h · 6 samples · room 21.5 °C\n"),
        "{text}"
    );
    let band = text.lines().find(|line| line.starts_with("mode ")).unwrap();
    assert!(
        band.trim_end().ends_with("······hhhhhhcccc"),
        "letters tell the modes apart without colors: {text}"
    );
    assert!(
        text.contains("now\n     h heat  c cool  · off  blank: not read\n\nFROM "),
        "a legend of what the band shows: {text}"
    );
    let changes: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.starts_with("FROM"))
        .collect();
    assert_eq!(
        changes.len(),
        5,
        "header, the first state and three changes: {text}"
    );
    assert!(
        changes[0].ends_with("POWER  MODE  TARGET  FAN    ROOM"),
        "{text}"
    );
    assert!(
        changes[1].ends_with("off    heat   22.0°  auto  21.5°"),
        "{text}"
    );
    assert!(
        changes[2].ends_with("on     heat   22.0°  auto  21.5°"),
        "{text}"
    );
    assert!(
        changes[3].ends_with("on     heat   23.5°  auto  21.5°"),
        "{text}"
    );
    assert!(
        changes[4].ends_with("on     cool   23.5°  auto  21.5°"),
        "{text}"
    );
    assert_eq!(klima.requests_to("/Device/Get").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_works_offline_with_an_old_device_list() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;
    let devices = klima.run(&["devices"]).await;
    assert_eq!(code(&devices), 0, "{}", stderr(&devices));
    let cached = klima.file("cache/devices.json");
    let mut list: Value = serde_json::from_str(&fs::read_to_string(&cached).unwrap()).unwrap();
    list["fetched_at"] = json!(Timestamp::now() - SignedDuration::from_hours(24 * 30));
    fs::write(&cached, list.to_string()).unwrap();
    fs::remove_file(klima.file("state/session.json")).unwrap();
    write_history(&klima, &[(60, true, "cool", 23.5)]);
    let before = klima.server.received_requests().await.unwrap().len();

    let output = klima.run(&[&LIVING_ROOM[..], &["history"]].concat()).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stdout(&output).contains("· 1 sample ·"),
        "{}",
        stdout(&output)
    );
    let after = klima.server.received_requests().await.unwrap().len();
    assert_eq!(after, before, "asks MELCloud for nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn history_lists_the_latest_changes_and_counts_the_rest() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;
    let samples: Vec<(i64, bool, &str, f64)> = (0..30)
        .map(|i| {
            (
                300 - i * 10,
                true,
                "heat",
                20.0 + f64::from(i32::try_from(i).unwrap()) / 2.0,
            )
        })
        .collect();
    write_history(&klima, &samples);

    let output = klima.run(&[&LIVING_ROOM[..], &["history"]].concat()).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("10 earlier changes hidden; --json has all"),
        "{}",
        stderr(&output)
    );
    let text = stdout(&output);
    let rows = text
        .lines()
        .skip_while(|line| !line.starts_with("FROM"))
        .count();
    assert_eq!(rows, 21, "header and 20 rows: {text}");
    assert!(text.trim_end().ends_with("34.5°  auto  21.5°"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn history_as_json_covers_the_asked_span() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;
    write_history(
        &klima,
        &[(300, false, "heat", 22.0), (60, true, "cool", 23.5)],
    );

    let output = klima
        .run(&[&LIVING_ROOM[..], &["history", "--since", "2h", "--json"]].concat())
        .await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["device"]["name"], "Living room");
    let samples = report["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 1, "{report}");
    assert_eq!(samples[0]["mode"], "cool");
    assert_eq!(samples[0]["target_temperature"], 23.5);
    assert_eq!(samples[0]["outside_temperature"], 3.0);
    assert_eq!(samples[0]["fan_speed"], "auto");
}

#[tokio::test(flavor = "multi_thread")]
async fn history_grows_with_every_read_of_the_unit() {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_account().await;

    let empty = klima.run(&[&LIVING_ROOM[..], &["history"]].concat()).await;
    assert_eq!(code(&empty), 0, "{}", stderr(&empty));
    assert!(stdout(&empty).contains("· 0 samples"), "{}", stdout(&empty));
    assert!(
        stderr(&empty).contains("adds a sample"),
        "{}",
        stderr(&empty)
    );

    let status = klima.run(&[&LIVING_ROOM[..], &["status"]].concat()).await;
    assert_eq!(code(&status), 0, "{}", stderr(&status));
    let output = klima
        .run(&[&LIVING_ROOM[..], &["history", "--json"]].concat())
        .await;
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    let samples = report["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 1, "{report}");
    assert_eq!(samples[0]["room_temperature"], 24.5);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_rejects_a_span_that_is_not_one() {
    let klima = Klima::new().await;
    for since in ["yesterday", "0h", "-2h"] {
        let output = klima.run(&["history", &format!("--since={since}")]).await;
        assert_eq!(code(&output), 2, "{since}");
        assert!(
            stderr(&output).contains("expected a duration such as 24h, 90m or 7d"),
            "{}",
            stderr(&output)
        );
    }
    assert_eq!(klima.server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn completions_and_man_pages_need_no_setup() {
    let klima = Klima::new().await;
    for (shell, expected) in [
        ("bash", "_klima()"),
        ("zsh", "#compdef klima"),
        ("fish", "complete -c klima"),
    ] {
        let output = klima.run(&["completions", shell]).await;
        assert_eq!(code(&output), 0, "{}", stderr(&output));
        let script = stdout(&output);
        assert!(script.contains(expected), "{shell}: {script}");
        assert!(script.contains("history"), "{shell} completes the commands");
    }

    let dir = klima.file("man/man1");
    let output = klima.run(&["man", dir.to_str().unwrap()]).await;
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let page = fs::read_to_string(dir.join("klima.1")).unwrap();
    assert!(page.contains(".TH klima 1"), "{page}");
    assert!(page.contains("klima\\-history(1)"), "{page}");
    assert!(dir.join("klima-history.1").exists());
    assert!(!dir.join("klima-help.1").exists());

    assert_eq!(klima.server.received_requests().await.unwrap().len(), 0);
    assert!(!klima.file("state").exists() && !klima.file("config").exists());
}
