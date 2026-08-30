//! Signed GitHub release discovery and macOS self-update support.
//!
//! Update payloads are verified against a public key compiled into the app.
//! A checksum published beside a release is useful for humans, but cannot
//! protect an automatic installer from a compromised release account because
//! the attacker could replace both files. The independent minisign key does.

mod macos;

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};

const REPOSITORY: &str = "OcHub-team/OcHerdr";
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_UPDATE_BYTES: u64 = 1024 * 1024 * 1024;
const SUPPORTED_MANIFEST_SCHEMA: u32 = 1;
pub(crate) const AUTO_CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// Public by construction. Release CI injects it into the signed binary.
const PUBLIC_KEY: Option<&str> = option_env!("OCHERDR_UPDATER_PUBKEY");

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdateInfo {
    pub(crate) current_version: String,
    pub(crate) latest_version: Option<String>,
    pub(crate) has_update: bool,
    pub(crate) release_url: String,
    pub(crate) release_notes: Option<String>,
    pub(crate) can_self_install: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UpdateState {
    #[serde(default)]
    pub(crate) last_check_at: Option<i64>,
    #[serde(default)]
    pub(crate) notified_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct PlatformEntry {
    signature: String,
    url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct UpdateManifest {
    #[serde(default = "default_manifest_schema")]
    schema_version: u32,
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
    #[serde(default)]
    platforms: BTreeMap<String, PlatformEntry>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

pub(crate) struct PreparedUpdate {
    pub(crate) version: String,
    payload: Vec<u8>,
}

impl PreparedUpdate {
    pub(crate) fn install_and_arm_restart(self) -> Result<()> {
        let bundle = macos::apply(&self.payload, &self.version)?;
        arm_relaunch(&bundle)
    }
}

pub(crate) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub(crate) fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}

pub(crate) fn auto_check_due(state: &UpdateState, now: i64) -> bool {
    match state.last_check_at {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now - last >= AUTO_CHECK_INTERVAL_SECONDS,
    }
}

pub(crate) fn should_notify(info: &UpdateInfo, state: &UpdateState) -> bool {
    info.has_update
        && match (
            info.latest_version.as_deref(),
            state.notified_version.as_deref(),
        ) {
            (Some(latest), Some(seen)) => normalize_version(latest) != normalize_version(seen),
            (Some(_), None) => true,
            _ => false,
        }
}

pub(crate) fn load_state() -> UpdateState {
    update_state_path()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_state(state: &UpdateState) -> Result<()> {
    let path = update_state_path().ok_or_else(|| anyhow!("应用支持目录不可用"))?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("更新状态路径没有父目录"))?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建 {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 写入更新状态", parent.display()))?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), state)?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("无法保存 {}", path.display()))?;
    Ok(())
}

fn update_state_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|directory| directory.join("OcHerdr/update-state.json"))
}

pub(crate) fn check_for_updates() -> Result<UpdateInfo> {
    match fetch_manifest() {
        Ok(Some(manifest)) => Ok(info_from_manifest(&manifest)),
        Ok(None) => check_via_github_api(),
        Err(manifest_error) => check_via_github_api().map_err(|api_error| {
            anyhow!("更新清单请求失败（{manifest_error}）；GitHub API 回退也失败（{api_error}）")
        }),
    }
}

fn info_from_manifest(manifest: &UpdateManifest) -> UpdateInfo {
    let latest_version = normalize_version(&manifest.version);
    let has_update = latest_version.as_deref().is_some_and(is_newer_than_current);
    let can_self_install = has_update
        && macos::running_bundle().is_some()
        && signing_configured()
        && manifest.platforms.contains_key(current_target_key());
    UpdateInfo {
        current_version: current_version().to_owned(),
        latest_version,
        has_update,
        release_url: release_tag_url(&manifest.version),
        release_notes: manifest.notes.clone(),
        can_self_install,
    }
}

fn check_via_github_api() -> Result<UpdateInfo> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let response = http_client(CHECK_TIMEOUT)?
        .get(&url)
        .send()
        .context("检查 GitHub Release 失败")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateInfo {
            current_version: current_version().to_owned(),
            latest_version: None,
            has_update: false,
            release_url: latest_release_url(),
            release_notes: None,
            can_self_install: false,
        });
    }
    let status = response.status();
    let body = response.text().context("读取 GitHub Release 响应失败")?;
    if !status.is_success() {
        bail!("GitHub Release 请求返回 {status}: {body}");
    }
    let release: GitHubRelease =
        serde_json::from_str(&body).context("解析 GitHub Release 响应失败")?;
    if release.draft {
        bail!("最新 GitHub Release 仍是草稿");
    }
    if release.prerelease {
        bail!("最新 GitHub Release 是预发布版本");
    }
    let latest_version = normalize_version(&release.tag_name);
    let has_update = latest_version.as_deref().is_some_and(is_newer_than_current);
    Ok(UpdateInfo {
        current_version: current_version().to_owned(),
        latest_version,
        has_update,
        release_url: release.html_url,
        release_notes: release.body,
        can_self_install: false,
    })
}

