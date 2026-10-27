//! A stand-in for MELCloud with one simulated air conditioner, for demos and screenshots: no
//! request reaches the real service.
//!
//! ```sh
//! export KLIMA_HOME=$(mktemp -d) KLIMA_API_URL=http://127.0.0.1:18890
//! cargo run -p klima --example demo_server -- 18890 "$KLIMA_HOME" &
//! echo any-password | klima login --email user@example.com --password-stdin
//! klima
//! ```
//!
//! It serves the synthetic test data with the unit heating, logs any account in, applies a
//! command a moment after it arrives, as an adapter would, and fills the history of a new home
//! directory with a day of samples.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const LOGIN: &str = include_str!("../../melcloud/tests/data/login.json");
const LIST_DEVICES: &str = include_str!("../../melcloud/tests/data/list-devices.json");
const DEVICE_GET: &str = include_str!("../../melcloud/tests/data/device-get.json");
const DEVICE_ID: i64 = 1_000_001;

/// How long the simulated adapter takes to pick a command up.
const ADAPTER_DELAY: Duration = Duration::from_millis(1500);

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(port) = args.next().and_then(|p| p.parse::<u16>().ok()) else {
        fail("usage: demo_server <port> [<new home directory>]");
    };
    let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) else {
        fail(&format!("port {port} is taken"));
    };
    if let Some(home) = args.next().map(PathBuf::from) {
        prepare(&home);
    }
    let server = MockServer::builder().listener(listener).start().await;

    let unit = Unit(Arc::new(Mutex::new(State {
        unit: heating(),
        contact: Timestamp::now(),
    })));
    Mock::given(path("/Login/ClientLogin3"))
        .respond_with(json_response(&login()))
        .mount(&server)
        .await;
    Mock::given(path("/User/ListDevices"))
        .respond_with(json_response(LIST_DEVICES))
        .mount(&server)
        .await;
    // The other device on the list is not simulated; reading it gets a 404.
    Mock::given(path("/Device/Get"))
        .and(query_param("id", DEVICE_ID.to_string()))
        .respond_with(ReadState(unit.clone()))
        .mount(&server)
        .await;
    Mock::given(path("/Device/SetAta"))
        .respond_with(TakeCommand(unit))
        .mount(&server)
        .await;
    println!("demo MELCloud on {}", server.uri());
    std::future::pending::<()>().await;
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

fn json_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

/// A login answer whose session lasts a year.
fn login() -> String {
    let mut login: Value = serde_json::from_str(LOGIN).expect("synthetic data");
    let expiry = (Timestamp::now() + SignedDuration::from_hours(24 * 365))
        .to_zoned(TimeZone::UTC)
        .datetime();
    login["LoginData"]["Expiry"] = json!(expiry.to_string());
    login.to_string()
}

/// The synthetic unit, heating with its vanes swinging.
fn heating() -> Value {
    let mut state: Value = serde_json::from_str(DEVICE_GET).expect("synthetic data");
    state["Power"] = json!(true);
    state["OperationMode"] = json!(1);
    state["SetTemperature"] = json!(22.5);
    state["RoomTemperature"] = json!(22.1);
    state["SetFanSpeed"] = json!(3);
    state["VaneVertical"] = json!(7);
    state["VaneHorizontal"] = json!(12);
    state
}

/// MELCloud's way of writing UTC times.
fn utc(at: Timestamp) -> String {
    at.to_zoned(TimeZone::UTC)
        .strftime("%Y-%m-%dT%H:%M:%S%.3f")
        .to_string()
}

/// Local times without a zone, as MELCloud writes the weather: the current hour, noon today,
/// the night after it, noon tomorrow and the night after that.
fn forecast_dates(now: Timestamp) -> [String; 5] {
    let today = now.to_zoned(TimeZone::system()).date();
    let day = |offset: i32, hour: &str| {
        let date = today
            .checked_add(SignedDuration::from_hours(24 * i64::from(offset)))
            .unwrap_or(today);
        format!("{date}T{hour}:00:00")
    };
    [
        now.to_zoned(TimeZone::system())
            .strftime("%Y-%m-%dT%H:00:00")
            .to_string(),
        day(0, "12"),
        day(1, "00"),
        day(1, "12"),
        day(2, "00"),
    ]
}

