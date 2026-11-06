//! Git 调用与状态采集。
//!
//! 所有子进程都带上 `GIT_TERMINAL_PROMPT=0`：批量操作里不该有命令停下来等
//! 输入凭据。远端地址写回索引前先剔除 URL 中的认证信息。

use crate::commands::repo::index::{Availability, Observation, Snapshot};
use crate::commands::repo::util::{normalize, now_rfc3339};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 一次 Git 调用的结果。
pub struct Output {
    /// 退出码是否成功。
    pub success: bool,
    /// 去掉首尾空白后的标准输出。
    pub stdout: String,
    /// 去掉首尾空白后的标准错误。
    pub stderr: String,
}

impl Output {
    /// 失败时拼一条可读的错误信息。
    fn failure(&self, args: &[&str]) -> anyhow::Error {
        let mut message = format!("git {} 失败", args.join(" "));
        if !self.stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&self.stderr);
        }
        anyhow::anyhow!(message)
    }
}

/// 运行 git（可选工作目录），只返回输出，不判断成败。
pub fn capture(dir: Option<&Path>, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    let output = command.output().map_err(|error| {
        // 工作目录不存在时 current_dir 也会报 NotFound，别误报成没装 Git。
        if error.kind() == std::io::ErrorKind::NotFound {
            match dir {
                Some(dir) if !dir.is_dir() => {
                    anyhow::anyhow!("无法在 {} 运行 git：目录不存在", dir.display())
                }
                _ => anyhow::anyhow!("找不到 git 命令，请安装 Git 并加入 PATH"),
            }
        } else {
            anyhow::anyhow!("无法运行 git: {error}")
        }
    })?;
    Ok(Output {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// 运行 git，失败即返回错误。
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let output = capture(Some(dir), args)?;
    if !output.success {
        return Err(output.failure(args));
    }
    Ok(output.stdout)
}

/// 运行 git，失败时返回 `None`；用于“没有提交”“没有 upstream”这类正常情况。
fn optional(dir: &Path, args: &[&str]) -> Option<String> {
    let output = capture(Some(dir), args).ok()?;
    if !output.success || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
}

/// 路径检查结果。
enum Inspection {
    /// 有效仓库及其 Git 目录。
    Repo {
        git_dir: PathBuf,
        common_git_dir: PathBuf,
    },
    /// 路径不存在。
    Missing,
    /// 路径存在但不可访问。
    Inaccessible(String),
    /// 路径存在但不是有效 Git 仓库。
    Invalid(String),
}

/// 检查路径：存在、是目录、`.git` 标记有效，并由 Git 确认工作目录。
///
/// 只用 `.git` 是否存在判断会被伪造标记骗过，所以最终以 `rev-parse` 的输出
/// 为准；worktree 与 submodule 的 `.git` 是文件，同样能通过。
fn inspect(path: &Path) -> Inspection {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Inspection::Missing,
        Err(error) => {
            return Inspection::Inaccessible(format!("无法访问 {}: {error}", path.display()));
        }
    };
    if !metadata.is_dir() {
        return Inspection::Invalid("路径不是目录".to_string());
    }
    if let Err(error) = std::fs::metadata(path.join(".git")) {
        return match error.kind() {
            std::io::ErrorKind::NotFound => Inspection::Invalid("未找到 .git".to_string()),
            _ => Inspection::Inaccessible(format!("无法读取 .git: {error}")),
        };
    }

    let output = match capture(
        Some(path),
        &[
            "rev-parse",
            "--absolute-git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ],
    ) {
        Ok(output) => output,
        Err(error) => return Inspection::Invalid(error.to_string()),
    };
    if !output.success {
        return Inspection::Invalid(
            output
                .stderr
                .lines()
                .next()
                .unwrap_or("Git 不认为是仓库")
                .to_string(),
        );
    }
    let mut lines = output.stdout.lines();
    let (Some(git_dir), Some(common), Some(top)) = (lines.next(), lines.next(), lines.next())
    else {
        return Inspection::Invalid("无法解析 rev-parse 输出".to_string());
    };

    // 工作目录必须就是该仓库根目录，避免把子目录当成独立仓库。
    if normalize(Path::new(top)) != normalize(path) {
        return Inspection::Invalid(format!("不是仓库根目录（根目录为 {top}）"));
    }
    let git_dir = normalize(Path::new(git_dir));
    if !git_dir.exists() {
        return Inspection::Invalid(format!("Git 目录不存在: {}", git_dir.display()));
    }
    let common_git_dir = {
        let declared = Path::new(common);
        if declared.is_absolute() {
            normalize(declared)
        } else {
            normalize(&path.join(declared))
        }
    };
    Inspection::Repo {
        git_dir,
        common_git_dir,
    }
}

/// 读取全部远端地址（远端名 → 地址），认证信息已剔除。
fn remotes(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(output) = capture(
        Some(path),
        &["config", "--get-regexp", r"^remote\..*\.url$"],
    ) else {
        return map;
    };
    if !output.success {
        return map;
    }
    for line in output.stdout.lines() {
        let Some((key, url)) = line.split_once(' ') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
        else {
            continue;
        };
        map.entry(name.to_string())
            .or_insert_with(|| scrub_url(url.trim()));
    }
    map
}

/// 去掉 URL 里的用户名和口令；scp 形式（`git@host:path`）的 `git@` 不是凭据，保留。
pub fn scrub_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}{}", &url[..scheme_end + 3], &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// 探测仓库：只取可访问状态与 Git 目录，不采集状态快照。
pub fn probe(path: &Path, id: Option<String>) -> Observation {
    let path = normalize(path);
    let mut observation = Observation::probe(path.clone(), id);
    match inspect(&path) {
        Inspection::Repo {
            git_dir,
            common_git_dir,
        } => {
            observation.availability = Availability::Available;
            observation.remote = remotes(&path).remove("origin");
            observation.git_dir = Some(git_dir);
            observation.common_git_dir = Some(common_git_dir);
        }
        Inspection::Missing => {
            observation.availability = Availability::Missing;
            observation.error = Some("目录不存在".to_string());
        }
        Inspection::Inaccessible(message) => {
            observation.availability = Availability::Inaccessible;
            observation.error = Some(message);
        }
        Inspection::Invalid(message) => {
            observation.availability = Availability::Invalid;
            observation.error = Some(message);
        }
    }
    observation.at = now_rfc3339();
    observation
}

/// 探测并采集完整状态快照。
pub fn observe(path: &Path, id: Option<String>) -> Observation {
    let mut observation = probe(path, id);
    if !observation.availability.is_available() {
        return observation;
    }
    match snapshot(&observation.path) {
        Ok(snapshot) => observation.snapshot = Some(snapshot),
        Err(error) => observation.error = Some(format!("{error:#}")),
    }
    observation
}

/// 采集状态快照：远端、分支、HEAD、改动、upstream 与领先/落后。
///
/// 领先/落后来自本地远端跟踪引用，不联网；需要最新远端数据时先 `fetch`。
pub fn snapshot(path: &Path) -> Result<Snapshot> {
    let mut snapshot = Snapshot {
        observed_at: Some(now_rfc3339()),
        ..Snapshot::default()
    };
    snapshot.remotes = remotes(path);
    snapshot.branch = optional(path, &["symbolic-ref", "--short", "-q", "HEAD"]);
    snapshot.head = optional(path, &["rev-parse", "--short", "HEAD"]);
    snapshot.dirty = !run(path, &["status", "--porcelain"])?.is_empty();
    snapshot.upstream = optional(
        path,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    );
    if let Some(upstream) = &snapshot.upstream {
        let range = format!("{upstream}...HEAD");
        if let Some(counts) = optional(path, &["rev-list", "--left-right", "--count", &range]) {
            let mut parts = counts.split_whitespace();
            if let (Some(behind), Some(ahead)) = (parts.next(), parts.next()) {
                snapshot.behind = behind.parse().ok();
                snapshot.ahead = ahead.parse().ok();
            }
        }
    }
    Ok(snapshot)
}

/// 解析远端地址推导仓库名：取最后一段，去掉 `.git` 后缀。
pub fn name_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.ends_with('/') {
        return None;
    }
    let last = url.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// 克隆仓库到目标目录。
pub fn clone(url: &str, dir: &Path) -> Result<()> {
    let output = capture(None, &["clone", url, &dir.to_string_lossy()])?;
    if !output.success {
        return Err(output.failure(&["clone", url]))
            .context(format!("克隆到 {} 失败", dir.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_names_strip_suffix_and_scp_prefix() {
        assert_eq!(
            name_from_remote("https://github.com/originlang/bootidea.cli.git"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(
            name_from_remote("git@github.com:originlang/bootidea.cli.git"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(
            name_from_remote("https://github.com/originlang/bootidea.cli"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(name_from_remote("https://github.com/"), None);
    }

    #[test]
    fn credentials_are_scrubbed_from_urls() {
        assert_eq!(
            scrub_url("https://user:token@example.com/a/b.git"),
            "https://example.com/a/b.git"
        );
        assert_eq!(
            scrub_url("https://example.com/a/b.git"),
            "https://example.com/a/b.git"
        );
        assert_eq!(
            scrub_url("git@github.com:owner/repo.git"),
            "git@github.com:owner/repo.git"
        );
    }

    #[test]
    fn probe_reports_missing_and_fake_markers() {
        let root = std::env::temp_dir().join(format!("bi-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("fake/.git")).expect("mkdir");

        let fake = probe(&root.join("fake"), None);
        assert_eq!(
            fake.availability,
            Availability::Invalid,
            "空的 .git 目录是伪造标记"
        );

        let missing = probe(&root.join("nope"), None);
        assert_eq!(missing.availability, Availability::Missing);

        assert_eq!(probe(&root, None).availability, Availability::Invalid);
        let _ = std::fs::remove_dir_all(&root);
    }
}
