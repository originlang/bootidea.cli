//! 开发期运行与 agent 调用。

use crate::config::Settings;
use crate::engine;
use anyhow::{Context, Result, bail};
use originlang_ext_host::{Host, HostConfig, Paths};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// 在一次性安装目录里运行本地包的一个方法。
///
/// 每次运行都新建临时根目录，是为了让“装了哪些扩展”成为一次运行的显式输入：
/// 只有 `--with` 指定的包会出现在里面，测试结果不依赖开发机上碰巧装过什么。
pub fn run(
    settings: &Settings,
    dir: &Path,
    method: &str,
    params: &str,
    grant_all: bool,
    with: &[PathBuf],
) -> Result<()> {
    let params: Value = serde_json::from_str(params).context("--params 必须是合法 JSON")?;
    let source =
        fs::canonicalize(dir).with_context(|| format!("无法解析包目录 {}", dir.display()))?;

    let scratch = ScratchHome::new()?;
    let host = Host::new(
        HostConfig::new(Paths::new(scratch.path()))
            .with_startup_load(false)
            .with_require_signature(settings.require_signature),
        engine::provider(settings.backend)?,
    )?;

    // 配套包先装：被测包最后装，这样它激活时能看见前面每一个。
    for companion in with {
        let path = fs::canonicalize(companion)
            .with_context(|| format!("无法解析 --with 目录 {}", companion.display()))?;
        let summary = host.install(&path, false)?;
        if grant_all {
            host.grant(&summary.id, &summary.capabilities)?;
        }
    }

    let summary = host.install(&source, false)?;
    if grant_all {
        host.grant(&summary.id, &summary.capabilities)?;
    }
    let result = host.call(&summary.id, method, Some(params));
    host.shutdown();
    println!("{}", serde_json::to_string_pretty(&result?)?);
    Ok(())
}

/// 跑一次 agent 循环。
///
/// 模型调用发生在 harness 插件里，这里只负责把它需要的参数拼好。密钥刻意只从
/// 环境变量读取：命令行参数会出现在进程列表和 shell 历史里。
pub fn agent(
    settings: &Settings,
    goal: &str,
    model: Option<String>,
    base_url: Option<String>,
    api_key_env: &str,
    extension: &str,
) -> Result<()> {
    let model_name = model
        .or_else(|| env_value("BOOTIDEA_MODEL"))
        .context("缺少模型名：传 --model 或设置 BOOTIDEA_MODEL")?;
    let base = base_url
        .or_else(|| env_value("BOOTIDEA_BASE_URL"))
        .context("缺少接口地址：传 --base-url 或设置 BOOTIDEA_BASE_URL")?;

    let mut model_config = json!({ "baseUrl": base, "model": model_name });
    if let Some(api_key) = env_value(api_key_env) {
        model_config["apiKey"] = json!(api_key);
    } else {
        crate::log::warn(format!("环境变量 {api_key_env} 未设置，将不带密钥请求"));
    }

    let params = json!({ "goal": goal, "model": model_config });
    engine::with_host(settings, false, |host| {
        ensure_installed(host, extension)?;
        let result = host.call(extension, "run", Some(params.clone()))?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        Ok(())
    })
}

/// 缺包时给出安装提示，而不是让调用方对着 `-32001` 猜。
fn ensure_installed(host: &Host, id: &str) -> Result<()> {
    if host.summaries()?.iter().any(|summary| summary.id == id) {
        return Ok(());
    }
    bail!(
        "扩展 '{id}' 未安装；先运行 `bootidea ext install <包目录> --grant extension.call,...` 安装提供 agent 能力的插件"
    )
}

fn env_value(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// 临时安装根目录，析构时删除。
struct ScratchHome(PathBuf);

impl ScratchHome {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "bootidea-run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&root)?;
        Ok(Self(root))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
