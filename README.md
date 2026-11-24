# telegrambot-xiao

A single Telegram bot combining Ukranian's slash commands and Deficiente's message and media reactions. Commands remain slash commands; their text is also checked by the message handler. Ordinary text is not interpreted as a command.

## Running

Install Rust and FFmpeg, and configure the new bot in Telegram. Set:

- `TELOXIDE_TOKEN`: the new bot's token.
- `URL`: its publicly reachable HTTPS webhook URL (forwarded to port 8080).
- `openweather_key`: OpenWeather API key for `/weather`.
- `RUST_LOG` (optional): tracing filter, default `info`.
- `REMINDERS_CHAT_ID` (optional): numeric Telegram group ID. Leave unset to disable reminders.

```sh
cargo run --release
```

The bot must be able to receive ordinary group messages and documents for Deficiente's features; configure Telegram's bot privacy settings and group permissions accordingly. Do not run another webhook or polling consumer using the same token.

Media downloads are limited to 20 MB and must use a public HTTP(S) address. Redirect destinations are checked against the same restriction, with at most five redirects. FFmpeg conversions have a timeout and output size limit.

## Reminders

When `REMINDERS_CHAT_ID` is set, the bot sends special-date messages at midnight, the Thursday greeting at midnight on Thursdays, and tomorrow's Spanish public holidays at 21:00, all in `Europe/Madrid`. It queries OpenHolidaysAPI and preserves the original reminders bot's plain-text formatting. Stop the old reminders bot before enabling this scheduler to avoid duplicate messages. Jobs are in memory: reminders missed during downtime are not replayed, and running multiple replicas can send duplicates.

## Development

Install the pinned tools with `mise install`, then run `prek install` to enable the checks before commits. Run `prek run --all-files` manually at any time.

## Production deployment

`compose.yaml` targets the existing Traefik setup, not local development. It uses the published GHCR image and the external `proxy` network. Set `TELOXIDE_TOKEN`, `URL`, `OPENWEATHER_KEY`, and `BOT_HOST` (the hostname of `URL`), then run `docker compose up -d`. The proxy must provide the `http` and `https` entrypoints and the `letsencrypt` certificate resolver. Pushes to `main` publish the amd64 image tagged `latest`; release tags such as `v1.0.0` publish versioned images. CI also builds the image on pull requests.

## Checks

```sh
cargo fmt --check
cargo clippy --all-targets
cargo test
```
