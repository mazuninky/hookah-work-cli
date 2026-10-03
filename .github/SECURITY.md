# Security Policy

## Reporting a vulnerability

**Please do not open a public issue or pull request for security reports.**

Report privately through [GitHub Security Advisories](https://github.com/mazuninky/hookah-work-cli/security/advisories/new).
Include a description and impact, steps to reproduce, and the affected version (`hw --version`).

## Scope

`hw` is a read-only client for the HookahWork CRM REST API. Security-relevant surfaces:

- **Credential handling** — API tokens in the profile config file (the default) or the OS keyring
  (default `~/.config/hw/config.toml`, written with `0600` permissions) or the `HW_TOKEN`
  environment variable; keys and passwords typed into `hw auth login` (masked input) or read from
  stdin by `--with-token` / `--password-stdin`.
- **HTTP client** — TLS (rustls), request construction, response parsing.
- **Read-only guarantee** — `hw` must never send a request that creates, changes or deletes
  CRM data. A code path that does is a security bug.

Vulnerabilities in the HookahWork CRM itself are out of scope — report those to the vendor.

## How `hw` handles secrets

- Tokens and passwords are never accepted as command-line flag values, so they do not end up
  in shell history or process listings; they come from a masked prompt, stdin, the keyring, the config file or
  `HW_TOKEN`.
- Tokens are not printed in logs (`-vv`), error messages or `Debug` output. The only command
  that prints a token is `hw auth token`, by design.

If you find a case where a token leaks into logs, error messages or stored state, please report it.
