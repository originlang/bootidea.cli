# bootidea.cli

BootIDEA 的命令行入口。宿主与插件都不带 CLI：`originlang.extension.host` 是库，`originlang.extension.harness` 是动态库插件，用户能敲的每一条命令都在这里。

```sh
bootidea ext install ./examples/toolbox --grant storage.read,storage.write,log.write
bootidea ext list
bootidea agent "what is 2+3?" --model gpt-4o-mini --base-url https://api.openai.com/v1
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
src/main.rs            入口与退出码
src/cli.rs             clap 定义、命令分发
src/config.rs          分层配置与 Settings
src/engine.rs          宿主装配：安装目录、后端路由、shutdown 保证
src/log.rs             级别过滤的 stderr 输出
src/commands/          info / ext / run / agent / serve / keygen / sign / trust
src/serve/             JSON-RPC 的 stdio、TCP、HTTP 三种传输
tests/transports.rs    真实进程的传输与鉴权测试
```

## 配置

优先级（高 → 低）：flag > 环境变量 > 配置文件 > 默认值。

| 配置项 | flag | 环境变量 | 配置文件键 | 默认值 |
|---|---|---|---|---|
| 数据目录 | `--home` | `BOOTIDEA_HOME` | `home` | 宿主按平台解析 |
| 执行后端 | `--backend` | `BOOTIDEA_BACKEND` | `backend` | `auto` |
| 强制签名 | `--require-signature` | `BOOTIDEA_REQUIRE_SIGNATURE` | `require_signature` | `false` |
| 日志级别 | `--log-level` | `BOOTIDEA_LOG_LEVEL` | `log_level` | `info` |
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
bootidea.cli  ──path 依赖──▶  originlang.extension.host（引擎库）
      │                              │
      └── 安装并调用 ──▶  originlang.extension.harness（harness.dll 插件）
```

三个仓库并列检出即可构建；宿主与插件都不需要单独的可执行入口。升级宿主的库契约只需改 `Cargo.toml` 里的 path 依赖，入口的适配在这里独立演进。

## 命令

| 命令 | 用途 |
| --- | --- |
| `info` | 宿主版本、可用后端、扩展计数 |
| `ext list/info/install/uninstall/enable/disable/grant/revoke/logs/init` | 扩展全生命周期 |
| `run` | 在一次性安装目录里跑本地包的一个方法，`--with` 可带配套包 |
| `agent` | 跑一次 agent 循环，模型参数来自 flag，密钥只从环境变量读 |
| `serve` | stdio / TCP / HTTP 三种 JSON-RPC 传输 |
| `keygen` / `sign` | Ed25519 包签名 |
| `trust list/add/remove` | 发布者信任库 |

`agent` 要求提供该能力的扩展已安装（默认 id `harness`）。密钥不走命令行参数——那会出现在进程列表和 shell 历史里，所以只认 `--api-key-env` 指定的环境变量（默认 `BOOTIDEA_API_KEY`）。

## 构建

```sh
cargo build --release
cargo test
```
