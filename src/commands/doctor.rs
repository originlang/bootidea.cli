//! 基础环境诊断。
//!
//! 只诊断、给修复建议，不做任何修复动作。可选能力（Host、harness 插件）
//! 缺失不应导致诊断失败，所以这里不加载宿主引擎，只检查 CLI 自身依赖。

use crate::config::Settings;
use anyhow::Result;
use std::process::Command;

/// 单项诊断结果。
enum Verdict {
    /// 正常。
    Ok,
    /// 有影响但 CLI 仍可用。
    Warn,
    /// 基础功能受损。
    Fail,
}

/// `doctor` 命令入口：全部检查完再汇总；出现 FAIL 项时以非零码退出。
pub fn doctor(settings: &Settings) -> Result<()> {
    let mut failed = 0usize;
    let mut warned = 0usize;

    let checks: Vec<(String, Verdict, Option<String>)> = vec![
        check_git(),
        check_config_file(),
        check_repo_root(settings),
        check_editor(settings),
        check_data_dir(settings),
    ];

    for (name, verdict, advice) in checks {
        let tag = match verdict {
            Verdict::Ok => {
                println!("[OK]   {name}");
                continue;
            }
            Verdict::Warn => {
                warned += 1;
                "[WARN]"
            }
            Verdict::Fail => {
                failed += 1;
                "[FAIL]"
            }
        };
        println!("{tag}  {name}");
        if let Some(advice) = advice {
            println!("       建议: {advice}");
        }
    }

    if failed > 0 {
        println!();
        println!("诊断完成：{failed} 项失败，{warned} 项警告");
        Err(anyhow::anyhow!("{failed} 项基础环境检查未通过"))
    } else {
        println!();
        println!("诊断完成：基础环境可用（{warned} 项警告）");
        Ok(())
    }
}

/// git 是否可用。
fn check_git() -> (String, Verdict, Option<String>) {
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (format!("git 可用（{version}）"), Verdict::Ok, None)
        }
        _ => (
            "git 不可用".to_string(),
            Verdict::Fail,
            Some("安装 Git（https://git-scm.com）并加入 PATH；repo 组命令依赖它".to_string()),
        ),
    }
}

/// 配置文件生效来源。
fn check_config_file() -> (String, Verdict, Option<String>) {
    if let Some(path) = std::env::var_os("BOOTIDEA_CONFIG") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return (format!("配置文件: {}", path.display()), Verdict::Ok, None);
        }
        return (
            format!("BOOTIDEA_CONFIG 指向的文件不存在: {}", path.display()),
            Verdict::Fail,
            Some("修正 BOOTIDEA_CONFIG，或取消该环境变量".to_string()),
        );
    }
    match crate::config::config_candidates()
        .into_iter()
        .find(|p| p.is_file())
    {
        Some(path) => (format!("配置文件: {}", path.display()), Verdict::Ok, None),
        None => (
            "未使用配置文件（全部默认值）".to_string(),
            Verdict::Ok,
            Some("需要时用 config set repo.root 或 editor 生成一个".to_string()),
        ),
    }
}

/// repo.root 配置与目录存在性；未设置只是提示，不算失败。
fn check_repo_root(settings: &Settings) -> (String, Verdict, Option<String>) {
    match &settings.repo.root {
        Some(root) if root.is_dir() => (
            format!("repo.root = {}（目录存在）", root.display()),
            Verdict::Ok,
            None,
        ),
        Some(root) => (
            format!("repo.root = {}（目录不存在）", root.display()),
            Verdict::Warn,
            Some("创建目录，或 config set repo.root 改为有效路径".to_string()),
        ),
        None => (
            "repo.root 未设置（repo clone 需要 --dir 或此配置）".to_string(),
            Verdict::Warn,
            Some("config set repo.root <目录>".to_string()),
        ),
    }
}

/// editor 配置；未设置时回退链可用，不算失败。
fn check_editor(settings: &Settings) -> (String, Verdict, Option<String>) {
    if let Some(editor) = &settings.editor {
        return (format!("editor = {editor}"), Verdict::Ok, None);
    }
    let from_env = std::env::var_os("EDITOR")
        .or_else(|| std::env::var_os("VISUAL"))
        .map(|value| value.to_string_lossy().to_string());
    match from_env {
        Some(editor) => (
            format!("editor 未配置，回退环境变量 {editor}"),
            Verdict::Ok,
            None,
        ),
        None => (
            "editor 未配置（repo open 将用系统默认打开方式）".to_string(),
            Verdict::Warn,
            Some("config set editor <命令>（如 code --wait）".to_string()),
        ),
    }
}

/// 数据根目录可写性：登记表、宿主状态都落在这里。
fn check_data_dir(settings: &Settings) -> (String, Verdict, Option<String>) {
    let paths = settings.paths();
    let root = paths.root().to_path_buf();
    if let Err(error) = std::fs::create_dir_all(&root) {
        return (
            format!("数据目录不可创建: {}", root.display()),
            Verdict::Fail,
            Some(format!(
                "检查权限，或用 --home / BOOTIDEA_HOME 换一个目录 ({error})"
            )),
        );
    }
    let probe = root.join(".bootidea-doctor");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            (
                format!("数据目录可写: {}", root.display()),
                Verdict::Ok,
                None,
            )
        }
        Err(error) => (
            format!("数据目录不可写: {}", root.display()),
            Verdict::Fail,
            Some(format!(
                "检查权限，或用 --home / BOOTIDEA_HOME 换一个目录 ({error})"
            )),
        ),
    }
}
