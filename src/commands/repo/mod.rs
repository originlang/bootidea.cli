//! Git 仓库管理：登记、扫描、索引维护与跨仓库批量操作。
//!
//! 索引保存在数据根目录的 `repos.json`（v2），只记录仓库位置和状态快照，
//! 不复制仓库本身。首次 `scan / add / clone` 登记，日常命令读取索引，
//! 显式 `scan` 才展开目录；`refresh` 只刷新已知路径，不发现新仓库。
//!
//! 所有能力都在本机完成，不依赖 Host，也不启动后台服务。

mod exec;
mod git;
mod index;
mod scan;
mod select;
mod util;

use crate::cli::{BatchArgs, GroupCommand, RepoCommand, TargetArgs};
use crate::config::Settings;
use anyhow::{Context, Result, anyhow, bail};
use index::{
    Availability, Group, Index, Observation, RepoRecord, ScanRoot, Store, merge_observation,
};
use scan::Rules;
use select::Target;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `repo` 命令组入口。
pub fn repo(settings: &Settings, command: RepoCommand) -> Result<()> {
    let store = Store::new(settings.paths().root());
    match command {
        RepoCommand::Clone { url, dir, name } => clone(&store, settings, &url, dir, name),
        RepoCommand::Add { path, name } => add(&store, path, name),
        RepoCommand::Scan {
            dir,
            depth,
            exclude,
            nested,
            dry_run,
            known_roots,
            json,
        } => scan_cmd(
            &store,
            dir,
            depth,
            exclude,
            nested,
            dry_run,
            known_roots,
            json,
        ),
        RepoCommand::List { group, json } => list(&store, group.as_deref(), json),
        RepoCommand::Info { target, json } => info(&store, &target, json),
        RepoCommand::Open { target } => open(&store, settings, target.as_deref()),
        RepoCommand::Status {
            select,
            dirty,
            cached,
            jobs,
            json,
        } => status(&store, &select, dirty, cached, jobs, json),
        RepoCommand::Fetch(batch) => batch_action(&store, batch, Action::Fetch),
        RepoCommand::Pull(batch) => batch_action(&store, batch, Action::Pull),
        RepoCommand::Diff { batch, staged } => batch_action(&store, batch, Action::Diff { staged }),
        RepoCommand::Branch(batch) => batch_action(&store, batch, Action::Branch),
        RepoCommand::Switch { branch, batch } => {
            batch_action(&store, batch, Action::Switch(branch))
        }
        RepoCommand::Start { branch, batch } => batch_action(&store, batch, Action::Start(branch)),
        RepoCommand::Push(batch) => batch_action(&store, batch, Action::Push),
        RepoCommand::Foreach { batch, command } => {
            batch_action(&store, batch, Action::Foreach(command))
        }
        RepoCommand::Refresh {
            targets,
            jobs,
            json,
        } => refresh(&store, &targets, jobs, json),
        RepoCommand::Relocate { target, path } => relocate(&store, &target, path),
        RepoCommand::Remove { target } => remove(&store, &target),
        RepoCommand::Group(command) => group(&store, command),
    }
}

// ---------------------------------------------------------------------------
// 登记：clone / add / scan
// ---------------------------------------------------------------------------

/// 克隆并登记。
fn clone(
    store: &Store,
    settings: &Settings,
    url: &str,
    dir: Option<PathBuf>,
    name: Option<String>,
) -> Result<()> {
    let name = match name {
        Some(name) => name,
        None => git::name_from_remote(url).context("无法从远端地址推导仓库名，请用 --name 指定")?,
    };
    let dir = match dir {
        Some(dir) => dir,
        None => {
            let root =
                settings.repo.root.as_ref().context(
                    "未配置 repo.root，请用 --dir 指定目标目录或先 config set repo.root",
                )?;
            root.join(&name)
        }
    };
    if dir.exists() {
        bail!("目标目录已存在: {}", dir.display());
    }
    git::clone(url, &dir)?;
    let observation = git::probe(&dir, None);
    if !observation.availability.is_available() {
        bail!(
            "克隆完成但无法验证仓库 {}: {}",
            dir.display(),
            observation.error.as_deref().unwrap_or("未知原因")
        );
    }
    let outcome = store.update(|index| Ok(insert_record(index, name.clone(), &observation)))?;
    match outcome {
        InsertOutcome::Added(name) => {
            println!("已克隆并登记 {name}: {}", observation.path.display())
        }
        InsertOutcome::Existing(name) => {
            println!("已克隆到 {}（{name} 已登记）", observation.path.display())
        }
        InsertOutcome::NameTaken { name, owner } => bail!(
            "登记名称 {name} 已被 {} 占用；请用 --name 指定另一个名字",
            owner.display()
        ),
    }
    Ok(())
}

/// 登记已有仓库。
fn add(store: &Store, path: PathBuf, name: Option<String>) -> Result<()> {
    let observation = git::probe(&path, None);
    if !observation.availability.is_available() {
        bail!(
            "{} 不是有效 Git 仓库: {}",
            util::normalize(&path).display(),
            observation.error.as_deref().unwrap_or("未知原因")
        );
    }
    let name = name.unwrap_or_else(|| derive_name(&observation));
    let requested = name.clone();
    let outcome = store.update(|index| Ok(insert_record(index, name.clone(), &observation)))?;
    match outcome {
        InsertOutcome::Added(name) => {
            println!("已登记 {name}: {}", observation.path.display())
        }
        InsertOutcome::Existing(name) => {
            if name == requested {
                println!("已登记过 {name}: {}", observation.path.display())
            } else {
                println!(
                    "该路径已登记为 {name}，名称保持不变: {}",
                    observation.path.display()
                )
            }
        }
        InsertOutcome::NameTaken { name, owner } => bail!(
            "登记名称 {name} 已被 {} 占用；请用 --name 指定另一个名字",
            owner.display()
        ),
    }
    Ok(())
}

