//! 扩展管理。

use crate::cli::ExtCommand;
use crate::config::Settings;
use crate::engine;
use anyhow::{Context, Result, bail};
use originlang_ext_host::{ExtensionSummary, Host};
use originlang_ext_manifest::Manifest;
use originlang_ext_permission::validate_identity;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// 执行一条 `bootidea ext ...`。
pub fn ext(settings: &Settings, command: ExtCommand) -> Result<()> {
    engine::with_host(settings, false, |host| execute(host, command))
}

fn execute(host: &Host, command: ExtCommand) -> Result<()> {
    match command {
        ExtCommand::List => list(host),
        ExtCommand::Info { id } => info(host, &id),
        ExtCommand::Install { path, force, grant } => install(host, &path, force, &grant),
        ExtCommand::Uninstall { id } => {
            host.uninstall(&id)?;
            println!("已卸载 {id}");
            Ok(())
        }
        ExtCommand::Enable { id } => {
            let summary = host.set_enabled(&id, true)?;
            println!("已启用 {} v{}", summary.id, summary.version);
            Ok(())
        }
        ExtCommand::Disable { id } => {
            let summary = host.set_enabled(&id, false)?;
            println!("已禁用 {} v{}", summary.id, summary.version);
            Ok(())
        }
        ExtCommand::Grant { id, capabilities } => {
            let summary = find(host, &id)?;
            ensure_declared(&summary.capabilities, &capabilities)?;
            let granted = host.grant(&id, &capabilities)?;
            println!("{}: {}", id, granted.join(", "));
            Ok(())
        }
        ExtCommand::Revoke { id, capability } => {
            let granted = host.revoke(&id, &capability)?;
            let report = if granted.is_empty() {
                "-".to_owned()
            } else {
                granted.join(", ")
            };
            println!("{}: {}", id, report);
            Ok(())
        }
        ExtCommand::Logs { id, limit } => logs(host, id.as_deref(), limit),
        ExtCommand::Init { id, dir, name } => init(&id, dir, name),
    }
}

fn list(host: &Host) -> Result<()> {
    let summaries = host.summaries()?;
    if summaries.is_empty() {
        println!(
            "{} 中没有已安装的扩展",
            host.paths().extensions_dir().display()
        );
        return Ok(());
    }

    println!(
        "{:<24} {:<10} {:<9} {:<7} {:<20} CAPABILITIES",
        "ID", "VERSION", "STATE", "LOADED", "SIGNED"
    );
    for summary in summaries {
        // 末尾的 `*` 表示发布者的公钥在信任库里；只有名字则是“签名了，但签名人
        // 未被核实”，这个区别必须看得见。
        let signed = match (&summary.publisher, summary.trusted) {
            (Some(publisher), true) => format!("{publisher} *"),
            (Some(publisher), false) => publisher.clone(),
            (None, _) => "-".to_owned(),
        };
        println!(
            "{:<24} {:<10} {:<9} {:<7} {:<20} {}",
            summary.id,
            summary.version,
            if summary.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if summary.loaded { "yes" } else { "no" },
            signed,
            if summary.capabilities.is_empty() {
                "-".to_owned()
            } else {
                summary.capabilities.join(", ")
            }
        );
    }
    Ok(())
}

