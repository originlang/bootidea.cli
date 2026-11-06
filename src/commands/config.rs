//! 用户配置查询与修改。
//!
//! `list`/`get` 展示合并后的生效值；`set` 只写入用户配置文件里属于用户的键
//! （`repo.root`、`editor`），核心配置（backend、log_level 等）仍走 flag、
//! 环境变量与既有配置文件。

use crate::cli::ConfigCommand;
use crate::config::Settings;
use anyhow::{Context, Result, bail};

/// 可写的用户配置键及其在 TOML 里的位置。
const WRITABLE_KEYS: &[(&str, &str)] = &[
    ("repo.root", "未指定目标目录时的仓库克隆位置"),
    (
        "editor",
        "repo open 使用的编辑器命令，可带参数（如 code --wait）",
    ),
];

/// `config` 命令组入口。
///
/// `explicit_config` 是 `--config`/`BOOTIDEA_CONFIG` 指定的文件；`set` 会把
/// 值写进去，因此必须存在，避免写完却不生效。
pub fn config(
    settings: &Settings,
    explicit_config: Option<&std::path::PathBuf>,
    command: ConfigCommand,
) -> Result<()> {
    match command {
        ConfigCommand::List => {
            println!("backend = {:?}", settings.backend);
            println!("require_signature = {}", settings.require_signature);
            println!("log_level = {:?}", settings.log_level);
            match &settings.home {
                Some(home) => println!("home = {}", home.display()),
                None => println!("home = （默认，{}）", settings.paths().root().display()),
            }
            match &settings.repo.root {
                Some(root) => println!("repo.root = {}", root.display()),
                None => println!("repo.root = （未设置）"),
            }
            match &settings.editor {
                Some(editor) => println!("editor = {editor}"),
                None => println!("editor = （未设置，回退 EDITOR/VISUAL/系统默认）"),
            }
        }
        ConfigCommand::Get { key } => {
            let value = lookup(settings, &key).context("未知配置键")?;
            println!("{value}");
        }
        ConfigCommand::Set { key, value } => {
            if !WRITABLE_KEYS.iter().any(|(name, _)| *name == key) {
                bail!(
                    "配置键 {key} 不可写；可写键: {}",
                    WRITABLE_KEYS
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            let path = crate::config::writable_config_path(explicit_config)?;
            set_key(&path, &key, &value)?;
            println!("已写入 {key} = {value} → {}", path.display());
        }
    }
    Ok(())
}

/// 生效值的文本形式，供 `get` 使用。
fn lookup(settings: &Settings, key: &str) -> Option<String> {
    match key {
        "backend" => Some(format!("{:?}", settings.backend)),
        "require_signature" => Some(settings.require_signature.to_string()),
        "log_level" => Some(format!("{:?}", settings.log_level)),
        "home" => Some(
            settings
                .home
                .clone()
                .unwrap_or_else(|| settings.paths().root().to_path_buf())
                .display()
                .to_string(),
        ),
        "repo.root" => Some(
            settings
                .repo
                .root
                .clone()
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "（未设置）".to_string()),
        ),
        "editor" => Some(
            settings
                .editor
                .clone()
                .unwrap_or_else(|| "（未设置）".to_string()),
        ),
        _ => None,
    }
}

/// 把点分键写入 TOML 配置文件，保留文件里的其他内容。
fn set_key(path: &std::path::Path, key: &str, value: &str) -> Result<()> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(raw) => raw
            .parse()
            .with_context(|| format!("解析 {} 失败", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(error).context(format!("无法读取 {}", path.display())),
    };

    let segments: Vec<&str> = key.split('.').collect();
    let mut cursor = &mut table;
    for segment in &segments[..segments.len() - 1] {
        cursor = cursor
            .entry(segment.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("配置键 {key} 与已有内容冲突"))?;
    }
    let last = segments[segments.len() - 1];
    cursor.insert(last.to_string(), toml::Value::String(value.to_string()));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建 {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(&table).context("配置序列化失败")?;
    std::fs::write(path, raw).with_context(|| format!("无法写入 {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_table(raw: &str) -> toml::Table {
        raw.parse().expect("parse toml")
    }

    #[test]
    fn set_key_writes_nested_and_flat() {
        let dir = std::env::temp_dir().join(format!("bi-config-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("config.toml");

        set_key(&path, "repo.root", "E:/repos").expect("nested set");
        set_key(&path, "editor", "code --wait").expect("flat set");
        let table = write_table(&std::fs::read_to_string(&path).expect("read"));
        assert_eq!(
            table["repo"]["root"].as_str(),
            Some("E:/repos"),
            "点分键应写成嵌套表"
        );
        assert_eq!(table["editor"].as_str(), Some("code --wait"));

        // 覆盖旧值不影响其他键。
        set_key(&path, "editor", "idea").expect("overwrite");
        let table = write_table(&std::fs::read_to_string(&path).expect("read"));
        assert_eq!(table["editor"].as_str(), Some("idea"));
        assert_eq!(table["repo"]["root"].as_str(), Some("E:/repos"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_key_rejects_conflicting_shape() {
        let dir = std::env::temp_dir().join(format!("bi-config-test2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("config.toml");
        std::fs::write(&path, "repo = \"not-a-table\"\n").expect("seed");

        let result = set_key(&path, "repo.root", "E:/repos");
        assert!(result.is_err(), "标量与嵌套键冲突必须报错");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_writable_keys_accepted() {
        let settings = Settings::default();
        let result = config(
            &settings,
            None,
            ConfigCommand::Set {
                key: "backend".to_string(),
                value: "quickjs".to_string(),
            },
        );
        assert!(result.is_err(), "核心配置不允许用 config set 覆盖");
    }
}
