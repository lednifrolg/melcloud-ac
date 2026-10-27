//! End to end: commands that change the unit, against a simulated unit and adapter.

mod common;

use common::*;
use jiff::SignedDuration;
use serde_json::{Value, json};

/// A logged-in klima whose account has the living room unit behind `adapter`.
async fn setup(adapter: Adapter) -> (Klima, Unit) {
    let klima = Klima::new().await;
    klima.login_for(SignedDuration::from_hours(1)).await;
    klima.mock_devices().await;
    let unit = Unit::mount(&klima.server, adapter).await;
    (klima, unit)
}

async fn run(klima: &Klima, args: &[&str]) -> std::process::Output {
    klima.run(&[&LIVING_ROOM[..], args].concat()).await
}

/// The synthetic state with `changes`, as the command body should look.
fn expected_body(changes: &Value) -> Value {
    let mut body: Value = serde_json::from_str(DEVICE_GET).unwrap();
    for (key, value) in changes.as_object().unwrap() {
        body[key] = value.clone();
    }
    body
}

#[tokio::test(flavor = "multi_thread")]
async fn temperature_is_sent_once_and_confirmed_by_the_adapter() {
    let (klima, unit) = setup(Adapter::Applies).await;

    let output = run(&klima, &["temp", "23"]).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("✓ sent    temp 22.0 °C → 23.0 °C"), "{text}");
    assert!(text.contains("SetAta, flags 0x04"), "{text}");
    assert!(text.contains("✓ applied the adapter took it"), "{text}");
    assert_eq!(
        klima.commands().await,
        [expected_body(
            &json!({ "SetTemperature": 23.0, "EffectiveFlags": 4 })
        )]
    );
    // One read to build the command on, one to confirm it.
    assert_eq!(klima.requests_to("/Device/Get").await, 2);
    assert_eq!(unit.state()["SetTemperature"], json!(23.0));

    let status = run(&klima, &["status"]).await;
    assert!(
        stdout(&status).contains("23.0 °C target"),
        "{}",
        stdout(&status)
    );
    assert!(stdout(&status).contains("cached"), "{}", stdout(&status));
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_temperatures_start_from_the_current_target() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let lower = run(&klima, &["temp", "-0.5", "--no-wait"]).await;
    let higher = run(&klima, &["temp", "+2", "--no-wait"]).await;

    assert_eq!(code(&lower), 0, "{}", stderr(&lower));
    assert_eq!(code(&higher), 0, "{}", stderr(&higher));
    let temperatures: Vec<Value> = klima
        .commands()
        .await
        .iter()
        .map(|body| body["SetTemperature"].clone())
        .collect();
    assert_eq!(temperatures, [json!(21.5), json!(23.5)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_values_are_refused_before_anything_is_sent() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let hot = run(&klima, &["temp", "35"]).await;
    let odd = run(&klima, &["temp", "22.3"]).await;

    assert_eq!(code(&hot), 2);
    assert!(
        stderr(&hot).contains("35.0 °C is outside 16-31 °C for cool mode"),
        "{}",
        stderr(&hot)
    );
    assert_eq!(code(&odd), 2);
    assert!(
        stderr(&odd).contains("not a multiple of the 0.5 °C step"),
        "{}",
        stderr(&odd)
    );
    assert_eq!(klima.commands().await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_sent_when_the_unit_already_has_the_settings() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let output = run(&klima, &["set", "--power", "on", "--mode", "cool"]).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Nothing to send: the unit already has power on, mode cool."),
        "{}",
        stderr(&output)
    );
    assert_eq!(klima.commands().await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn several_settings_go_out_as_one_command() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let output = run(
        &klima,
        &[
            "set",
            "--mode",
            "heat",
            "--temp",
            "21",
            "--fan",
            "auto",
            "--no-wait",
        ],
    )
    .await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(
        klima.commands().await,
        [expected_body(&json!({
            "OperationMode": 1,
            "SetTemperature": 21.0,
            "SetFanSpeed": 0,
            "EffectiveFlags": 2 | 4 | 8,
        }))]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn toggle_and_vanes_map_to_their_fields() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    run(&klima, &["toggle", "--no-wait"]).await;
    run(
        &klima,
        &[
            "vane",
            "--horizontal",
            "split",
            "--vertical",
            "swing",
            "--no-wait",
        ],
    )
    .await;

    let commands = klima.commands().await;
    assert_eq!(commands[0]["Power"], json!(false));
    assert_eq!(commands[0]["EffectiveFlags"], json!(1));
    assert_eq!(commands[1]["VaneVertical"], json!(7));
    assert_eq!(commands[1]["VaneHorizontal"], json!(8));
    assert_eq!(commands[1]["EffectiveFlags"], json!(16 | 256));
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_shows_the_command_without_sending_it() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let text = run(&klima, &["fan", "4", "--dry-run"]).await;
    let json = run(&klima, &["fan", "4", "--dry-run", "--json"]).await;

    assert_eq!(code(&text), 0, "{}", stderr(&text));
    assert!(
        stdout(&text).contains("→ dry run fan 2 → 4"),
        "{}",
        stdout(&text)
    );
    assert!(stdout(&text).contains("flags 0x08"), "{}", stdout(&text));
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["sent"], json!(false));
    assert_eq!(report["dry_run"], json!(true));
    assert_eq!(report["body"]["SetFanSpeed"], json!(4));
    assert_eq!(klima.commands().await, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn silent_adapter_is_checked_twice_and_the_command_is_not_resent() {
    let (klima, _unit) = setup(Adapter::Silent).await;

    let output = run(&klima, &["temp", "23"]).await;

    assert_eq!(code(&output), 5);
    assert!(stdout(&output).contains("✓ sent"), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("did not confirm the command"),
        "{}",
        stderr(&output)
    );
    assert_eq!(klima.commands().await.len(), 1);
    assert_eq!(klima.requests_to("/Device/Get").await, 1 + 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_unit_did_not_take_is_reported() {
    let (klima, _unit) = setup(Adapter::Keeps("SetTemperature")).await;

    let output = run(&klima, &["temp", "23"]).await;

    assert_eq!(code(&output), 5);
    assert!(
        stderr(&output).contains("the unit reports temp 22.0 °C instead of 23.0 °C"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn json_reports_the_change_and_the_confirmation() {
    let (klima, _unit) = setup(Adapter::Applies).await;

    let output = run(&klima, &["off", "--json"]).await;

    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["device"]["name"], json!("Living room"));
    assert_eq!(
        report["changes"],
        json!([{ "field": "power", "from": "on", "to": "off" }])
    );
    assert_eq!(report["flags"], json!(1));
    assert_eq!(report["sent"], json!(true));
    assert_eq!(report["confirmed"], json!(true));
    assert!(report["adapter_seconds"].is_number(), "{report}");
    assert!(report.get("body").is_none());
}
