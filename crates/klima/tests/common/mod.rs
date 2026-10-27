//! Helpers shared by the end-to-end tests: the `klima` binary run against a mock MELCloud.
// Each test crate uses a different part of this module.
#![allow(dead_code)]

use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const LOGIN: &str = include_str!("../../../melcloud/tests/data/login.json");
pub const LIST_DEVICES: &str = include_str!("../../../melcloud/tests/data/list-devices.json");
pub const DEVICE_GET: &str = include_str!("../../../melcloud/tests/data/device-get.json");
pub const EMAIL: &str = "user@example.com";
pub const PASSWORD: &str = "hunter2-synthetic";
pub const LIVING_ROOM: [&str; 2] = ["--device", "Living room"];

pub struct Klima {
    pub home: tempfile::TempDir,
    pub server: MockServer,
}

impl Klima {
    pub async fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            server: MockServer::start().await,
        }
    }

    pub async fn run(&self, args: &[&str]) -> Output {
        self.run_with_input(args, "").await
    }

    pub async fn run_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_klima"))
            .args(args)
            .env("KLIMA_HOME", self.home.path())
            .env("KLIMA_API_URL", self.server.uri())
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        drop(stdin);
        child.wait_with_output().await.unwrap()
    }

    /// Logs in through the CLI with a session that expires in `valid_for`.
    pub async fn login_for(&self, valid_for: SignedDuration) {
        Mock::given(path("/Login/ClientLogin3"))
            .respond_with(json_response(&login_response(valid_for)))
            .up_to_n_times(1)
            .mount(&self.server)
            .await;
        let output = self.login().await;
        assert!(output.status.success(), "{}", stderr(&output));
    }

    pub async fn login(&self) -> Output {
        self.run_with_input(
            &["login", "--email", EMAIL, "--password-stdin"],
            &format!("{PASSWORD}\n"),
        )
        .await
    }

    pub async fn mock_devices(&self) {
        Mock::given(path("/User/ListDevices"))
            .respond_with(json_response(LIST_DEVICES))
            .mount(&self.server)
            .await;
    }

    /// The bodies of every `Device/SetAta` request, in order.
    pub async fn commands(&self) -> Vec<Value> {
        let requests = self.server.received_requests().await.unwrap();
        requests
            .iter()
            .filter(|r| r.url.path() == "/Device/SetAta")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    pub async fn mock_account(&self) {
        Mock::given(path("/User/ListDevices"))
            .respond_with(json_response(LIST_DEVICES))
            .mount(&self.server)
            .await;
        Mock::given(path("/Device/Get"))
            .respond_with(json_response(DEVICE_GET))
            .mount(&self.server)
            .await;
    }

    pub async fn requests_to(&self, endpoint: &str) -> usize {
        let requests = self.server.received_requests().await.unwrap();
        requests.iter().filter(|r| r.url.path() == endpoint).count()
    }

    pub fn file(&self, name: &str) -> std::path::PathBuf {
        self.home.path().join(name)
    }
}

pub fn json_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

pub fn login_response(valid_for: SignedDuration) -> String {
    let mut login: Value = serde_json::from_str(LOGIN).unwrap();
    let expiry = (Timestamp::now() + valid_for)
        .to_zoned(TimeZone::UTC)
        .datetime();
    login["LoginData"]["Expiry"] = json!(expiry.to_string());
    login.to_string()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

/// What the simulated adapter does with a command.
#[derive(Debug, Clone, Copy)]
pub enum Adapter {
    /// Picks the command up at once and applies it.
    Applies,
    /// Picks the command up but leaves this field as it was.
    Keeps(&'static str),
    /// Never talks to the cloud.
    Silent,
}

/// The living room unit behind the mock server: `Device/Get` answers with its state and
/// `Device/SetAta` changes it the way [`Adapter`] says.
#[derive(Clone)]
pub struct Unit {
    state: Arc<Mutex<Value>>,
    adapter: Adapter,
}

impl Unit {
    pub async fn mount(server: &MockServer, adapter: Adapter) -> Self {
        let unit = Self {
            state: Arc::new(Mutex::new(serde_json::from_str(DEVICE_GET).unwrap())),
            adapter,
        };
        Mock::given(path("/Device/Get"))
            .respond_with(ReadState(unit.clone()))
            .mount(server)
            .await;
        Mock::given(path("/Device/SetAta"))
            .respond_with(TakeCommand(unit.clone()))
            .mount(server)
            .await;
        unit
    }

    pub fn state(&self) -> Value {
        self.state.lock().unwrap().clone()
    }
}

struct ReadState(Unit);

impl Respond for ReadState {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        json_response(&self.0.state().to_string())
    }
}

struct TakeCommand(Unit);

impl Respond for TakeCommand {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let command: Value = serde_json::from_slice(&request.body).unwrap();
        let mut state = self.0.state.lock().unwrap();
        let before = state.clone();
        *state = command.clone();
        match self.0.adapter {
            Adapter::Applies | Adapter::Keeps(_) => {
                if let Adapter::Keeps(field) = self.0.adapter {
                    state[field] = before[field].clone();
                }
                state["EffectiveFlags"] = json!(0);
                let now = Timestamp::now().to_zoned(TimeZone::UTC).datetime();
                state["LastCommunication"] = json!(now.to_string());
            }
            Adapter::Silent => {}
        }
        // Like MELCloud, the answer carries the new values and the pending flags.
        json_response(&command.to_string())
    }
}