/// 扫描并登记；`--dry-run` 只展示。
#[allow(clippy::too_many_arguments)]
fn scan_cmd(
    store: &Store,
    dir: Option<PathBuf>,
    depth: Option<u8>,
    exclude: Vec<String>,
    nested: bool,
    dry_run: bool,
    known_roots: bool,
    json: bool,
) -> Result<()> {
    let index = store.load()?;
    let plans: Vec<(PathBuf, Rules)> = if known_roots {
        if index.roots.is_empty() {
            bail!("没有保存的扫描根目录；先执行 repo scan <目录>");
        }
        index
            .roots
            .iter()
            .map(|root| {
                (
                    root.path.clone(),
                    Rules {
                        depth: root.depth,
                        exclude: root.exclude.clone(),
                        nested: root.nested,
                    },
                )
            })
            .collect()
    } else {
        let dir = dir.context("请指定扫描目录，或使用 --known-roots")?;
        let path = util::normalize(&dir);
        let saved = index.roots.iter().find(|root| root.path == path);
        let rules = Rules {
            depth: depth
                .or_else(|| saved.map(|root| root.depth))
                .unwrap_or(scan::DEFAULT_DEPTH),
            exclude: if exclude.is_empty() {
                saved
                    .map(|root| root.exclude.clone())
                    .unwrap_or_else(scan::default_exclude)
            } else {
                scan::merge_exclude(&exclude)
            },
            nested: nested || saved.map(|root| root.nested).unwrap_or(false),
        };
        vec![(path, rules)]
    };

    let mut scanned = Vec::new();
    let mut failures = Vec::new();
    for (path, rules) in plans {
        let outcome = scan::scan(&path, &rules);
        match &outcome.error {
            Some(error) => failures.push(format!("{}: {error}", path.display())),
            None => scanned.push((path, rules, outcome)),
        }
    }

    if dry_run {
        for (path, rules, outcome) in &scanned {
            if !json {
                println!(
                    "扫描 {}（深度 {}，{}，排除 {}）",
                    path.display(),
                    rules.depth,
                    if rules.nested {
                        "含嵌套"
                    } else {
                        "不含嵌套"
                    },
                    if rules.exclude.is_empty() {
                        "-".to_string()
                    } else {
                        rules.exclude.join(", ")
                    }
                );
            }
            for observation in &outcome.found {
                if json {
                    continue;
                }
                let known = index.by_path(&observation.path).is_some();
                println!(
                    "  {} {}",
                    if known { "=" } else { "+" },
                    observation.path.display()
                );
            }
            if !json {
                for issue in &outcome.issues {
                    println!("  ? {issue}");
                }
            }
        }
        if json {
            print_json(&json!({
                "dry_run": true,
                "roots": scanned.iter().map(|(path, rules, outcome)| json!({
                    "path": path.display().to_string(),
                    "depth": rules.depth,
                    "nested": rules.nested,
                    "exclude": rules.exclude,
                    "found": outcome.found.iter().map(|observation| json!({
                        "name": derive_name(observation),
                        "path": observation.path.display().to_string(),
                        "known": index.by_path(&observation.path).is_some(),
                    })).collect::<Vec<_>>(),
                    "issues": outcome.issues,
                })).collect::<Vec<_>>(),
                "failures": failures,
            }))?;
        }
        if !failures.is_empty() {
            bail!(
                "{} 个根目录扫描失败: {}",
                failures.len(),
                failures.join("; ")
            );
        }
        return Ok(());
    }

    let summary = store.update(|index| {
        let mut added = Vec::new();
        let mut existing = Vec::new();
        let mut conflicts = Vec::new();
        let mut issues = Vec::new();
        for (path, rules, outcome) in &scanned {
            for observation in &outcome.found {
                match insert_record(index, derive_name(observation), observation) {
                    InsertOutcome::Added(name) => added.push(name),
                    InsertOutcome::Existing(name) => {
                        existing.push(name);
                        merge_observation(index, observation);
                    }
                    InsertOutcome::NameTaken { name, owner } => conflicts.push(json!({
                        "name": name,
                        "path": observation.path.display().to_string(),
                        "owner": owner.display().to_string(),
                    })),
                }
            }
            issues.extend(outcome.issues.iter().cloned());
            let recorded = ScanRoot {
                path: path.clone(),
                depth: rules.depth,
                exclude: rules.exclude.clone(),
                nested: rules.nested,
                last_scanned_at: Some(util::now_rfc3339()),
            };
            match index.roots.iter_mut().find(|root| root.path == *path) {
                Some(slot) => *slot = recorded,
                None => index.roots.push(recorded),
            }
        }
        Ok(json!({
            "added": added,
            "existing": existing,
            "conflicts": conflicts,
            "issues": issues,
            "total": index.repos.len(),
        }))
    })?;

    if json {
        let mut value = summary.clone();
        value["failures"] = json!(failures);
        print_json(&value)?;
    } else {
        let added = summary["added"].as_array().map(Vec::len).unwrap_or(0);
        let existing = summary["existing"].as_array().map(Vec::len).unwrap_or(0);
        let conflicts = summary["conflicts"].as_array().cloned().unwrap_or_default();
        let issues = summary["issues"].as_array().cloned().unwrap_or_default();
        for (path, rules, _) in &scanned {
            println!(
                "扫描 {}（深度 {}，{}）",
                path.display(),
                rules.depth,
                if rules.nested {
                    "含嵌套"
                } else {
                    "不含嵌套"
                }
            );
        }
        for name in summary["added"].as_array().cloned().unwrap_or_default() {
            println!("  + {}", name.as_str().unwrap_or_default());
        }
        for name in summary["existing"].as_array().cloned().unwrap_or_default() {
            println!("  = {}（已登记）", name.as_str().unwrap_or_default());
        }
        for conflict in &conflicts {
            println!(
                "  ! {} 与 {} 同名，未登记；可用 add --name 消歧",
                conflict["path"].as_str().unwrap_or_default(),
                conflict["owner"].as_str().unwrap_or_default()
            );
        }
        for issue in &issues {
            println!("  ? {}", issue.as_str().unwrap_or_default());
        }
        for failure in &failures {
            eprintln!("[失败] {failure}");
        }
        if !scanned.is_empty() {
            println!(
                "新登记 {added} 个，已有 {existing} 个，冲突 {} 个，共 {} 个仓库",
                conflicts.len(),
                summary["total"].as_u64().unwrap_or(0)
            );
        }
    }

    if !failures.is_empty() {
        bail!(
            "{} 个根目录扫描失败: {}",
            failures.len(),
            failures.join("; ")
        );
    }
    Ok(())
}

