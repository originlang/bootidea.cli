# repo 批量管理与仓库索引设计

更新日期：2026-11-06。状态：已按本设计实现（索引 v2、扫描规则、目标选择、批量命令、分组与 JSON 输出）。未实现范围见文末“实现状态”。

## 目标与实现边界

`bi repo` 通过识别 `.git` 发现本地仓库，保存索引，统一选择目标并批量调用本机 Git。首次扫描建立索引，日常命令读取索引，显式扫描更新索引。日常操作成本与已登记仓库数量有关，不依赖整个开发目录的文件数量。

这些能力属于独立 CLI，不依赖 Host，不启动后台扫描服务。提交、变基等复杂 Git 工作流可通过 Git 或 `foreach` 执行。暂不提供 `sync`；如后续引入可分享的仓库清单，再单独定义克隆、版本选择和同步语义。

当前命令为 `clone / add / scan / list / info / open / status / fetch / pull / diff / branch / switch / start / push / foreach / refresh / relocate / remove / group`。`repos.json` 已是下文描述的 v2 索引，扫描、缓存、目标选择与批量执行按本文实现；具体实现边界与未实现项见文末“实现状态”。

## 发现与登记

```sh
bi repo scan ~/Developer                # 扫描、登记，并保存扫描根目录及规则
bi repo scan . --depth 5                # 限制扫描深度
bi repo scan . --exclude 'archive/**'   # 排除相对扫描根目录的路径
bi repo scan . --nested                 # 继续搜索仓库内部的嵌套仓库
bi repo scan . --dry-run                # 展示发现结果，不写入索引或根目录配置
bi repo scan --known-roots              # 按保存的规则重扫全部已知根目录
bi repo add .                          # 登记当前仓库，不搜索周边目录
bi repo add ../api --name backend      # 指定唯一登记名称
bi repo clone <url> --dir ./backend     # 克隆并登记
bi repo remove backend                 # 取消登记，保留本地文件
bi repo relocate backend <新路径>      # 验证新路径并更新位置，保留 ID、名称和分组
```

扫描以 `.git` 目录或指针文件为候选标记，再调用 Git 验证工作目录及 Git 目录，支持 worktree 和 submodule。默认发现仓库后停止向下搜索；`--nested` 才继续遍历。始终跳过 `.git` 内部，默认跳过 `node_modules`、`target`、`dist`、`build`、`vendor`、`.venv`、`.gradle`，不跟随目录符号链接。裸仓库支持留待后续设计。

默认深度为 3，与现有行为一致；根目录为第 0 层，`--depth 0` 只检查根目录本身。排除规则针对相对根目录路径匹配，可重复传入。重扫同一路径时沿用保存规则，显式传入的规则覆盖对应配置。`--known-roots` 与位置参数互斥，使用各根目录已保存的规则。

按规范化工作目录路径去重，重叠根目录只产生一份仓库记录。各 worktree 是不同工作目录记录，但记录共同 Git 目录供执行调度使用。同名不同路径不得静默覆盖；报告冲突，用户可通过 `add --name` 消歧，其余可登记结果继续保存。

扫描合并新增记录，保留已有 ID、名称和分组，不因未被本次扫描覆盖而删除或标记已有记录。路径缺失必须通过直接检查该路径确认；权限不足或根目录不可访问单独报告。`remove` 不加入忽略名单，后续显式扫描仍可能重新登记该路径。

## 索引、元信息与状态快照

继续使用当前数据目录中的 `repos.json`，升级为版本化索引，不另建一份重复登记表。

| 数据 | 保存内容 | 更新时机 |
| --- | --- | --- |
| 扫描根目录 | 规范化路径、深度、排除规则、嵌套开关、上次成功扫描时间 | 显式 `scan` |
| 仓库索引 | 稳定 ID、唯一名称、工作目录、分组、Git 目录、共同 Git 目录、可访问状态、最后验证时间 | `scan / add / clone / relocate / refresh` |
| 状态快照 | 远端、分支、HEAD、改动情况、upstream、领先/落后数量、采集时间 | `refresh`、实时 `status / info` |

示例结构如下，时间统一使用 UTC，展示时可转换成本地时间：

```json
{
  "version": 2,
  "roots": [
    {
      "path": "/Users/snow/Developer",
      "depth": 3,
      "exclude": [".git", "node_modules", "target", "dist", "build", "vendor", ".venv", ".gradle"],
      "nested": false,
      "last_scanned_at": "2026-11-06T08:00:00Z"
    }
  ],
  "groups": ["platform"],
  "repos": [
    {
      "id": "repo_01",
      "name": "bootidea.cli",
      "path": "/Users/snow/Developer/bootidea.cli",
      "groups": ["platform"],
      "git_dir": "/Users/snow/Developer/bootidea.cli/.git",
      "common_git_dir": "/Users/snow/Developer/bootidea.cli/.git",
      "availability": "available",
      "last_seen_at": "2026-11-06T08:00:00Z",
      "snapshot": {
        "remotes": {"origin": "https://example.com/bootidea.cli.git"},
        "branch": "main",
        "head": null,
        "dirty": false,
        "upstream": null,
        "ahead": null,
        "behind": null,
        "observed_at": "2026-11-06T08:00:00Z",
        "stale": false
      }
    }
  ]
}
```

