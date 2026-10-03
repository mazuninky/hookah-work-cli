# hw

[![CI](https://github.com/mazuninky/hookah-work-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/mazuninky/hookah-work-cli/actions/workflows/ci.yml)
[![Release](https://github.com/mazuninky/hookah-work-cli/actions/workflows/release.yml/badge.svg)](https://github.com/mazuninky/hookah-work-cli/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.95-blue)](Cargo.toml)

Read-only command-line client for the [HookahWork](https://hookah.work) CRM API. Written in Rust,
scriptable (only `hw auth login` asks questions, and only on a terminal), with structured output and
multi-profile config.

- **Read-only by construction.** Every command is a `GET`, plus the two non-mutating `POST`s the API
  needs for logging in and for looking up a client. `hw` never creates, changes or deletes CRM data —
  there is no code path that could.
- Covers the read side of the [HookahWork API](https://hookah.work/api/): reference data, products,
  storage, clients, bookings, sales, hookahs, daily reports, analytics and the public menu.
- Structured output: `console`, `json`, `toon`, `toml`, `csv`, plus `--jq` and `--template`.
- Business-day aware dates: `--date today|yesterday|tomorrow|YYYY-MM-DD`, resolved with the CRM's
  time zone and day-start hour.
- Multiple named profiles, token in the config file (0600) or the OS keyring, env-var overrides, no
  secrets in flags.
- A generic `hw api` passthrough (GET only) for anything the dedicated commands don't cover.

## Installation

### From GitHub Releases with attestation verification (recommended)

Release archives are built by this repo's `release.yml` and carry [SLSA build provenance](https://slsa.dev/):

```sh
gh release download --repo mazuninky/hookah-work-cli --pattern 'hw-*-x86_64-unknown-linux-gnu.tar.gz'
gh attestation verify hw-*-x86_64-unknown-linux-gnu.tar.gz --repo mazuninky/hookah-work-cli
tar -xzf hw-*-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 hw-*/hw /usr/local/bin/hw
```

Prebuilt artifacts: Linux (x86_64), macOS (arm64), Windows (x86_64).

### Install script

[`scripts/install.sh`](scripts/install.sh) downloads a release, verifies its checksum and installs it
(default `/usr/local/bin`, override with `--install-dir`). Review it or pin a version before piping
it into `sh`:

```sh
curl -sSfL https://raw.githubusercontent.com/mazuninky/hookah-work-cli/master/scripts/install.sh | sh -s -- --version <YYYY.WW.BUILD>
```

### From source

Requires Rust **1.95 or newer**.

```sh
git clone https://github.com/mazuninky/hookah-work-cli.git
cd hookah-work-cli
cargo install --path . --root ~/.local --force
codesign -s - -f ~/.local/bin/hw   # macOS only: lets the binary use the login keychain
```

## Quick start

Every lounge has its own CRM at `https://<crm>.hookah.work`; `hw` takes the subdomain (`demo`),
the host or the full URL.

```sh
# 1. Log in interactively: hw asks for the CRM, then for an API key (generate one at
#    https://<crm>.hookah.work/v2/settings/users) or a CRM user's email and password.
#    Secrets are typed into a masked prompt (`***`) and never passed as flags.
hw auth login
#    In scripts/CI, pipe the secret instead:
echo "$TOKEN" | hw auth login --crm demo --with-token
printf '%s\n' "$PASSWORD" | hw auth login --crm demo --email api@example.com --password-stdin
#    (A dedicated CRM user for integrations is best: changing its password rotates the token.)

# 2. Read.
hw ref tables
hw booking list --date today --expand client,table
hw sale list --date yesterday --expand product
hw report expenses --date 2026-09-30
hw analytics sale --from 2026-09-01 --till 2026-09-30
hw client find --phone 9175555111 --expand bonus-level
hw product list --search чай --all -F json

# The public menu needs no token at all.
hw --crm demo menu
```

Run `hw --help` or `hw <command> --help` for every flag; the full reference is
[`docs/reference/hw.md`](docs/reference/hw.md).

| Command | What it reads |
|---|---|
| `hw ref settings\|tables\|payment-methods\|employees\|services\|bonus-levels` | Reference data |
| `hw product list\|categories` | Products (paginated), categories |
| `hw storage list\|purchases` | Storages, purchases of a business day |
| `hw client list\|groups\|history\|find` | Clients (paginated), groups, visit history, lookup by id/card/phone |
| `hw booking list\|timetable\|view\|my` | Bookings of a day (flat or grouped by table), one booking, a device's bookings |
| `hw sale list`, `hw hookah list` | Sales / hookahs by booking(s) and/or business day |
| `hw report expenses\|credits\|bonus-points` | Daily reports |
| `hw analytics hookah\|sale` | Period summaries |
| `hw menu` | Public electronic menu |
| `hw api <path>` | Any GET endpoint |
| `hw auth`, `hw config` | Login/token and profile management |

### Dates and the business day

HookahWork reports by **business day**: it starts at the hour set as `midnight` in the CRM settings,
so a sale at 03:00 belongs to the previous date. `--date`, `--from` and `--till` accept
`today`, `yesterday`, `tomorrow` or `YYYY-MM-DD`; relative values are resolved with the CRM's time
zone and day-start hour (fetched from `/api/settings`), not your machine's clock.

### Pagination

Products, clients and client history are paginated by the API. Without flags `hw` prints the first
page and a hint on stderr when there are more; use `--page N`, `--per-page N` (max 100) or `--all`.

## Configuration

Profiles live in `~/.config/hw/config.toml` (`$XDG_CONFIG_HOME/hw/config.toml` if set; on Windows the
platform config dir). `hw auth login` creates them; you rarely need to edit the file:

```toml
default_profile = "demo"

[profiles.demo]
crm = "demo"                # subdomain, host or base URL
email = "api@example.com"   # set by password login; informational
token_storage = "config"    # config (default; file is written 0600) | keyring
api_token = "…"             # only with token_storage = "config"
```

Environment overrides:

| Variable | Effect |
|---|---|
| `HW_TOKEN` | API token; wins over the config file and the keyring (handy in CI) |
| `HW_CRM` | CRM to use (same as `--crm`) — with `HW_TOKEN`, no config file is needed |
| `HW_PROFILE` | Profile to use (same as `-p`) |
| `HW_CONFIG` | Config file path (same as `--config`) |
| `HW_PAGER` / `PAGER`, `HW_NO_PAGER` | Pager for console output |
| `NO_COLOR` | Disable colors |

A profile's stored token is only ever sent to that profile's own CRM origin (scheme, host and port):
pointing `--crm`/`HW_CRM` at a different CRM — or downgrading it to `http://` — requires `HW_TOKEN` or a
separate `hw auth login`.

`hw auth status` shows which profile, CRM and token source are in effect and whether the token works;
`hw config list|show|set-default|delete|path` manage profiles.

### Permissions and rate limits

The API acts as the user who owns the token and follows that user's rights in «Пользователи»:
reports need «Расходы»/«Бухгалтерия», analytics needs «Аналитика», and so on — a missing right is
exit code 4. One key may make 120 requests a minute; `hw` waits for `X-Rate-Limit-Reset` and retries
on `429`, and backs off on `5xx`/network errors (`--retries`, default 3).

## Output formats

```sh
hw -F json booking view 715 --expand payments,total
hw -F toon sale list --date today          # compact, for LLMs
hw -F csv analytics sale --from 2026-09-01 --till 2026-09-30 > sales.csv
hw booking list --jq '[.[] | select(.status == 2) | .name]'
hw ref tables --template '{% for t in this %}{{ t.id }} {{ t.name }}
{% endfor %}'
```

In `--template`, the whole result is `this`; for an object its keys are also top-level variables.

`console` (the default) renders tables; long output goes through a pager on a terminal
(`--no-pager` to disable). Diagnostics and hints go to stderr, data to stdout.

## Generic GET passthrough

```sh
hw api tables
hw api /api/products --query search=чай --query per-page=50 --all
hw api menu --no-auth
```

`hw api` only sends `GET` requests; there is no method flag.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Runtime, API or network error |
| 2 | Not found (404) |
| 3 | Configuration error |
| 4 | Authentication error (no token, invalid token, 401/403) |
| 5 | Invalid input (usage error, 400/422) |

## Claude Code skill

[`skill/`](skill/SKILL.md) is a [Claude Code](https://claude.ai/code) skill that teaches Claude to
answer questions about a lounge with `hw` (bookings, sales, reports, clients):

```sh
npx skills add mazuninky/hookah-work-cli
```

| File | Contents |
|---|---|
| [`SKILL.md`](skill/SKILL.md) | Entry point: safety rules, composing a command, domain essentials, anti-examples |
| [`references/domain.md`](skill/references/domain.md) | The HookahWork domain: entities and fields, booking statuses, checks and payments, bonuses and debts, business day, access rights, data pitfalls, Russian glossary |
| [`references/usage.md`](skill/references/usage.md) | Using the CLI: auth, flags, formats, dates, pagination, `--expand` values, every command, exit codes, scripting |
| [`references/recipes.md`](skill/references/recipes.md) | Typical lounge questions answered with ready commands and `--jq` |
| [`references/commands.md`](skill/references/commands.md) | Generated flag reference (`scripts/gen-docs.sh`) |
| [`evals/evals.json`](skill/evals/evals.json) | Prompts and assertions for evaluating the skill |

The HookahWork CRM also ships its own MCP server (`https://<crm>.hookah.work/api/mcp`); `hw` is the
scriptable, read-only alternative for terminals, CI and agents.

## Shell completions

```sh
hw completions bash > ~/.local/share/bash-completion/completions/hw
hw completions zsh  > "${fpath[1]}/_hw"
hw completions fish > ~/.config/fish/completions/hw.fish
```

## Contributing

See [`docs/project-layout.md`](docs/project-layout.md) for a tour of the source tree and how to add a
command. Build, test and lint with:

```sh
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
scripts/gen-docs.sh   # after changing any flag or help text
```

Report security issues privately — see [`.github/SECURITY.md`](.github/SECURITY.md).

## License

[MIT](LICENSE).
