//! `hw self check` and `hw self update`. The handlers only decide and orchestrate: releases
//! come from a [`ReleaseSource`], the executable is swapped by an [`Installer`].

use std::cmp::Ordering;

use serde_json::{Value, json};
use tracing::info;

use super::Outcome;
use crate::cli::args::{SelfCommand, SelfUpdateArgs};
use crate::error::Error;
use crate::update::{
    Installer, REPOSITORY, ReleaseSource, Target, Version, extract_binary, parse_sha256_sidecar,
    release_page_url, verify_sha256,
};

/// What `hw self` runs against.
pub struct SelfContext<'a> {
    /// Published releases.
    pub releases: &'a dyn ReleaseSource,
    /// The executable to replace.
    pub installer: &'a dyn Installer,
    /// Version of the running binary.
    pub current: Version,
    /// Release target of this platform; `None` where no prebuilt release exists.
    pub target: Option<Target>,
}

/// Runs a `hw self` subcommand.
pub fn run(cmd: &SelfCommand, ctx: &SelfContext<'_>) -> anyhow::Result<Outcome> {
    match cmd {
        SelfCommand::Check => check(ctx),
        SelfCommand::Update(args) => update(args, ctx),
    }
}

fn check(ctx: &SelfContext<'_>) -> anyhow::Result<Outcome> {
    let latest = ctx.releases.latest()?;
    let available = latest > ctx.current;
    let outcome = Outcome::json(json!({
        "current": ctx.current.to_string(),
        "latest": latest.to_string(),
        "update_available": available,
        "release_url": release_page_url(&latest),
    }));
    Ok(if available {
        outcome.with_hint(format!(
            "hw {latest} is available; install it with `hw self update`"
        ))
    } else {
        outcome
    })
}

/// Version checks first (nothing downloaded when there is nothing to do), then the platform
/// and the install location, then the checksum before the archive is unpacked.
fn update(args: &SelfUpdateArgs, ctx: &SelfContext<'_>) -> anyhow::Result<Outcome> {
    let current = ctx.current;
    let wanted = match args.to {
        Some(version) => version,
        None => ctx.releases.latest()?,
    };
    match wanted.cmp(&current) {
        Ordering::Equal => {
            return Ok(unchanged(current).with_hint(format!("hw {current} is up to date")));
        }
        Ordering::Less if args.to.is_none() => {
            return Ok(unchanged(current).with_hint(format!(
                "hw {current} is newer than the latest release {wanted}"
            )));
        }
        Ordering::Less if !args.allow_downgrade => {
            return Err(Error::InvalidInput(format!(
                "{wanted} is older than the running hw {current}; pass --allow-downgrade to install it anyway"
            ))
            .into());
        }
        Ordering::Less | Ordering::Greater => {}
    }

    let target = ctx.target.ok_or_else(unsupported_platform)?;
    let exe = ctx.installer.locate()?;
    let asset = target.asset_name(&wanted);
    info!("downloading {asset}");
    let sidecar = ctx
        .releases
        .download(&wanted, &target.checksum_name(&wanted))?;
    let expected = parse_sha256_sidecar(&String::from_utf8_lossy(&sidecar), &asset)?;
    let archive = ctx.releases.download(&wanted, &asset)?;
    verify_sha256(&archive, &expected, &asset)?;
    let binary = extract_binary(&archive, target.archive, &target.binary_path(&wanted))?;
    ctx.installer.replace(&exe, &binary)?;

    Ok(report(current, wanted, Value::from(exe.as_str()), true)
        .with_hint(format!("hw updated from {current} to {wanted} at {exe}")))
}

fn unchanged(current: Version) -> Outcome {
    report(current, current, Value::Null, false)
}

fn report(previous: Version, installed: Version, path: Value, updated: bool) -> Outcome {
    Outcome::json(json!({
        "previous": previous.to_string(),
        "installed": installed.to_string(),
        "path": path,
        "updated": updated,
    }))
}

