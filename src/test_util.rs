//! Hand-written test doubles for unit tests and, through the `test-util` feature,
//! integration tests: a scripted [`FakeApi`], [`FakeConnector`], [`FakeIssuer`],
//! [`RecordingSleeper`], [`FakeReleases`] and [`FakeInstaller`], plus in-memory release
//! archives ([`release_archive`]).

use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Write as _};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;

use crate::auth::Secret;
use crate::client::{
    Api, ApiResponse, Connector, Login, Pagination, ResponseBody, Sleeper, TokenIssuer,
    error_for_status,
};
use crate::error::{Error, Result};
use crate::update::{ArchiveKind, Installer, ReleaseSource, Target, Version, sha256_hex};

/// Which [`Api`] method a call went through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// [`Api::get`].
    Get,
    /// [`Api::find_client`].
    FindClient,
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The method used.
    pub kind: CallKind,
    /// Request path (`/api/client/find` for lookups).
    pub path: String,
    /// Query pairs in order.
    pub query: Vec<(String, String)>,
    /// Form pairs (lookups only).
    pub form: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
enum Reply {
    Ok(ApiResponse),
    Status(u16, String),
}

/// Scripted [`Api`]: replies are queued per path; the last reply of a path repeats.
/// A path without replies answers 404.
#[derive(Debug, Default)]
pub struct FakeApi {
    replies: Mutex<HashMap<String, VecDeque<Reply>>>,
    calls: Mutex<Vec<Call>>,
}

impl FakeApi {
    /// An API with no scripted replies.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn with_reply(self, path: &str, reply: Reply) -> Self {
        lock(&self.replies)
            .entry(path.to_owned())
            .or_default()
            .push_back(reply);
        self
    }

    /// Queues a JSON reply for `path`.
    #[must_use]
    pub fn with_json(self, path: &str, value: Value) -> Self {
        self.with_reply(path, Reply::Ok(ApiResponse::json(value)))
    }

    /// Queues a JSON reply with pagination headers for `path`.
    #[must_use]
    pub fn with_page(self, path: &str, value: Value, pagination: Pagination) -> Self {
        self.with_reply(
            path,
            Reply::Ok(ApiResponse {
                body: ResponseBody::Json(value),
                pagination: Some(pagination),
            }),
        )
    }

    /// Queues a non-JSON reply for `path`.
    #[must_use]
    pub fn with_text(self, path: &str, text: &str) -> Self {
        self.with_reply(
            path,
            Reply::Ok(ApiResponse {
                body: ResponseBody::Text(text.to_owned()),
                pagination: None,
            }),
        )
    }

    /// Queues an error status for `path`, mapped exactly like a real response with this body.
    #[must_use]
    pub fn with_status(self, path: &str, status: u16, body: &str) -> Self {
        self.with_reply(path, Reply::Status(status, body.to_owned()))
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.calls).clone()
    }

    /// `(path, query)` of every call so far — the usual assertion target.
    #[must_use]
    pub fn requests(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.calls()
            .into_iter()
            .map(|call| (call.path, call.query))
            .collect()
    }

    fn answer(&self, call: Call) -> Result<ApiResponse> {
        let path = call.path.clone();
        lock(&self.calls).push(call);
        let reply = {
            let mut replies = lock(&self.replies);
            match replies.get_mut(&path) {
                Some(queue) if queue.len() > 1 => queue.pop_front(),
                Some(queue) => queue.front().cloned(),
                None => None,
            }
        };
        match reply {
            Some(Reply::Ok(response)) => Ok(response),
            Some(Reply::Status(status, body)) => Err(error_for_status(status, &body, &[])),
            None => Err(Error::NotFound(format!("FakeApi has no reply for {path}"))),
        }
    }
}

impl Api for FakeApi {
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse> {
        self.answer(Call {
            kind: CallKind::Get,
            path: path.to_owned(),
            query: query.to_vec(),
            form: Vec::new(),
        })
    }

    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse> {
        self.answer(Call {
            kind: CallKind::FindClient,
            path: "/api/client/find".to_owned(),
            query: query.to_vec(),
            form: form.to_vec(),
        })
    }
}

/// Lets several boxed clients share one [`FakeApi`].
struct SharedApi(Arc<FakeApi>);

impl Api for SharedApi {
    fn get(&self, path: &str, query: &[(String, String)]) -> Result<ApiResponse> {
        self.0.get(path, query)
    }

    fn find_client(
        &self,
        form: &[(String, String)],
        query: &[(String, String)],
    ) -> Result<ApiResponse> {
        self.0.find_client(form, query)
    }
}

/// [`Connector`] handing out one shared [`FakeApi`] and recording `(base_url, token)`.
#[derive(Debug, Default)]
pub struct FakeConnector {
    api: Arc<FakeApi>,
    connections: Mutex<Vec<(String, Option<Secret>)>>,
}

impl FakeConnector {
    /// A connector serving `api`.
    #[must_use]
    pub fn new(api: FakeApi) -> Self {
        Self {
            api: Arc::new(api),
            connections: Mutex::default(),
        }
    }