`snapshot` 可以为 `null`，表示尚未采集。没有提交时 HEAD 可为空，detached HEAD 时分支为空，无 upstream 时领先/落后数为空，不显示成 0。`dirty` 包含暂存、未暂存和未跟踪文件；忽略文件不计入。远端地址中的认证信息不得写入快照或显示。

`availability` 区分 `unknown / available / missing / inaccessible / invalid`。目录消失、磁盘未挂载、权限变化或不再是 Git 仓库时保留登记信息；失败不覆盖旧快照为“干净”。仓库移动通过 `relocate` 显式确认，不能仅凭相同远端认定为同一个仓库。

## 缓存读取与刷新

```sh
bi repo list                           # 只读取索引，不遍历仓库或执行 Git
bi repo info backend                   # 实时读取单个仓库信息
bi repo status --all                   # 仅访问已登记路径，实时查询 Git
bi repo status --all --cached          # 只读快照，展示采集时间和过期标记
bi repo refresh                        # 刷新全部已登记仓库，不发现新仓库
bi repo refresh backend                # 刷新指定仓库
```

`refresh` 不 fetch、不递归、不搜索新仓库；更新已知路径的可访问状态、Git 目录及快照。它是默认作用于全部登记仓库的索引维护命令，与默认只作用于当前仓库的 Git 操作命令区分。`list` 同样默认列出全部登记记录。

缓存不设置自动触发扫描的 TTL。`--cached` 永远显示采集时间，没有快照则显示“未采集”；`stale: false` 只表示未被 CLI 已知操作失效，不保证外部 Git 操作后仍然最新。执行 `fetch / pull / switch / start / push / foreach` 后使受影响快照失效，包括失败后可能已发生部分修改的情况；下次实时查询或 `refresh` 才重新采集。

`pull`、`switch` 等写操作始终实时验证目标，不依据缓存判断工作区是否干净。`status` 的领先/落后值来自本地远端跟踪引用，不自动联网；需要最新远端状态时先执行 `fetch`。

## 统一目标选择

```sh
bi repo status                         # 当前仓库，可从仓库子目录执行
bi repo status backend frontend        # 多个名称或本地路径
bi repo status --all                   # 所有已登记仓库
bi repo status --group platform        # 分组中的仓库
bi repo status --under ~/Developer     # 显式临时扫描，不保存发现结果
bi repo status --all --dirty           # 仅工作区有改动的仓库
bi repo status --all --match 'boot*'    # 按登记名称 glob 匹配
bi repo status --all --exclude 'demo*'  # 排除登记名称
```

Git 操作不带范围时只作用于当前仓库，不隐式扩大为全部仓库；当前仓库和显式路径不要求预先登记。名称/路径列表、`--all`、`--group`、`--under` 互斥，多个筛选条件取交集，执行前去重并固定目标集合。不存在的显式目标或分组报错；合法范围筛选后为空时输出“0 个目标”，不回退到其他范围。

`--under` 是明确请求临时递归发现的例外，默认沿用扫描默认规则；它不作为日常使用的必需参数，与 `--cached` 互斥。`--exclude` 在批量选择中匹配登记名称，在 `scan` 中匹配相对路径，帮助信息必须写明差别。

`--dirty` 在实时模式查询真实工作区；在 `status --cached` 中仅使用快照，缺失快照的记录不视为干净，单独报告未知数量。`--cached` 仅用于状态读取，不用于写操作。

## 批量命令与分组

| 命令 | 语义 |
| --- | --- |
| `status` | 汇总分支、改动、领先/落后情况 |
| `fetch` | 获取各仓库所有已配置远端的更新，不切换分支或修改工作区 |
| `pull` | 更新当前分支，仅允许快进；无 upstream、工作区不干净或分叉时报错 |
| `diff` | 默认展示未暂存差异；`--staged` 展示暂存差异，未跟踪内容不计入 diff |
| `branch` | 列出各仓库本地分支 |
| `switch <branch>` | 切换已有分支，不隐式创建或强制丢弃改动 |
| `start <branch>` | 基于各仓库当前 HEAD 创建并切换分支，已有同名分支时报错 |
| `push` | 推送当前分支到已配置 upstream，不隐式 force 或设置 upstream |
| `foreach -- <程序> <参数>` | 在各目标工作目录运行程序，直接传递参数，默认不经过 shell |

```sh
bi repo group create platform
bi repo group add platform bootidea.cli bootidea.host
bi repo group list
bi repo group remove platform bootidea.host
bi repo group delete platform

bi repo fetch --group platform --jobs 8
bi repo pull --all --dry-run
bi repo start feat/login --group platform
bi repo switch main --group platform
bi repo foreach --all -- git log -1 --oneline
bi repo foreach --group platform -- cargo check
```

