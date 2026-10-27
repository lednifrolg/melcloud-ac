# melcloud-ac

Terminal client for Mitsubishi Electric air conditioners on the classic MELCloud service
(`app.melcloud.com`): one-shot commands for the shell and an interactive TUI.

Unofficial project, not affiliated with Mitsubishi Electric. It talks to an undocumented API
and controls a real device on a real account.

## Status

Design phase. The API has been verified live for login, device listing, reading unit state and
changing the target temperature. No CLI yet.

## Credentials

The client will read `~/.config/melcloud/credentials.json`, kept outside the repository and
readable only by the owner (`chmod 600`):

```json
{"email": "user@example.com", "password": "..."}
```

## Using the API responsibly

- Never retry a failed login in a loop. Repeated bad passwords lock the account behind a
  CAPTCHA that a headless client cannot solve.
- Stop on HTTP 429. Throttling applies to the whole account and lasts for hours.
- Read state sparingly (at most once every few minutes); one check after a command is enough.
- Never log or commit the session key (`ContextKey`), password, MAC address or serial number.