fn unsupported_platform() -> Error {
    Error::SelfUpdate(format!(
        "no prebuilt hw release for this platform ({}-{}); build it from source: cargo install --git https://github.com/{REPOSITORY}",
        std::env::consts::ARCH,
        std::env::consts::OS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{exit_code, exit_code_for_error};
    use crate::test_util::{FakeInstaller, FakeReleases, release_archive};
    use crate::update::sha256_hex;

    const CURRENT: Version = Version::new(2026, 40, 1);
    const NEWER: Version = Version::new(2026, 41, 2);
    const OLDER: Version = Version::new(2026, 38, 1);
    const TARGET: Target = Target::MACOS_AARCH64;

    struct Setup {
        releases: FakeReleases,
        installer: FakeInstaller,
        target: Option<Target>,
    }

    impl Setup {
        fn new(releases: FakeReleases) -> Self {
            Self {
                releases,
                installer: FakeInstaller::new("/home/me/.local/bin/hw"),
                target: Some(TARGET),
            }
        }

        fn run(&self, cmd: &SelfCommand) -> anyhow::Result<Outcome> {
            run(
                cmd,
                &SelfContext {
                    releases: &self.releases,
                    installer: &self.installer,
                    current: CURRENT,
                    target: self.target,
                },
            )
        }

        fn update(&self, to: Option<Version>, allow_downgrade: bool) -> anyhow::Result<Outcome> {
            self.run(&SelfCommand::Update(SelfUpdateArgs {
                to,
                allow_downgrade,
            }))
        }
    }

    #[test]
    fn check_reports_a_newer_release_with_a_hint() {
        let setup = Setup::new(FakeReleases::new(NEWER));
        let outcome = setup.run(&SelfCommand::Check).unwrap();
        assert_eq!(
            outcome.json_value(),
            Some(&json!({
                "current": "2026.40.1",
                "latest": "2026.41.2",
                "update_available": true,
                "release_url": "https://github.com/mazuninky/hookah-work-cli/releases/tag/v2026.41.2",
            }))
        );
        assert_eq!(
            outcome.hints,
            ["hw 2026.41.2 is available; install it with `hw self update`"]
        );
        assert_eq!(setup.releases.calls(), ["latest"]);
        assert_eq!(setup.installer.located(), 0);
    }

    #[test]
    fn check_without_a_newer_release_succeeds_quietly() {
        for latest in [CURRENT, OLDER] {
            let outcome = Setup::new(FakeReleases::new(latest))
                .run(&SelfCommand::Check)
                .unwrap();
            assert_eq!(outcome.json_value().unwrap()["update_available"], false);
            assert!(outcome.hints.is_empty());
        }
    }

    #[test]
    fn check_without_any_release_is_not_found() {
        let err = Setup::new(FakeReleases::default())
            .run(&SelfCommand::Check)
            .unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::NOT_FOUND);
    }

    #[test]
    fn update_installs_the_latest_release() {
        let setup = Setup::new(FakeReleases::new(NEWER).with_release(&NEWER, TARGET, b"new-hw"));
        let outcome = setup.update(None, false).unwrap();
        assert_eq!(
            outcome.json_value(),
            Some(&json!({
                "previous": "2026.40.1",
                "installed": "2026.41.2",
                "path": "/home/me/.local/bin/hw",
                "updated": true,
            }))
        );
        assert_eq!(
            outcome.hints,
            ["hw updated from 2026.40.1 to 2026.41.2 at /home/me/.local/bin/hw"]
        );
        assert_eq!(
            setup.releases.calls(),
            [
                "latest",
                "download v2026.41.2/hw-2026.41.2-aarch64-apple-darwin.tar.gz.sha256",
                "download v2026.41.2/hw-2026.41.2-aarch64-apple-darwin.tar.gz",
            ]
        );
        assert_eq!(
            setup.installer.replaced(),
            [(
                camino::Utf8PathBuf::from("/home/me/.local/bin/hw"),
                b"new-hw".to_vec()
            )]
        );
    }

    #[test]
    fn update_unpacks_the_windows_zip() {
        let target = Target::WINDOWS_X86_64;
        let mut setup = Setup::new(FakeReleases::new(NEWER).with_release(&NEWER, target, b"MZ"));
        setup.target = Some(target);
        setup.update(None, false).unwrap();
        assert_eq!(setup.installer.replaced()[0].1, b"MZ");
    }

    #[test]
    fn update_to_the_running_version_downloads_nothing() {
        for (releases, to, hint) in [
            (
                FakeReleases::new(CURRENT),
                None,
                "hw 2026.40.1 is up to date",
            ),
            (
                FakeReleases::default(),
                Some(CURRENT),
                "hw 2026.40.1 is up to date",
            ),
            (
                FakeReleases::new(OLDER),
                None,
                "hw 2026.40.1 is newer than the latest release 2026.38.1",
            ),
        ] {
            let mut setup = Setup::new(releases);
            setup.target = None;
            let outcome = setup.update(to, false).unwrap();
            assert_eq!(
                outcome.json_value(),
                Some(&json!({
                    "previous": "2026.40.1",
                    "installed": "2026.40.1",
                    "path": null,
                    "updated": false,
                }))
            );
            assert_eq!(outcome.hints, [hint]);
            assert!(
                setup.releases.calls().iter().all(|c| c == "latest"),
                "{:?}",
                setup.releases.calls()
            );
            assert_eq!(setup.installer.located(), 0);
        }
    }

    #[test]
    fn downgrade_needs_allow_downgrade() {
        let setup = Setup::new(FakeReleases::default().with_release(&OLDER, TARGET, b"old-hw"));
        let err = setup.update(Some(OLDER), false).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::INPUT_ERROR);
        assert!(err.to_string().contains("--allow-downgrade"), "{err}");
        assert!(setup.releases.calls().is_empty());

        let outcome = setup.update(Some(OLDER), true).unwrap();
        assert_eq!(outcome.json_value().unwrap()["installed"], "2026.38.1");
        assert_eq!(setup.installer.replaced()[0].1, b"old-hw");
    }

    #[test]
    fn unsupported_platform_fails_before_locating_or_downloading() {
        let mut setup = Setup::new(FakeReleases::new(NEWER).with_release(&NEWER, TARGET, b"x"));
        setup.target = None;
        let err = setup.update(None, false).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
        assert!(
            err.to_string()
                .contains("cargo install --git https://github.com/mazuninky/hookah-work-cli"),
            "{err}"
        );
        assert_eq!(setup.releases.calls(), ["latest"]);
        assert_eq!(setup.installer.located(), 0);
    }

    #[test]
    fn refused_location_fails_before_any_download() {
        let mut setup = Setup::new(FakeReleases::new(NEWER).with_release(&NEWER, TARGET, b"x"));
        setup.installer = FakeInstaller::refusing("hw at /usr/bin/hw is managed by apt");
        let err = setup.update(None, false).unwrap_err();
        assert_eq!(
            err.to_string(),
            "self-update error: hw at /usr/bin/hw is managed by apt"
        );
        assert_eq!(setup.releases.calls(), ["latest"]);
    }

    #[test]
    fn checksum_mismatch_replaces_nothing() {
        let archive = release_archive(&NEWER, TARGET, b"tampered");
        let forged = format!(
            "{}  {}\n",
            sha256_hex(b"something else"),
            TARGET.asset_name(&NEWER)
        );
        let releases = FakeReleases::new(NEWER)
            .with_asset(&NEWER, &TARGET.asset_name(&NEWER), archive)
            .with_asset(&NEWER, &TARGET.checksum_name(&NEWER), forged);
        let setup = Setup::new(releases);
        let err = setup.update(None, false).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::RUNTIME_ERROR);
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert!(setup.installer.replaced().is_empty());
    }

    #[test]
    fn missing_release_is_not_found() {
        let setup = Setup::new(FakeReleases::new(CURRENT));
        let err = setup.update(Some(NEWER), false).unwrap_err();
        assert_eq!(exit_code_for_error(&err), exit_code::NOT_FOUND);
        assert!(setup.installer.replaced().is_empty());
    }
}
