//! BootIDEA 命令行入口。
//!
//! 宿主（`originlang.extension.host`）与插件（`originlang.extension.harness`）
//! 都不自带可执行入口：前者是库，后者是动态库插件。所有命令——安装、调用、
//! 服务、签名、跑 agent——都由这个二进制统一提供。

mod cli;
mod commands;
mod config;
mod engine;
mod log;
mod serve;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    // 错误统一在出口格式化一次：`{:#}` 保留 anyhow 的上下文链，便于定位是
    // 哪一步失败的，而不是只看到最内层的一句。
    let code = match cli.dispatch() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("错误: {error:#}");
            1
        }
    };
    std::process::exit(code);
}
