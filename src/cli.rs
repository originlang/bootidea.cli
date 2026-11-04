//! 命令行入口定义。
//!
//! 这一层只做两件事：把参数解析成结构体，把结构体分发给命令实现。业务逻辑
//! 一律在 [`crate::commands`] 里，配置合并一律在 [`crate::config`] 里，这样
//! 新增一个命令不需要碰配置，新增一个配置项也不需要碰命令。

use crate::commands;
use crate::config::{self, Backend, LogLevel};
use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// BootIDEA 命令行。
#[derive(Parser, Debug)]
#[command(
    name = "bootidea",
    version,
    about = "BootIDEA 命令行：扩展宿主、插件管理与 agent 运行",
    long_about = "BootIDEA 命令行。宿主与插件以库和动态库形式提供，所有用户入口都在这里。"
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// 所有子命令共享的参数，也就是配置优先级最高的一层。
#[derive(Args, Debug, Clone, Default)]
pub struct GlobalArgs {
    /// 数据目录（默认取 BOOTIDEA_HOME 或平台数据目录）
    #[arg(long, global = true, value_name = "PATH")]
    pub home: Option<PathBuf>,

    /// 配置文件路径
    #[arg(short, long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// 扩展执行后端
    #[arg(long, global = true, value_enum)]
    pub backend: Option<Backend>,

    /// 拒绝未签名或签名不可信的包
    #[arg(long, global = true)]
    pub require_signature: bool,

    /// 日志级别
    #[arg(long, global = true, value_enum)]
    pub log_level: Option<LogLevel>,
}

/// 具体动作。
#[derive(Subcommand, Debug)]
pub enum Command {
    /// 显示宿主版本、后端与目录
    Info,

    /// 运行本地包的一个方法（开发用）
    Run {
        /// 包含 extension.json 的包目录
        dir: PathBuf,
        /// 要调用的方法
        #[arg(long)]
        method: String,
        /// JSON 参数
        #[arg(long, default_value = "null")]
        params: String,
        /// 授予清单声明的全部能力
        #[arg(long)]
        grant_all: bool,
        /// 先把这些包装进同一个临时安装目录
        #[arg(long = "with", value_name = "DIR")]
        with: Vec<PathBuf>,
    },

    /// 运行一次 agent 循环（需要 harness 插件已安装）
    Agent {
        /// 目标
        goal: String,
        /// 模型名，默认取 BOOTIDEA_MODEL
        #[arg(long)]
        model: Option<String>,
        /// OpenAI 兼容接口地址，默认取 BOOTIDEA_BASE_URL
        #[arg(long)]
        base_url: Option<String>,
        /// 从该环境变量读取密钥，默认 BOOTIDEA_API_KEY
        #[arg(long, default_value = "BOOTIDEA_API_KEY")]
        api_key_env: String,
        /// 提供 agent 能力的扩展 id
        #[arg(long, default_value = "harness")]
        extension: String,
    },

    /// 以 JSON-RPC 对外提供服务
    Serve {
        /// 传输方式
        #[arg(long, value_enum, default_value_t = Transport::Stdio)]
        transport: Transport,
        /// 监听地址（TCP/HTTP）
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// 监听端口，0 表示自动选择（TCP/HTTP）
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// 不加载 onStartup 扩展
        #[arg(long)]
        no_startup: bool,
    },

    /// 管理已安装扩展
    #[command(subcommand)]
    Ext(ExtCommand),

    /// 生成签名密钥
    Keygen {
        /// 密钥文件写到何处（不要放进包目录）
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// 写入签名的发布者名称
        #[arg(long, default_value = "local")]
        publisher: String,
    },

    /// 签名包目录，写出 extension.sig
    Sign {
        /// 包含 extension.json 的包目录
        dir: PathBuf,
        /// keygen 生成的密钥文件
        #[arg(long, value_name = "PATH")]
        key: PathBuf,
        /// 覆盖密钥文件里的发布者名称
        #[arg(long)]
        publisher: Option<String>,
    },

    /// 管理受信任发布者
    #[command(subcommand)]
    Trust(TrustCommand),
}

/// 传输方式。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Transport {
    /// stdin/stdout 上的换行分隔 JSON-RPC
    #[value(alias = "std")]
    Stdio,
    /// HTTP POST /rpc，Bearer 令牌鉴权
    Http,
    /// TCP 上的换行分隔 JSON-RPC，令牌鉴权
    Tcp,
}

/// 扩展管理子命令。
#[derive(Subcommand, Debug)]
pub enum ExtCommand {
    /// 列出已安装扩展
    List,
    /// 查看单个扩展
    Info { id: String },
    /// 安装包目录
    Install {
        path: PathBuf,
        /// 覆盖已有安装
        #[arg(long)]
        force: bool,
        /// 要授予的能力（逗号分隔，必须是清单已声明的）
        #[arg(long = "grant", value_delimiter = ',')]
        grant: Vec<String>,
    },
    /// 卸载、清空存储并删除包
    Uninstall { id: String },
    /// 启用
    Enable { id: String },
    /// 禁用（会先卸载）
    Disable { id: String },
    /// 授予能力
    Grant {
        id: String,
        #[arg(required = true)]
        capabilities: Vec<String>,
    },
    /// 撤销一项能力
    Revoke { id: String, capability: String },
    /// 查看宿主日志
    Logs {
        #[arg(long)]
        id: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// 生成起步包
    Init {
        id: String,
        /// 目标目录（默认 ./<id>）
        #[arg(long)]
        dir: Option<PathBuf>,
        /// 显示名（默认同 id）
        #[arg(long)]
        name: Option<String>,
    },
}

/// 发布者信任库子命令。
#[derive(Subcommand, Debug)]
pub enum TrustCommand {
    /// 列出受信任发布者
    List,
    /// 信任一个发布者公钥
    Add {
        /// 十六进制公钥
        public_key: String,
        /// 便于识别的名称
        #[arg(long)]
        name: String,
    },
    /// 移除受信任发布者
    Remove {
        /// 信任时记录的名称
        name: String,
    },
}

impl Cli {
    /// 解析配置并执行。
    pub fn dispatch(self) -> Result<()> {
        let settings = config::load(&self.global)?;
        crate::log::init(settings.log_level);

        match self.command {
            Command::Info => commands::info(&settings),
            Command::Run {
                dir,
                method,
                params,
                grant_all,
                with,
            } => commands::run(&settings, &dir, &method, &params, grant_all, &with),
            Command::Agent {
                goal,
                model,
                base_url,
                api_key_env,
                extension,
            } => commands::agent(&settings, &goal, model, base_url, &api_key_env, &extension),
            Command::Serve {
                transport,
                host,
                port,
                no_startup,
            } => commands::serve(&settings, transport, host, port, !no_startup),
            Command::Ext(command) => commands::ext(&settings, command),
            Command::Keygen { out, publisher } => commands::keygen(&out, &publisher),
            Command::Sign {
                dir,
                key,
                publisher,
            } => commands::sign(&dir, &key, publisher.as_deref()),
            Command::Trust(command) => commands::trust(&settings, command),
        }
    }
}
