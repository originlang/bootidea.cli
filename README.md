# bootidea.cli

基于 **Cobra + pflag** 的 CLI 应用骨架。核心只提供命令框架、分层配置与日志，具体能力后续由功能模块/插件提供。

## 目录结构

```
bootidea.cli/
├── cmd/bootidea/main.go        # 入口：信号感知的 context + 退出码
└── internal/
    ├── cli/
    │   ├── root.go             # root 命令、PersistentPreRunE 装配运行时、内置命令注册缝
    │   ├── flags.go            # 全局 persistent flag 与覆盖项提取
    │   ├── configcmd.go        # bootidea config view
    │   └── version.go          # bootidea version
    ├── app/app.go              # App（配置+日志+流）的 context 注入/取出
    ├── config/                 # 分层配置：默认值 → 文件 → 环境变量 → flag
    ├── log/                    # slog 工厂：级别、text/json、ANSI 着色
    ├── iostreams/              # stdin/stdout/stderr 封装 + TTY 探测
    └── version/version.go      # 版本信息（ldflags 注入）
```

## 构建与运行

```sh
go build -o bootidea ./cmd/bootidea
bootidea --help
bootidea version
bootidea config view
bootidea --log-level debug config view
```

注入版本号：

```sh
go build -ldflags "-X bootidea.cli/internal/version.Version=v0.1.0 \
  -X bootidea.cli/internal/version.Commit=$(git rev-parse --short HEAD) \
  -X bootidea.cli/internal/version.Date=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  -o bootidea ./cmd/bootidea
```

## 配置

优先级（高 → 低）：flag > 环境变量 > 配置文件 > 默认值。

| 配置项 | flag | 环境变量 | 配置文件键 | 默认值 |
|---|---|---|---|---|
| 数据目录 | `--home` | `BOOTIDEA_HOME` | `home` | `~/.bootidea` |
| 日志级别 | `--log-level` | `BOOTIDEA_LOG_LEVEL` | `log_level` | `info` |
| 日志格式 | `--log-format` | `BOOTIDEA_LOG_FORMAT` | `log_format` | `text` |
| 颜色 | `--color` | `BOOTIDEA_COLOR` | `color` | `auto` |
| 配置文件路径 | `-c/--config` | `BOOTIDEA_CONFIG` | — | 见下 |

配置文件查找顺序：`--config`/`$BOOTIDEA_CONFIG`（必须存在）→ `$XDG_CONFIG_HOME/bootidea/config.yaml` → `~/.bootidea/config.yaml`。YAML 键名有误会被拒绝（严格模式）。约定：`NO_COLOR` 非空时颜色强制为 `never`（仍可被 `--color` 覆盖）。

## 添加命令（后续插件接入点）

在 `internal/cli/root.go` 的 `builtinCommands()` 中追加命令；新命令包通过 `app.From(cmd.Context())` 获取 `App`（配置、日志、输出流）。核心不感知具体功能，模块化/插件化在此基础上演进。

## 更换模块路径

当前使用占位模块名 `bootidea.cli`，替换为正式路径只需：

```sh
go mod edit -module=<你的路径> && go mod tidy
```

代码中仅 `internal/version` 的 ldflags 参数与 README 出现模块路径。
