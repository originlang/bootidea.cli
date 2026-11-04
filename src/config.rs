//! 分层配置。
//!
//! 优先级（高 → 低）：命令行 flag > 环境变量 > 配置文件 > 默认值。
//!
//! 四层不在代码里逐层覆盖，而是合并成一张键值表交给 serde 反序列化：
//! `config` crate 负责合并与类型转换，[`Settings`] 只描述最终形状。这样新增
//! 一个配置项只需在结构体里加字段，不需要在每个来源里重复一遍。

use anyhow::{Context, Result, bail};
use config::{Config, Environment, File};
use originlang_ext_host::Paths;
use serde::Deserialize;
use std::path::PathBuf;

/// 环境变量前缀：`BOOTIDEA_HOME`、`BOOTIDEA_BACKEND`、`BOOTIDEA_LOG_LEVEL`。
const ENV_PREFIX: &str = "BOOTIDEA";

/// 扩展代码在哪里执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// 按清单声明的后端逐个路由，两种插件可以同时存在
    Auto,
    /// 每实例一个受限进程
    Sandbox,
    /// 进程内 QuickJS 工作线程
    Quickjs,
    /// 进程内原生插件
    Dylib,
}

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// 仅错误
    Error,
    /// 警告与错误
    Warn,
    /// 常规信息
    Info,
    /// 调试细节
    Debug,
    /// 全部输出
    Trace,
}

/// 合并后的运行配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 数据目录；缺省时由宿主按平台规则解析。
    pub home: Option<PathBuf>,
    /// 扩展执行后端。
    pub backend: Backend,
    /// 是否拒绝未签名或签名不可信的包。
    pub require_signature: bool,
    /// 日志级别。
    pub log_level: LogLevel,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            home: None,
            backend: Backend::Auto,
            require_signature: false,
            log_level: LogLevel::Info,
        }
    }
}

impl Settings {
    /// 宿主使用的安装根目录。
    ///
    /// 没有显式配置时交给 `Paths::from_env()`，让 CLI 与宿主自身对“默认目录在
    /// 哪里”保持同一个答案，而不是各算一份。
    pub fn paths(&self) -> Paths {
        match &self.home {
            Some(root) => Paths::new(root.clone()),
            None => Paths::from_env(),
        }
    }
}

/// 合并命令行、环境变量与配置文件。
pub fn load(global: &crate::cli::GlobalArgs) -> Result<Settings> {
    let mut builder = Config::builder()
        .set_default("backend", "auto")?
        .set_default("require_signature", false)?
        .set_default("log_level", "info")?;

    if let Some(path) = explicit_config(global) {
        if !path.is_file() {
            bail!("配置文件不存在: {}", path.display());
        }
        builder = builder.add_source(File::from(path).required(true));
    } else if let Some(path) = default_config() {
        // 默认位置上的文件是可选的：存在就合并，不存在不算错误。
        builder = builder.add_source(File::from(path).required(false));
    }

    // `separator("__")` 保持单层键的扁平写法：BOOTIDEA_LOG_LEVEL → log_level。
    builder = builder.add_source(
        Environment::with_prefix(ENV_PREFIX)
            .separator("__")
            .try_parsing(true),
    );

    if let Some(home) = &global.home {
        builder = builder.set_override("home", home.to_string_lossy().to_string())?;
    }
    if let Some(backend) = global.backend {
        builder = builder.set_override("backend", backend_name(backend))?;
    }
    if global.require_signature {
        builder = builder.set_override("require_signature", true)?;
    }
    if let Some(level) = global.log_level {
        builder = builder.set_override("log_level", level_name(level))?;
    }

    builder
        .build()
        .context("无法合并配置")?
        .try_deserialize::<Settings>()
        .context("配置内容不合法")
}

/// `--config` 优先于 `BOOTIDEA_CONFIG`；两者都表示“必须存在”。
fn explicit_config(global: &crate::cli::GlobalArgs) -> Option<PathBuf> {
    global
        .config
        .clone()
        .or_else(|| std::env::var_os("BOOTIDEA_CONFIG").map(PathBuf::from))
}

/// 默认配置文件：`$XDG_CONFIG_HOME/bootidea/` 下的第一个候选，或 `~/.bootidea/`。
fn default_config() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(base) = std::env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(base).join("bootidea"));
    } else if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".config").join("bootidea"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".bootidea"));
    }

    roots
        .into_iter()
        .flat_map(|dir| {
            ["config.toml", "config.yaml", "config.yml"]
                .map(|name| dir.join(name))
                .into_iter()
        })
        .find(|path| path.is_file())
}

fn backend_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto => "auto",
        Backend::Sandbox => "sandbox",
        Backend::Quickjs => "quickjs",
        Backend::Dylib => "dylib",
    }
}

fn level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_stable() {
        let settings = Settings::default();
        assert_eq!(settings.backend, Backend::Auto);
        assert!(!settings.require_signature);
        assert_eq!(settings.log_level, LogLevel::Info);
        assert!(settings.home.is_none());
    }

    #[test]
    fn backend_names_round_trip_through_serde() {
        for (name, backend) in [
            ("auto", Backend::Auto),
            ("sandbox", Backend::Sandbox),
            ("quickjs", Backend::Quickjs),
            ("dylib", Backend::Dylib),
        ] {
            assert_eq!(backend_name(backend), name);
            let parsed: Backend = serde_json::from_str(&format!("\"{name}\"")).expect("parse");
            assert_eq!(parsed, backend);
        }
    }

    #[test]
    fn level_names_round_trip_through_serde() {
        for (name, level) in [
            ("error", LogLevel::Error),
            ("warn", LogLevel::Warn),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("trace", LogLevel::Trace),
        ] {
            assert_eq!(level_name(level), name);
            let parsed: LogLevel = serde_json::from_str(&format!("\"{name}\"")).expect("parse");
            assert_eq!(parsed, level);
        }
    }
}