    /// The shared fake, for call assertions.
    #[must_use]
    pub fn api(&self) -> &FakeApi {
        &self.api
    }

    /// Every `(base_url, token)` a client was built for.
    #[must_use]
    pub fn connections(&self) -> Vec<(String, Option<Secret>)> {
        lock(&self.connections).clone()
    }
}

impl Connector for FakeConnector {
    fn connect(&self, base_url: &str, token: Option<&Secret>) -> Box<dyn Api> {
        lock(&self.connections).push((base_url.to_owned(), token.cloned()));
        Box::new(SharedApi(Arc::clone(&self.api)))
    }
}

/// [`TokenIssuer`] returning a fixed login or status error and recording
/// `(base_url, email, password)`.
#[derive(Debug)]
pub struct FakeIssuer {
    reply: std::result::Result<Login, (u16, String)>,
    calls: Mutex<Vec<(String, String, Secret)>>,
}

impl FakeIssuer {
    /// Succeeds with `login`.
    #[must_use]
    pub fn ok(login: Login) -> Self {
        Self {
            reply: Ok(login),
            calls: Mutex::default(),
        }
    }

    /// Fails like a response with `status` and `body`.
    #[must_use]
    pub fn status(status: u16, body: &str) -> Self {
        Self {
            reply: Err((status, body.to_owned())),
            calls: Mutex::default(),
        }
    }

    /// Every login attempt so far.
    #[must_use]
    pub fn calls(&self) -> Vec<(String, String, Secret)> {
        lock(&self.calls).clone()
    }
}

impl TokenIssuer for FakeIssuer {
    fn login(&self, base_url: &str, email: &str, password: &Secret) -> Result<Login> {
        lock(&self.calls).push((base_url.to_owned(), email.to_owned(), password.clone()));
        match &self.reply {
            Ok(login) => Ok(login.clone()),
            Err((status, body)) => Err(error_for_status(*status, body, &[])),
        }
    }
}

/// [`Sleeper`] that records requested delays instead of sleeping.
#[derive(Debug, Default)]
pub struct RecordingSleeper {
    slept: Mutex<Vec<Duration>>,
}

impl RecordingSleeper {
    /// A sleeper with no recorded delays.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every delay requested so far.
    #[must_use]
    pub fn slept(&self) -> Vec<Duration> {
        lock(&self.slept).clone()
    }
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, duration: Duration) {
        lock(&self.slept).push(duration);
    }
}

/// Scripted [`ReleaseSource`]: an optional latest version and assets per `(version, name)`.
/// Records every call as `latest` or `download v<version>/<name>`; anything unscripted is
/// [`Error::NotFound`].
#[derive(Debug, Default)]
pub struct FakeReleases {
    latest: Option<Version>,
    assets: HashMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
}

impl FakeReleases {
    /// Releases whose latest is `latest` (and no assets yet); `default()` has no release at all.
    #[must_use]
    pub fn new(latest: Version) -> Self {
        Self {
            latest: Some(latest),
            ..Self::default()
        }
    }

    /// Publishes asset `name` of release `version`.
    #[must_use]
    pub fn with_asset(mut self, version: &Version, name: &str, bytes: impl Into<Vec<u8>>) -> Self {
        self.assets.insert(asset_key(version, name), bytes.into());
        self
    }

    /// Publishes `binary` as release `version` for `target`: the archive and a matching
    /// `.sha256` sidecar, as the release workflow does.
    #[must_use]
    pub fn with_release(self, version: &Version, target: Target, binary: &[u8]) -> Self {
        let archive = release_archive(version, target, binary);
        let asset = target.asset_name(version);
        let sidecar = format!("{}  {asset}\n", sha256_hex(&archive));
        self.with_asset(version, &asset, archive).with_asset(
            version,
            &target.checksum_name(version),
            sidecar,
        )
    }

    /// Every call so far.
    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        lock(&self.calls).clone()
    }
}

fn asset_key(version: &Version, name: &str) -> String {
    format!("v{version}/{name}")
}

impl ReleaseSource for FakeReleases {
    fn latest(&self) -> Result<Version> {
        lock(&self.calls).push("latest".to_owned());
        self.latest
            .ok_or_else(|| Error::NotFound("FakeReleases has no release".into()))
    }

    fn download(&self, version: &Version, name: &str) -> Result<Vec<u8>> {
        let key = asset_key(version, name);
        lock(&self.calls).push(format!("download {key}"));
        self.assets
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("FakeReleases has no {key}")))
    }
}

/// Scripted [`Installer`]: `locate` answers a fixed path (or refuses with
/// [`Error::SelfUpdate`]), `replace` records what it was given and touches nothing.
#[derive(Debug)]
pub struct FakeInstaller {
    path: Utf8PathBuf,
    refusal: Option<String>,
    located: Mutex<usize>,
    replaced: Mutex<Vec<(Utf8PathBuf, Vec<u8>)>>,
}

