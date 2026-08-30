use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, anyhow, bail};

pub(super) fn running_bundle() -> Option<PathBuf> {
    bundle_for_executable(&std::env::current_exe().ok()?)
}

fn bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    let bundle = executable.parent()?.parent()?.parent()?;
    (bundle
        .extension()
        .is_some_and(|extension| extension == "app")
        && executable
            .parent()
            .is_some_and(|directory| directory.ends_with("MacOS")))
    .then(|| bundle.to_owned())
}

pub(super) fn apply(payload: &[u8], expected_version: &str) -> Result<PathBuf> {
    let bundle = running_bundle().ok_or_else(|| anyhow!("当前不是从 OcHerdr.app 运行"))?;
    let parent = bundle
        .parent()
        .ok_or_else(|| anyhow!("无法定位 OcHerdr.app 所在目录"))?;
    ensure_writable(parent)?;

    let staging = tempfile::Builder::new()
        .prefix(".ocherdr-update-")
        .tempdir_in(parent)
        .with_context(|| format!("无法在 {} 创建更新暂存目录", parent.display()))?;
    let archive = staging.path().join("update.tar.gz");
    fs::write(&archive, payload).context("写入已验证的更新包失败")?;
    extract(&archive, staging.path())?;
    let candidate = staging.path().join("OcHerdr.app");
    if !candidate.is_dir() {
        bail!("更新包中没有 OcHerdr.app");
    }
    verify_bundle_version(&candidate, expected_version)?;
    verify_code_signature(&candidate)?;
    verify_signature_lineage(&bundle, &candidate)?;
    swap(&bundle, &candidate)?;
    Ok(bundle)
}

fn ensure_writable(directory: &Path) -> Result<()> {
    let probe = directory.join(".ocherdr-update-write-test");
    fs::write(&probe, b"")
        .with_context(|| format!("没有写入 {} 的权限，无法应用内更新", directory.display()))?;
    let _ = fs::remove_file(probe);
    Ok(())
}

fn extract(archive: &Path, into: &Path) -> Result<()> {
    let output = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .output()
        .context("启动系统 tar 失败")?;
    if !output.status.success() {
        bail!(
            "解压更新包失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn verify_bundle_version(bundle: &Path, expected: &str) -> Result<()> {
    let plist = bundle.join("Contents/Info.plist");
    let actual =
        plist_value(&plist, "CFBundleShortVersionString").context("更新包缺少可读取的版本信息")?;
    if actual != expected {
        bail!("更新包版本 {actual} 与清单版本 {expected} 不一致");
    }
    let identifier = plist_value(&plist, "CFBundleIdentifier").context("更新包缺少应用标识")?;
    if identifier != "io.github.ochub-team.ocherdr" {
        bail!("更新包应用标识不正确: {identifier}");
    }
    Ok(())
}

fn plist_value(plist: &Path, key: &str) -> Result<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .with_context(|| format!("读取 {key} 失败"))?;
    if !output.status.success() {
        bail!("无法读取 {key}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn verify_code_signature(bundle: &Path) -> Result<()> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "--verbose=2"])
        .arg(bundle)
        .output()
        .context("校验更新应用代码签名失败")?;
    if !output.status.success() {
        bail!(
            "更新应用代码签名无效: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn verify_signature_lineage(current: &Path, candidate: &Path) -> Result<()> {
    let Some(current_team) = team_identifier(current) else {
        return Ok(());
    };
    match team_identifier(candidate) {
        Some(candidate_team) if candidate_team == current_team => {
            verify_apple_team_anchor(candidate, &current_team)
        }
        Some(candidate_team) => {
            bail!("更新包签名团队 {candidate_team} 与当前应用 {current_team} 不一致")
        }
        None => bail!("当前应用有 Developer ID 签名，但更新包没有 Team ID"),
    }
}

fn verify_apple_team_anchor(bundle: &Path, team: &str) -> Result<()> {
    if team.len() != 10
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        bail!("当前应用的 Team ID 格式无效");
    }
    let requirement =
        format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "-R"])
        .arg(requirement)
        .arg(bundle)
        .output()
        .context("校验更新应用的 Apple 签名链失败")?;
    if !output.status.success() {
        bail!(
            "更新应用不是同一 Apple Developer 团队签发: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn team_identifier(bundle: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(bundle)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_team_identifier(&String::from_utf8_lossy(&output.stderr))
}

fn parse_team_identifier(report: &str) -> Option<String> {
    report.lines().find_map(|line| {
        line.strip_prefix("TeamIdentifier=")
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "not set")
            .map(ToOwned::to_owned)
    })
}

fn swap(target: &Path, candidate: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("OcHerdr.app 没有父目录"))?;
    let reservation = tempfile::Builder::new()
        .prefix(".ocherdr-backup-")
        .tempdir_in(parent)
        .context("无法预留旧版本备份路径")?;
    let backup = reservation.path().to_owned();
    reservation.close().context("无法准备旧版本备份路径")?;
    fs::rename(target, &backup).context("移开旧版本失败，应用未被修改")?;
    if let Err(error) = fs::rename(candidate, target) {
        if let Err(restore) = fs::rename(&backup, target) {
            bail!(
                "安装新版本失败（{error}），恢复旧版本也失败（{restore}）；旧版本位于 {}",
                backup.display()
            );
        }
        bail!("安装新版本失败（{error}），已恢复旧版本");
    }
    let _ = fs::remove_dir_all(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_a_real_macos_bundle_layout() {
        assert_eq!(
            bundle_for_executable(Path::new(
                "/Applications/OcHerdr.app/Contents/MacOS/ocherdr"
            )),
            Some(PathBuf::from("/Applications/OcHerdr.app"))
        );
        assert_eq!(
            bundle_for_executable(Path::new("/tmp/target/debug/ocherdr")),
            None
        );
    }

    #[test]
    fn team_identifier_ignores_adhoc_signatures() {
        assert_eq!(
            parse_team_identifier("Identifier=x\nTeamIdentifier=X5A2GR87V7\n"),
            Some("X5A2GR87V7".into())
        );
        assert_eq!(
            parse_team_identifier("Identifier=x\nTeamIdentifier=not set\n"),
            None
        );
    }

    #[test]
    fn swap_is_atomic_and_restores_after_a_failed_second_rename() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("OcHerdr.app");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("version"), "old").unwrap();
        let missing = directory.path().join("missing.app");
        assert!(swap(&target, &missing).is_err());
        assert_eq!(fs::read_to_string(target.join("version")).unwrap(), "old");

        let candidate = directory.path().join("candidate.app");
        fs::create_dir(&candidate).unwrap();
        fs::write(candidate.join("version"), "new").unwrap();
        swap(&target, &candidate).unwrap();
        assert_eq!(fs::read_to_string(target.join("version")).unwrap(), "new");
    }
}
