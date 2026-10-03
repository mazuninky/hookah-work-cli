# Project layout

A tour of the source tree and a walkthrough for adding a command. Architecture rules (read-only
surface, secrets, exit codes) are in [`CLAUDE.md`](../CLAUDE.md); user docs in [`README.md`](../README.md).

## Request flow

```
main.rs ─ SIGPIPE reset → Cli::try_parse_from → logging → app::run_system
app.rs  ─ run_system builds real deps (IoStreams, SystemKeyring, HttpConnector, Env, now)
          run → execute: resolve config/profile/CRM/token, connect, call the handler
                render: Outcome → --jq/--template/--format → stdout; hints → stderr
cli/commands/<group>.rs ─ handler: args + &dyn Api (+ now) → Outcome (pure, no I/O of its own)
client/  ─ HookahClient: GET / login / find_client over ureq, retries, error mapping, pagination
```

Handlers never print, read config or build clients: they receive `&dyn Api` and return an
`Outcome { payload, hints, failure }`. That is what lets every handler be tested with
`test_util::FakeApi` and the whole binary be tested in-process through `app::run` with fake `Deps`.

## Source tree

| Path | Responsibility |
|---|---|
| `src/main.rs` | The four jobs only: SIGPIPE, parse (usage errors → exit 5), logging, call `app::run_system` |
| `src/app.rs` | Composition root: `Env`, `Deps`, `run_system`, `run`, `execute` (dispatch), `render` |
| `src/error.rs` | Domain `Error` (`thiserror`), `exit_code` constants, `exit_code_for_error` |
| `src/cli/args/` | clap structs: global flags on `Cli` in `mod.rs`, one file per command group; `ValueEnum`s for `--expand`/`--type` with their API names |
| `src/cli/commands/` | One handler module per command group, plus `api` (GET passthrough), `auth`, `config`, `menu`, `docs` (hidden `generate-docs`); `mod.rs` has `Outcome`/`Payload` and the shared `fetch` helper |
| `src/client/` | `Api` / `Connector` / `TokenIssuer` traits, `HookahClient`, `HttpConnector`, `Query`; `retry.rs` (429 + `X-Rate-Limit-Reset`, 5xx backoff, injectable sleeper), `pagination.rs` (`X-Pagination-*`), `status.rs` (Yii error JSON → `Error`) |
| `src/config/` | `Config`/`Profile` (serde, `deny_unknown_fields`), `ConfigLoader` (0600 writes; parse errors never quote the file), `ConfigStore` trait + `FileConfigStore` (injected into `auth`/`config` handlers), `crm.rs` (subdomain/host/URL normalisation, cleartext warning) |
| `src/auth/` | `Secret` (redacting newtype), `SecretStore` + `SystemKeyring` (`InMemoryStore` is test-util only), token resolution chain scoped to the profile's CRM host |
| `src/dates.rs` | `DateArg` (`today`/`yesterday`/`tomorrow`/`YYYY-MM-DD`) and business-day resolution via `/api/settings` |
| `src/io/` | `IoStreams` (TTY detection, colour, pager, test buffers) |
| `src/output/` | `Reporter` + console/json/toon/toml/csv reporters, `terminal_text`/`server_text` (escape control characters on a terminal; piped output stays exact), `transform.rs` (`--jq` via jaq, `--template` via minijinja; compiled before the request) |
| `src/test_util.rs` | `FakeApi`, `FakeConnector`, `FakeIssuer` — behind `cfg(test)` / the `test-util` feature |
| `tests/` | Integration tests: `read_only.rs` (every leaf command → only GET + the two allowed POSTs; a new command must be added to its table), `http_client.rs` (httpmock), `cli_*.rs` (`assert_cmd`, exit codes, auth/config), `cli_snapshots.rs` (insta), `business_day.rs`; helpers and fixtures in `tests/common/`, `tests/fixtures/` |
| `docs/reference/hw.md` | Generated CLI reference — run `scripts/gen-docs.sh`, never edit by hand |

## Adding a read-only command

Example: a hypothetical `GET /api/discounts?date=…` as `hw report discounts --date …`.

1. **Confirm it is read-only.** Only `GET` endpoints (and the two existing POSTs) are allowed. If the
   endpoint changes CRM data, it does not belong in `hw`.
2. **Args** — in `src/cli/args/report.rs`, add a `Discounts(DiscountsArgs)` variant to
   `ReportCommand` with a `///` doc line (it is the `--help` text), a `DateArg` field and, if the API
   has `expand`, a `ValueEnum` whose variants map to the API names via `ApiName`.
3. **Handler** — in `src/cli/commands/report.rs`, add the match arm: build a `Query` (`push`,
   `push_opt`; resolve dates with `DateResolver`) and return `fetch(api, "/api/discounts", &query)`.
   Paginated endpoints take a flattened `PageArgs` and call `fetch_paged` instead, like
   `product list` does.
4. **Tests** — a `FakeApi` test asserting the exact path and query pairs (see the existing tests in the
   same module), and a row in the `CASES` table of `tests/read_only.rs` (it fails until every leaf
   command is listed); an httpmock/CLI test if the command has new behaviour beyond a GET.
5. **Docs** — run `scripts/gen-docs.sh` and commit `docs/reference/hw.md`; add the command to the
   README table and to `skill/` if it is user-facing.

A new top-level group additionally needs a variant in `Command` (`src/cli/args/mod.rs`) and an arm in
`app::execute` (`connect(cli, deps, true)` for authenticated endpoints, `false` for public ones).

## Gates

```sh
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
scripts/gen-docs.sh && git diff --exit-code docs/reference/
```
