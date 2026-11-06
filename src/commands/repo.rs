//! Git 仓库管理：登记、克隆、打开与跨仓库状态汇总。
//!
//! 登记表持久化在数据根目录的 `repos.json`，只记录名称、路径和远端，不复制
//! 仓库本身。`remove` 只取消登记；`fetch` 只获取远端更新，不合并、不推送、
//! 不切换分支。批量操作逐仓库报告结果，部分失败时给出汇总与非零退出码。

use crate::cli::RepoCommand;
use crate::config::Settings;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 已登记仓库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// 登记名称，登记时保证唯一。
    pub name: String,
    /// 本地路径。
    pub path: PathBuf,
    /// 远端地址（没有远端的本地仓库可以为空）。
    pub remote: Option<String>,
}

/// 登记表文件的内容。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Registry {
    repos: Vec<RepoEntry>,
}

/// 扫描目录时跳过的子目录名。
const SCAN_SKIP: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    ".gradle",
];

/// 登记表文件的位置：数据根目录下的 `repos.json`。
fn registry_path(settings: &Settings) -> PathBuf {
    settings.paths().root().join("repos.json")
}

/// 从登记表文件加载；文件不存在视为空表。
fn load_registry(path: &Path) -> Result<Registry> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Registry::default());
        }
        Err(error) => return Err(error).context(format!("无法读取 {}", path.display())),
    };
    serde_json::from_str(&raw).with_context(|| format!("登记表损坏: {}", path.display()))
}

/// 保存登记表，目录不存在时先创建。
fn save_registry(path: &Path, registry: &Registry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建 {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(registry).context("登记表序列化失败")?;
    std::fs::write(path, raw + "\n").with_context(|| format!("无法写入 {}", path.display()))
}

/// 规范化路径：尽量 canonicalize，失败时退回绝对路径；去掉 Windows 的 `\\?\` 前缀。
pub fn normalize(path: &Path) -> PathBuf {
    let resolved = path
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(path));
    let text = resolved.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => resolved,
    }
}

/// 运行 git 并返回标准输出；失败时把 stderr 一并放进错误信息。
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command.args(args);
    let output = command.output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow!("找不到 git 命令，请安装 Git 并加入 PATH")
        } else {
            anyhow!("无法运行 git: {error}")
        }
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        let mut message = format!("git {} 失败", args.join(" "));
        if !stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&stderr);
        }
        bail!(message);
    }
    Ok(stdout)
}

/// 目录是否是 Git 仓库（`.git` 可能是目录，也可能是 worktree 的指针文件）。
fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// 从远端地址推导仓库名：取最后一段，去掉 `.git` 后缀。
fn repo_name_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    // 以 / 结尾指向目录而非仓库，无法推导名字。
    if url.ends_with('/') {
        return None;
    }
    // scp 形式 git@host:owner/name.git 与普通 URL 统一按 `/` 分段。
    let last = url.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// 登记结果。
#[derive(Debug, PartialEq, Eq)]
enum InsertOutcome {
    /// 新登记。
    Added,
    /// 该路径已在登记表中（名称不变）。
    AlreadyRegistered,
}

/// 把一条记录插入登记表：路径去重；同名不同路径视为冲突。
fn insert_entry(repos: &mut Vec<RepoEntry>, entry: RepoEntry) -> Result<InsertOutcome> {
    if repos.iter().any(|existing| existing.path == entry.path) {
        return Ok(InsertOutcome::AlreadyRegistered);
    }
    if let Some(clash) = repos.iter().find(|existing| existing.name == entry.name) {
        bail!(
            "登记名称 {} 已被 {} 占用；请换一个名字（--name），或先 repo remove {}",
            entry.name,
            clash.path.display(),
            clash.name
        );
    }
    repos.push(entry);
    Ok(InsertOutcome::Added)
}

