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

    /// 管理已登记的 Git 仓库
    #[command(subcommand)]
    Repo(RepoCommand),

    /// 查询与修改用户配置
    #[command(subcommand)]
    Config(ConfigCommand),

    /// 汇总基础环境诊断
    Doctor,
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

/// 仓库管理子命令。
///
/// 仓库目标支持登记名称或本地路径；登记名称在登记时保证唯一。批量命令共用
/// [`TargetArgs`]：名称/路径列表、`--all`、`--group`、`--under` 互斥，其余
/// 筛选条件取交集。
#[derive(Subcommand, Debug)]
pub enum RepoCommand {
    /// 克隆并登记仓库
    Clone {
        /// 远端地址
        url: String,
        /// 目标目录（默认取 repo.root 配置下的仓库名）
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// 登记名称（默认取仓库名）
        #[arg(long)]
        name: Option<String>,
    },
    /// 登记已有仓库
    Add {
        path: PathBuf,
        /// 指定唯一登记名称
        #[arg(long)]
        name: Option<String>,
    },
    /// 扫描目录，发现并登记其中的仓库
    Scan {
        /// 扫描根目录；与 --known-roots 互斥
        #[arg(value_name = "DIR", conflicts_with = "known_roots")]
        dir: Option<PathBuf>,
        /// 扫描深度，根目录为第 0 层（默认 3；重扫同一根目录时沿用保存值）
        #[arg(long, value_name = "N")]
        depth: Option<u8>,
        /// 排除相对扫描根目录的路径（glob，可重复；不含 / 时也匹配目录名）
        #[arg(long, value_name = "GLOB")]
        exclude: Vec<String>,
        /// 继续搜索仓库内部的嵌套仓库
        #[arg(long)]
        nested: bool,
        /// 只展示发现结果，不写索引或根目录配置
        #[arg(long)]
        dry_run: bool,
        /// 按保存的规则重扫全部已知根目录
        #[arg(long)]
        known_roots: bool,
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 列出已登记仓库（只读索引，不调用 Git）
    List {
        /// 只列出该分组内的仓库
        #[arg(long, value_name = "GROUP")]
        group: Option<String>,
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 查看单个仓库的实时信息
    Info {
        target: String,
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 用配置的编辑器打开仓库
    Open {
        /// 登记名称或本地路径（默认当前仓库）
        target: Option<String>,
    },
    /// 查看仓库状态；不带目标时查看当前仓库
    Status {
        #[command(flatten)]
        select: TargetArgs,
        /// 仅列出工作区有改动的仓库
        #[arg(long)]
        dirty: bool,
        /// 只读状态快照，不调用 Git
        #[arg(long, conflicts_with = "under")]
        cached: bool,
        /// 并发数，默认串行
        #[arg(long, default_value_t = 1, value_name = "N")]
        jobs: usize,
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 获取各仓库所有已配置远端的更新
    Fetch(BatchArgs),
    /// 仅快进更新当前分支
    Pull(BatchArgs),
    /// 查看各仓库的差异
    Diff {
        #[command(flatten)]
        batch: BatchArgs,
        /// 展示已暂存的差异
        #[arg(long)]
        staged: bool,
    },
    /// 列出各仓库本地分支
    Branch(BatchArgs),
    /// 切换到已有分支
    Switch {
        /// 分支名（必须已存在）
        branch: String,
        #[command(flatten)]
        batch: BatchArgs,
    },
    /// 基于各仓库当前 HEAD 创建并切换分支
    Start {
        /// 新分支名（已存在时报错）
        branch: String,
        #[command(flatten)]
        batch: BatchArgs,
    },
    /// 推送当前分支到已配置的 upstream
    Push(BatchArgs),
    /// 在各目标工作目录运行程序
    Foreach {
        #[command(flatten)]
        batch: BatchArgs,
        /// `--` 之后的程序与参数，直接传递，默认不经过 shell
        #[arg(last = true, required = true, num_args = 1.., value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// 刷新已登记仓库的可访问状态与状态快照
    Refresh {
        /// 登记名称或本地路径；缺省刷新全部已登记仓库
        #[arg(value_name = "TARGET")]
        targets: Vec<String>,
        /// 并发数，默认串行
        #[arg(long, default_value_t = 1, value_name = "N")]
        jobs: usize,
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 更新仓库位置，保留 ID、名称和分组
    Relocate {
        /// 登记名称或本地路径
        target: String,
        /// 新位置
        path: PathBuf,
    },
    /// 取消登记，保留本地文件
    Remove { target: String },
    /// 管理仓库分组
    #[command(subcommand)]
    Group(GroupCommand),
}

/// 批量命令的目标选择参数。
#[derive(Args, Debug, Clone, Default)]
pub struct TargetArgs {
    /// 登记名称或本地路径；可写多个
    #[arg(value_name = "TARGET", conflicts_with_all = ["all", "group", "under"])]
    pub targets: Vec<String>,
    /// 选择全部已登记仓库
    #[arg(long, conflicts_with_all = ["group", "under"])]
    pub all: bool,
    /// 选择分组内的仓库
    #[arg(long, value_name = "GROUP", conflicts_with = "under")]
    pub group: Option<String>,
    /// 临时扫描该目录，不保存发现结果
    #[arg(long, value_name = "DIR")]
    pub under: Option<PathBuf>,
    /// 按登记名称 glob 过滤
    #[arg(long = "match", value_name = "GLOB")]
    pub pattern: Option<String>,
    /// 排除登记名称（glob，可重复）
    #[arg(long, value_name = "GLOB")]
    pub exclude: Vec<String>,
}

/// 批量 Git 命令的公共参数。
#[derive(Args, Debug, Clone)]
pub struct BatchArgs {
    #[command(flatten)]
    pub select: TargetArgs,
    /// 并发数，默认串行；共享共同 Git 目录的 worktree 始终串行
    #[arg(long, default_value_t = 1, value_name = "N")]
    pub jobs: usize,
    /// 展示固定后的目标与操作，不执行写操作
    #[arg(long)]
    pub dry_run: bool,
    /// 某个仓库失败后停止调度新任务
    #[arg(long)]
    pub fail_fast: bool,
    /// JSON 输出
    #[arg(long)]
    pub json: bool,
}

impl Default for BatchArgs {
    fn default() -> Self {
        Self {
            select: TargetArgs::default(),
            jobs: 1,
            dry_run: false,
            fail_fast: false,
            json: false,
        }
    }
}

/// 仓库分组子命令。
#[derive(Subcommand, Debug)]
pub enum GroupCommand {
    /// 创建分组
    Create { name: String },
    /// 把仓库加入分组
    Add {
        name: String,
        #[arg(required = true, value_name = "TARGET")]
        repos: Vec<String>,
    },
    /// 列出分组及成员
    List {
        /// JSON 输出
        #[arg(long)]
        json: bool,
    },
    /// 把仓库移出分组
    Remove {
        name: String,
        #[arg(required = true, value_name = "TARGET")]
        repos: Vec<String>,
    },
    /// 删除分组（不删除仓库登记）
    Delete { name: String },
}

/// 用户配置子命令。
#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// 查看配置
    List,
    /// 查看单个配置项
    Get { key: String },
    /// 写入用户配置文件（repo.root、editor）
    Set { key: String, value: String },
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
            Command::Repo(command) => commands::repo(&settings, command),
            Command::Config(command) => {
                commands::config(&settings, self.global.config.as_ref(), command)
            }
            Command::Doctor => commands::doctor(&settings),
        }
    }
}
