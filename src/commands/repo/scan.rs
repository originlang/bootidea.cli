//! 目录扫描：按深度与排除规则发现仓库。
//!
//! `.git` 只是候选标记，最终以 Git 的 `rev-parse` 结果为准（见
//! [`super::git::probe`]）。默认发现仓库后不再向下搜索，`nested` 才继续；
//! 目录符号链接不跟随，避免环和越界。

use crate::commands::repo::git;
use crate::commands::repo::index::Observation;
use crate::commands::repo::util::{glob_match, normalize};
use std::path::{Path, PathBuf};

/// 默认扫描深度；根目录为第 0 层。
pub const DEFAULT_DEPTH: u8 = 3;

/// 始终跳过的目录名：Git 内部、依赖、构建产物与虚拟环境。
pub const DEFAULT_EXCLUDE: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    ".gradle",
];

/// 一次扫描的规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// 扫描深度。
    pub depth: u8,
    /// 排除规则，匹配相对根目录的路径。
    pub exclude: Vec<String>,
    /// 是否继续搜索仓库内部的嵌套仓库。
    pub nested: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            exclude: default_exclude(),
            nested: false,
        }
    }
}

/// 默认排除规则。
pub fn default_exclude() -> Vec<String> {
    DEFAULT_EXCLUDE
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// 合并默认规则与用户规则，保持顺序并去重。
pub fn merge_exclude(user: &[String]) -> Vec<String> {
    let mut merged = default_exclude();
    for pattern in user {
        if !merged.contains(pattern) {
            merged.push(pattern.clone());
        }
    }
    merged
}

/// 扫描结果。
#[derive(Debug, Default)]
pub struct Outcome {
    /// 发现的有效仓库。
    pub found: Vec<Observation>,
    /// 跳过或无法读取的路径说明。
    pub issues: Vec<String>,
    /// 根目录本身无法扫描时的原因。
    pub error: Option<String>,
}

/// 扫描 `root`。
pub fn scan(root: &Path, rules: &Rules) -> Outcome {
    let root = normalize(root);
    let mut outcome = Outcome::default();
    match std::fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            outcome.error = Some(format!("{} 不是目录", root.display()));
            return outcome;
        }
        Err(error) => {
            outcome.error = Some(format!("无法访问 {}: {error}", root.display()));
            return outcome;
        }
    }
    walk(&root, &root, 0, rules, &mut outcome);
    outcome
}

/// 递归遍历；`level` 是相对根目录的层数。
fn walk(dir: &Path, root: &Path, level: u8, rules: &Rules, outcome: &mut Outcome) {
    if level > 0 && is_excluded(dir, root, rules) {
        return;
    }
    if has_git_marker(dir) {
        let observation = git::probe(dir, None);
        if observation.availability.is_available() {
            outcome.found.push(observation);
            if !rules.nested {
                return;
            }
        } else {
            outcome.issues.push(format!(
                "跳过 {}: {}",
                relative_display(dir, root),
                observation.error.as_deref().unwrap_or("不是有效仓库")
            ));
            // 伪造或损坏的标记不再向下找；继续深入只会重复同一条噪声。
            if !rules.nested {
                return;
            }
        }
    }
    if level >= rules.depth {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            outcome
                .issues
                .push(format!("无法读取 {}: {error}", relative_display(dir, root)));
            return;
        }
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                outcome.issues.push(format!(
                    "无法读取 {}: {error}",
                    relative_display(&entry.path(), root)
                ));
                continue;
            }
        };
        // 不跟随目录符号链接：可能指向根目录之外，也可能成环。
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        children.push(entry.path());
    }
    children.sort();
    for child in children {
        walk(&child, root, level + 1, rules, outcome);
    }
}

/// `.git` 目录或指针文件是否存在（worktree、submodule 用文件）。
fn has_git_marker(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// 排除规则匹配相对根目录的路径；不含 `/` 的规则同时匹配目录名。
fn is_excluded(dir: &Path, root: &Path, rules: &Rules) -> bool {
    let Ok(relative) = dir.strip_prefix(root) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    rules.exclude.iter().any(|pattern| {
        glob_match(pattern, &relative) || (!pattern.contains('/') && glob_match(pattern, &name))
    })
}

/// 相对根目录的展示路径，根目录本身用 `.`。
fn relative_display(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if !relative.as_os_str().is_empty() => {
            relative.to_string_lossy().replace('\\', "/")
        }
        _ => ".".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "bi-scan-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("mkdir");
            Self { root }
        }

        fn repo(&self, relative: &str) -> PathBuf {
            let dir = self.root.join(relative);
            std::fs::create_dir_all(&dir).expect("mkdir");
            let output = std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&dir)
                .output()
                .expect("git init");
            assert!(output.status.success(), "git init 失败");
            dir
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let dir = self.root.join(relative);
            std::fs::create_dir_all(&dir).expect("mkdir");
            dir
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn names(outcome: &Outcome, root: &Path) -> Vec<String> {
        let root = &normalize(root);
        let mut found: Vec<String> = outcome
            .found
            .iter()
            .map(|observation| relative_display(&observation.path, root))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn depth_limits_the_search() {
        let fixture = Fixture::new("depth");
        fixture.repo("a");
        fixture.repo("x/y/deep");

        let shallow = scan(
            &fixture.root,
            &Rules {
                depth: 1,
                ..Rules::default()
            },
        );
        assert_eq!(names(&shallow, &fixture.root), vec!["a"]);

        let deep = scan(&fixture.root, &Rules::default());
        assert_eq!(names(&deep, &fixture.root), vec!["a", "x/y/deep"]);

        let zero = scan(
            &fixture.root,
            &Rules {
                depth: 0,
                ..Rules::default()
            },
        );
        assert!(zero.found.is_empty(), "depth 0 只检查根目录本身");
    }

    #[test]
    fn repo_stops_descent_unless_nested() {
        let fixture = Fixture::new("nested");
        let outer = fixture.repo("outer");
        std::fs::create_dir_all(outer.join("inner/.git")).expect("mkdir");
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(outer.join("inner"))
            .output()
            .expect("git init");

        let flat = scan(&fixture.root, &Rules::default());
        assert_eq!(names(&flat, &fixture.root), vec!["outer"]);

        let nested = scan(
            &fixture.root,
            &Rules {
                nested: true,
                ..Rules::default()
            },
        );
        assert_eq!(names(&nested, &fixture.root), vec!["outer", "outer/inner"]);
    }

    #[test]
    fn default_skip_list_and_custom_excludes_apply() {
        let fixture = Fixture::new("exclude");
        fixture.repo("keep");
        fixture.repo("node_modules/fake");
        fixture.repo("archive/2020/old");

        let outcome = scan(
            &fixture.root,
            &Rules {
                exclude: merge_exclude(&["archive/**".to_string()]),
                ..Rules::default()
            },
        );
        assert_eq!(names(&outcome, &fixture.root), vec!["keep"]);
    }

    #[test]
    fn fake_markers_are_reported_not_registered() {
        let fixture = Fixture::new("fake");
        fixture.dir("fake/.git");
        let outcome = scan(&fixture.root, &Rules::default());
        assert!(outcome.found.is_empty());
        assert_eq!(outcome.issues.len(), 1, "{:?}", outcome.issues);
        assert!(outcome.issues[0].contains("fake"));
    }

    #[test]
    fn missing_root_is_an_error() {
        let fixture = Fixture::new("missing");
        let outcome = scan(&fixture.root.join("nope"), &Rules::default());
        assert!(outcome.error.is_some());
    }
}