/// 登记结果。
enum InsertOutcome {
    /// 新登记。
    Added(String),
    /// 该路径已登记。
    Existing(String),
    /// 同名不同路径。
    NameTaken { name: String, owner: PathBuf },
}

/// 把探测结果插入索引；路径去重，同名不同路径不静默覆盖。
fn insert_record(index: &mut Index, name: String, observation: &Observation) -> InsertOutcome {
    if let Some(existing) = index.by_path(&observation.path) {
        return InsertOutcome::Existing(existing.name.clone());
    }
    if let Some(owner) = index.by_name(&name) {
        return InsertOutcome::NameTaken {
            name,
            owner: owner.path.clone(),
        };
    }
    let id = index.next_id();
    index.repos.push(RepoRecord {
        id,
        name: name.clone(),
        path: observation.path.clone(),
        groups: Vec::new(),
        remote: observation.remote.clone(),
        git_dir: observation.git_dir.clone(),
        common_git_dir: observation.common_git_dir.clone(),
        availability: observation.availability,
        last_seen_at: Some(observation.at.clone()),
        snapshot: observation.snapshot.clone(),
    });
    InsertOutcome::Added(name)
}

/// 登记名称：优先取远端仓库名，否则取目录名。
fn derive_name(observation: &Observation) -> String {
    observation
        .remote
        .as_deref()
        .and_then(git::name_from_remote)
        .or_else(|| {
            observation
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| observation.path.display().to_string())
}

// ---------------------------------------------------------------------------
// 读取：list / info / status / refresh
// ---------------------------------------------------------------------------

/// 列出已登记仓库，只读索引。
fn list(store: &Store, group: Option<&str>, json: bool) -> Result<()> {
    let index = store.load()?;
    let repos: Vec<&RepoRecord> = match group {
        Some(name) => {
            let group = index
                .group(name)
                .with_context(|| format!("未知分组 {name}；用 repo group list 查看已有分组"))?;
            group
                .members
                .iter()
                .filter_map(|id| index.position_of(id))
                .map(|position| &index.repos[position])
                .collect()
        }
        None => index.repos.iter().collect(),
    };

    if json {
        return print_json(&json!({
            "version": index.version,
            "repos": repos.iter().map(|repo| json!({
                "id": repo.id,
                "name": repo.name,
                "path": repo.path.display().to_string(),
                "groups": repo.groups,
                "remote": repo.remote,
                "availability": repo.availability,
                "last_seen_at": repo.last_seen_at,
                "snapshot": repo.snapshot,
            })).collect::<Vec<_>>(),
        }));
    }

    if repos.is_empty() {
        println!("没有符合条件的仓库；用 repo scan、repo add 或 repo clone 登记");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = repos
        .iter()
        .map(|repo| {
            vec![
                repo.name.clone(),
                repo.path.display().to_string(),
                if repo.groups.is_empty() {
                    "-".to_string()
                } else {
                    repo.groups.join(",")
                },
                repo.remote.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    print_table(&["名称", "路径", "分组", "远端"], &rows);
    Ok(())
}

/// 单个仓库的实时信息，并回写快照。
fn info(store: &Store, target: &str, json: bool) -> Result<()> {
    let index = store.load()?;
    let target = select::resolve(&index, target)?;
    let observation = git::observe(&target.path, target.id.clone());
    persist(store, std::slice::from_ref(&observation))?;

    let record = target
        .id
        .as_deref()
        .and_then(|id| index.position_of(id))
        .map(|position| &index.repos[position]);

    if json {
        print_json(&json!({
            "id": target.id,
            "name": target.name,
            "path": observation.path.display().to_string(),
            "groups": record.map(|record| record.groups.clone()).unwrap_or_default(),
            "availability": observation.availability,
            "git_dir": observation.git_dir,
            "common_git_dir": observation.common_git_dir,
            "snapshot": &observation.snapshot,
            "error": &observation.error,
        }))?;
    } else {
        println!("名称:     {}{}", target.name, id_suffix(&target));
        println!("路径:     {}", observation.path.display());
        if let Some(record) = record
            && !record.groups.is_empty()
        {
            println!("分组:     {}", record.groups.join(", "));
        }
        println!("可用性:   {}", observation.availability.label());
        match &observation.snapshot {
            Some(snapshot) => {
                println!("分支:     {}", snapshot.branch.as_deref().unwrap_or("-"));
                println!("提交:     {}", snapshot.head.as_deref().unwrap_or("-"));
                println!("改动:     {}", if snapshot.dirty { "有" } else { "无" });
                let remotes = if snapshot.remotes.is_empty() {
                    "-".to_string()
                } else {
                    snapshot
                        .remotes
                        .iter()
                        .map(|(name, url)| format!("{name} {url}"))
                        .collect::<Vec<_>>()
                        .join("；")
                };
                println!("远端:     {remotes}");
                match &snapshot.upstream {
                    Some(upstream) => println!(
                        "upstream: {upstream}（领先 {}，落后 {}）",
                        snapshot
                            .ahead
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "-".into()),
                        snapshot
                            .behind
                            .map(|n| n.to_string())
                            .unwrap_or_else(|| "-".into())
                    ),
                    None => println!("upstream: -"),
                }
                println!(
                    "采集:     {}{}",
                    snapshot.observed_at.as_deref().unwrap_or("未采集"),
                    if snapshot.stale {
                        "（已过期）"
                    } else {
                        ""
                    }
                );
            }
            None => println!("状态:     未采集"),
        }
        if let Some(error) = &observation.error {
            println!("错误:     {error}");
        }
    }

    match observation.availability {
        Availability::Available => Ok(()),
        other => bail!(
            "{} 当前{}: {}",
            observation.path.display(),
            other.label(),
            observation.error.as_deref().unwrap_or("未知原因")
        ),
    }
}

/// 状态行：文本与 JSON 共用同一份字段。
#[derive(Debug, Clone, Serialize)]
struct StatusRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    path: String,
    groups: Vec<String>,
    availability: Availability,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    /// `None` 表示未知（未采集或不可访问），不能当成干净。
    #[serde(skip_serializing_if = "Option::is_none")]
    dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ahead: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    behind: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<String>,
    stale: bool,
    cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl StatusRow {
    /// 状态列：不可访问时直接显示可用性。
    fn state_text(&self) -> String {
        if !self.availability.is_available() {
            return self.availability.label().to_string();
        }
        match self.dirty {
            Some(true) => "有改动".to_string(),
            Some(false) => "干净".to_string(),
            None => "未采集".to_string(),
        }
    }

    /// 领先/落后列；未知用 `-`，不显示成 0。
    fn tracking_text(&self) -> String {
        match (self.ahead, self.behind) {
            (None, None) => "-".to_string(),
            (ahead, behind) => format!(
                "{}/{}",
                ahead.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                behind.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
            ),
        }
    }

    /// 采集时间列。
    fn observed_text(&self) -> String {
        match &self.observed_at {
            None => "未采集".to_string(),
            Some(at) if self.stale => format!("{at}（过期）"),
            Some(at) => at.clone(),
        }
    }
}

/// 查看仓库状态；`cached` 只读快照，不调用 Git。
fn status(
    store: &Store,
    target_args: &TargetArgs,
    dirty_only: bool,
    cached: bool,
    jobs: usize,
    json: bool,
) -> Result<()> {
    if jobs == 0 {
        bail!("--jobs 必须大于 0");
    }
    let index = store.load()?;
    let targets = select::select(&index, target_args)?;
    if targets.is_empty() {
        return empty_scope(json);
    }

    let mut rows = Vec::new();
    if cached {
        for target in &targets {
            rows.push(cached_row(&index, target));
        }
    } else {
        let reports = exec::run(&targets, jobs, false, |target| {
            Ok(git::observe(&target.path, target.id.clone()))
        });
        let mut observations = Vec::new();
        for report in &reports {
            let target = &targets[report.index];
            match (&report.output, &report.error) {
                (Some(observation), _) => {
                    rows.push(live_row(&index, target, observation));
                    observations.push(observation.clone());
                }
                (None, error) => rows.push(unavailable_row(&index, target, error)),
            }
        }
        persist(store, &observations)?;
    }

    let total = rows.len();
    let unknown_dirty = rows.iter().filter(|row| row.dirty.is_none()).count();
    let unavailable = rows
        .iter()
        .filter(|row| !row.availability.is_available())
        .count();
    if dirty_only {
        rows.retain(|row| row.dirty == Some(true));
    }
    let dirty = rows.iter().filter(|row| row.dirty == Some(true)).count();

    if json {
        let listed = rows.len();
        return print_json(&json!({
            "cached": cached,
            "repos": rows,
            "summary": {
                "total": total,
                "listed": listed,
                "dirty": dirty,
                "unknown": unknown_dirty,
                "unavailable": unavailable,
            },
        }));
    }

    if rows.is_empty() {
        println!("0 个目标");
    } else {
        let body: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.name.clone(),
                    row.branch.clone().unwrap_or_else(|| "-".to_string()),
                    row.state_text(),
                    row.tracking_text(),
                    row.observed_text(),
                    row.path.clone(),
                ]
            })
            .collect();
        print_table(
            &["名称", "分支", "状态", "领先/落后", "采集", "路径"],
            &body,
        );
    }
    println!(
        "共 {total} 个目标：干净 {}，有改动 {dirty}，未知 {unknown_dirty}，不可用 {unavailable}",
        rows.iter().filter(|row| row.dirty == Some(false)).count()
    );
    if dirty_only && unknown_dirty > 0 {
        println!("另有 {unknown_dirty} 个仓库没有可用状态，未计入 --dirty");
    }
    Ok(())
}

/// 实时采集得到的状态行。
fn live_row(index: &Index, target: &Target, observation: &Observation) -> StatusRow {
    let record = target
        .id
        .as_deref()
        .and_then(|id| index.position_of(id))
        .map(|position| &index.repos[position]);
    StatusRow {
        id: target.id.clone(),
        name: target.name.clone(),
        path: target.display_path(),
        groups: record
            .map(|record| record.groups.clone())
            .unwrap_or_default(),
        availability: observation.availability,
        branch: observation
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.branch.clone()),
        dirty: observation.snapshot.as_ref().map(|snapshot| snapshot.dirty),
        upstream: observation
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.upstream.clone()),
        ahead: observation
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.ahead),
        behind: observation
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.behind),
        observed_at: observation
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.observed_at.clone()),
        stale: false,
        cached: false,
        error: observation.error.clone(),
    }
}

