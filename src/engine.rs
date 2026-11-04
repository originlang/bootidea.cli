//! 宿主装配。
//!
//! 这里只回答两个问题：安装目录在哪、每种扩展由哪个后端执行。命令本身不关心
//! 后端差异——`Router` 按清单里的 `runtime` 字段把每个扩展分派到对应 provider，
//! 所以 `--backend auto` 时一份进程可以同时跑 QuickJS 脚本和原生插件。

use crate::config::{Backend, Settings};
use anyhow::{Context, Result};
use originlang_ext_host::{Host, HostConfig, Router};
use originlang_ext_runtime::RuntimeProvider;
use originlang_ext_runtime_dylib::DylibProvider;
use originlang_ext_runtime_process::SandboxProvider;
use originlang_ext_runtime_quickjs::QuickJsProvider;
use std::sync::Arc;

/// 按配置打开宿主。
///
/// `startup` 决定是否加载声明了 `onStartup` 的扩展；管理类命令通常关掉它，
/// 避免为了列个清单就把所有插件跑起来。
pub fn open(settings: &Settings, startup: bool) -> Result<Host> {
    let paths = settings.paths();
    crate::log::debug(format!(
        "打开宿主: {}（后端 {:?}，启动加载 {}）",
        paths.root().display(),
        settings.backend,
        startup
    ));
    let config = HostConfig::new(paths)
        .with_startup_load(startup)
        .with_require_signature(settings.require_signature);
    Host::new(config, provider(settings.backend)?).context("无法打开扩展宿主")
}

/// 构造后端。
pub fn provider(backend: Backend) -> Result<Arc<dyn RuntimeProvider>> {
    match backend {
        // 顺序只影响报错时的列举顺序：Router 按名字查找，不按位置。沙箱二进制
        // 缺失时跳过它，而不是让整个安装无法启动。
        Backend::Auto => {
            let mut router = Router::new()
                .with(Arc::new(QuickJsProvider::new()))
                .with(Arc::new(DylibProvider::new()));
            if let Ok(sandbox) = SandboxProvider::discover() {
                router = router.with(Arc::new(sandbox));
            }
            Ok(Arc::new(router))
        }
        // 沙箱刻意不走 PATH 查找：它是本次安装的一部分，让任意目录提供这个
        // 二进制等于把隔离交给环境变量决定。
        Backend::Sandbox => Ok(Arc::new(
            SandboxProvider::discover().map_err(|error| anyhow::anyhow!("{error}"))?,
        )),
        Backend::Quickjs => Ok(Arc::new(QuickJsProvider::new())),
        Backend::Dylib => Ok(Arc::new(DylibProvider::new())),
    }
}

/// 打开宿主、执行、确保关闭。
///
/// 宿主持有扩展实例与后台线程，`shutdown` 必须在退出前跑到，否则进程结束时
/// 只能靠操作系统收尸——在 Windows 上那意味着插件里的临时文件可能来不及清理。
pub fn with_host<T>(
    settings: &Settings,
    startup: bool,
    body: impl FnOnce(&Host) -> Result<T>,
) -> Result<T> {
    let host = open(settings, startup)?;
    let outcome = body(&host);
    host.shutdown();
    outcome
}
