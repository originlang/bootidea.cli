//! 目标选择：把名称、路径、`--all`、分组、临时扫描与筛选统一成固定目标集合。
//!
//! 名称/路径列表、`--all`、`--group`、`--under` 互斥；筛选条件取交集。执行前
//! 按规范化路径去重并保持顺序，避免同一仓库被重复操作。

use crate::cli::TargetArgs;
use crate::commands::repo::git;
use crate::commands::repo::index::{Index, RepoRecord};
use crate::commands::repo::scan::{self, Rules, merge_exclude};
use crate::commands::repo::util::{glob_match, normalize};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 一个执行目标：可能已登记，也可能是当前仓库或 `--under` 临时发现的路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// 已登记时为稳定 ID。
    pub id: Option<String>,
    /// 展示名称；未登记时取目录名。
    pub name: String,
    /// 规范化工作目录。
    pub path: PathBuf,
    /// 共同 Git 目录，用于串行调度共享 Git 目录的 worktree。
    pub common_git_dir: Option<PathBuf>,
}

impl Target {
    /// 由索引记录构造。
    pub fn from_record(record: &RepoRecord) -> Self {
        Self {
            id: Some(record.id.clone()),
            name: record.name.clone(),
            path: record.path.clone(),
            common_git_dir: record.common_git_dir.clone(),
        }
    }

    /// 由未登记路径构造，名称取目录名。
    pub fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| path.display().to_string());
        Self {
            id: None,
            name,
            path,
            common_git_dir: None,
        }
    }

    /// 输出用的路径展示。
    pub fn display_path(&self) -> String {
        self.path.display().to_string()
    }
}

/// 按参数解析目标集合。
pub fn select(index: &Index, args: &TargetArgs) -> Result<Vec<Target>> {
    let set = if args.all {
        index.repos.iter().map(Target::from_record).collect()
    } else if let Some(group) = &args.group {
        group_targets(index, group)?
    } else if let Some(under) = &args.under {
        under_targets(under)
    } else if !args.targets.is_empty() {
        args.targets
            .iter()
            .map(|text| resolve(index, text))
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![current(index)?]
    };

    let filtered = set
        .into_iter()
        .filter(|target| matches_filters(target, args))
        .collect();
    Ok(dedupe(filtered))
}