/// 采集失败（够不到 Git 或无法运行）时的行。
fn unavailable_row(index: &Index, target: &Target, error: &Option<String>) -> StatusRow {
    let record = target
        .id
        .as_deref()
        .and_then(|id| index.position_of(id))
        .map(|position| &index.repos[position]);
    StatusRow {
        id: target.id.clone(),
        name: target.name.clone(),
        path: target.display_path(),
        groups: record
            .map(|record| record.groups.clone())
            .unwrap_or_default(),
        availability: Availability::Unknown,
        branch: None,
        dirty: None,
        upstream: None,
        ahead: None,
        behind: None,
        observed_at: None,
        stale: false,
        cached: false,
        error: error.clone(),
    }
}

/// 只读快照得到的状态行。
fn cached_row(index: &Index, target: &Target) -> StatusRow {
    let record = match &target.id {
        Some(id) => index.position_of(id).map(|position| &index.repos[position]),
        None => index.by_path(&target.path),
    };
    let snapshot = record.and_then(|record| record.snapshot.as_ref());
    StatusRow {
        id: target.id.clone(),
        name: target.name.clone(),
        path: target.display_path(),
        groups: record
            .map(|record| record.groups.clone())
            .unwrap_or_default(),
        availability: record
            .map(|record| record.availability)
            .unwrap_or(Availability::Unknown),
        branch: snapshot.and_then(|snapshot| snapshot.branch.clone()),
        dirty: snapshot.map(|snapshot| snapshot.dirty),
        upstream: snapshot.and_then(|snapshot| snapshot.upstream.clone()),
        ahead: snapshot.and_then(|snapshot| snapshot.ahead),
        behind: snapshot.and_then(|snapshot| snapshot.behind),
        observed_at: snapshot.and_then(|snapshot| snapshot.observed_at.clone()),
        stale: snapshot.map(|snapshot| snapshot.stale).unwrap_or(false),
        cached: true,
        error: None,
    }
}

