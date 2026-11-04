//! 命令实现。
//!
//! 每个命令只依赖 [`crate::config::Settings`] 与 [`crate::engine`]，不直接读
//! 环境变量、也不自己拼安装路径——那两件事分别归配置层与宿主。

pub mod ext;
pub mod package;
pub mod run;

pub use ext::ext;
pub use package::{keygen, sign, trust};
pub use run::{agent, run};

use crate::cli::Transport;
use crate::config::Settings;
use crate::engine;
use anyhow::Result;

/// 显示宿主信息。
pub fn info(settings: &Settings) -> Result<()> {
    engine::with_host(settings, false, |host| {
        println!("{}", serde_json::to_string_pretty(&host.info())?);
        Ok(())
    })
}

/// 以 JSON-RPC 提供服务。
pub fn serve(
    settings: &Settings,
    transport: Transport,
    bind: String,
    port: u16,
    startup: bool,
) -> Result<()> {
    let host = engine::open(settings, startup)?;
    let (label, transport) = match transport {
        Transport::Stdio => ("stdio", crate::serve::Transport::Stdio),
        Transport::Tcp => ("tcp", crate::serve::Transport::Tcp { host: bind, port }),
        Transport::Http => ("http", crate::serve::Transport::Http { host: bind, port }),
    };
    crate::log::info(format!("以 {label} 传输提供服务"));
    let options = crate::serve::ServeOptions { transport };
    let outcome = crate::serve::serve(host.clone(), options);
    host.shutdown();
    outcome
}
