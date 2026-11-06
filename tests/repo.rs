//! 端到端验证 `repo` 命令组：索引、扫描、目标选择与批量执行。
//!
//! 这里跑真实二进制和真实 Git，覆盖单元测试碰不到的参数解析与写盘行为。

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 一台机器上没有 Git 时跳过整组测试，而不是报一堆无关失败。
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 独立数据目录 + 工作目录，测试结束整体删除。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bi-repo-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).expect("mkdir");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// 运行 CLI；`--home` 统一指向沙箱，避免碰用户的真实索引。
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_bootidea"))
            .arg("--home")
            .arg(self.home())
            .args(args)
            .output()
            .expect("运行 bootidea")
    }

    /// 运行并断言成功，返回标准输出。
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?} 失败\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// 运行 `--json` 命令并解析。
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).expect("JSON 输出")
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("运行 git");
        assert!(
            output.status.success(),
            "git {args:?} 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// 新建一个带初始提交的仓库。
    fn repo(&self, relative: &str) -> PathBuf {
        let dir = self.work().join(relative);
        fs::create_dir_all(&dir).expect("mkdir");
        self.git(&dir, &["init", "-q"]);
        self.git(&dir, &["config", "user.email", "test@example.com"]);
        self.git(&dir, &["config", "user.name", "test"]);
        self.git(&dir, &["config", "commit.gpgsign", "false"]);
        fs::write(dir.join("file.txt"), "hello\n").expect("write");
        self.git(&dir, &["add", "."]);
        self.git(&dir, &["commit", "-qm", "init"]);
        dir
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn scan_registers_and_list_reads_the_index() {
    if !git_available() {
        eprintln!("跳过：没有 git");
        return;
    }
    let sandbox = Sandbox::new("scan");
    sandbox.repo("alpha");
    sandbox.repo("beta");

    let scanned = sandbox.ok(&["repo", "scan", sandbox.work().to_str().unwrap()]);
    assert!(scanned.contains("新登记 2 个"), "{scanned}");

    let index_path = sandbox.home().join("repos.json");
    let before = fs::read_to_string(&index_path).expect("索引存在");
    let listed = sandbox.json(&["repo", "list", "--json"]);
    let after = fs::read_to_string(&index_path).expect("索引存在");
    assert_eq!(before, after, "list 只读索引，不应改写文件");
    assert_eq!(listed["version"], 2);
    let repos = listed["repos"].as_array().expect("repos");
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0]["id"], "repo_01");
    assert_eq!(repos[1]["id"], "repo_02");
    assert_eq!(repos[0]["availability"], "available");

    // 重复扫描不新增记录。
    let again = sandbox.ok(&["repo", "scan", sandbox.work().to_str().unwrap()]);
    assert!(again.contains("新登记 0 个"), "{again}");
    assert_eq!(
        sandbox.json(&["repo", "list", "--json"])["repos"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

#[test]
fn v1_registry_migrates_on_first_write() {
    if !git_available() {
        eprintln!("跳过：没有 git");
        return;
    }
    let sandbox = Sandbox::new("migrate");
    let repo = sandbox.repo("legacy");
    fs::create_dir_all(sandbox.home()).expect("mkdir");
    let legacy = format!(
        r#"{{"repos":[{{"name":"legacy","path":"{}","remote":null}}]}}"#,
        repo.display()
    );
    let index_path = sandbox.home().join("repos.json");
    fs::write(&index_path, &legacy).expect("seed");

    // 只读命令不改写文件。
    let listed = sandbox.json(&["repo", "list", "--json"]);
    assert_eq!(listed["repos"][0]["id"], "repo_01");
    assert_eq!(fs::read_to_string(&index_path).expect("read"), legacy);

    // 第一次写入时迁移并备份。
    sandbox.ok(&["repo", "refresh"]);
    let migrated: Value =
        serde_json::from_str(&fs::read_to_string(&index_path).expect("read")).expect("json");
    assert_eq!(migrated["version"], 2);
    assert_eq!(migrated["repos"][0]["availability"], "available");
    assert!(
        sandbox.home().join("repos.json.v1.bak").is_file(),
        "迁移前应备份 v1 文件"
    );
}

#[test]
fn groups_filter_targets_and_removal_keeps_files() {
    if !git_available() {
        eprintln!("跳过：没有 git");
        return;
    }
    let sandbox = Sandbox::new("group");
    let alpha = sandbox.repo("alpha");
    sandbox.repo("beta");
    sandbox.ok(&["repo", "scan", sandbox.work().to_str().unwrap()]);

    sandbox.ok(&["repo", "group", "create", "platform"]);
    sandbox.ok(&["repo", "group", "add", "platform", "alpha"]);

    let grouped = sandbox.json(&[
        "repo", "status", "--group", "platform", "--cached", "--json",
    ]);
    assert_eq!(grouped["repos"].as_array().map(Vec::len), Some(1));
    assert_eq!(grouped["repos"][0]["name"], "alpha");

    let unknown = sandbox.run(&["repo", "status", "--group", "nope"]);
    assert!(!unknown.status.success(), "未知分组必须报错");

    sandbox.ok(&["repo", "remove", "alpha"]);
    assert!(alpha.is_dir(), "remove 只取消登记，不删本地文件");
    assert_eq!(
        sandbox.json(&["repo", "list", "--json"])["repos"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let groups = sandbox.json(&["repo", "group", "list", "--json"]);
    assert_eq!(
        groups["groups"][0]["members"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn scan_honours_depth_exclude_and_dry_run() {
    if !git_available() {
        eprintln!("跳过：没有 git");
        return;
    }
    let sandbox = Sandbox::new("rules");
    sandbox.repo("top");
    sandbox.repo("x/y/z");
    sandbox.repo("node_modules/fake");

    let dry = sandbox.ok(&[
        "repo",
        "scan",
        sandbox.work().to_str().unwrap(),
        "--depth",
        "1",
        "--dry-run",
    ]);
    assert!(dry.contains("top"), "{dry}");
    assert!(!dry.contains("/x/y/z"), "深度 1 不应发现更深仓库: {dry}");
    assert!(
        !sandbox.home().join("repos.json").exists(),
        "--dry-run 不写索引"
    );

    sandbox.ok(&["repo", "scan", sandbox.work().to_str().unwrap()]);
    let listed = sandbox.json(&["repo", "list", "--json"]);
    let names: Vec<&str> = listed["repos"]
        .as_array()
        .expect("repos")
        .iter()
        .filter_map(|repo| repo["name"].as_str())
        .collect();
    assert_eq!(names, vec!["top", "z"], "默认排除依赖目录");

    // 重扫同一根目录沿用保存的深度；显式 --depth 覆盖。
    let known = sandbox.ok(&["repo", "scan", "--known-roots"]);
    assert!(known.contains("已有 2 个"), "{known}");
}

#[test]
fn pull_fast_forwards_and_refuses_dirty_worktree() {
    if !git_available() {
        eprintln!("跳过：没有 git");
        return;
    }
    let sandbox = Sandbox::new("pull");
    let origin = sandbox.repo("origin");
    let bare = sandbox.root.join("origin.git");
    sandbox.git(
        &sandbox.work(),
        &[
            "clone",
            "--bare",
            "-q",
            &origin.display().to_string(),
            &bare.display().to_string(),
        ],
    );
    let remote = format!("file://{}", bare.display());

    let clone = |name: &str| -> PathBuf {
        let dir = sandbox.work().join(name);
        let output = Command::new("git")
            .args(["clone", "-q", &remote])
            .arg(&dir)
            .output()
            .expect("git clone");
        assert!(
            output.status.success(),
            "clone 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        sandbox.git(&dir, &["config", "user.email", "test@example.com"]);
        sandbox.git(&dir, &["config", "user.name", "test"]);
        sandbox.git(&dir, &["config", "commit.gpgsign", "false"]);
        dir
    };

    let ahead = clone("ahead");
    let behind = clone("behind");

    fs::write(ahead.join("new.txt"), "new\n").expect("write");
    sandbox.git(&ahead, &["add", "."]);
    sandbox.git(&ahead, &["commit", "-qm", "add new"]);
    sandbox.git(&ahead, &["push", "-q"]);

    sandbox.ok(&["repo", "add", behind.to_str().unwrap()]);
    sandbox.ok(&["repo", "pull", "--all"]);
    assert!(behind.join("new.txt").is_file(), "pull 应快进到最新提交");

    // 工作区不干净时拒绝更新。
    fs::write(behind.join("local.txt"), "local\n").expect("write");
    let dirty = sandbox.run(&["repo", "pull", "--all"]);
    assert!(!dirty.status.success(), "脏工作区必须拒绝 pull");
    let stderr = String::from_utf8_lossy(&dirty.stderr);
    assert!(stderr.contains("拒绝非快进更新"), "{stderr}");
}