/// The unit as MELCloud reports it, and when its adapter was last in touch.
struct State {
    unit: Value,
    contact: Timestamp,
}

#[derive(Clone)]
struct Unit(Arc<Mutex<State>>);

impl Unit {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `Device/Get`: the unit as it is now, with a recent contact and today's weather.
struct ReadState(Unit);

impl Respond for ReadState {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        let now = Timestamp::now();
        let mut state = self.0.lock();
        // In touch once a minute, the last time at most 20 s ago, or when it took a command.
        let contact = state.contact.max(now - SignedDuration::from_secs(20));
        let state = &mut state.unit;
        state["LastCommunication"] = json!(utc(contact));
        state["NextCommunication"] = json!(utc(contact + SignedDuration::from_secs(60)));
        if let Some(weather) = state["WeatherObservations"].as_array_mut() {
            for (observation, date) in weather.iter_mut().zip(forecast_dates(now)) {
                observation["Date"] = json!(date);
            }
        }
        json_response(&state.to_string())
    }
}

/// `Device/SetAta`: answers with the command, as MELCloud does, and applies it a moment later.
struct TakeCommand(Unit);

impl Respond for TakeCommand {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let command = serde_json::from_slice::<Value>(&request.body)
            .ok()
            .filter(|command| command.is_object() && command["DeviceID"] == DEVICE_ID);
        let Some(command) = command else {
            return ResponseTemplate::new(400);
        };
        let unit = self.0.clone();
        let applied = command.clone();
        std::thread::spawn(move || {
            std::thread::sleep(ADAPTER_DELAY);
            let now = Timestamp::now();
            let mut state = unit.lock();
            state.unit = applied;
            state.unit["EffectiveFlags"] = json!(0);
            state.contact = now;
        });
        json_response(&command.to_string())
    }
}

/// Writes a configuration and a day of history into `home`: off at night, heating in the
/// morning and in the evening, and for the last two hours whatever the time.
fn prepare(home: &Path) {
    let config = home.join("config");
    let state = home.join("state");
    // Never a real klima home: only a new or empty directory.
    if std::fs::read_dir(home).is_ok_and(|mut entries| entries.next().is_some()) {
        fail(&format!(
            "{} is not empty; give a new, empty directory",
            home.display()
        ));
    }
    for dir in [&config, &state] {
        std::fs::create_dir_all(dir).expect("the demo home can be written");
    }
    std::fs::write(
        config.join("config.toml"),
        "email = \"user@example.com\"\ndevice = \"Living room\"\n",
    )
    .expect("the demo home can be written");

    let now = Timestamp::now();
    let zone = TimeZone::system();
    let mut room = 20.8;
    let mut target = 22.0;
    let mut lines = Vec::new();
    for step in (0..=144_i32).rev() {
        let at = now - SignedDuration::from_mins(i64::from(10 * step));
        let hour = at.to_zoned(zone.clone()).hour();
        let heating = matches!(hour, 6..=8 | 17..=22) || step <= 12;
        // The target is set when the heating starts, and stays while the unit is off.
        if heating {
            target = if hour >= 17 || step <= 12 { 22.5 } else { 22.0 };
        }
        let goal = if heating { target } else { 18.5 };
        let pace = if heating { 0.12 } else { 0.03 };
        room += (goal - room) * pace;
        let wobble = (f64::from(step) * 1.7).sin() * 0.05;
        lines.push(
            json!({
                "at": at,
                "device": DEVICE_ID,
                "power": heating,
                "mode": "heat",
                "target": target,
                "room": ((room + wobble) * 10.0).round() / 10.0,
                "outside": 2.0,
                "fan": "3",
                "error_code": 8000,
            })
            .to_string(),
        );
    }
    std::fs::write(state.join("history.jsonl"), lines.join("\n") + "\n")
        .expect("the demo home can be written");
}
