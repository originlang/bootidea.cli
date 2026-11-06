//! 仓库索引：数据结构、版本迁移、锁与原子写入。
//!
//! 索引保存在数据根目录的 `repos.json`，当前版本为 2。它同时记录扫描根目录
//! 规则、分组、仓库登记和状态快照。读取不写盘；写入在独立锁文件保护下
//! “重新读取 → 修改 → 临时文件 → 原子替换”，耗时 Git 操作不持有锁。

use crate::commands::repo::util::{normalize, now_rfc3339};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 当前索引版本。
pub const INDEX_VERSION: u32 = 2;

/// 等锁的最长时间；超时说明有进程长时间占用，直接报错而不是无限等待。
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// 仓库可访问状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    /// 尚未验证。
    #[default]
    Unknown,
    /// 路径存在且是有效 Git 仓库。
    Available,
    /// 登记路径不存在（移动、删除或磁盘未挂载）。
    Missing,
    /// 路径存在但无法访问（权限变化等）。
    Inaccessible,
    /// 路径存在但不再是有效 Git 仓库。
    Invalid,
}

impl Availability {
    /// 中文展示名。
    pub fn label(self) -> &'static str {
        match self {
            Availability::Unknown => "未知",
            Availability::Available => "可用",
            Availability::Missing => "缺失",
            Availability::Inaccessible => "不可访问",
            Availability::Invalid => "无效",
        }
    }

    /// 是否确认可访问。
    pub fn is_available(self) -> bool {
        self == Availability::Available
    }
}

/// 扫描根目录及其规则，显式 `scan` 时更新。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRoot {
    /// 规范化后的根目录。
    pub path: PathBuf,
    /// 扫描深度，根目录为第 0 层。
    pub depth: u8,
    /// 排除规则，匹配相对根目录的路径。
    pub exclude: Vec<String>,
    /// 是否继续搜索仓库内部的嵌套仓库。
    pub nested: bool,
    /// 上次成功扫描时间（UTC RFC 3339）。
    pub last_scanned_at: Option<String>,
}

/// 仓库分组，成员使用仓库稳定 ID 关联。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// 分组名，唯一。
    pub name: String,
    /// 成员仓库 ID。
    pub members: Vec<String>,
}

/// 状态快照，由 `refresh` 或实时 `status / info` 采集。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// 远端名 → 地址，地址中的认证信息已剔除。
    #[serde(default)]
    pub remotes: BTreeMap<String, String>,
    /// 当前分支；detached HEAD 时为空。
    #[serde(default)]
    pub branch: Option<String>,
    /// 当前提交短哈希；没有提交时为空。
    #[serde(default)]
    pub head: Option<String>,
    /// 工作区是否有改动（含暂存、未暂存和未跟踪，忽略文件不计）。
    #[serde(default)]
    pub dirty: bool,
    /// upstream 的完整引用名。
    #[serde(default)]
    pub upstream: Option<String>,
    /// 领先 upstream 的提交数；没有 upstream 时为空。
    #[serde(default)]
    pub ahead: Option<i64>,
    /// 落后 upstream 的提交数；没有 upstream 时为空。
    #[serde(default)]
    pub behind: Option<i64>,
    /// 采集时间（UTC RFC 3339）。
    #[serde(default)]
    pub observed_at: Option<String>,
    /// CLI 已知操作已使快照失效，需要重新采集。
    #[serde(default)]
    pub stale: bool,
}

/// 一条仓库登记。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRecord {
    /// 稳定 ID，登记时生成，迁移与改名保持不变。
    pub id: String,
    /// 唯一登记名称。
    pub name: String,
    /// 规范化后的工作目录。
    pub path: PathBuf,
    /// 所属分组名；保存前由 [`Index::sync_groups`] 从分组定义重建。
    #[serde(default)]
    pub groups: Vec<String>,
    /// 缓存的 origin 地址（认证信息已剔除）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Git 目录；worktree 与 submodule 各有自己的 Git 目录。
    #[serde(default)]
    pub git_dir: Option<PathBuf>,
    /// 共同 Git 目录；共享它的 worktree 在执行时串行调度。
    #[serde(default)]
    pub common_git_dir: Option<PathBuf>,
    /// 可访问状态。
    #[serde(default)]
    pub availability: Availability,
    /// 最后一次确认可访问的时间（UTC RFC 3339）。
    #[serde(default)]
    pub last_seen_at: Option<String>,
    /// 状态快照；`null` 表示尚未采集。
    #[serde(default)]
    pub snapshot: Option<Snapshot>,
}

