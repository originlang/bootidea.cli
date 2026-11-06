# CLI 子命令设计

更新日期：2026-11-06。实施进度：`bi`/`bootidea` 双入口、`repo` 命令组（含索引 v2、缓存、分组与批量命令）、`config` 与 `doctor` 已实现；`app`、`env` 仍为规划。当前可执行入口为 `bootidea` 与 `bi`，共用同一份实现。架构职责遵循根目录 `AGENTS.md`。

## 命名与职责

计划新增 `bi` 作为快捷入口，保留 `bootidea` 兼容现有脚本。两个入口共用命令实现，继续沿用现有配置目录及 `BOOTIDEA_*` 环境变量。暂不增加 `boot` 入口，避免与仅用于启动的命令含义混淆。

统一语法：`bi <对象> <动作> [目标] [选项]`。

| 命令组 | 管理对象 | 主要职责 |
| --- | --- | --- |
| `bi app` | 软件 | 搜索、安装、更新、卸载和启动 |
| `bi repo` | Git 仓库 | 克隆、登记、查找、打开和跨仓库状态汇总 |
| `bi env` | 开发环境 | 工具链安装、版本选择和项目环境准备 |
| `bi config` | CLI 配置 | 查询与修改用户配置 |
| `bi doctor` | 本机工作环境 | 汇总 CLI、软件来源、Git 和工具链诊断 |

这些基础能力不依赖 Host。现有 `ext` 用于 BootIDEA 插件管理，与桌面软件和开发工具链分开；扩展能力按目标架构通过可选 Host 获取。本文不新增宿主服务入口，现有 `serve` 的迁移仍按路线图进行。

## app：管理软件

```sh
bi app search vscode           # 搜索软件
bi app install vscode          # 安装软件
bi app list                    # 列出已安装软件
bi app info vscode             # 查看版本、路径和安装来源
bi app update vscode           # 更新指定软件
bi app update --all             # 更新全部软件
bi app uninstall vscode        # 卸载软件
bi app open vscode             # 启动软件
```

优先对接系统包管理器。搜索、安装和信息输出应显示实际来源及软件标识；多个来源或软件匹配同一名称时，不静默选择。支持的平台、来源适配及消歧参数在实施时确定。

`app` 管理用户软件，`env` 管理需要按项目选择版本的开发工具链。应识别已有安装来源，避免两个命令组重复接管同一安装。

## repo：管理 Git 仓库

维护已登记仓库的名称、路径和远端信息，使用户无需记住每个项目的位置。目标采用“首次扫描建立索引，日常命令读取索引，显式扫描更新索引”，不在每次操作前递归开发目录。完整命令、索引格式、刷新规则和验收要求见 [repo 批量管理与仓库索引设计](repo-design.md)。

已按 [repo 批量管理与仓库索引设计](repo-design.md) 实现：登记表升级为版本化索引 `repos.json`（v2），记录扫描根目录、分组、可访问状态与状态快照；日常命令只读索引，显式扫描才展开目录。

```sh
bi repo scan ~/Developer        # 扫描、登记并保存根目录规则
bi repo scan . --depth 5 --exclude 'archive/**' --nested
bi repo scan --known-roots      # 按保存的规则重扫
bi repo add . --name backend    # 登记已有仓库
bi repo clone <url> --dir ./x   # 克隆并登记
bi repo list                    # 只读索引，不调用 Git
bi repo status --all            # 实时汇总全部仓库
bi repo status --all --cached   # 只读快照，显示采集时间与过期标记
bi repo refresh                 # 刷新全部已登记仓库，不发现新仓库
bi repo pull --all --dry-run    # 只快进，先看计划
bi repo foreach --group platform -- cargo check
bi repo group create platform && bi repo group add platform bootidea.cli
bi repo relocate backend ~/code/backend
bi repo remove backend          # 取消登记，保留本地文件
```

- 仓库目标支持登记名称或本地路径；名称/路径列表与 `--all`、`--group`、`--under` 互斥，`--match`/`--exclude` 取交集后按路径去重。
- `add` 和 `scan` 重复执行不重复登记同一路径；扫描时同名不同路径报告冲突，不静默覆盖。
- `remove` 只移除登记信息，不删除工作目录；`relocate` 保留 ID、名称与分组。
- `fetch` 获取远端更新；`pull` 只允许快进，不合并、不推送、不切换分支；`push` 不隐式 force 或设置 upstream。
- 批量操作逐仓库报告结果，支持 `--jobs`、`--dry-run`、`--fail-fast` 与 `--json`，部分失败时以非零码退出。
- 提交、变基等复杂流程继续使用 Git 或 `foreach`；不提供含义模糊的 `sync`。裸仓库暂不支持。

## env：管理开发环境

区分机器上已安装的工具链版本和项目声明使用的版本。

```sh
bi env list                    # 列出已安装工具链和版本
bi env install node@22         # 安装指定工具链版本
bi env install python@3.12
bi env install rust@stable
bi env uninstall node@20       # 卸载指定版本

bi env init                    # 为当前项目生成环境配置
bi env use node@22             # 设置当前项目的 Node 版本
bi env use python@3.12
bi env use node@22 --global     # 设置用户默认版本
bi env setup                   # 根据项目配置安装所需工具链
bi env exec -- node -v         # 在项目环境下运行命令
bi env shell                   # 进入使用项目环境的子 shell
bi env doctor                  # 检查缺失工具、版本和 PATH 冲突
```

- `install` 负责安装，`use` 负责记录版本选择；选择尚未安装的版本时提示执行安装或 `setup`。
- 项目版本选择优先于用户默认版本；具体配置文件名、格式及与现有工具版本文件的兼容规则待实施时确定。
- `setup` 安装项目声明的工具链，不隐式安装项目业务依赖；重复执行应复用已满足要求的版本。
- 示例中的主版本和 `stable` 是版本选择器，实际解析出的具体版本应展示并记录；锁定格式另行确定。
- 普通 CLI 进程不能修改父终端环境。`use` 不承诺立即改变当前终端的 PATH，通过 `exec` 或 `shell` 使用项目环境，自动切换需要后续 shell 集成。
- `exec` 传递 `--` 后的程序与参数，并传递子进程退出码；退出子 shell 后恢复原终端环境。

## 通用配置与诊断

```sh
bi config list                         # 查看配置
bi config get repo.root                # 查看默认仓库目录
bi config set repo.root E:\originlang  # 设置默认仓库目录
bi doctor                              # 汇总基础环境诊断
```

`repo.root` 为拟新增配置项，用于未指定目标目录时的仓库克隆位置。配置命令应遵循已有配置优先级，并标明生效值的来源；敏感值不得明文展示。编辑器选择等配置键在实施时补充。

`doctor` 默认只诊断，给出具体修复建议；未安装可选 Host 不应导致基础环境诊断失败。

## 第一阶段交付范围

优先打通“安装编辑器 → 获取项目 → 准备环境 → 开始开发”的完整流程。以下均为规划示例，假设目标仓库已提交项目环境配置，且项目使用 npm：

```sh
bi app install vscode
bi repo clone <url>
# 进入克隆出的项目目录
bi env setup
bi env exec -- npm install
bi repo open .
```

第一阶段包括双入口兼容、一个软件来源适配、仓库登记与克隆/打开、一个工具链适配及项目环境执行；其余软件来源、工具链和批量管理能力后续扩展。具体首发平台与适配器待实施时确定。

验收重点：两个入口行为一致；无 Host 时基础命令可用；仓库取消登记不删除文件；批量 fetch 不修改工作区或分支；项目版本选择仅影响受控执行环境；文档明确列出各平台实际支持的能力。