impl FakeInstaller {
    /// An installer whose executable is `path`.
    #[must_use]
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            path: path.into(),
            refusal: None,
            located: Mutex::default(),
            replaced: Mutex::default(),
        }
    }

    /// An installer whose `locate` fails with `message`.
    #[must_use]
    pub fn refusing(message: &str) -> Self {
        Self {
            refusal: Some(message.to_owned()),
            ..Self::default()
        }
    }

    /// How many times `locate` ran.
    #[must_use]
    pub fn located(&self) -> usize {
        *lock(&self.located)
    }

    /// Every `(exe, binary)` passed to `replace`.
    #[must_use]
    pub fn replaced(&self) -> Vec<(Utf8PathBuf, Vec<u8>)> {
        lock(&self.replaced).clone()
    }
}

impl Default for FakeInstaller {
    fn default() -> Self {
        Self::new("/home/user/.local/bin/hw")
    }
}

impl Installer for FakeInstaller {
    fn locate(&self) -> Result<Utf8PathBuf> {
        *lock(&self.located) += 1;
        match &self.refusal {
            Some(message) => Err(Error::SelfUpdate(message.clone())),
            None => Ok(self.path.clone()),
        }
    }

    fn replace(&self, exe: &Utf8Path, binary: &[u8]) -> Result<()> {
        lock(&self.replaced).push((exe.to_owned(), binary.to_vec()));
        Ok(())
    }
}

/// A release archive packed like the release workflow does: `hw-<version>-<triple>/` holding
/// the executable and a `LICENSE`.
#[must_use]
pub fn release_archive(version: &Version, target: Target, binary: &[u8]) -> Vec<u8> {
    let binary_path = target.binary_path(version);
    let license = binary_path.replace(target.binary, "LICENSE");
    let entries = [
        (license.as_str(), b"MIT".as_slice()),
        (binary_path.as_str(), binary),
    ];
    match target.archive {
        ArchiveKind::TarGz => tar_gz_archive(&entries),
        ArchiveKind::Zip => zip_archive(&entries),
    }
}

/// A gzipped tarball of regular files (mode 0755). Names are stored verbatim (`./` prefixes
/// included), so they must fit the 100-byte tar name field.
#[must_use]
pub fn tar_gz_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, data) in entries {
        let mut header = tar::Header::new_ustar();
        let field = &mut header.as_old_mut().name;
        assert!(name.len() <= field.len(), "tar name too long: {name}");
        field[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append(&header, *data).expect("append to tar");
    }
    builder
        .into_inner()
        .and_then(flate2::write::GzEncoder::finish)
        .expect("finish tar.gz")
}

/// A deflate-compressed zip of files; names are stored verbatim, backslash separators included.
#[must_use]
pub fn zip_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zip.start_file(*name, options).expect("start zip entry");
        zip.write_all(data).expect("write zip entry");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fake_api_replays_queue_and_repeats_last() {
        let api = FakeApi::new()
            .with_json("/api/a", json!(1))
            .with_json("/api/a", json!(2));
        let body = |api: &FakeApi| api.get("/api/a", &[]).unwrap().into_json().unwrap();
        assert_eq!(body(&api), json!(1));
        assert_eq!(body(&api), json!(2));
        assert_eq!(body(&api), json!(2));
        assert!(matches!(api.get("/api/b", &[]), Err(Error::NotFound(_))));
        assert_eq!(api.requests().len(), 4);
    }

    #[test]
    fn fake_api_maps_status_like_the_client() {
        let api = FakeApi::new().with_status("/api/x", 403, r#"{"message":"Нет доступа"}"#);
        assert!(matches!(api.get("/api/x", &[]), Err(Error::Auth(m)) if m == "Нет доступа"));
    }

    #[test]
    fn fake_releases_serve_scripted_assets_and_record_calls() {
        let version = Version::new(2026, 41, 1);
        let releases = FakeReleases::new(version).with_asset(&version, "a.zip", b"zip".as_slice());
        assert_eq!(releases.latest().unwrap(), version);
        assert_eq!(releases.download(&version, "a.zip").unwrap(), b"zip");
        assert!(matches!(
            releases.download(&version, "b.zip"),
            Err(Error::NotFound(_))
        ));
        assert_eq!(
            releases.calls(),
            [
                "latest",
                "download v2026.41.1/a.zip",
                "download v2026.41.1/b.zip"
            ]
        );
        assert!(matches!(
            FakeReleases::default().latest(),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn fake_installer_records_and_refuses() {
        let installer = FakeInstaller::new("/x/hw");
        let exe = installer.locate().unwrap();
        installer.replace(&exe, b"bin").unwrap();
        assert_eq!(installer.located(), 1);
        assert_eq!(
            installer.replaced(),
            [(Utf8PathBuf::from("/x/hw"), b"bin".to_vec())]
        );
        assert!(matches!(
            FakeInstaller::refusing("no").locate(),
            Err(Error::SelfUpdate(m)) if m == "no"
        ));
    }

    #[test]
    fn recording_sleeper_records() {
        let sleeper = RecordingSleeper::new();
        sleeper.sleep(Duration::from_millis(5));
        assert_eq!(sleeper.slept(), vec![Duration::from_millis(5)]);
    }
}
