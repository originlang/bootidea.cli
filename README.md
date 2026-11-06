# bootidea.cli

BootIDEA 的命令行入口。宿主与插件都不带 CLI：`originlang.extension.host` 是库，`originlang.extension.harness` 是动态库插件，用户能敲的每一条命令都在这里。

两个可执行入口 `bootidea` 与 `bi` 共用同一份实现，行为完全一致；`bi` 是日常快捷入口，`bootidea` 兼容既有脚本。

```sh
bi repo scan ~/Developer --depth 4
bi repo status --all
bi repo status --all --dirty --cached
bi repo group create platform && bi repo group add platform bootidea.cli
bi repo pull --all --dry-run
bi config set repo.root ~/Developer
bi doctor

bootidea ext install ./examples/toolbox --grant storage.read,storage.write,log.write
bootidea serve --transport http --port 7311
```

## 三个部分

| 关注点 | 归谁 | 说明 |
| --- | --- | --- |
| 命令入口 | `clap` | 参数解析成结构体，`dispatch` 分发给命令实现 |
| 配置合并 | `config` | 默认值、配置文件、环境变量、flag 合成一张表 |
| 类型转换 | `serde` | 那张表反序列化成 `Settings` |

`src/cli.rs` 只管形状，`src/config.rs` 只管来源，`src/commands/` 只管动作。新增命令不动配置，新增配置项不动命令。

## 目录

```
src/main.rs            入口与退出码（bi 与 bootidea 两个 bin 共用）
src/cli.rs             clap 定义、命令分发
src/config.rs          分层配置与 Settings
src/engine.rs          宿主装配：安装目录、后端路由、shutdown 保证
src/log.rs             级别过滤的 stderr 输出
src/commands/          info / repo / config / doctor / ext / run / agent / serve / keygen / sign / trust
src/commands/repo/     repo 命令组：索引与锁、扫描、Git 采集、目标选择、批量调度
src/serve/             JSON-RPC 的 stdio、TCP、HTTP 三种传输
tests/transports.rs    真实进程的传输与鉴权测试
tests/repo.rs          repo 端到端测试（真实二进制 + 真实 Git）
```

## 配置

优先级（高 → 低）：flag > 环境变量 > 配置文件 > 默认值。

| 配置项 | flag | 环境变量 | 配置文件键 | 默认值 |
|---|---|---|---|---|
| 数据目录 | `--home` | `BOOTIDEA_HOME` | `home` | 宿主按平台解析 |
| 执行后端 | `--backend` | `BOOTIDEA_BACKEND` | `backend` | `auto` |
| 强制签名 | `--require-signature` | `BOOTIDEA_REQUIRE_SIGNATURE` | `require_signature` | `false` |
| 日志级别 | `--log-level` | `BOOTIDEA_LOG_LEVEL` | `log_level` | `info` |
| 仓库克隆根目录 | — | `BOOTIDEA_REPO__ROOT` | `repo.root` | 未设置 |
| 编辑器 | — | `BOOTIDEA_EDITOR` | `editor` | 回退 `EDITOR`/`VISUAL`/系统默认 |
| 配置文件 | `-c/--config` | `BOOTIDEA_CONFIG` | — | 见下 |

配置文件查找顺序：

1. `--config` / `$BOOTIDEA_CONFIG`，指定的文件必须存在；
2. `$XDG_CONFIG_HOME/bootidea/config.{toml,yaml}`，或 `~/.config/bootidea/`；
3. `~/.bootidea/config.{toml,yaml}`。

默认位置上的文件是可选的；显式指定却不存在会直接报错，而不是静默用默认值跑下去。

```toml
# ~/.bootidea/config.toml
home = "/srv/bootidea"
backend = "quickjs"
log_level = "debug"
```

## 与两个仓库的关系

```
bootidea.cli  ──git 依赖（rev 锁定）──▶  originlang.extension.host（引擎库）
      │                                        │
      └── 安装并调用 ──▶  originlang.extension.harness（harness.dll 插件）
```

三个仓库互不要求并列检出：入口从 `Cargo.toml` 里写死的 git rev 取宿主 crate，单独 clone 本仓库就能构建。宿主与插件都不需要单独的可执行入口。宿主的库契约升级时，改这里的 rev 再 `cargo update`；入口的适配在这里独立演进。

## 命令

`bi` / `bootidea` 两个入口可用同一套命令。`repo`、`config`、`doctor` 已实现；`app`（软件）、`env`（开发环境）见 [CLI 子命令设计](docs/command-design.md)，仍为规划。