/// 版本化索引文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// 格式版本。
    pub version: u32,
    /// 已知扫描根目录。
    #[serde(default)]
    pub roots: Vec<ScanRoot>,
    /// 分组。
    #[serde(default)]
    pub groups: Vec<Group>,
    /// 仓库登记。
    #[serde(default)]
    pub repos: Vec<RepoRecord>,
    /// 读入时是 v1 格式，下次写入前需要备份旧文件。
    #[serde(skip)]
    pub legacy: bool,
}

impl Default for Index {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            roots: Vec::new(),
            groups: Vec::new(),
            repos: Vec::new(),
            legacy: false,
        }
    }
}

impl Index {
    /// 生成一个未被占用的稳定 ID。
    pub fn next_id(&self) -> String {
        let mut used: Vec<u32> = self
            .repos
            .iter()
            .filter_map(|repo| repo.id.strip_prefix("repo_"))
            .filter_map(|suffix| suffix.parse::<u32>().ok())
            .collect();
        used.sort_unstable();
        let mut next = 1u32;
        for id in used {
            if id >= next {
                next = id + 1;
            }
        }
        format!("repo_{next:02}")
    }

    /// 按规范化路径查记录。
    pub fn by_path(&self, path: &Path) -> Option<&RepoRecord> {
        self.repos.iter().find(|repo| repo.path == path)
    }

    /// 按名称查记录。
    pub fn by_name(&self, name: &str) -> Option<&RepoRecord> {
        self.repos.iter().find(|repo| repo.name == name)
    }

    /// 按 ID 定位下标。
    pub fn position_of(&self, id: &str) -> Option<usize> {
        self.repos.iter().position(|repo| repo.id == id)
    }

    /// 分组名对应的成员 ID。
    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.name == name)
    }

    /// 由分组定义重建每条记录的 `groups`，并去掉已不存在的分组名。
    ///
    /// 分组成员关系以 [`Group::members`] 为唯一事实来源：手写文件里记录的
    /// `repos[].groups` 会被覆盖，避免两处不一致。
    pub fn sync_groups(&mut self) {
        let valid: Vec<String> = self.repos.iter().map(|repo| repo.id.clone()).collect();
        for group in &mut self.groups {
            let mut seen: Vec<String> = Vec::new();
            group.members.retain(|id| {
                let keep = valid.contains(id) && !seen.contains(id);
                if keep {
                    seen.push(id.clone());
                }
                keep
            });
        }
        let memberships: Vec<(String, String)> = self
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .members
                    .iter()
                    .map(move |id| (group.name.clone(), id.clone()))
            })
            .collect();
        for repo in &mut self.repos {
            repo.groups = memberships
                .iter()
                .filter(|(_, id)| id == &repo.id)
                .map(|(name, _)| name.clone())
                .collect();
        }
    }

    /// 删除仓库登记并清理分组中的成员关系。
    pub fn remove_repo(&mut self, id: &str) {
        self.repos.retain(|repo| repo.id != id);
        for group in &mut self.groups {
            group.members.retain(|member| member != id);
        }
    }
}

/// v1 登记表：只有名称、路径和远端。
#[derive(Debug, Deserialize)]
struct LegacyIndex {
    #[serde(default)]
    repos: Vec<LegacyRepo>,
}

#[derive(Debug, Deserialize)]
struct LegacyRepo {
    name: String,
    path: PathBuf,
    #[serde(default)]
    remote: Option<String>,
}

/// 索引文件的读写入口。
pub struct Store {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Store {
    /// `root` 是数据根目录，索引固定为其中的 `repos.json`。
    pub fn new(root: &Path) -> Self {
        Self {
            path: root.join("repos.json"),
            lock_path: root.join("repos.lock"),
        }
    }