/// 单个目标：先按登记名称，再按本地路径。
pub fn resolve(index: &Index, text: &str) -> Result<Target> {
    let by_name: Vec<&RepoRecord> = index
        .repos
        .iter()
        .filter(|repo| repo.name == text)
        .collect();
    match by_name.as_slice() {
        [record] => return Ok(Target::from_record(record)),
        [] => {}
        many => bail!(
            "登记名称 {text} 对应多个仓库（{}），请先清理索引",
            many.iter()
                .map(|repo| repo.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }

    let path = normalize(Path::new(text));
    if let Some(record) = index.by_path(&path) {
        return Ok(Target::from_record(record));
    }
    if path.is_dir() {
        return Ok(Target::from_path(path));
    }
    bail!(
        "{text} 既不是已登记名称也不是已存在路径；已登记: {}",
        names(index)
    )
}

/// 当前仓库：从当前目录向上找 `.git`，可从仓库子目录执行。
pub fn current(index: &Index) -> Result<Target> {
    let cwd = std::env::current_dir().context("无法获取当前目录")?;
    let mut dir = normalize(&cwd);
    loop {
        if dir.join(".git").symlink_metadata().is_ok() {
            let root = git::capture(Some(&dir), &["rev-parse", "--show-toplevel"])
                .ok()
                .filter(|output| output.success && !output.stdout.is_empty())
                .map(|output| {
                    normalize(Path::new(output.stdout.lines().next().unwrap_or_default()))
                })
                .unwrap_or(dir);
            return Ok(match index.by_path(&root) {
                Some(record) => Target::from_record(record),
                None => Target::from_path(root),
            });
        }
        if !dir.pop() {
            break;
        }
    }
    bail!("当前目录不是 Git 仓库；请指定目标或使用 --all")
}

/// 分组内的仓库；未知分组直接报错。
fn group_targets(index: &Index, name: &str) -> Result<Vec<Target>> {
    let group = index.group(name).with_context(|| {
        format!(
            "未知分组 {name}；已登记分组: {}",
            if index.groups.is_empty() {
                "（无）".to_string()
            } else {
                index
                    .groups
                    .iter()
                    .map(|group| group.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    })?;
    Ok(group
        .members
        .iter()
        .filter_map(|id| index.position_of(id))
        .map(|position| Target::from_record(&index.repos[position]))
        .collect())
}

/// `--under`：临时递归发现，不写入索引。
fn under_targets(dir: &Path) -> Vec<Target> {
    let rules = Rules {
        exclude: merge_exclude(&[]),
        ..Rules::default()
    };
    scan::scan(dir, &rules)
        .found
        .into_iter()
        .map(|observation| Target::from_path(observation.path))
        .collect()
}

/// 名称筛选：`--match` 与 `--exclude` 都按登记名称做 glob 匹配。
fn matches_filters(target: &Target, args: &TargetArgs) -> bool {
    if let Some(pattern) = &args.pattern
        && !glob_match(pattern, &target.name)
    {
        return false;
    }
    !args
        .exclude
        .iter()
        .any(|pattern| glob_match(pattern, &target.name))
}

/// 按路径去重，保留首次出现的顺序。
fn dedupe(targets: Vec<Target>) -> Vec<Target> {
    let mut seen = HashSet::new();
    targets
        .into_iter()
        .filter(|target| seen.insert(target.path.clone()))
        .collect()
}

/// 可用登记名称列表，用于错误提示。
fn names(index: &Index) -> String {
    if index.repos.is_empty() {
        "（索引为空）".to_string()
    } else {
        index
            .repos
            .iter()
            .map(|repo| repo.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::repo::index::{Availability, Group, RepoRecord};

    fn record(id: &str, name: &str, path: &str) -> RepoRecord {
        RepoRecord {
            id: id.into(),
            name: name.into(),
            path: PathBuf::from(path),
            groups: Vec::new(),
            remote: None,
            git_dir: None,
            common_git_dir: None,
            availability: Availability::Unknown,
            last_seen_at: None,
            snapshot: None,
        }
    }

    fn index() -> Index {
        let mut index = Index::default();
        index
            .repos
            .push(record("repo_01", "bootidea.cli", "/work/bootidea.cli"));
        index
            .repos
            .push(record("repo_02", "bootidea.host", "/work/bootidea.host"));
        index.repos.push(record("repo_03", "demo", "/work/demo"));
        index.groups.push(Group {
            name: "platform".into(),
            members: vec!["repo_02".into(), "repo_01".into()],
        });
        index.sync_groups();
        index
    }

    fn blank() -> TargetArgs {
        TargetArgs::default()
    }

    #[test]
    fn all_filters_by_name_glob() {
        let index = index();
        let mut args = blank();
        args.all = true;
        args.pattern = Some("boot*".into());
        let targets = select(&index, &args).expect("select");
        assert_eq!(
            targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["bootidea.cli", "bootidea.host"]
        );

        args.exclude = vec!["*host".into()];
        let targets = select(&index, &args).expect("select");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "bootidea.cli");
    }

    #[test]
    fn group_selection_uses_membership_order() {
        let index = index();
        let mut args = blank();
        args.group = Some("platform".into());
        let targets = select(&index, &args).expect("select");
        assert_eq!(
            targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["bootidea.host", "bootidea.cli"]
        );
    }

    #[test]
    fn unknown_group_and_target_are_errors() {
        let index = index();
        let mut args = blank();
        args.group = Some("nope".into());
        assert!(select(&index, &args).is_err());

        let mut args = blank();
        args.targets = vec!["missing".into()];
        let error = select(&index, &args).unwrap_err();
        assert!(format!("{error}").contains("已登记"), "{error}");
    }

    #[test]
    fn explicit_path_is_resolved_even_when_unregistered() {
        let dir = std::env::temp_dir().join(format!("bi-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let index = index();
        let mut args = blank();
        args.targets = vec![dir.display().to_string()];
        let targets = select(&index, &args).expect("select");
        assert_eq!(targets.len(), 1);
        assert!(targets[0].id.is_none(), "未登记路径没有 ID");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn selection_is_deduped_by_path() {
        let index = index();
        let mut args = blank();
        args.targets = vec!["bootidea.cli".into(), "/work/bootidea.cli".into()];
        let targets = select(&index, &args).expect("select");
        assert_eq!(targets.len(), 1);
    }
}
