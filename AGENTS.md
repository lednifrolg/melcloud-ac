# Agent instructions

Project: terminal client (CLI + TUI, Rust) for Mitsubishi Electric air conditioners on the
classic MELCloud API.

## Internal material (local only)

`research/` is gitignored and exists only on the owner's machine; a fresh clone does not have it.
It holds the API contract (`research/api.md`), live verification log
(`research/live-verification.md`), app analysis, one-off scripts and captured server responses
(`research/fixtures/`). Read `research/api.md` before changing API code, and update it and
`research/live-verification.md` when API behavior changes or something new is verified.
The agreed product and technical design (milestones, decisions, keymap) is in
`.lavish/klima-design.html`, also local only.

## Code

- `crates/melcloud` must stay free of UI and terminal dependencies; the CLI and TUI live in
  `crates/klima`.
- Before committing, run the checks listed in README "Development"; CI runs the same ones.
- Synthetic test data is in `crates/melcloud/tests/data`. When a captured response is available
  locally, `tests/synthetic_data.rs` fails if the synthetic data uses a field or JSON type the
  server does not send; keep it passing when adding data.

## Working with the live API

- Every request hits the owner's real account; commands change a real unit at home. Ask for
  consent before sending any command (`Device/Set*`, `Group/Set*`, anything but reads).
- Never retry login in a loop. Stop immediately on HTTP 429.
- Do not poll more often than once every few minutes.
- Credentials for live checks with the research scripts live in
  `~/.config/melcloud/credentials.json` (mode 600). The app itself never stores the password.
  Never print credentials, and never log `ContextKey`, password, MAC address or serial number.

## Repository

- The repository is public. Commit only project files: source, tests, synthetic test data, CI
  config, README and this file.
- Never commit anything from `research/`, captured server responses, credentials, session
  tokens, real IDs, addresses, coordinates, MAC addresses or serial numbers. Test data must be
  synthetic.
- Everything committed (code, comments, docs, commit messages, UI text) is in English.
