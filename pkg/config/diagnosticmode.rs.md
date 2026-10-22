# `pkg/config/diagnosticmode.rs`

## 文件定位

保存未初始化、禁用、启用三态；提供启动期初始化、查询和受保护的测试覆盖；共享 DDL 拒绝错误。

## 核心职责

保存未初始化、禁用、启用三态；提供启动期初始化、查询和受保护的测试覆盖；共享 DDL 拒绝错误。它不负责 SQL 分类、HTTP 路由或存储只读。

## 主要符号

私有 `ModeState` 为 repr(u8) 枚举，`CURRENT_MODE` 是 AtomicU8；公共接口为 `Initialize`、`Enabled`、`SetForTest`，错误为 `DiagnosticModeError::{DdlNotAllowed, AlreadyInitialized}`，`ErrDDLNotAllowed` 为共享 DDL 错误值。`reset_for_test` 仅 cfg(test) 可见。

## 执行流程

`Initialize(enabled)` 先读取当前状态；与目标相同则成功，已初始化成相反状态则返回携带 current/desired 的错误，未初始化则 store 目标。`Enabled` 在初始化前返回 false。`SetForTest` swap 新值并返回恢复旧值的 FnOnce；调用方必须执行闭包，简单丢弃不会恢复。

## 数据与状态

三态避免将“未初始化”与“明确禁用”混淆。ModeState::load 将未知 u8 映射为未初始化；状态没有动态配置重载入口。所有原子访问使用 SeqCst。

## 依赖与调用关系

下游使用 std 原子类型、thiserror 与 `astersql_util_intest::InTest`。冻结版本 `pkg/domain/serverinfo/syncer.rs` 和 `pkg/server/{http_status,server}.rs` 读取 Enabled；`pkg/config/lib.rs` 独立挂载 `diagnosticmode_test.rs`。冻结 `cmd/tidb-server/main.rs` 没有 Initialize 调用，不能据模块导出推断 CLI 主链已接入。

## 错误处理与边界

反向重新初始化返回 AlreadyInitialized；DDL 错误只提供错误身份，调用者必须实际检查。SetForTest 若既非本 crate 的 cfg(test) 构建、InTest 也为 false，则 panic。生产调用方不应使用测试覆盖绕过初始化合同。

## 并发与资源生命周期

原子可见性不等于竞争初始化互斥：Initialize 用独立 load/store，而非 compare_exchange。契约要求启动阶段串行初始化；两个相反首次初始化并发时不能保证其中一个报错。测试覆盖是进程全局状态，不得与读取者并行，应由测试守卫保证恢复。

## 与 Go 版本的对应关系

Go 对照位于 `pkg/config/diagnosticmode/`，doc.go 定义同样的固定初始化、初始化前 false 和 intest 测试限制。Rust 用 AtomicU8 与 Result/FnOnce 表达状态、错误与恢复，不因此拥有 Go 端所有诊断调用点。

## 扩展指南

新增消费者时明确其阻止的具体行为并补独立测试；不要把 Enabled 当作整机只读证明。调整初始化必须同时审查串行启动前提及测试恢复路径；扩展错误保持既有 DDL 文本和状态转换兼容。