/// 刷新已登记仓库：不 fetch、不递归、不发现新仓库。
fn refresh(store: &Store, targets: &[String], jobs: usize, json: bool) -> Result<()> {
    if jobs == 0 {
        bail!("--jobs 必须大于 0");
    }
    let index = store.load()?;
    let selected: Vec<Target> = if targets.is_empty() {
        index.repos.iter().map(Target::from_record).collect()
    } else {
        targets
            .iter()
            .map(|text| select::resolve(&index, text))
            .collect::<Result<Vec<_>>>()?
    };
    if selected.is_empty() {
        return empty_scope(json);
    }

    let reports = exec::run(&selected, jobs, false, |target| {
        Ok(git::observe(&target.path, target.id.clone()))
    });
    let mut observations = Vec::new();
    let mut refreshed: Vec<(String, String, String)> = Vec::new();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for report in &reports {
        if report.skipped {
            continue;
        }
        let target = &selected[report.index];
        match (&report.output, &report.error) {
            (Some(observation), _) => {
                rows.push(json!({
                    "id": target.id,
                    "name": target.name,
                    "path": observation.path.display().to_string(),
                    "availability": observation.availability,
                    "snapshot": &observation.snapshot,
                    "error": &observation.error,
                }));
                refreshed.push((
                    target.name.clone(),
                    observation.availability.label().to_string(),
                    observation.path.display().to_string(),
                ));
                observations.push(observation.clone());
            }
            (None, error) => failures.push(json!({
                "name": target.name,
                "path": target.display_path(),
                "error": error,
            })),
        }
    }
    persist(store, &observations)?;

    if json {
        print_json(&json!({
            "repos": rows,
            "failures": failures,
            "summary": { "total": selected.len(), "ok": rows.len(), "failed": failures.len() },
        }))?;
    } else {
        for (name, availability, path) in &refreshed {
            println!("{name}  {availability}  {path}");
        }
        println!(
            "共 {} 个仓库：成功 {}，失败 {}",
            selected.len(),
            rows.len(),
            failures.len()
        );
    }
    if !failures.is_empty() {
        bail!("{} 个仓库刷新失败", failures.len());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 写入：批量 Git 命令
// ---------------------------------------------------------------------------

/// 批量命令的动作。
enum Action {
    Fetch,
    Pull,
    Diff { staged: bool },
    Branch,
    Switch(String),
    Start(String),
    Push,
    Foreach(Vec<String>),
}

impl Action {
    /// 展示用的动作描述。
    fn describe(&self) -> String {
        match self {
            Action::Fetch => "fetch".to_string(),
            Action::Pull => "pull".to_string(),
            Action::Diff { staged: false } => "diff".to_string(),
            Action::Diff { staged: true } => "diff --staged".to_string(),
            Action::Branch => "branch".to_string(),
            Action::Switch(branch) => format!("switch {branch}"),
            Action::Start(branch) => format!("start {branch}"),
            Action::Push => "push".to_string(),
            Action::Foreach(command) => format!("foreach {}", command.join(" ")),
        }
    }

    /// 是否会使已有快照失效。
    fn invalidates(&self) -> bool {
        !matches!(self, Action::Diff { .. } | Action::Branch)
    }

    /// 是否可能修改工作区或引用。
    fn mutates(&self) -> bool {
        !matches!(self, Action::Diff { .. } | Action::Branch)
    }

    /// 在单个工作目录执行。
    fn execute(&self, path: &Path) -> Result<String> {
        match self {
            Action::Fetch => {
                git::run(path, &["fetch", "--all"])?;
                Ok("已获取远端更新".to_string())
            }
            Action::Pull => pull(path),
            Action::Diff { staged } => {
                let args: &[&str] = if *staged {
                    &["diff", "--staged"]
                } else {
                    &["diff"]
                };
                let output = git::run(path, args)?;
                Ok(if output.is_empty() {
                    "（无差异）".to_string()
                } else {
                    output
                })
            }
            Action::Branch => git::run(
                path,
                &["branch", "--no-color", "--format=%(HEAD) %(refname:short)"],
            ),
            Action::Switch(branch) => {
                git::run(path, &["switch", branch])?;
                Ok(format!("已切换到 {branch}"))
            }
            Action::Start(branch) => {
                git::run(path, &["switch", "-c", branch])?;
                Ok(format!("已创建并切换到 {branch}"))
            }
            Action::Push => {
                git::run(path, &["push"])?;
                Ok("已推送当前分支".to_string())
            }
            Action::Foreach(command) => foreach(path, command),
        }
    }
}

/// 仅快进更新：工作区不干净或无 upstream 时拒绝，分叉时由 Git 拒绝合并。
///
/// 领先/落后在 pull 前只反映本地跟踪引用（可能已过期），因此不据此提前判定
/// “已是最新”；是否可快进交给 `git pull --ff-only` 决定，失败后再用刷新过的
/// 引用区分“分叉”与其他错误。
fn pull(path: &Path) -> Result<String> {
    let snapshot = git::snapshot(path)?;
    if snapshot.dirty {
        bail!("工作区有未提交改动，拒绝非快进更新；请先提交或暂存");
    }
    let upstream = snapshot
        .upstream
        .clone()
        .context("当前分支没有 upstream；请先 git branch --set-upstream-to")?;

    let output = git::capture(Some(path), &["pull", "--ff-only"])?;
    if !output.success {
        if let Ok(after) = git::snapshot(path) {
            let ahead = after.ahead.unwrap_or(0);
            let behind = after.behind.unwrap_or(0);
            if ahead > 0 && behind > 0 {
                bail!("与 {upstream} 已分叉（领先 {ahead}，落后 {behind}），拒绝非快进更新");
            }
        }
        let mut message = String::from("git pull --ff-only 失败");
        if !output.stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&output.stderr);
        }
        bail!(message);
    }
    Ok(if output.stdout.is_empty() {
        format!("已更新（{upstream}）")
    } else {
        output.stdout
    })
}