| 命令 | 用途 |
| --- | --- |
| `repo clone/add/scan/list/info/open/status/refresh/relocate/remove` | Git 仓库登记与索引维护；索引在数据根目录 `repos.json`（v2） |
| `repo fetch/pull/diff/branch/switch/start/push/foreach` | 跨仓库批量操作，支持 `--all`、`--group`、`--jobs`、`--dry-run`、`--fail-fast`、`--json` |
| `repo group create/add/list/remove/delete` | 仓库分组，成员按稳定 ID 关联 |
| `config list/get/set` | 查询生效配置；`set` 仅写入用户键（`repo.root`、`editor`） |
| `doctor` | 基础环境诊断：git、配置文件、repo.root、editor、数据目录可写性 |
| `info` | 宿主版本、可用后端、扩展计数 |
| `ext list/info/install/uninstall/enable/disable/grant/revoke/logs/init` | 扩展全生命周期 |
| `run` | 在一次性安装目录里跑本地包的一个方法，`--with` 可带配套包 |
| `agent` | 跑一次 agent 循环，模型参数来自 flag，密钥只从环境变量读 |
| `serve` | stdio / TCP / HTTP 三种 JSON-RPC 传输 |
| `keygen` / `sign` | Ed25519 包签名 |
| `trust list/add/remove` | 发布者信任库 |

`repo` 组说明：首次 `scan/add/clone` 建立索引，日常命令读索引，显式扫描才展开目录。目标支持登记名称或本地路径；名称/路径列表与 `--all`、`--group`、`--under` 互斥，`--match`/`--exclude` 过滤后按路径去重。`list` 与 `status --cached` 不调用 Git；`--cached` 始终显示采集时间，缺失快照显示“未采集”而不是“干净”。`fetch` 只获取远端更新；`pull` 只允许快进；`push` 不隐式 force 或设置 upstream；写类命令执行前实时验证目标，执行后让相关快照过期。批量操作逐仓库报告，部分失败时以非零码退出。`remove` 只取消登记、保留本地文件。`repo open` 的编辑器按 `editor` 配置 > `EDITOR`/`VISUAL` > 系统默认解析。

仓库索引 `repos.json` 版本为 2，记录扫描根目录规则、分组、可访问状态（`unknown/available/missing/inaccessible/invalid`）与状态快照（远端、分支、HEAD、改动、upstream、领先/落后、采集时间）。读取不写盘；写入用 `repos.lock` 排他锁保护“重新读取 → 修改 → 临时文件 → 原子替换”，Git 操作不持锁，提交时按稳定 ID 合并。旧版无 `version` 的登记表在首次写入时迁移，并把原文件备份为 `repos.json.v1.bak`；损坏或不支持的版本直接报错，不覆盖为空表。时间统一以 UTC RFC 3339 记录。裸仓库暂不支持。完整行为见 [repo 批量管理与仓库索引设计](docs/repo-design.md)。

`agent` 要求提供该能力的扩展已安装（默认 id `harness`）。密钥不走命令行参数——那会出现在进程列表和 shell 历史里，所以只认 `--api-key-env` 指定的环境变量（默认 `BOOTIDEA_API_KEY`）。

## 构建

```sh
cargo build --release
cargo test
```

宿主 crate 以 git 依赖锁定在固定 rev 上：本仓库可单独检出，不要求宿主仓库放在同级目录，但首次构建需要能访问 GitHub（之后走 `~/.cargo/git` 缓存与 `Cargo.lock`）。

本地改动**不会**自动编译成 `target/debug/bootidea.exe`——Rust 没有热重建，必须显式执行 `cargo build` / `cargo run` / `cargo test`。仓库里也没有 `build.rs`，构建不产生额外的代码生成步骤。

## CI 与发布

| 工作流 | 触发 | 作用 |
| --- | --- | --- |
| `.github/workflows/ci.yml` | push / PR 到 `feature/dev` | Linux + Windows 编译并跑测试；fmt 与 clippy 作为门禁 |
| `.github/workflows/release.yml` | push `vX.Y.Z` 标签 | 按 tag 检出代码，测试并构建双平台产物，创建 GitHub Release 并挂资产 |

发布采用 **push tag 即发布**：版本号就是 tag 本身（`git tag v0.1.1 && git push origin v0.1.1`）。打 tag 前把 `Cargo.toml` 的 `version` 改成目标版本即可，无需 release-please，也无需配置任何额外 secret。
