# Using the `hw` CLI

How to drive `hw`: setup, global flags, output formats, dates, pagination, `--expand`, every command
with its flag values, the GET passthrough, errors and scripting. What the data *means* is in
`domain.md`; ready-made answers to common questions are in `recipes.md`; the generated flag-by-flag
reference is `commands.md`.

## Contents

1. [Setup and authentication](#setup-and-authentication)
2. [Command shape and global flags](#command-shape-and-global-flags)
3. [Output formats](#output-formats)
4. [`--jq` and `--template`](#--jq-and---template)
5. [Dates and the business day](#dates-and-the-business-day)
6. [Pagination](#pagination)
7. [`--expand`](#--expand)
8. [Command guide](#command-guide)
9. [Generic GET: `hw api`](#generic-get-hw-api)
10. [Updating `hw`](#updating-hw)
11. [Errors, exit codes, retries](#errors-exit-codes-retries)
12. [Scripting and CI](#scripting-and-ci)

---

## Setup and authentication

Every lounge has its own CRM at `https://<crm>.hookah.work`. `--crm` accepts the subdomain (`demo`),
the host (`demo.hookah.work`) or a base URL. One profile = one CRM + one token.

```bash
hw auth login        # interactive; the USER runs it in their own terminal
hw auth status       # profile, CRM, token source (env|config|keyring|none), token validity
hw config list       # profiles: name, CRM, token storage, default marker
```

`hw auth login` on a terminal asks for the CRM, then for an API key (masked) or for a CRM user's
email and password, then where to save the token (config file or OS keyring). It prints where a
director generates the key: `https://<crm>.hookah.work/v2/settings/users`. **Never run it for the
user and never ask them to paste a key or password into the chat** — ask them to run it themselves.

Non-interactive login (scripts, CI) reads the secret from stdin, never from a flag:

```bash
echo "$TOKEN" | hw auth login --crm demo --with-token
printf '%s\n' "$PASSWORD" | hw auth login --crm demo --email api@example.com --password-stdin
hw auth login --crm demo --with-token --storage keyring < token.txt
```

Without a flag and without a terminal, `auth login` fails with exit 5. The token is checked with
`GET /api/settings` before it is saved; nothing is saved if it is invalid.

| Command | Effect |
|---|---|
| `hw auth status` | Shows what is in effect; **exit 4** when the token is missing or rejected |
| `hw auth token` | Prints the token — for the user's own scripts only; never run it to show the token in chat |
| `hw auth logout` | Removes the profile's stored token; the profile stays |
| `hw config show [NAME]` | One profile, token redacted |
| `hw config set-default NAME` | Switch the default profile |
| `hw config delete NAME` | Delete a profile and its keyring token |
| `hw config path` | Config file path, even if the file does not exist yet |

Resolution order:

- **Token:** `HW_TOKEN` > the profile's `api_token` (config file) > OS keyring.
- **CRM:** `--crm` / `HW_CRM` > the profile's `crm`.
- **Profile:** `-p` / `HW_PROFILE` > `default_profile` > the only profile.
- **Config file:** `--config` / `HW_CONFIG` > `$XDG_CONFIG_HOME/hw/config.toml` > `~/.config/hw/config.toml`.

A stored token is only sent to its own profile's CRM (same scheme, host and port). `--crm other`
with a stored token fails with exit 4; use `HW_TOKEN` or a separate profile for another lounge:

```bash
hw -p lounge2 booking list                 # a second profile
HW_CRM=demo HW_TOKEN=… hw ref tables       # no config at all (CI)
```

## Command shape and global flags

```text
hw [global-flags] <group> <action> [args]
```

Global flags work before or after the subcommand.

| Flag | Purpose |
|---|---|
| `-F, --format console\|json\|toon\|toml\|csv` | Output format (default `console`) |
| `--jq EXPR` | Filter the JSON result with jq before formatting |
| `--template TPL` | Render with a minijinja template instead of `--format` |
| `-p, --profile NAME` | Profile (`HW_PROFILE`) |
| `--crm CRM` | CRM subdomain, host or URL (`HW_CRM`) |
| `--config PATH` | Config file (`HW_CONFIG`) |
| `--no-pager` | Don't page console output (`HW_NO_PAGER=1`) |
| `--no-color` | No colours (`NO_COLOR` too) |
| `--retries N` | Retries for 429/5xx/network errors (default 3, `0` = off) |
| `-v, -vv, -vvv` | Logs on stderr: requests at `-vv` (the token is never logged) |
| `-q, --quiet` | Errors only on stderr: no hints, no warnings (conflicts with `-v`) |

Data goes to stdout; logs, warnings and hints (`hint: …`) go to stderr.

## Output formats

| Format | Use it when |
|---|---|
| `toon` | **You** read the result — compact, keeps every field, far fewer tokens than JSON |
| `json` | A script parses it, or you use `--jq` and want to see the exact JSON |
| `csv` | The user wants a spreadsheet. List of objects → header (union of keys) + one row per item; nested values become compact JSON cells |
| `console` | A person reads it in a terminal (tables; paged on a TTY) |
| `toml` | Config-like dumps; a top-level list is wrapped as `items` |

```bash
hw -F toon booking list --date today --expand client,table   # for your own reading
hw -F csv analytics sale --from 2026-09-01 --till 2026-09-30 > sales.csv
```

## `--jq` and `--template`

`--jq` runs first (jaq, a jq clone), then the template or the formatter. Zero results → `null`, one →
that value, several → an array. Expressions are compiled before the request, so a syntax error
costs no API call (exit 5).

```bash
hw -F json ref tables --jq '[.[] | {id, name}]'
hw -F json sale list --date today --jq 'map(.price * .count) | add'
hw booking list --jq '[.[] | select(.status == 2) | .name]'
```

In `--template`, the whole result is `this`; an object's keys are also top-level variables:

```bash
hw ref tables --template '{% for t in this %}{{ t.id }} {{ t.name }}
{% endfor %}'
```

Before writing an aggregation, look at one record: `hw -F json <cmd> --jq '.[0]'`. Field names differ
per endpoint; `domain.md` lists them.

## Dates and the business day

`--date`, `--from`, `--till` accept `today`, `yesterday`, `tomorrow` or `YYYY-MM-DD`. Periods are
inclusive on both ends.

The CRM reports by **business day**, which starts at the hour `midnight` from `hw ref settings` in the
CRM's `timezone` (with `midnight = 6`, a sale at 03:00 on the 2nd belongs to the 1st). `hw` resolves
relative dates accordingly:

- `booking list` / `booking timetable` send `today|yesterday|tomorrow` to the API as is (the CRM
  shifts them itself); `--date` defaults to today.
- Every other command resolves them client-side with one extra `GET /api/settings`.

Prefer the relative words over computing calendar dates yourself — the user's clock and time zone
may differ from the lounge's. For "last month", use explicit dates: `--from 2026-09-01 --till 2026-09-30`.

## Pagination

Paginated: `product list`, `client list`, `client history` (and `hw api --all`). Everything else
returns the whole list in one response.

| Flag | Effect |
|---|---|
| (none) | First page only; stderr hint `page 1 of 5 (432 total); use --all or --page <N>` when more exist |
| `--all` | Every page, concatenated into one list (100 per request) |
| `--page N` | Page N (from 1); conflicts with `--all` |
| `--per-page N` | 1–100; server default 100 (20 for `client history`) |

Use `--all` whenever the answer must be complete (counts, sums, "all clients who…").

## `--expand`

Adds related records to each item. Comma-separated or repeated; CLI names are kebab-case, `hw` maps
them to the API's names. **In the JSON the expanded key keeps the API spelling** — `--expand
bonus-operations` adds `.bonusOperations`, `--expand payment-method` adds `.paymentMethod`,
`--expand bonus-level` adds `.bonusLevel`, `--expand new-bookings` adds `.newBookings`.

| Command | `--expand` values |
|---|---|
| `booking list`, `booking view` | `client`, `table`, `sales`, `hookahs`, `payments`, `credit`, `bonus-operations`, `total` |
| `sale list` | `product`, `booking`, `seller` |
| `hookah list` | `service`, `booking` |
| `client find` | `bonus-level`, `new-bookings` |
| `product list` | `components` |
| `storage purchases` | `product`, `storage` |
| `report expenses` | `category`, `payment-method` |
| `report credits` | `client`, `payment-method` |
| `report bonus-points` | `client` |

`total` on `booking list` costs the CRM one query per booking — fine for a day, avoid it in loops.
Expanding is cheaper than calling `booking view` once per booking.

## Command guide

### Reference data — `hw ref`

| Command | Returns |
|---|---|
| `hw ref settings` | Booking grid step, opening hours, business-day start (`midnight`), `timezone` |
| `hw ref tables` | Tables, in interface order |
| `hw ref payment-methods` | Payment methods, in checkout order |
| `hw ref employees` | Active employees, by name |
| `hw ref services` | Services — hookah types and related items |
| `hw ref bonus-levels` | Bonus program levels, entry level first |

Use them to turn ids (`table_id`, `service_id`, `payment_method_id`, …) into names.

### Products and storage

```bash
hw product list [--category ID] [--type product|dish|ingredient|semi] [--search TEXT] [--expand components] [--all]
hw product categories
hw storage list
hw storage purchases --date DATE [--storage ID] [--expand product,storage]
```

`--search` matches part of the name. `components` lists what a dish or semi-finished product is made of.

### Clients

```bash
hw client list [--search TEXT] [--group ID] [--all]      # oldest first; --search: name or card number
hw client groups
hw client history ID [--all]                             # the client's bookings, newest first
hw client find --phone 9175555111 [--expand bonus-level,new-bookings]
hw client find --card 1234
hw client find --id 42
```

`client find` returns **one** client (the first match) and exits 2 if there is none. `--phone` takes
the number or a part of it, with or without the country code. `--id`, `--card`, `--phone` can be
combined; at least one is required.

### Bookings

```bash
hw booking list [--date DATE] [--expand …]      # one business day, deleted bookings excluded
hw booking timetable [--date DATE]              # same day, object keyed by table id
hw booking view ID [--expand …]                 # one booking, deleted ones included (status 0)
hw booking my --device ID                       # upcoming new bookings made from one device
```

### Sales and hookahs

```bash
hw sale list --date DATE [--expand product,booking,seller]
hw sale list --booking 495,496                  # or --booking 495 --booking 496
hw hookah list --date DATE [--expand service,booking]
hw hookah list --booking 495
```

At least one of `--booking` / `--date` is required; both together narrow to those bookings on that
day. Deleted items are excluded. Sales need the «Продажи» right; hookahs and bookings need «Столы».

### Daily reports — `hw report`

```bash
hw report expenses --date DATE [--expand category,payment-method]
hw report credits --date DATE [--type debt|return|debt-off-cash|deposit-off-cash] [--expand client,payment-method]
hw report bonus-points --date DATE [--type visit|spend|gift|confiscation|expiry] [--expand client]
```

`report credits` without `--type` returns debts and returns (types 1 and 2). Expenses need the
«Расходы» right; credits and bonus points need «Бухгалтерия».

### Analytics — `hw analytics`

```bash
hw analytics hookah --from DATE --till DATE     # hookahs per service, by revenue
hw analytics sale --from DATE --till DATE       # products sold per product, by turnover
```

Counts closed tables only — the current evening is incomplete here; for "so far today" use
`sale list` / `hookah list --date today`.

### Public menu

```bash
hw --crm demo menu      # no token needed
```

## Generic GET: `hw api`

For read endpoints without a dedicated command. **GET only** — there is no method flag.

```bash
hw api tables
hw api /api/products --query search=чай --query per-page=50 --all
hw api 'timetable/list?date=today&expand=client'
hw --crm demo api menu --no-auth
```

`PATH` accepts `tables`, `api/tables` or `/api/tables`, with an optional `?query`. A non-JSON body is
printed as text. Prefer the dedicated commands: they validate flags and resolve business-day dates.

## Updating `hw`

```bash
hw -F toon self check                            # {current, latest, update_available, release_url}
hw self update                                   # ONLY when the user asked to update hw
hw self update --to 2026.41.1 --allow-downgrade  # pin or roll back (older than the current needs the flag)
```

`self update` downloads the platform's release archive from GitHub, verifies its SHA-256 checksum
and replaces the running binary; it refuses binaries managed by Homebrew/Nix or in system
directories. It needs no config or token and never contacts the CRM. Versions are `YYYY.WW.BUILD`
(year, ISO week without a leading zero, build).

## Errors, exit codes, retries

| Exit | Meaning | What to do |
|---|---|---|
| 0 | Success (an empty list `[]` is a success too) | — |
| 1 | Runtime, API or network error | Retry later; `-vv` shows the requests |
| 2 | Not found (404, or no client matched) | Check the id |
| 3 | Configuration error: no CRM, unknown profile, broken config | `hw auth status`, `hw config list`, or pass `--crm` |
| 4 | Auth: no token, invalid token, 401/403, token for another CRM | Ask the user to run `hw auth login`; 403 on a report usually means the CRM user lacks that section's right |
| 5 | Invalid input: bad flag/date, 400/422 from the API | `hw <cmd> --help` |

Errors print one line on stderr, never the token. `429` is retried after `X-Rate-Limit-Reset`
(the CRM allows 120 requests a minute per key); `5xx` and network errors back off exponentially.

## Scripting and CI

- `HW_CRM` + `HW_TOKEN` are enough — no config file or keyring needed.
- Use `-F json` + `--jq` for machine parsing, not `toon`.
- Use `--all` for complete lists.
- Pager and colours turn off automatically when stdout is not a terminal.
- Check the exit code, not stderr text.

```bash
export HW_CRM=demo HW_TOKEN="$HOOKAH_TOKEN"
if ! revenue=$(hw -F json sale list --date yesterday --jq 'map(.price * .count) | add // 0'); then
  echo "hw failed with exit $?" >&2
  exit 1
fi
echo "yesterday's bar revenue: $revenue"
```