/// 在工作目录直接运行程序，默认不经过 shell。
fn foreach(path: &Path, command: &[String]) -> Result<String> {
    let (program, args) = command.split_first().context("foreach 需要一个程序")?;
    let output = Command::new(program)
        .args(args)
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| anyhow!("无法运行 {program}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_string();
    if !output.status.success() {
        let mut message = format!("{program} 退出码 {:?}", output.status.code());
        if !stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&stderr);
        }
        if !stdout.is_empty() {
            message.push('\n');
            message.push_str(&stdout);
        }
        bail!(message);
    }
    Ok(match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    })
}

/// 批量执行入口：固定目标、逐仓库报告、部分失败非零退出。
fn batch_action(store: &Store, args: BatchArgs, action: Action) -> Result<()> {
    if args.jobs == 0 {
        bail!("--jobs 必须大于 0");
    }
    let index = store.load()?;
    let targets = select::select(&index, &args.select)?;
    if targets.is_empty() {
        return empty_scope(args.json);
    }

    if args.dry_run {
        let kind = if action.mutates() {
            "写操作"
        } else {
            "只读"
        };
        if args.json {
            print_json(&json!({
                "dry_run": true,
                "action": action.describe(),
                "mutates": action.mutates(),
                "repos": targets.iter().map(|target| json!({
                    "id": target.id,
                    "name": target.name,
                    "path": target.display_path(),
                })).collect::<Vec<_>>(),
            }))?;
        } else {
            println!(
                "计划执行 {}（{kind}，共 {} 个目标）：",
                action.describe(),
                targets.len()
            );
            for target in &targets {
                println!("  {}  {}", target.name, target.display_path());
            }
        }
        return Ok(());
    }

    let reports = exec::run(&targets, args.jobs, args.fail_fast, |target| {
        // 写操作始终实时验证目标，不依据旧快照判断工作区状态。
        if action.mutates() {
            let observation = git::probe(&target.path, target.id.clone());
            if !observation.availability.is_available() {
                bail!(
                    "仓库当前{}：{}",
                    observation.availability.label(),
                    observation.error.as_deref().unwrap_or("无法验证")
                );
            }
        }
        action.execute(&target.path)
    });

    let mut failures = 0usize;
    let mut skipped = 0usize;
    let mut results = Vec::new();
    for report in &reports {
        let target = &targets[report.index];
        let output = report.output.as_deref();
        results.push(json!({
            "id": target.id,
            "name": target.name,
            "path": target.display_path(),
            "ok": report.ok(),
            "skipped": report.skipped,
            "output": output,
            "error": report.error,
        }));
        if report.skipped {
            skipped += 1;
        } else if !report.ok() {
            failures += 1;
        }
    }

    if args.json {
        print_json(&json!({
            "action": action.describe(),
            "results": results,
            "summary": {
                "total": targets.len(),
                "ok": targets.len() - failures - skipped,
                "failed": failures,
                "skipped": skipped,
            },
        }))?;
    } else {
        for report in &reports {
            let target = &targets[report.index];
            if report.skipped {
                println!("[跳过] {}（--fail-fast 已停止调度）", target.name);
            } else if let Some(error) = &report.error {
                eprintln!("[失败] {}: {error}", target.name);
            } else {
                println!("== {} ==", target.name);
                if let Some(output) = &report.output
                    && !output.is_empty()
                {
                    println!("{output}");
                }
            }
        }
        println!(
            "{}：共 {} 个目标，成功 {}，失败 {failures}，跳过 {skipped}",
            action.describe(),
            targets.len(),
            targets.len() - failures - skipped
        );
    }

    if action.invalidates() {
        invalidate(store, &targets)?;
    }
    if failures > 0 {
        bail!("{failures} 个仓库执行失败");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 索引维护与其他命令
// ---------------------------------------------------------------------------

/// 更新仓库位置，保留 ID、名称和分组。
fn relocate(store: &Store, target: &str, path: PathBuf) -> Result<()> {
    let index = store.load()?;
    let resolved = select::resolve(&index, target)?;
    let id = resolved
        .id
        .clone()
        .context("该路径未登记，先 repo add 再 relocate")?;
    let new_path = util::normalize(&path);
    if new_path == resolved.path {
        println!("路径未变化: {}", new_path.display());
        return Ok(());
    }
    let observation = git::probe(&new_path, Some(id.clone()));
    if !observation.availability.is_available() {
        bail!(
            "{} 不是有效 Git 仓库: {}",
            new_path.display(),
            observation.error.as_deref().unwrap_or("未知原因")
        );
    }
    let name = store.update(|index| {
        if let Some(other) = index.by_path(&new_path)
            && other.id != id
        {
            bail!("{} 已被 {} 登记", new_path.display(), other.name);
        }
        let position = index.position_of(&id).context("仓库已被其他进程移除")?;
        let record = &mut index.repos[position];
        record.path = new_path.clone();
        record.git_dir = observation.git_dir.clone();
        record.common_git_dir = observation.common_git_dir.clone();
        record.remote = observation.remote.clone().or(record.remote.clone());
        record.availability = Availability::Available;
        record.last_seen_at = Some(util::now_rfc3339());
        record.snapshot = None;
        Ok(record.name.clone())
    })?;
    println!("已把 {name} 移到 {}", new_path.display());
    Ok(())
}

/// 取消登记，保留本地文件。
fn remove(store: &Store, target: &str) -> Result<()> {
    let index = store.load()?;
    let resolved = select::resolve(&index, target)?;
    let id = resolved.id.context("该路径未登记，无需取消")?;
    let (name, path) = store.update(|index| {
        let position = index.position_of(&id).context("仓库已被其他进程移除")?;
        let record = index.repos[position].clone();
        index.remove_repo(&id);
        Ok((record.name, record.path))
    })?;
    println!("已取消登记 {name}（本地文件保留在 {}）", path.display());
    Ok(())
}

/// 分组管理。
fn group(store: &Store, command: GroupCommand) -> Result<()> {
    match command {
        GroupCommand::Create { name } => {
            store.update(|index| {
                if index.group(&name).is_some() {
                    bail!("分组 {name} 已存在");
                }
                index.groups.push(Group {
                    name: name.clone(),
                    members: Vec::new(),
                });
                Ok(())
            })?;
            println!("已创建分组 {name}");
        }
        GroupCommand::Add { name, repos } => {
            let index = store.load()?;
            let ids = member_ids(&index, &repos)?;
            let count = store.update(|index| {
                let position = index
                    .groups
                    .iter()
                    .position(|group| group.name == name)
                    .with_context(|| format!("未知分组 {name}；先 repo group create {name}"))?;
                let group = &mut index.groups[position];
                for id in &ids {
                    if !group.members.contains(id) {
                        group.members.push(id.clone());
                    }
                }
                Ok(group.members.len())
            })?;
            println!("已把 {} 个仓库加入 {name}（共 {count} 个成员）", ids.len());
        }
        GroupCommand::List { json } => {
            let index = store.load()?;
            if json {
                return print_json(&json!({
                    "groups": index.groups.iter().map(|group| json!({
                        "name": group.name,
                        "members": group.members.iter()
                            .filter_map(|id| index.position_of(id))
                            .map(|position| json!({
                                "id": index.repos[position].id,
                                "name": index.repos[position].name,
                                "path": index.repos[position].path.display().to_string(),
                            }))
                            .collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                }));
            }
            if index.groups.is_empty() {
                println!("没有分组；用 repo group create <名称> 创建");
                return Ok(());
            }
            for group in &index.groups {
                println!("{}", group.name);
                for id in &group.members {
                    match index.position_of(id) {
                        Some(position) => println!("  - {}", index.repos[position].name),
                        None => println!("  - （已移除的仓库 {id}）"),
                    }
                }
            }
        }
        GroupCommand::Remove { name, repos } => {
            let index = store.load()?;
            let ids = member_ids(&index, &repos)?;
            let remaining = store.update(|index| {
                let position = index
                    .groups
                    .iter()
                    .position(|group| group.name == name)
                    .with_context(|| format!("未知分组 {name}"))?;
                let group = &mut index.groups[position];
                group.members.retain(|id| !ids.contains(id));
                Ok(group.members.len())
            })?;
            println!(
                "已从 {name} 移除 {} 个仓库（剩余 {remaining} 个）",
                ids.len()
            );
        }
        GroupCommand::Delete { name } => {
            store.update(|index| {
                let before = index.groups.len();
                index.groups.retain(|group| group.name != name);
                if index.groups.len() == before {
                    bail!("未知分组 {name}");
                }
                Ok(())
            })?;
            println!("已删除分组 {name}（仓库登记保留）");
        }
    }
    Ok(())
}

/// 把名称/路径解析成已登记仓库 ID。
fn member_ids(index: &Index, repos: &[String]) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for text in repos {
        let target = select::resolve(index, text)?;
        let id = target
            .id
            .context(format!("{text} 未登记；先 repo add 再分组"))?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// 用配置的编辑器打开目录。
///
/// 解析顺序：`editor` 配置 > `EDITOR`/`VISUAL` 环境变量 > 平台默认打开方式。
/// 配置值支持带参数（如 `code --wait`），目录路径附加在参数后面。
fn open(store: &Store, settings: &Settings, target: Option<&str>) -> Result<()> {
    let index = store.load()?;
    let resolved = match target {
        Some(text) => select::resolve(&index, text)?,
        None => select::current(&index)?,
    };
    open_in_editor(settings, &resolved.path)
}

/// 用配置的编辑器或系统默认方式打开目录。
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

// ---------------------------------------------------------------------------
// 公共小工具
// ---------------------------------------------------------------------------

/// 把采集结果合并回索引；没有已登记记录时不写盘。
fn persist(store: &Store, observations: &[Observation]) -> Result<()> {
    if !observations
        .iter()
        .any(|observation| observation.id.is_some())
    {
        return Ok(());
    }
    store.update(|index| {
        for observation in observations {
            if observation.id.is_some() {
                merge_observation(index, observation);
            }
        }
        Ok(())
    })
}

/// 使受影响仓库的快照失效，包括失败后可能已发生部分修改的情况。
fn invalidate(store: &Store, targets: &[Target]) -> Result<()> {
    if !targets.iter().any(|target| target.id.is_some()) {
        return Ok(());
    }
    store.update(|index| {
        for target in targets {
            let Some(id) = &target.id else { continue };
            let Some(position) = index.position_of(id) else {
                continue;
            };
            if let Some(snapshot) = &mut index.repos[position].snapshot {
                snapshot.stale = true;
            }
        }
        Ok(())
    })
}

/// 空范围不算失败：输出 0 个目标，不回退到其他范围。
fn empty_scope(json_output: bool) -> Result<()> {
    if json_output {
        print_json(&json!({"repos": [], "summary": {"total": 0}}))
    } else {
        println!("0 个目标");
        Ok(())
    }
}

/// 名称后缀，`repo info` 用。
fn id_suffix(target: &Target) -> String {
    match &target.id {
        Some(id) => format!("（{id}）"),
        None => "（未登记）".to_string(),
    }
}

/// 输出 JSON；失败时不会混入进度文本。
fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// 按显示宽度对齐的简单表格，最后一列不补空格。
fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|text| util::display_width(text))
        .collect();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            if column < widths.len() {
                widths[column] = widths[column].max(util::display_width(cell));
            }
        }
    }
    println!("{}", render_row(headers, &widths));
    let separators: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    println!("{}", render_row(&separators, &widths));
    for row in rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        println!("{}", render_row(&cells, &widths));
    }
}

/// 渲染一行：除最后一列外按宽度补空格。
fn render_row<S: AsRef<str>>(cells: &[S], widths: &[usize]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        let text = cell.as_ref();
        if index + 1 == cells.len() {
            line.push_str(text);
        } else {
            line.push_str(&util::pad(text, widths[index]));
        }
    }
    line.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_dedupes_by_path_and_rejects_name_clash() {
        let mut index = Index::default();
        let mut first = Observation::probe(PathBuf::from("/repos/a"), None);
        first.availability = Availability::Available;
        first.at = util::now_rfc3339();
        assert!(matches!(
            insert_record(&mut index, "a".into(), &first),
            InsertOutcome::Added(_)
        ));

        let mut again = Observation::probe(PathBuf::from("/repos/a"), None);
        again.availability = Availability::Available;
        assert!(matches!(
            insert_record(&mut index, "other".into(), &again),
            InsertOutcome::Existing(_)
        ));

        let mut clash = Observation::probe(PathBuf::from("/repos/b"), None);
        clash.availability = Availability::Available;
        assert!(matches!(
            insert_record(&mut index, "a".into(), &clash),
            InsertOutcome::NameTaken { .. }
        ));
        assert_eq!(index.repos.len(), 1);
    }

    #[test]
    fn derived_name_prefers_remote_then_directory() {
        let mut observation = Observation::probe(PathBuf::from("/work/demo"), None);
        assert_eq!(derive_name(&observation), "demo");
        observation.remote = Some("https://example.com/team/renamed.git".into());
        assert_eq!(derive_name(&observation), "renamed");
    }

    #[test]
    fn render_row_pads_only_between_columns() {
        let widths = [4, 6];
        assert_eq!(render_row(&["名称", "main"], &widths), "名称  main");
        assert_eq!(render_row(&["ab", "cd"], &widths), "ab    cd");
    }
}
