# hw

Read-only CLI для REST API HookahWork CRM (`https://<crm>.hookah.work/api/…`, документация —
https://hookah.work/api/). Один Rust-крейт в корне (edition 2024, MSRV 1.95, тулчейн запинен в
`rust-toolchain.toml` — `cargo +<channel>` не нужен). Синхронный HTTP (`ureq`), без tokio.
Тур по исходникам и «как добавить команду» — [`docs/project-layout.md`](docs/project-layout.md).

## Build & verify

```bash
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
scripts/gen-docs.sh          # после любой правки флагов/--help; CI сверяет docs/reference/
```

Pre-commit — `lefthook.yml` (fmt, clippy, test, guard на `.claude/`); обход хуков запрещён
`.claude/settings.json`.

## Инварианты (не видны из кода за минуту)

- **Только чтение.** К API уходят только `GET` + ровно два не-мутирующих `POST`: `/api/login`
  (токен по Basic auth) и `/api/client/find` (поиск). У клиента нет общего post/put/delete,
  `hw api` — GET-only без флага метода. Любой мутирующий запрос/эндпоинт — блокер, даже «по
  просьбе»; MCP-сервер CRM (`/api/mcp`) не используем.
- **Интерактив — только `hw auth login` на TTY** (решение пользователя, отступление от «non-interactive
  always»; как в atl): спрашивает CRM, способ входа и секрет маскированным вводом (`***`), показывает
  `<crm>/v2/settings/users` для генерации ключа. Вне TTY — только флаги, без промптов.
- **Секреты.** Токен/пароль только из маскированного промпта или stdin (`--with-token`,
  `--password-stdin`), keyring, конфига
  или `HW_TOKEN` — никогда из флагов. Не попадают в логи, ошибки, `Debug` (тип `Secret`), stdout —
  кроме `hw auth token`. `log`-мост tracing выключен намеренно: ureq пишет `Authorization` на TRACE.
- **Хранение токена** по умолчанию — `api_token` в конфиге (0600); интерактивный `auth login` без
  `--storage` спрашивает «конфиг / keyring»; смена хранилища удаляет копию из другого.
- **Резолв.** Токен: `HW_TOKEN` > `api_token` профиля > keyring (`hw:<profile>`, account = host CRM);
  токен профиля уходит только на origin CRM этого профиля — схема+хост+порт (`--crm` на другой
  origin, в т.ч. https→http, → auth-ошибка). Секреты маскируются и в сыром теле, и в декодированном
  сообщении ошибки.
  CRM: `--crm`/`HW_CRM` > профиль. Профиль: `-p`/`HW_PROFILE` > `default_profile` > единственный.
  Конфиг: `--config`/`HW_CONFIG` > `$XDG_CONFIG_HOME/hw/config.toml` > `~/.config/hw/config.toml`.
- **Бизнес-день.** Относительные даты (`today`…) для `booking list|timetable` уходят в API как есть;
  для остальных резолвятся на клиенте по `midnight` + `timezone` из `/api/settings`.
- **Вывод.** Хендлеры возвращают `Outcome` (данные + hints), рендерит один раз `app::run`
  через reporter; подсказки — только в stderr.
- **Exit-коды** (`error::exit_code`) — публичный контракт, под тестами: 0 ok · 1 runtime/API/сеть ·
  2 not found · 3 config · 4 auth (нет/битый токен, 401/403) · 5 invalid input (clap usage-ошибки
  тоже 5, не 2; 400/422). Новый вариант `Error` = новая поверхность exit-кодов — расширяй осознанно.
- **`docs/reference/hw.md` генерируется** (`hw generate-docs`) — руками не править.
- **`.claude/` не версионируется** (кроме `settings.json`): агенты, конвенции, правила, память —
  локальные. Упоминать в репозитории организацию, откуда портированы конвенции, нельзя — сторож в
  локальном `lefthook-local.yml`.

## Агенты и конвенции

Код в `src/`/`tests/` делегируется тройке `rust-cli-writer` / `rust-cli-test-writer` /
`rust-cli-reviewer` по [`.claude/rules/rust-cli-delegation.md`](.claude/rules/rust-cli-delegation.md);
конвенции — `.claude/conventions/rust-cli/` (локально). Перед кодом против сторонней библиотеки
сверяй её доку через context7 на запиненной версии.

## PR, ветки, релизы

- Всё в `master` — через PR; **никогда не мержить без явного одобрения пользователя**, не обходить
  branch protection (`--admin`).
- Версия — календарная `YYYY.WW.BUILD`, только через `scripts/bump-version.sh`; релиз по тегу
  `vYYYY.WW.BUILD` ([`docs/releasing.md`](docs/releasing.md)).
- GitHub Actions пинятся по commit SHA с `# vX` комментарием.