fn info(host: &Host, id: &str) -> Result<()> {
    let summary = find(host, id)?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

fn install(host: &Host, path: &Path, force: bool, grant: &[String]) -> Result<()> {
    let installed = host.install(path, force)?;
    if !grant.is_empty() {
        ensure_declared(&installed.capabilities, grant)?;
        host.grant(&installed.id, grant)?;
    }

    // 授权后重新读一次，让输出反映存储里的策略，而不是安装那一刻的快照。
    let summary = find(host, &installed.id)?;
    println!(
        "已安装 {} v{} 到 {}",
        summary.id,
        summary.version,
        host.paths().extensions_dir().join(&summary.id).display()
    );
    match (&summary.publisher, summary.trusted) {
        (Some(publisher), true) => println!("签名者: {publisher}（可信）"),
        (Some(publisher), false) => {
            println!("签名者: {publisher}（公钥未受信任；用 `bootidea trust add` 接受该发布者）")
        }
        (None, _) => println!("未签名: 包内没有找到签名"),
    }
    if !summary.granted.is_empty() {
        println!("已授予: {}", summary.granted.join(", "));
    } else if !summary.capabilities.is_empty() {
        println!(
            "尚未授予: {}（用 `bootidea ext grant {} ...`）",
            summary.capabilities.join(", "),
            summary.id
        );
    }
    Ok(())
}

fn logs(host: &Host, id: Option<&str>, limit: usize) -> Result<()> {
    let records = host.logs(id, limit);
    if records.is_empty() {
        println!("没有日志记录");
    }
    for record in records {
        println!(
            "{:>13} {:<5} {:<16} {}",
            record.at_ms,
            record.level,
            record.extension.as_deref().unwrap_or("-"),
            record.message
        );
    }
    Ok(())
}

/// 写一个起步包。
fn init(id: &str, dir: Option<PathBuf>, name: Option<String>) -> Result<()> {
    validate_identity(id).map_err(|error| anyhow::anyhow!(error))?;
    let target = dir.unwrap_or_else(|| PathBuf::from(id));
    if target.exists()
        && target
            .read_dir()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        bail!("{} 已存在且不为空", target.display());
    }
    fs::create_dir_all(&target)?;

    let name = name.unwrap_or_else(|| id.to_owned());
    let manifest = json!({
        "id": id,
        "name": name,
        "version": "0.1.0",
        "apiVersion": "1",
        "main": "main.js",
        "description": "An OriginLang extension",
        "capabilities": ["log.write"],
        "activationEvents": ["onStartup"]
    });
    fs::write(
        target.join(originlang_ext_manifest::MANIFEST_FILE),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    fs::write(target.join("main.js"), MAIN_JS.replace("__ID__", id))?;
    fs::write(target.join("README.md"), README.replace("__ID__", id))?;

    // 校验刚写出的东西：一个连自己清单都过不了的模板，比没有模板更糟。
    Manifest::read_dir(&target)?;
    println!("已在 {} 创建扩展 '{id}'", target.display());
    println!(
        "下一步: bootidea ext install {} --grant log.write",
        target.display()
    );
    Ok(())
}

fn find(host: &Host, id: &str) -> Result<ExtensionSummary> {
    host.summaries()?
        .into_iter()
        .find(|summary| summary.id == id)
        .with_context(|| format!("扩展 '{id}' 未安装"))
}

/// 拒绝授予清单没有声明的能力。
///
/// 授予一个未声明能力，轻则是死重量，重则掩盖了“这个包需要更新清单”的事实。
fn ensure_declared(declared: &[String], requested: &[String]) -> Result<()> {
    for capability in requested {
        if !declared.iter().any(|candidate| candidate == capability) {
            bail!(
                "清单未声明能力 '{capability}'（已声明: {}）",
                if declared.is_empty() {
                    "无".to_owned()
                } else {
                    declared.join(", ")
                }
            );
        }
    }
    Ok(())
}

const MAIN_JS: &str = r#"// 入口脚本。扩展能做的一切都经过 `Origin.host`，
// 宿主按清单声明的能力逐项授权。
Origin.define({
  id: "__ID__",
  version: "0.1.0",
  capabilities: ["log.write"],

  activate() {
    Origin.host.log("extension activated", "debug");
  },

  deactivate() {
    Origin.host.log("extension deactivated", "debug");
  },

  methods: {
    hello(params) {
      return { message: `Hello, ${params?.name ?? "world"}` };
    },
  },

  onEvent(event) {
    Origin.host.log(`event received: ${event.topic}`, "debug");
  },
});
"#;

const README: &str = r#"# __ID__

一个 OriginLang 扩展。

## 结构

| 路径 | 用途 |
| --- | --- |
| `extension.json` | 清单：身份、入口、声明的能力。 |
| `main.js` | 入口脚本，通过 `Origin.define` 注册方法。 |

## 试一下

```sh
bootidea run . --method hello --params '{"name":"OriginLang"}' --grant-all
bootidea ext install . --grant log.write
bootidea ext list
```

## Host API

| 调用 | 所需能力 |
| --- | --- |
| `Origin.host.log(message, level)` | `log.write` |
| `Origin.host.storage.get/set/remove/list` | `storage.read` / `storage.write` |
| `Origin.host.timers.set/clear` | `timer.schedule` |
| `Origin.host.events.emit` / `subscribe` | `events.emit` / `events.subscribe` |
| `Origin.host.callExtension(id, method, params)` | `extension.call` |

`onEvent(event)` 接收其他扩展发布的事件。在 `activationEvents` 里声明
`onEvent:<topic>` 可以按需加载。
"#;