    /// 索引文件位置，测试断言与提示使用。
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 只读加载；文件不存在视为空索引。
    pub fn load(&self) -> Result<Index> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Index::default());
            }
            Err(error) => return Err(error).context(format!("无法读取 {}", self.path.display())),
        };
        if raw.trim().is_empty() {
            return Ok(Index::default());
        }
        let value: serde_json::Value = serde_json::from_str(&raw)
            .with_context(|| format!("索引文件不是合法 JSON: {}", self.path.display()))?;

        match value.get("version") {
            Some(serde_json::Value::Number(number)) if number.as_u64() == Some(2) => {
                let mut index: Index = serde_json::from_value(value)
                    .with_context(|| format!("索引文件损坏: {}", self.path.display()))?;
                index.sync_groups();
                Ok(index)
            }
            Some(other) => bail!(
                "索引版本 {} 不受支持（支持 {}）；请升级 bootidea 或手动处理 {}",
                other,
                INDEX_VERSION,
                self.path.display()
            ),
            None => {
                let legacy: LegacyIndex = serde_json::from_value(value).with_context(|| {
                    format!(
                        "v1 登记表损坏: {}；修复或移走该文件后重试",
                        self.path.display()
                    )
                })?;
                let mut index = Index {
                    legacy: true,
                    ..Index::default()
                };
                for (position, repo) in legacy.repos.into_iter().enumerate() {
                    index.repos.push(RepoRecord {
                        id: format!("repo_{:02}", position + 1),
                        name: repo.name,
                        // v1 存的是登记当时的路径，可能带符号链接或未规范化；
                        // 统一成索引约定的规范化路径，便于按路径合并。
                        path: normalize(&repo.path),
                        groups: Vec::new(),
                        remote: repo.remote,
                        git_dir: None,
                        common_git_dir: None,
                        availability: Availability::Unknown,
                        last_seen_at: None,
                        snapshot: None,
                    });
                }
                Ok(index)
            }
        }
    }

    /// 在锁内重新读取、修改并原子写回。
    ///
    /// 修改闭包应只做内存操作：耗时 Git 调用放在闭包外，提交时再按 ID 合并，
    /// 避免长时间持有索引锁。
    pub fn update<T>(&self, apply: impl FnOnce(&mut Index) -> Result<T>) -> Result<T> {
        let _lock = self.lock()?;
        let mut index = self.load()?;
        let value = apply(&mut index)?;
        self.write(&mut index)?;
        Ok(value)
    }

    /// 拿不到锁时的等待与提示。
    fn lock(&self) -> Result<Lock> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&self.lock_path)
            .with_context(|| format!("无法打开锁文件 {}", self.lock_path.display()))?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { file }),
                Err(error) => {
                    if start.elapsed() >= LOCK_TIMEOUT {
                        bail!(
                            "等待索引锁超时（{}）：{error}；如有残留进程占用，请稍后重试",
                            self.lock_path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }

    /// 临时文件写完并 fsync 后原子替换，避免写到一半留下半个索引。
    fn write(&self, index: &mut Index) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 {}", parent.display()))?;
        }
        let migrating = index.legacy;
        if migrating && self.path.is_file() {
            let backup = self.path.with_extension("json.v1.bak");
            std::fs::copy(&self.path, &backup)
                .with_context(|| format!("无法备份旧登记表到 {}", backup.display()))?;
        }
        index.version = INDEX_VERSION;
        index.sync_groups();
        let raw = serde_json::to_string_pretty(&*index).context("索引序列化失败")?;
        let temp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        {
            let mut file =
                File::create(&temp).with_context(|| format!("无法创建 {}", temp.display()))?;
            file.write_all(raw.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.sync_all())
                .with_context(|| format!("无法写入 {}", temp.display()))?;
        }
        std::fs::rename(&temp, &self.path)
            .with_context(|| format!("无法替换 {}", self.path.display()))?;
        if migrating {
            index.legacy = false;
        }
        Ok(())
    }
}

