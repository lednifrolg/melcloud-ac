# melcloud-ac

Terminal client for Mitsubishi Electric air conditioners on the classic MELCloud service
(`app.melcloud.com`): one-shot commands for the shell and an interactive TUI.

Unofficial project, not affiliated with Mitsubishi Electric. It talks to an undocumented API
and controls a real device on a real account.

## Status

Early development: the `melcloud` API client crate is in place, the `klima` CLI does not do
anything yet.
The API has been verified live for login, device listing, reading unit state and changing the
target temperature.

## Login

`klima login` asks for your email and password. The password is never stored; only the session
token is kept locally, readable only by you. When the session expires, log in again.

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