pub(crate) fn prepare(
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Option<PreparedUpdate>> {
    if macos::running_bundle().is_none() {
        bail!("当前不是从 OcHerdr.app 运行，不能在应用内替换安装");
    }
    if !signing_configured() {
        bail!("此版本没有内置 OcHerdr 更新签名公钥，请从发布页手动更新");
    }
    let manifest = fetch_manifest()?.ok_or_else(|| anyhow!("最新发布没有更新清单"))?;
    if !is_newer_than_current(&manifest.version) {
        return Ok(None);
    }
    let entry = manifest
        .platforms
        .get(current_target_key())
        .ok_or_else(|| anyhow!("最新发布没有当前架构的更新包"))?;
    validate_download_url(&entry.url)?;
    let payload = download(&entry.url, &mut progress)?;
    verify_payload(&payload, &entry.signature)?;
    Ok(Some(PreparedUpdate {
        version: normalize_version(&manifest.version).ok_or_else(|| anyhow!("更新清单版本无效"))?,
        payload,
    }))
}

fn fetch_manifest() -> Result<Option<UpdateManifest>> {
    let url = format!("https://github.com/{REPOSITORY}/releases/latest/download/latest.json");
    let response = http_client(CHECK_TIMEOUT)?
        .get(url)
        .send()
        .context("获取更新清单失败")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body = response.text().context("读取更新清单失败")?;
    if !status.is_success() {
        bail!("更新清单请求返回 {status}: {body}");
    }
    let manifest: UpdateManifest = serde_json::from_str(&body).context("解析更新清单失败")?;
    if manifest.schema_version != SUPPORTED_MANIFEST_SCHEMA {
        bail!(
            "更新清单协议版本 {} 不受支持（当前支持 {}）",
            manifest.schema_version,
            SUPPORTED_MANIFEST_SCHEMA
        );
    }
    Ok(Some(manifest))
}

const fn default_manifest_schema() -> u32 {
    1
}

fn http_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .user_agent(format!("OcHerdr/{}", current_version()))
        .build()
        .context("创建更新网络客户端失败")
}

fn download(url: &str, progress: &mut impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>> {
    let mut response = http_client(DOWNLOAD_TIMEOUT)?
        .get(url)
        .send()
        .context("下载更新包失败")?;
    if !response.status().is_success() {
        bail!("更新包下载返回 {}", response.status());
    }
    let total = response.content_length();
    if total.is_some_and(|size| size > MAX_UPDATE_BYTES) {
        bail!("更新包超过 1 GiB 安全上限");
    }
    let mut payload = Vec::with_capacity(total.unwrap_or(0).min(usize::MAX as u64) as usize);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response.read(&mut buffer).context("读取更新包失败")?;
        if read == 0 {
            break;
        }
        payload.extend_from_slice(&buffer[..read]);
        if payload.len() as u64 > MAX_UPDATE_BYTES {
            bail!("更新包超过 1 GiB 安全上限");
        }
        progress(payload.len() as u64, total);
    }
    Ok(payload)
}

fn signing_configured() -> bool {
    PUBLIC_KEY.is_some_and(|key| !key.trim().is_empty())
}

fn verify_payload(payload: &[u8], signature: &str) -> Result<()> {
    let public_key = PUBLIC_KEY
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| anyhow!("此版本没有内置更新签名公钥"))?;
    verify_payload_with_key(payload, signature, public_key)
}

fn verify_payload_with_key(payload: &[u8], signature: &str, public_key: &str) -> Result<()> {
    let public_key = decode_minisign_block(public_key).context("更新签名公钥无法解码")?;
    let public_key = PublicKey::decode(public_key.trim()).context("更新签名公钥无效")?;
    let signature = decode_minisign_block(signature).context("更新包签名无法解码")?;
    let signature = Signature::decode(signature.trim()).context("更新包签名无效")?;
    public_key
        .verify(payload, &signature, true)
        .context("更新包签名校验失败，已丢弃下载内容")
}

fn decode_minisign_block(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.starts_with("untrusted comment:") {
        return Ok(trimmed.to_owned());
    }
    let compact = trimmed
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .context("base64 内容无效")?;
    String::from_utf8(bytes).context("minisign 内容不是 UTF-8")
}

fn validate_download_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("更新包 URL 无效")?;
    if url.scheme() != "https" {
        bail!("更新包 URL 必须使用 HTTPS");
    }
    let host = url.host_str().unwrap_or_default();
    if host != "github.com"
        && host != "objects.githubusercontent.com"
        && !host.ends_with(".githubusercontent.com")
    {
        bail!("更新包 URL 主机不受信任: {host}");
    }
    Ok(())
}

fn current_target_key() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    return "darwin-aarch64";
    #[cfg(target_arch = "x86_64")]
    return "darwin-x86_64";
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    return "unsupported";
}

fn normalize_version(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
    Version::parse(value)
        .ok()
        .map(|version| version.to_string())
}

