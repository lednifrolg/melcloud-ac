# melcloud-ac

Terminal client for Mitsubishi Electric air conditioners on the classic MELCloud service
(`app.melcloud.com`): one-shot commands for the shell and an interactive TUI.

Unofficial project, not affiliated with Mitsubishi Electric. It talks to an undocumented API
and controls a real device on a real account.

## Status

The command line and the interactive TUI both show and control the unit. Animations and a
richer history chart are next.
The API has been verified live for login, device listing, reading unit state and changing the
target temperature.

## Usage

```sh
cargo install --path crates/klima

klima                  # the interactive TUI
klima login            # asks for email and password; stores only the session
klima devices          # devices on the account (cached for a day, --fresh to ask again)
klima status           # unit state (cached for 5 minutes, --fresh to ask again)
klima status --json    # the same for scripts

klima on | off | toggle
klima temp 22.5        # or relative to the current target: +1, -0.5
klima mode cool        # heat, dry, cool, fan, auto
klima fan auto         # or a level from 1
klima vane --vertical swing --horizontal split
klima set --power on --mode heat --temp 21 --fan auto   # several changes, one command

klima logout
```

A command is checked against what the unit supports and what the account may change before
anything is sent, leaves out settings the unit already has, and goes out as one MELCloud
command. klima then waits until the unit's adapter has picked it up (a few seconds with MQTT)
and checks the unit reports the new values. `--dry-run` shows the command without sending it,
`--no-wait` returns right after sending.

With several devices, pick one with `--device <name or ID>` or set it in
`~/.config/klima/config.toml`:

```toml
email = "user@example.com"   # offered by `klima login`
device = "Living room"
refresh_minutes = 10         # automatic refresh in the TUI, at least 5
```

### TUI

`klima` without a command opens a dashboard: the unit's settings with big digits and a
temperature scale, the weather at the building, the adapter's WiFi and last contact, the room
temperature of the last 24 hours and an activity log. It refreshes on its own every 10 minutes
within the request budget, shows the countdown and the budget in the status line, and picks up
changes made by `klima` commands in other terminals at once.

To change the unit, select a setting with `↑`/`↓` (or `k`/`j`) and change it with `←`/`→` (or
`h`/`l`), or use `+`/`-` for the temperature, `m` mode, `f` fan, `s`/`S` swing and `P` power. The
mouse works too: click a mode or a fan level, scroll over the temperature. Changes made within 3
seconds of each other go out as one command, the same way `klima set` sends them, and the
control pane shows the countdown, the wait for the adapter and the result. `⏎` sends at once,
`u` discards, and `q` asks before dropping changes that are not sent yet. `?` lists every key; in
a narrow window the panes become tabs (`Tab`, `1`-`4`). Colors come from the terminal's palette,
so the TUI follows the terminal theme.

`klima login --email <email> --password-stdin` reads the password from standard input, e.g. from
a password manager. The password is never stored; only the session token is kept, readable only
by you. When the session expires, log in again.

Files: `~/.config/klima` (config), `~/.local/state/klima` (session, request ledger, history of
unit states), `~/.cache/klima` (last server responses). `KLIMA_HOME` puts all three in one
directory.

### Request budget

MELCloud throttles a whole account for hours after too many requests and publishes no limit.
Every request from every running klima process is counted in one shared ledger:

- at most 30 requests in any hour,
- at least 60 seconds between two reads of the same unit (a command uses one request, plus one
  or two reads to confirm it),
- after HTTP 429, no requests for 6 hours (`klima unblock` lifts this locally; MELCloud keeps
  its own limit),
- after a login lockout, no login attempts until it ends.

When a fresh read is not possible, `klima status` shows the last known state and says why.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | error talking to MELCloud, or a local file problem |
| 2 | wrong usage, e.g. an unknown or ambiguous device |
| 3 | stopped by the request budget, throttling or a login lockout |
| 4 | not logged in, or the session expired |
| 5 | the command was sent, but the adapter did not confirm it or the unit reports other values |

## Using the API responsibly

- Never retry a failed login in a loop. Repeated bad passwords lock the account behind a
  CAPTCHA that a headless client cannot solve.
- Stop on HTTP 429. Throttling applies to the whole account and lasts for hours.
- Read state sparingly (at most once every few minutes); one check after a command is enough.
- Never log or commit the session key (`ContextKey`), password, MAC address or serial number.

## Development

| Crate | Purpose |
|---|---|
| `crates/melcloud` | MELCloud API client, no UI dependencies |
| `crates/klima` | the `klima` binary: CLI and TUI |

Test data in `crates/melcloud/tests/data` is synthetic. Never commit captured server responses,
credentials, session tokens or real device identifiers. Code, docs and commit messages are in
English.

The end-to-end tests run the `klima` binary against a mock server; `KLIMA_API_URL` points klima
at a server other than MELCloud.

The checks CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --locked
cargo deny check
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
