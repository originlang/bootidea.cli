# 项目范围与协作边界

本文件约束本仓库内的开发工作。

## 职责

- `bootidea` 是扩展宿主的统一可执行入口，负责命令解析、配置合并、宿主装配、终端输出和服务传输。
- 以现有锁定 revision 的 Rust 依赖方式使用 `originlang.extension.host`，Host 库编译进入 CLI 可执行文件。
- `serve` 将宿主能力通过 stdio / TCP / HTTP JSON-RPC 提供给调用方；宿主业务逻辑保留在 Host 库。
- agent 命令调用已安装的 harness 插件，不在 CLI 内重写模型循环，也不静态编入 harness。

## 项目关系

```text
用户 / HomeWorld → bootidea 可执行文件 → Host 库
Host → 运行时加载 harness 动态库插件
Host → 按需启动独立 oext-sandbox 执行体
```

- HomeWorld 可下载和启动本项目发布的可执行文件，它不是本项目的编译依赖。
- HomeWorld 使用独立数据目录管理可选 Host 进程；CLI 必须支持显式配置、数据目录和可预测的启动/退出行为。
- stdio 协议输出不得混入普通日志；诊断信息写 stderr，stdin 关闭应触发宿主清理退出。
- CLI、沙箱执行体和插件是不同交付物，发布文档必须准确列明包含内容及兼容性。
- 保持独立检出可构建，不将相邻仓库的本地路径作为正式依赖要求。

## 范围限制

- 扩展安装、能力裁决、运行隔离与存储实现归 Host。
- 模型调用、工具编排、预算与上下文策略归 harness。
- 网页、家庭账号、资料库和可选 Host 的界面管理归 HomeWorld。
