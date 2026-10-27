# melcloud-ac

Terminal client for Mitsubishi Electric air conditioners on the classic MELCloud service
(`app.melcloud.com`): one-shot commands for the shell and an interactive TUI.

Unofficial project, not affiliated with Mitsubishi Electric. It talks to an undocumented API
and controls a real device on a real account.

## Status

The command line and the interactive TUI both show and control the unit. The API has been
verified live for login, device listing, reading the unit's state and every kind of command.
klima runs on Linux and macOS; Windows is not supported.

## Install

Prebuilt binaries for Linux (x86_64 and aarch64, static) and macOS 11 or later (Apple silicon
and Intel) are on the [releases page](https://github.com/lednifrolg/melcloud-ac/releases). In
bash or zsh, this picks the one for the machine from the latest release and checks it against
the release's checksums:

```sh
case "$(uname -sm)" in
  "Linux x86_64") target=x86_64-unknown-linux-musl ;;
  "Linux aarch64") target=aarch64-unknown-linux-musl ;;
  "Darwin arm64") target=aarch64-apple-darwin ;;
  "Darwin x86_64") target=x86_64-apple-darwin ;;
  *) target=none; echo "no prebuilt klima for $(uname -sm); build it from source" ;;
esac
latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/lednifrolg/melcloud-ac/releases/latest)
tag=${latest##*/}
cd "$(mktemp -d)"
url="https://github.com/lednifrolg/melcloud-ac/releases/download/$tag"
[ "$target" != none ] &&
curl -fsSLO "$url/klima-$target.tar.gz" &&
curl -fsSLO "$url/SHA256SUMS" &&
grep " klima-$target.tar.gz\$" SHA256SUMS > checksum &&
if command -v sha256sum > /dev/null; then sha256sum -c checksum; else shasum -a 256 -c checksum; fi ||
rm -f "klima-$target.tar.gz"
```

An archive that fails the check is deleted, so the steps below cannot install it. With the
GitHub CLI logged in, this checks that the archive was built from that tag by this
repository's release workflow:

```sh
gh attestation verify "klima-$target.tar.gz" --repo lednifrolg/melcloud-ac \
  --cert-identity "https://github.com/lednifrolg/melcloud-ac/.github/workflows/release.yml@refs/tags/$tag" \
  --deny-self-hosted-runners
```

Then, in the same terminal, this installs it into `~/.local/bin`:

```sh
tar -xzf "klima-$target.tar.gz" &&
mkdir -p ~/.local/bin &&
install -m 755 "klima-$target/klima" ~/.local/bin/ &&
~/.local/bin/klima --version &&
type klima
```

The last line should say `klima is` followed by the path in `~/.local/bin`. If klima is not
found, add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc` or `~/.bashrc` and open a new
terminal. If it names another file, an older klima comes first on PATH; remove it, with
`cargo uninstall klima` for one built from source. The same commands update klima. Other
systems, like a 32-bit Raspberry Pi, build from source (below). The archive also holds shell
completions in `completions/` and man pages in `man/man1/`, the same files `klima completions`
and `klima man` write (see [Usage](#usage)).

The installed binary is attested as well, so it can be checked later from any terminal:

```sh
gh attestation verify ~/.local/bin/klima --repo lednifrolg/melcloud-ac \
  --cert-identity "https://github.com/lednifrolg/melcloud-ac/.github/workflows/release.yml@refs/tags/v$(~/.local/bin/klima --version | cut -d ' ' -f 2)" \
  --deny-self-hosted-runners
```

On macOS, a binary downloaded with a browser instead is quarantined and refused as unverified;
`xattr -d com.apple.quarantine ~/.local/bin/klima` lifts that.

From source, with a recent Rust from [rustup](https://rustup.rs), for a release like `v0.1.0`:
`cargo install --git https://github.com/lednifrolg/melcloud-ac --tag v0.1.0 --locked klima`.

## Usage

```sh
klima                  # the interactive TUI
klima login            # asks for email and password; stores only the session
klima devices          # devices on the account (cached for a day, --fresh to ask again)
klima status           # unit state (cached for 5 minutes, --fresh to ask again)
klima status --json    # the same for scripts
klima history          # room temperature and changes of the last 24 h (--since 7d, --json)

klima on | off | toggle
klima temp 22.5        # or relative to the current target: +1, -0.5
klima mode cool        # heat, dry, cool, fan, auto
klima fan auto         # or a level from 1
klima vane --vertical swing --horizontal split
klima set --power on --mode heat --temp 21 --fan auto   # several changes, one command

klima logout
```

Shell completions and man pages come from the binary itself; run the line for your shell, and
again after updating klima. For bash, with the bash-completion package installed:

```sh
mkdir -p ~/.local/share/bash-completion/completions && klima completions bash > ~/.local/share/bash-completion/completions/klima
```

For zsh:

```sh
mkdir -p ~/.zfunc && klima completions zsh > ~/.zfunc/_klima
```

and in `~/.zshrc`, `fpath=(~/.zfunc $fpath)` before `compinit` runs (with oh-my-zsh, before
`source $ZSH/oh-my-zsh.sh`). A `~/.zshrc` with neither, as on a fresh macOS, needs
`fpath=(~/.zfunc $fpath); autoload -Uz compinit && compinit`. For fish:

```sh
mkdir -p ~/.config/fish/completions && klima completions fish > ~/.config/fish/completions/klima.fish
```

elvish and PowerShell work the same way. The man pages, for `man klima`, `man klima-set` and the
other commands:

```sh
klima man ~/.local/share/man/man1
```

man finds them while `~/.local/bin` is on PATH; otherwise `export MANPATH="$HOME/.local/share/man:"`
in the shell's startup file helps.

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
animations = true            # spinning fan, swinging vanes, flashing digits in the TUI
```

### TUI

`klima` without a command opens a dashboard: the unit's settings with big digits and a
temperature scale, the weather at the building, the adapter's WiFi and last contact, a chart of
the last 24 hours and an activity log. The chart draws the room temperature, and the target
while the unit runs, over clock hours with a band underneath for when the unit was off or
running and in which mode; it is built from every read of the unit, by the TUI and by commands
alike, and leaves gaps where nothing was read. In the shell, `klima history` draws the room
temperature and the modes and lists the changes of the settings; the history keeps 90 days. The
TUI refreshes on its own every 10 minutes within the request budget, shows the countdown and the
budget in the status line, and picks up changes made by `klima` commands in other terminals at
once. While the unit runs, the fan turns at its speed and swinging vanes sweep; set
`animations = false` for a still screen.

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