一个仓库可属于多个组；组成员使用仓库稳定 ID 关联。删除分组不删除仓库登记，取消仓库登记同时移除成员关系。未知组名或成员名报错。

## 执行、输出与持久化

- `--jobs N` 控制并发，N 必须大于 0，默认串行。共享共同 Git 目录的 worktree，在修改类操作和任意 `foreach` 执行中串行调度。
- `--dry-run` 展示固定后的目标及操作，不执行写操作；`push --dry-run` 也只是本地计划，不隐式联网探测。
- 默认某仓库失败后继续其他目标；`--fail-fast` 停止调度新任务，等待已启动任务结束。跨仓库操作不提供事务或自动回滚。
- 每个仓库输出成功、失败或跳过结果及错误原因，最后汇总；无法处理的目标、扫描错误或执行失败返回非零退出码，筛选排除不算失败。
- `status` 默认表格输出；`list / info / status / refresh / scan` 和批量执行支持 `--json`，包含仓库 ID/路径、结果、错误、快照时间及汇总。JSON 模式不向 stdout 混入进度文本；`foreach` 子进程输出作为每仓库结果字段，进度发往 stderr。
- 索引更新使用独立锁文件保护读改写，临时文件写完后原子替换 `repos.json`。耗时 Git 操作不持有全局索引锁，提交结果时重新读取并按 ID 合并；期间被删除或迁移的记录不得被旧结果恢复或覆盖。
- 旧版无 `version` 的登记表按 v1 读取，保留名称、路径、远端并生成稳定 ID；未知 Git 目录、根目录历史和状态不凭空推导。首次需要写入时在锁内备份旧文件并迁移到 v2，纯只读命令不强制写盘。损坏或不支持的版本应报错，不覆盖为空表。

## 交付顺序与验收

第一阶段：索引 v2、兼容迁移、扫描规则与根目录记录、`refresh / relocate`、实时状态表格及 `--cached`。第二阶段：统一目标选择、分组、并发 fetch、仅快进 pull、foreach 和 JSON 输出。第三阶段：diff、branch、switch、start、push 等分支协作命令。

验收要求：

1. 首次扫描后，重复 `list`、`status --all`、`fetch --all` 均不递归扫描开发目录；`list` 和 `status --cached` 不调用 Git。
2. 新仓库只通过显式 `scan / add / clone` 登记；`refresh` 不发现新仓库，临时 `--under` 不持久登记。
3. 支持有效 `.git` 目录及文件，拒绝伪造标记；嵌套、排除、深度及符号链接规则可验证。
4. 重叠扫描不重复登记，不丢失名称、分组和 ID；同名冲突、部分不可访问和缺失路径均可见且不误删记录。
5. 缓存显示采集时间，未知状态不显示为干净；实时状态和修改操作不依赖旧快照。
6. 旧登记表可迁移，并发更新不丢记录；写入中断后保留完整旧文件或完整新文件。
7. 批量部分失败返回非零码；只快进 pull 不自动合并或变基，共享 Git 目录的写操作不并发执行。
8. 无 Host 时功能可用；文档与帮助明确区分已实现和规划能力。

## 实现状态

已实现（代码在 `src/commands/repo/`，端到端测试在 `tests/repo.rs`）：

- 索引 v2：扫描根目录、分组、仓库登记、可访问状态与状态快照；v1 首次写入时备份为 `repos.json.v1.bak` 并迁移。
- 并发安全：`repos.lock` 排他锁保护读改写，临时文件 + 原子替换；Git 操作不持锁，提交时按 ID 合并，已删除或迁移的记录不会被旧结果恢复。
- 扫描：`.git` 目录或指针文件为候选，`rev-parse` 验证工作目录；深度、排除 glob、`--nested`、`--dry-run`、`--known-roots`；默认跳过依赖与构建目录，不跟随目录符号链接；worktree 与 submodule 可识别。
- 目标选择：名称/路径列表、`--all`、`--group`、`--under`、`--match`、`--exclude`，按规范化路径去重；从仓库子目录可定位当前仓库。
- 缓存：`list` 与 `status --cached` 不调用 Git；`--cached` 显示采集时间与过期标记，缺失快照显示“未采集”而非“干净”。
- 批量：`fetch / pull / diff / branch / switch / start / push / foreach`，支持 `--jobs`、`--dry-run`、`--fail-fast`、`--json`；共享共同 Git 目录的 worktree 串行调度；写操作执行前实时验证目标。
- 维护：`refresh`（不 fetch、不发现新仓库）、`relocate`、`remove`、`group create/add/list/remove/delete`；写类命令后使相关快照失效。

未实现或有意简化：

- 裸仓库（无工作目录）尚不支持，按设计留待后续。
- `--jobs` 的并发粒度是“共享 Git 目录的单元”，同一 worktree 组内始终串行；不提供跨仓库事务或回滚。
- 时间统一以 UTC RFC 3339 写入索引，`--cached` 与表格直接展示 UTC，未做本地时区换算。
- 索引文件名与锁文件固定为数据根目录下的 `repos.json` 与 `repos.lock`。