/// 持有期间保持排他锁，Drop 时释放。
struct Lock {
    file: File,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// 采集结果合并进索引：只更新仍然存在、路径未变的记录。
///
/// 期间被 `remove` 或 `relocate` 改动的记录不得被旧结果恢复。
pub fn merge_observation(index: &mut Index, observation: &Observation) -> bool {
    let position = match &observation.id {
        Some(id) => index.position_of(id),
        None => index
            .repos
            .iter()
            .position(|repo| repo.path == observation.path),
    };
    let Some(position) = position else {
        return false;
    };
    let record = &mut index.repos[position];
    if record.path != observation.path {
        return false;
    }
    record.availability = observation.availability;
    if observation.git_dir.is_some() {
        record.git_dir = observation.git_dir.clone();
    }
    if observation.common_git_dir.is_some() {
        record.common_git_dir = observation.common_git_dir.clone();
    }
    if let Some(remote) = &observation.remote {
        record.remote = Some(remote.clone());
    }
    if let Some(snapshot) = &observation.snapshot {
        record.snapshot = Some(snapshot.clone());
    }
    if observation.availability.is_available() {
        record.last_seen_at = Some(observation.at.clone());
    }
    true
}

/// 一次路径检查/采集的结果，用于合并回索引。
#[derive(Debug, Clone)]
pub struct Observation {
    /// 对应记录的 ID；未登记路径为空。
    pub id: Option<String>,
    /// 检查的规范化路径。
    pub path: PathBuf,
    /// 可访问状态。
    pub availability: Availability,
    /// Git 目录。
    pub git_dir: Option<PathBuf>,
    /// 共同 Git 目录。
    pub common_git_dir: Option<PathBuf>,
    /// origin 地址。
    pub remote: Option<String>,
    /// 完整快照；只做探测时为空。
    pub snapshot: Option<Snapshot>,
    /// 采集时间。
    pub at: String,
    /// 失败原因，用于输出。
    pub error: Option<String>,
}

impl Observation {
    /// 只有状态、没有快照的探测结果。
    pub fn probe(path: PathBuf, id: Option<String>) -> Self {
        Self {
            id,
            path,
            availability: Availability::Unknown,
            git_dir: None,
            common_git_dir: None,
            remote: None,
            snapshot: None,
            at: now_rfc3339(),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn temp_store(tag: &str) -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!(
            "bi-index-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let store = Store::new(&dir);
        (dir, store)
    }

    #[test]
    fn missing_file_is_empty_index() {
        let (dir, store) = temp_store("empty");
        let index = store.load().expect("load");
        assert!(index.repos.is_empty());
        assert_eq!(index.version, INDEX_VERSION);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_registry_migrates_and_is_backed_up() {
        let (dir, store) = temp_store("migrate");
        std::fs::write(
            store.path(),
            r#"{"repos":[{"name":"demo","path":"/tmp/demo","remote":"https://example.com/demo.git"}]}"#,
        )
        .expect("seed");

        let index = store.load().expect("legacy load");
        assert!(index.legacy);
        assert_eq!(index.repos.len(), 1);
        assert_eq!(index.repos[0].id, "repo_01");
        assert_eq!(
            index.repos[0].remote.as_deref(),
            Some("https://example.com/demo.git")
        );

        store
            .update(|index| {
                index.repos[0].name = "renamed".into();
                Ok(())
            })
            .expect("migrating save");

        let saved = store.load().expect("reload");
        assert!(!saved.legacy);
        assert_eq!(saved.version, INDEX_VERSION);
        assert_eq!(saved.repos[0].name, "renamed");
        assert!(
            dir.join("repos.json.v1.bak").is_file(),
            "迁移前应备份旧文件"
        );

        // 迁移只在第一次写入时备份，第二次不会把 v2 覆盖成备份。
        store.update(|_| Ok(())).expect("second save");
        let backup = std::fs::read_to_string(dir.join("repos.json.v1.bak")).expect("backup");
        assert!(backup.contains("\"demo\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_version_and_broken_json_are_errors() {
        let (dir, store) = temp_store("broken");
        std::fs::write(store.path(), r#"{"version":99,"repos":[]}"#).expect("seed");
        assert!(store.load().is_err());
        std::fs::write(store.path(), "{ not json").expect("seed");
        assert!(store.load().is_err(), "损坏的索引不能被当成空表");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_stable_and_gap_free() {
        let mut index = Index::default();
        assert_eq!(index.next_id(), "repo_01");
        index.repos.push(record("repo_01", "a", "/a"));
        index.repos.push(record("repo_03", "b", "/b"));
        assert_eq!(index.next_id(), "repo_04");
    }

    #[test]
    fn group_membership_is_rebuilt_from_groups() {
        let mut index = Index::default();
        index.repos.push(record("repo_01", "a", "/a"));
        index.repos.push(record("repo_02", "b", "/b"));
        index.groups.push(Group {
            name: "platform".into(),
            members: vec!["repo_02".into(), "repo_02".into(), "gone".into()],
        });
        index.sync_groups();
        assert_eq!(index.groups[0].members, vec!["repo_02".to_string()]);
        assert!(index.repos[0].groups.is_empty());
        assert_eq!(index.repos[1].groups, vec!["platform".to_string()]);
    }

    #[test]
    fn concurrent_updates_do_not_lose_records() {
        let (dir, store) = temp_store("concurrent");
        store
            .update(|index| {
                index.repos.push(record("repo_01", "a", "/a"));
                Ok(())
            })
            .expect("seed");

        std::thread::scope(|scope| {
            for n in 0..4 {
                let store = &store;
                scope.spawn(move || {
                    store
                        .update(|index| {
                            index.repos.push(record(
                                &format!("repo_0{}", n + 2),
                                &format!("r{n}"),
                                &format!("/r{n}"),
                            ));
                            Ok(())
                        })
                        .expect("update");
                });
            }
        });

        let index = store.load().expect("reload");
        assert_eq!(index.repos.len(), 5, "并发写入不应丢记录");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn observation_does_not_resurrect_removed_record() {
        let mut index = Index::default();
        index.repos.push(record("repo_01", "a", "/a"));
        index.remove_repo("repo_01");

        let mut observation = Observation::probe(PathBuf::from("/a"), Some("repo_01".into()));
        observation.availability = Availability::Available;
        assert!(!merge_observation(&mut index, &observation));
        assert!(index.repos.is_empty());
    }
}
