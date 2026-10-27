//! The client reads real server responses, not just the synthetic test data.
//!
//! When `MELCLOUD_CAPTURED_DIR` points to a directory of captured server responses, each one is
//! served from a mock server and read through the client. Without it, including in CI, the test
//! is skipped.

use std::path::{Path, PathBuf};

use melcloud::{BuildingId, Client, ContextKey, DeviceId, DeviceKind, Session, WeatherKind};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn captured(dir: &Path, name: &str) -> Vec<u8> {
    let file = dir.join(name);
    std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

#[tokio::test]
async fn captured_responses_are_readable() {
    let Some(dir) = std::env::var_os("MELCLOUD_CAPTURED_DIR").map(PathBuf::from) else {
        eprintln!("skipped: MELCLOUD_CAPTURED_DIR is not set");
        return;
    };
    let server = MockServer::start().await;
    for (endpoint, file) in [
        ("/Login/ClientLogin3", "login-response.json"),
        ("/User/ListDevices", "list-devices-response.json"),
        ("/Device/Get", "device-get-response.json"),
        ("/Device/SetAta", "setata-response.json"),
    ] {
        Mock::given(path(endpoint))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(captured(&dir, file), "application/json"),
            )
            .mount(&server)
            .await;
    }
    let client = Client::with_base_url("klima-test/1.0", &server.uri()).unwrap();

    let session = client
        .login("user@example.com", "secret", false)
        .await
        .unwrap();
    assert!(session.expires_at().is_some());

    let devices = client.devices(&session).await.unwrap().into_devices();
    for device in &devices {
        assert_eq!(
            device.capabilities.is_some(),
            device.kind == DeviceKind::AirToAir,
            "{device:?}"
        );
    }
    assert!(
        devices.iter().any(|d| d.capabilities.is_some()),
        "no air conditioner among the captured devices"
    );

    let restored = Session::new(ContextKey::new("restored"), None, false);
    let state = client
        .ata_state(&restored, DeviceId(1), BuildingId(1))
        .await
        .unwrap();
    assert!(state.view().last_communication.is_some());
    assert!(
        state
            .view()
            .weather
            .iter()
            .any(|w| w.kind == WeatherKind::Current)
    );
}