fn is_newer_than_current(latest: &str) -> bool {
    let Some(latest) = normalize_version(latest).and_then(|value| Version::parse(&value).ok())
    else {
        return false;
    };
    Version::parse(current_version()).is_ok_and(|current| latest > current)
}

pub(crate) fn latest_release_url() -> String {
    format!("https://github.com/{REPOSITORY}/releases/latest")
}

fn release_tag_url(version: &str) -> String {
    normalize_version(version).map_or_else(latest_release_url, |version| {
        format!("https://github.com/{REPOSITORY}/releases/tag/v{version}")
    })
}

pub(crate) fn open_release_page(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).context("发布页 URL 无效")?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
        bail!("发布页 URL 不受信任");
    }
    let status = Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .context("无法启动浏览器")?;
    if !status.success() {
        bail!("浏览器启动命令返回 {status}");
    }
    Ok(())
}

fn arm_relaunch(bundle: &std::path::Path) -> Result<()> {
    use std::process::Stdio;

    let script = r#"pid="$1"; target="$2"; waited=0
while kill -0 "$pid" 2>/dev/null; do
  sleep 0.2
  waited=$((waited + 1))
  if [ "$waited" -gt 150 ]; then break; fi
done
exec /usr/bin/open "$target""#;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("ocherdr-relaunch")
        .arg(std::process::id().to_string())
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("更新已安装，但无法安排自动重启")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDhFOEYyNjNBQjI4MkY2Q0IKUldUTDlvS3lPaWFQanVITThVZ3NNNVphWlRDQTVYdmlzMWdYc2dGQW1HK2pHcHJJZDZBdWNtQVEK";
    const TEST_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIGNhcmdvLXBhY2thZ2VyIHNlY3JldCBrZXkKUlVUTDlvS3lPaWFQanF3RElqclVPY0UrVytsK2ovdGZhUHdJRjNzQXJvV3NYZk9SUmk4UjBZUS9hTEh5TnR5U253ZlRJOGg0ZDZEUWFjdDg3Z3JNOUd2ZlRiYUlXZVBpemdFPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzg0OTc0MDI0CWZpbGU6cGF5bG9hZC5hcHAudGFyLmd6ClBkK3hha0dMWXFQZC81UU9LMlEvQkYxcTVMYzRjak9BaVRaa3pBVFZZSHBTUWs4cmFhZ3pCMERwSTUzTlFmNGlIdm8rbGliZytsSExWUDdwdGNkTkRnPT0K";
    const TEST_PAYLOAD: &[u8] = b"pretend this is an OcHub.app.tar.gz payload";

    #[test]
    fn semver_comparison_handles_prefixes_and_prereleases() {
        assert_eq!(normalize_version("v1.2.3"), Some("1.2.3".into()));
        assert!(Version::parse("1.0.0").unwrap() > Version::parse("1.0.0-beta.2").unwrap());
        assert_eq!(normalize_version("not-a-version"), None);
    }

    #[test]
    fn automatic_checks_are_daily_and_notifications_are_per_version() {
        let mut state = UpdateState::default();
        assert!(auto_check_due(&state, 1_000_000));
        state.last_check_at = Some(1_000_000);
        assert!(!auto_check_due(
            &state,
            1_000_000 + AUTO_CHECK_INTERVAL_SECONDS - 1
        ));
        assert!(auto_check_due(
            &state,
            1_000_000 + AUTO_CHECK_INTERVAL_SECONDS
        ));

        let info = UpdateInfo {
            current_version: "0.1.0".into(),
            latest_version: Some("0.2.0".into()),
            has_update: true,
            release_url: latest_release_url(),
            release_notes: None,
            can_self_install: true,
        };
        assert!(should_notify(&info, &state));
        state.notified_version = Some("v0.2.0".into());
        assert!(!should_notify(&info, &state));
    }

    #[test]
    fn generated_manifest_selects_the_current_architecture() {
        let manifest: UpdateManifest = serde_json::from_str(
            r#"{
                "schema_version": 1,
                "version": "0.2.0",
                "notes": "Release v0.2.0",
                "platforms": {
                    "darwin-aarch64": {"signature":"a", "url":"https://github.com/a"},
                    "darwin-x86_64": {"signature":"b", "url":"https://github.com/b"}
                }
            }"#,
        )
        .unwrap();
        assert!(manifest.platforms.contains_key(current_target_key()));
    }

    #[test]
    fn release_payload_signature_is_verified_and_tampering_is_rejected() {
        verify_payload_with_key(TEST_PAYLOAD, TEST_SIGNATURE, TEST_PUBLIC_KEY).unwrap();
        assert!(verify_payload_with_key(b"tampered", TEST_SIGNATURE, TEST_PUBLIC_KEY).is_err());
    }

    #[test]
    fn download_urls_are_restricted_to_github_release_hosts() {
        validate_download_url(
            "https://github.com/OcHub-team/OcHerdr/releases/download/v0.2.0/a.tar.gz",
        )
        .unwrap();
        assert!(validate_download_url("http://github.com/a").is_err());
        assert!(validate_download_url("https://github.com.evil.example/a").is_err());
    }
}
