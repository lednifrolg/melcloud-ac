//! HTTP behavior of the client against a local mock server.

use melcloud::{
    BuildingId, ChangeSet, Client, ContextKey, DeviceId, Error, LoginError, OperationMode, Session,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_AGENT: &str = "klima-test/1.0";
const KEY: &str = "SYNTHETIC-CONTEXT-KEY-0000000000";
const LOGIN: &str = include_str!("data/login.json");
const LIST_DEVICES: &str = include_str!("data/list-devices.json");
const DEVICE_GET: &str = include_str!("data/device-get.json");
const SET_ATA: &str = include_str!("data/set-ata.json");

fn json_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

fn client(server: &MockServer) -> Client {
    Client::with_base_url(USER_AGENT, &server.uri()).unwrap()
}

fn session() -> Session {
    Session::new(ContextKey::new(KEY), None, false)
}

#[tokio::test]
async fn login_sends_the_app_body_and_returns_the_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Login/ClientLogin3"))
        .and(header("user-agent", USER_AGENT))
        .and(header("content-type", "application/json; charset=utf-8"))
        .and(body_json(json!({
            "Email": "user@example.com",
            "Password": "hunter2",
            "Language": 0,
            "AppVersion": "1.39.0.0",
            "Persist": true,
            "CaptchaResponse": null,
        })))
        .respond_with(json_response(LOGIN))
        .expect(1)
        .mount(&server)
        .await;

    let session = client(&server)
        .login("user@example.com", "hunter2", true)
        .await
        .unwrap();

    assert_eq!(session.key().expose(), KEY);
    assert_eq!(
        session.expires_at(),
        Some("2026-01-15T10:30:00Z".parse().unwrap())
    );
    assert!(!session.use_fahrenheit());
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("x-mitscontextkey").is_none());
}

#[tokio::test]
async fn login_error_ids_map_to_reasons() {
    let cases = [
        (0, LoginError::TermsUnavailable),
        (1, LoginError::InvalidCredentials),
        (2, LoginError::EmailNotVerified),
        (3, LoginError::AccountBlocked),
        (4, LoginError::VerificationEmailSent),
        (5, LoginError::AppVersionUnsupported),
        (6, LoginError::Locked { minutes: 15 }),
        (7, LoginError::CaptchaRequired),
        (8, LoginError::PasswordResetRequired),
        (9, LoginError::ServerBusy),
        (42, LoginError::Other(42)),
    ];
    for (error_id, expected) in cases {
        let server = MockServer::start().await;
        // Assumed shape of a rejected login: the success envelope without `LoginData`.
        let body = json!({
            "ErrorId": error_id,
            "ErrorMessage": "rejected",
            "LoginStatus": 0,
            "LoginMinutes": 15,
            "LoginAttempts": 3,
            "LoginData": null,
        });
        Mock::given(path("/Login/ClientLogin3"))
            .respond_with(json_response(&body.to_string()))
            .expect(1)
            .mount(&server)
            .await;

        let error = client(&server)
            .login("user@example.com", "wrong", false)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Login(reason) if *reason == expected),
            "{error_id}: {error:?}"
        );
    }
}

#[tokio::test]
async fn login_without_session_asks_for_verification() {
    let server = MockServer::start().await;
    let body = json!({ "ErrorId": null, "LoginStatus": 2, "UserId": 7, "LoginData": null });
    Mock::given(path("/Login/ClientLogin3"))
        .respond_with(json_response(&body.to_string()))
        .mount(&server)
        .await;

    let error = client(&server)
        .login("user@example.com", "secret", false)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Login(LoginError::VerificationRequired(2))),
        "{error:?}"
    );
}

#[tokio::test]
async fn http_failures_map_to_errors_and_are_not_retried() {
    for status in [401, 403, 429, 500] {
        let server = MockServer::start().await;
        Mock::given(path("/User/ListDevices"))
            .respond_with(
                ResponseTemplate::new(status).insert_header("X-Mitsubishi-ErrorId", "abc-123"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let error = client(&server).devices(&session()).await.unwrap_err();
        match (status, &error) {
            (401, Error::Unauthorized) | (403, Error::Forbidden) | (429, Error::Throttled) => {}
            (
                500,
                Error::Status {
                    status: 500,
                    error_id,
                },
            ) => {
                assert_eq!(error_id.as_deref(), Some("abc-123"));
                assert_eq!(
                    error.to_string(),
                    "MELCloud answered HTTP 500 (error ID abc-123)"
                );
            }
            _ => panic!("HTTP {status} gave {error:?}"),
        }
    }
}

#[tokio::test]
async fn unexpected_bodies_name_the_endpoint() {
    let server = MockServer::start().await;
    Mock::given(path("/Device/Get"))
        .respond_with(json_response("{\"Power\": 1}"))
        .mount(&server)
        .await;

    let error = client(&server)
        .ata_state(&session(), DeviceId(1), BuildingId(2))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::Decode {
                endpoint: "/Device/Get",
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn devices_are_read_with_the_session_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/User/ListDevices"))
        .and(header("X-MitsContextKey", KEY))
        .and(header("accept", "application/json"))
        .respond_with(json_response(LIST_DEVICES))
        .expect(1)
        .mount(&server)
        .await;

    let devices = client(&server).devices(&session()).await.unwrap();
    let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Living room", "Bedroom"]);
}

#[tokio::test]
async fn state_is_read_for_device_and_building() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Device/Get"))
        .and(query_param("id", "1000001"))
        .and(query_param("buildingID", "2000001"))
        .and(header("X-MitsContextKey", KEY))
        .respond_with(json_response(DEVICE_GET))
        .expect(1)
        .mount(&server)
        .await;

    let state = client(&server)
        .ata_state(&session(), DeviceId(1_000_001), BuildingId(2_000_001))
        .await
        .unwrap();
    assert_eq!(state.view().mode, OperationMode::Cool);
}

#[tokio::test]
async fn command_sends_the_state_with_changes() {
    let server = MockServer::start().await;
    Mock::given(path("/User/ListDevices"))
        .respond_with(json_response(LIST_DEVICES))
        .mount(&server)
        .await;
    Mock::given(path("/Device/Get"))
        .respond_with(json_response(DEVICE_GET))
        .mount(&server)
        .await;
    let mut expected: Value = serde_json::from_str(DEVICE_GET).unwrap();
    expected["SetTemperature"] = json!(22.5);
    expected["EffectiveFlags"] = json!(4);
    Mock::given(method("POST"))
        .and(path("/Device/SetAta"))
        .and(header("X-MitsContextKey", KEY))
        .and(header("content-type", "application/json; charset=utf-8"))
        .and(body_json(expected))
        .respond_with(json_response(SET_ATA))
        .expect(1)
        .mount(&server)
        .await;

    let client = client(&server);
    let device = client.devices(&session()).await.unwrap().remove(0);
    let state = client
        .ata_state(&session(), device.id, device.building_id)
        .await
        .unwrap();
    let command = ChangeSet {
        target_temperature: Some(22.5),
        ..ChangeSet::default()
    }
    .build(&device, &state)
    .unwrap();
    let answer = client.set_ata(&session(), &command).await.unwrap();

    assert!((answer.view().target_temperature - 22.5).abs() < 1e-9);
    assert_eq!(answer.view().effective_flags, command.flags());
}