/// 按登记名称或本地路径解析目标；找不到时列出可用名称。
fn resolve_target<'a>(repos: &'a [RepoEntry], target: &str) -> Result<&'a RepoEntry> {
    let by_name: Vec<&RepoEntry> = repos.iter().filter(|r| r.name == target).collect();
    match by_name.as_slice() {
        [single] => return Ok(single),
        [] => {}
        many => bail!(
            "登记名称 {} 对应多个仓库（{}），登记表有重复名称，请先用 repos.json 清理",
            target,
            many.iter()
                .map(|r| r.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }

    let normalized = normalize(Path::new(target));
    if let Some(matched) = repos.iter().find(|r| r.path == normalized) {
        return Ok(matched);
    }
    bail!(
        "{} 既不是已登记名称也不是已登记路径；已登记: {}",
        target,
        names(repos)
    )
}

/// 可用登记名称列表，用于错误提示。
fn names(repos: &[RepoEntry]) -> String {
    if repos.is_empty() {
        "（登记表为空）".to_string()
    } else {
        repos
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// 递归找出 `depth` 层内的 Git 仓库目录，跳过构建产物与依赖目录。
fn scan_repos(dir: &Path, depth: u8) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if is_git_repo(dir) {
        found.push(dir.to_path_buf());
        return found;
    }
    if depth == 0 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skip = path
            .file_name()
            .map(|name| SCAN_SKIP.contains(&name.to_string_lossy().as_ref()))
            .unwrap_or(false);
        if skip {
            continue;
        }
        found.extend(scan_repos(&path, depth - 1));
    }
    found
}

/// 逐仓库执行批量操作：每个仓库独立报告，失败不中断其余仓库。
fn run_batch<F>(repos: &[RepoEntry], mut op: F) -> Result<()>
where
    F: FnMut(&RepoEntry) -> Result<()>,
{
    let mut failures = Vec::new();
    for repo in repos {
        if let Err(error) = op(repo) {
            eprintln!("[失败] {}: {error:#}", repo.name);
            failures.push(repo.name.clone());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{} 个仓库操作失败: {}", failures.len(), failures.join(", "))
    }
}

/// 批量任务的仓库集合：`--all` 是全部登记仓库，否则是解析出的单个目标。
///
/// 返回 owned 列表：不带目标时可能作用于未登记的当前仓库，无法借用登记表。
fn batch_repos(repos: &[RepoEntry], target: Option<&String>, all: bool) -> Result<Vec<RepoEntry>> {
    if all {
        if repos.is_empty() {
            bail!("登记表为空，请先 repo clone 或 repo add");
        }
        return Ok(repos.to_vec());
    }
    match target {
        Some(target) => Ok(vec![resolve_target(repos, target)?.clone()]),
        None => {
            // 不带目标：作用于当前仓库。当前目录未必登记过，直接让 git 处理。
            let cwd = std::env::current_dir().context("无法获取当前目录")?;
            if !is_git_repo(&cwd) {
                bail!("当前目录不是 Git 仓库；请指定目标或使用 --all");
            }
            let normalized = normalize(&cwd);
            Ok(vec![
                repos
                    .iter()
                    .find(|r| r.path == normalized)
                    .cloned()
                    .unwrap_or(RepoEntry {
                        name: String::new(),
                        path: normalized,
                        remote: None,
                    }),
            ])
        }
    }
}

/// `repo` 命令组入口。
pub fn repo(settings: &Settings, command: RepoCommand) -> Result<()> {
    let path = registry_path(settings);
    let registry = load_registry(&path)?;
    let mut repos = registry.repos;

    match command {
        RepoCommand::Clone { url, dir, name } => {
            let name = match name {
                Some(name) => name,
                None => repo_name_from_remote(&url)
                    .context("无法从远端地址推导仓库名，请用 --name 指定")?,
            };
            let dir = match dir {
                Some(dir) => dir,
                None => {
                    let root = settings.repo.root.as_ref().context(
                        "未配置 repo.root，请用 --dir 指定目标目录或先 config set repo.root",
                    )?;
                    root.join(&name)
                }
            };
            if dir.exists() {
                bail!("目标目录已存在: {}", dir.display());
            }
            git(None, &["clone", &url, &dir.to_string_lossy()])?;
            println!("已克隆到 {}", dir.display());
            register_repo(&mut repos, dir, Some(url), Some(name), false)?;
            save_registry(&path, &Registry { repos })?;
        }
        RepoCommand::Add { path: added } => {
            register_repo(&mut repos, added, None, None, true)?;
            save_registry(&path, &Registry { repos })?;
        }
        RepoCommand::Scan { dir } => {
            let mut added = 0usize;
            let mut known = 0usize;
            for found in scan_repos(&normalize(&dir), 3) {
                match register_repo(&mut repos, found, None, None, false)? {
                    InsertOutcome::Added => added += 1,
                    InsertOutcome::AlreadyRegistered => known += 1,
                }
            }
            let total = repos.len();
            save_registry(&path, &Registry { repos })?;
            println!("新登记 {added} 个，已有 {known} 个，共 {total} 个仓库");
        }
        RepoCommand::List => {
            if repos.is_empty() {
                println!("登记表为空；用 repo clone、repo add 或 repo scan 登记");
                return Ok(());
            }
            for repo in &repos {
                println!(
                    "{}\t{}\t{}",
                    repo.name,
                    repo.path.display(),
                    repo.remote.as_deref().unwrap_or("-")
                );
            }
        }
        RepoCommand::Info { target } => {
            let repo = resolve_target(&repos, &target)?;
            let branch = git(Some(&repo.path), &["rev-parse", "--abbrev-ref", "HEAD"])?;
            println!("名称:   {}", repo.name);
            println!("路径:   {}", repo.path.display());
            println!("远端:   {}", repo.remote.as_deref().unwrap_or("-"));
            println!("分支:   {branch}");
        }
        RepoCommand::Open { target } => {
            let repo = resolve_target(&repos, &target)?;
            open_in_editor(settings, &repo.path)?;
        }
        RepoCommand::Status { target, all } => {
            let targets = batch_repos(&repos, target.as_ref(), all)?;
            run_batch(&targets, |repo| {
                let status = git(Some(&repo.path), &["status", "--porcelain", "-b"])?;
                println!("== {} ==", repo.name);
                if status.is_empty() {
                    println!("（干净）");
                } else {
                    println!("{status}");
                }
                Ok(())
            })?;
        }
        RepoCommand::Fetch { target, all } => {
            let targets = batch_repos(&repos, target.as_ref(), all)?;
            run_batch(&targets, |repo| {
                git(Some(&repo.path), &["fetch", "--all"])?;
                println!("[完成] {}", repo.name);
                Ok(())
            })?;
        }
        RepoCommand::Remove { target } => {
            let repo = resolve_target(&repos, &target)?.clone();
            repos.retain(|r| r.path != repo.path);
            save_registry(&path, &Registry { repos })?;
            println!(
                "已取消登记 {}（本地文件保留在 {}）",
                repo.name,
                repo.path.display()
            );
        }
    }
    Ok(())
}

/// 校验并登记一个仓库。
///
/// `strict` 用于 `repo add`：目标必须是已有 Git 仓库。扫描场景下不是仓库的
/// 目录不会传进来，无需再验。
fn register_repo(
    repos: &mut Vec<RepoEntry>,
    path: PathBuf,
    remote_override: Option<String>,
    name_override: Option<String>,
    strict: bool,
) -> Result<InsertOutcome> {
    let normalized = normalize(&path);
    if !normalized.is_dir() {
        bail!("目录不存在: {}", normalized.display());
    }
    if strict && !is_git_repo(&normalized) {
        bail!("{} 不是 Git 仓库（未找到 .git）", normalized.display());
    }

    let remote = match remote_override {
        Some(remote) => Some(remote),
        None => git(Some(&normalized), &["remote", "get-url", "origin"]).ok(),
    };

    let name = match name_override {
        Some(name) => name,
        None => repo_name_from_remote(remote.as_deref().unwrap_or(""))
            .or_else(|| {
                normalized
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .context("无法推导仓库名，请先 git remote add origin")?,
    };

    let outcome = insert_entry(
        repos,
        RepoEntry {
            name: name.clone(),
            path: normalized.clone(),
            remote,
        },
    )?;
    match outcome {
        InsertOutcome::Added => println!("已登记 {name}: {}", normalized.display()),
        InsertOutcome::AlreadyRegistered => println!("已登记过: {}", normalized.display()),
    }
    Ok(outcome)
}

/// 用配置的编辑器打开目录。
///
/// 解析顺序：`editor` 配置 > `EDITOR`/`VISUAL` 环境变量 > 平台默认打开方式。
/// 配置值支持带参数（如 `code --wait`），目录路径附加在参数后面。
fn open_in_editor(settings: &Settings, path: &Path) -> Result<()> {
    let configured = settings
        .editor
        .clone()
        .or_else(|| std::env::var("EDITOR").ok())
        .or_else(|| std::env::var("VISUAL").ok());

    let mut parts = configured.map(|command| {
        command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    });

    match parts.take() {
        Some(words) if !words.is_empty() => {
            let (program, args) = words.split_first().expect("非空");
            let status = Command::new(program)
                .args(args)
                .arg(path)
                .status()
                .map_err(|error| anyhow!("无法启动编辑器 {program}: {error}"))?;
            if !status.success() {
                bail!("编辑器 {program} 退出码 {:?}", status.code());
            }
            Ok(())
        }
        _ => {
            #[cfg(target_os = "windows")]
            let status = Command::new("cmd")
                .args(["/C", "start", "", &path.to_string_lossy()])
                .status()
                .map_err(|error| anyhow!("无法打开 {}: {error}", path.display()))?;
            #[cfg(target_os = "macos")]
            let status = Command::new("open")
                .arg(path)
                .status()
                .map_err(|error| anyhow!("无法打开 {}: {error}", path.display()))?;
            #[cfg(all(unix, not(target_os = "macos")))]
            let status = Command::new("xdg-open")
                .arg(path)
                .status()
                .map_err(|error| anyhow!("无法打开 {}: {error}", path.display()))?;
            if !status.success() {
                bail!("系统打开方式返回失败；建议 config set editor 指定编辑器");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, path: &str) -> RepoEntry {
        RepoEntry {
            name: name.to_string(),
            path: PathBuf::from(path),
            remote: None,
        }
    }

    #[test]
    fn remote_names_strip_suffix_and_scp_prefix() {
        assert_eq!(
            repo_name_from_remote("https://github.com/originlang/bootidea.cli.git"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(
            repo_name_from_remote("git@github.com:originlang/bootidea.cli.git"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(
            repo_name_from_remote("https://github.com/originlang/bootidea.cli"),
            Some("bootidea.cli".to_string())
        );
        assert_eq!(repo_name_from_remote("https://github.com/"), None);
    }

    #[test]
    fn insert_dedupes_by_path_and_rejects_name_clash() {
        let mut repos = vec![entry("a", "E:/repos/a"), entry("b", "E:/repos/b")];
        assert_eq!(
            insert_entry(&mut repos, entry("again", "E:/repos/a")).expect("ok"),
            InsertOutcome::AlreadyRegistered
        );
        assert_eq!(repos.len(), 2);
        let clash = insert_entry(&mut repos, entry("a", "E:/repos/c"));
        assert!(clash.is_err(), "同名不同路径必须报错");
        assert_eq!(
            insert_entry(&mut repos, entry("c", "E:/repos/c")).expect("ok"),
            InsertOutcome::Added
        );
    }

    #[test]
    fn target_resolves_by_name_then_path() {
        let repos = vec![entry("bootidea.cli", "E:/originlang/bootidea.cli")];
        assert_eq!(
            resolve_target(&repos, "bootidea.cli")
                .expect("by name")
                .name,
            "bootidea.cli"
        );
        let missing = resolve_target(&repos, "nope");
        assert!(missing.is_err());
        let message = format!("{}", missing.unwrap_err());
        assert!(
            message.contains("已登记"),
            "错误信息应列出可用名称: {message}"
        );
    }

    #[test]
    fn registry_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("bi-repo-test-{}", std::process::id()));
        let path = dir.join("repos.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let registry = Registry {
            repos: vec![RepoEntry {
                name: "demo".into(),
                path: PathBuf::from("E:/repos/demo"),
                remote: Some("https://example.com/demo.git".into()),
            }],
        };
        save_registry(&path, &registry).expect("save");
        assert_eq!(load_registry(&path).expect("load"), registry);

        // 缺失文件视为空表，而不是报错。
        let missing = dir.join("nope.json");
        assert_eq!(load_registry(&missing).expect("empty"), Registry::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_finds_git_dirs_without_diving_into_skips() {
        let root = std::env::temp_dir().join(format!("bi-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo_a = root.join("a");
        let repo_deep = root.join("x").join("y").join("deep");
        let noise = root.join("node_modules").join("fake");
        for dir in [&repo_a, &repo_deep, &noise] {
            std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
        }

        let found = scan_repos(&root, 3);
        let mut names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a", "deep"], "node_modules 下的仓库不应出现");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_reports_partial_failures() {
        let repos = vec![entry("ok", "E:/ok"), entry("bad", "E:/bad")];
        let result = run_batch(&repos, |repo| {
            if repo.name == "bad" {
                bail!("模拟失败");
            }
            Ok(())
        });
        assert!(result.is_err(), "部分失败必须非零退出");
        let message = format!("{}", result.unwrap_err());
        assert!(message.contains("1 个仓库操作失败"), "{message}");
        assert!(message.contains("bad"), "{message}");
    }
}
