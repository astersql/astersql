# `br/pkg/task/lib.rs`

源文件：[`lib.rs`](./lib.rs)

## 文件定位

本文件是 Rust crate `astersql-br-pkg-task` 的根模块。`br/pkg/task/Cargo.toml` 通过 `[lib] path = "lib.rs"` 把它设为库入口，并用 `package.metadata.porting.go-package = "br/pkg/task"` 明确记录其 Go 对照包。它位于 BR 命令层与各备份、恢复、日志备份实现模块之间：`br/cmd/br/backup.rs`、`restore.rs`、`stream.rs` 等上游以 `astersql_br_pkg_task::{...}` 导入平铺 API，本文件再把请求导向同目录子模块。

这不是某个具体任务的实现文件，而是模块图和公开 API 门面。文件自身没有函数、结构体、枚举、常量或 `impl`，也没有进程入口；实际行为分别位于 `backup*.rs`、`restore*.rs`、`stream.rs`、`common.rs`、`encryption.rs` 和 `stubs.rs`。

## 核心职责

1. 用 `#[path = "..."] pub mod ...` 挂载 14 个生产模块：`stubs`、`encryption`、`common`、四个 `backup*` 模块、五个 `restore*` 模块、`stream` 和 `restore_lifecycle`。
2. 用 `pub use <module>::*` 将除 `restore_lifecycle` 外的 13 个生产模块的公开项平铺到 crate 根，使 CLI 可以写 `astersql_br_pkg_task::RunRestore`，而不必写 `astersql_br_pkg_task::restore::RunRestore`。
3. 用 `#[cfg(test)]` 和显式 `#[path]` 挂载 16 个独立 Rust 测试模块，保证测试逻辑不内嵌在生产源文件中。
4. 通过 crate 级 `#![allow(...)]` 容纳从 Go 迁移而来的命名风格、暂未使用的兼容符号和桩代码。该属性覆盖 `dead_code`、Go 风格命名、未使用项及全部 Clippy lint，因此新增代码不能把“无告警”误当作已通过细粒度 lint 审查。

## 主要符号

本文件没有可调用符号；其主要公开面由模块声明和再导出组成。

- `pub mod stubs`：本地兼容类型、trait、内存实现和错误类型。它既可经 `task::stubs::...` 显式访问，也因 `pub use stubs::*` 有一部分符号出现在 crate 根。
- `pub mod common` 与 `pub mod encryption`：共享配置、flag、存储/连接辅助及主密钥解析。典型根级导出包括 `Config`、`DefineCommonFlags`、`GetStorage`。
- `pub mod backup`、`backup_ebs`、`backup_raw`、`backup_txn`：逻辑备份、EBS、Raw KV 和 Txn KV 的配置与执行入口；`br/cmd/br/backup.rs` 使用 `BackupConfig`、`RunBackupWithDefaults`、`RunBackupEBS` 等再导出项。
- `pub mod restore`、`restore_data`、`restore_ebs_meta`、`restore_raw`、`restore_txn`：常规、数据解析、EBS 元数据、Raw KV 和 Txn KV 恢复；`br/cmd/br/restore.rs` 使用 `RestoreConfig`、`RunRestore`、`RunResolveKvData` 等再导出项。
- `pub mod stream`：日志备份/PiTR 配置与任务分派；`br/cmd/br/stream.rs` 使用 `StreamConfig`、`RunStreamCommand` 及各类 flag 定义函数。
- `pub mod restore_lifecycle`：恢复生命周期辅助模块。它声明在通配再导出块之后，且没有对应 `pub use restore_lifecycle::*`；其公开项只能经 `astersql_br_pkg_task::restore_lifecycle::...` 访问，除非未来显式增加再导出。
- `parity_test` 及各 `*_test` 模块均为私有且仅在 `cfg(test)` 下存在，不构成发布库 API。

## 执行流程

本文件在运行时没有独立控制流；它在编译期建立路径，实际调用链如下：

1. Cargo 按 `br/pkg/task/Cargo.toml` 将 `lib.rs` 编译为 `astersql_br_pkg_task`。
2. 模块声明把同目录生产文件纳入 crate；公开模块允许调用方选择模块限定路径。
3. `pub use` 将常用公开项提升到 crate 根。若多个通配导出产生冲突，必须在门面处消歧，不能依赖导出顺序表达优先级。
4. BR CLI 解析命令和 flag 后调用根级 API。例如 `br/cmd/br/backup.rs::runBackupCommand` 调用 `RunBackupWithDefaults`，`br/cmd/br/restore.rs::runRestoreCommand` 调用 `RunRestore`，`br/cmd/br/stream.rs` 将子命令交给 `RunStreamCommand`。
5. 被选中的子模块再完成配置校验、资源建立和具体任务编排；这些运行时步骤不由 `lib.rs` 持有。

测试构建另走一条编译期分支：启用 `cfg(test)` 后，16 个独立测试文件成为 crate 内部模块，因此能用 `crate::backup`、`crate::restore` 等路径检查非公开辅助项；发布构建不会包含这些测试模块。

## 数据与状态

本文件不拥有运行时数据、全局可变状态、缓存或持久化格式。它唯一维护的是编译期结构状态：模块集合、模块可见性、根级导出集合和测试模块集合。

这里最重要的不变量是“声明、导出和 Cargo 依赖保持一致”。新增子模块若只写文件而不加 `pub mod`，不会进入 crate；加了 `pub mod` 但未加 `pub use`，调用方只能用限定路径；删除或重命名通配导出中的公开项会改变 CLI 所依赖的根级 API。`restore_lifecycle` 证明“公开模块”与“根级平铺导出”是两个不同的兼容承诺。

## 依赖与调用关系

上游直接依赖者主要是 `br/cmd/br` crate。RustCodeGraph 对 `astersql_br_pkg_task` 的查询显示：

- `br/cmd/br/backup.rs` 从根级导入备份配置、flag 定义与 `RunBackup*` 执行入口，并从 `stubs` 导入存储抽象。
- `br/cmd/br/restore.rs` 从根级导入恢复配置、flag 定义与 `RunRestore*` 入口。
- `br/cmd/br/stream.rs` 从根级导入日志备份配置、flag 定义和 `RunStreamCommand`。
- `br/cmd/br/cmd.rs`、`debug.rs`、`abort.rs` 与 `stubs.rs` 也直接使用公共配置、存储、管理器和错误桥接；`br/cmd/br/Cargo.toml` 以路径依赖 `../../pkg/task` 接入本 crate。

下游依赖由各子模块消费，而不是由 `lib.rs` 直接调用。`br/pkg/task/Cargo.toml` 声明的本地 crate 包括 metaservice、restore、gc、aws、common、stream、utils、conn、registry、checkpoint 和 restore/log_client；外部依赖包括 `hex`、`regex`、`serde`、`serde_json`、`sha2`、`url`、`uuid`。Cargo 注释说明当前依赖面针对 arm64 Darwin 避开完整 kv/domain/kvproto/grpcio，并以本地 trait/stub 维持任务层边界；因此 `stubs` 是当前可编译架构的一部分，不能仅按“测试替身”删除。

## 错误处理与边界

`lib.rs` 不构造、捕获或转换运行时错误；错误类型及传播策略属于被导出的子模块。门面的错误边界主要发生在编译期：模块文件缺失、公开项改名、重复通配导出歧义或调用方依赖的根级再导出被移除都会造成编译失败或 API 不兼容。

本文件顶部的宽泛 `allow` 会隐藏未使用导出、命名和 Clippy 类问题。扩展时应在具体实现和独立测试中验证错误分支，不应借助这里的 lint 放宽来掩盖未接线代码。当前文件也没有 feature gate；生产模块全部无条件编译，只有测试模块受 `cfg(test)` 控制。

同目录存在 `meta_service_group_test.rs`，但 `lib.rs` 没有对应的 `mod meta_service_group_test` 声明；按当前模块图它不会因本 crate 的普通测试构建而自动纳入。若该测试应属于本 crate，需要显式接线并单独验证，而不能仅凭文件存在声称已覆盖。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、网络连接或文件句柄，也没有初始化/析构副作用。并发和资源清理由实际执行模块负责；例如 CLI 仅通过此门面取得 `RunBackup*`、`RunRestore*`、`RunStreamCommand`，生命周期语义仍在这些函数及其依赖中。

模块声明顺序是源码组织方式，不是运行时初始化顺序，也不提供并发先后保证。测试模块的生命周期仅由 Rust 测试构建控制；`cfg(test)` 确保它们不进入发布制品，但测试之间是否共享状态仍须逐个测试文件审查。

## 与 Go 版本的对应关系

Go 的 `br/pkg/task` 没有与 Rust `lib.rs` 一一对应的入口文件：Go 编译器按同一目录的 `package task` 自动聚合 `common.go`、`backup*.go`、`restore*.go`、`stream.go` 等文件，同包导出符号天然可用。Rust 必须显式声明模块并用 `pub use` 模拟这种“同包平铺可见性”，这正是本文件存在的主要原因。

业务拆分总体按同名文件对照，例如 `backup.rs` 对应 `backup.go`、`restore.rs` 对应 `restore.go`、`stream.rs` 对应 `stream.go`。Rust 的 `parity_test.rs` 明确把自身定位为 Go/Rust 公共契约测试，并覆盖 flag 常量、默认配置、空间估算、DDL 过滤、存储/桩契约等；各专门 `*_test.rs` 则与同名或相关 Go 测试对齐。差异在于 Rust 侧还显式提供 `stubs.rs` 和 `restore_lifecycle.rs` 作为迁移期边界，不能把它们假定为 Go 中存在同名完整实现。

## 扩展指南

- 新增任务实现时，先将逻辑放入独立生产 `.rs` 文件，并将测试放入独立 `*_test.rs`；不要把测试写进 `lib.rs` 或生产源文件。
- 在这里增加 `#[path] pub mod` 后，明确决定是否需要根级 `pub use`。若只是内部组织或希望控制 API 面，应保留模块限定访问；若要保持 Go 同包式调用体验，再增加有意的再导出，优先考虑显式导出以降低通配冲突风险。
- 同步更新 `br/pkg/task/Cargo.toml` 仅限模块确实引入新 crate 依赖时；本文件无 feature 分支，不能假设新增模块会被条件排除。
- 修改已有导出前，搜索 `astersql_br_pkg_task` 的所有导入者，重点检查 `br/cmd/br/{backup,restore,stream,cmd,debug,abort,stubs}.rs` 及其独立测试。
- 行为扩展应同步最接近的 Rust 独立测试和 Go 对照测试；门面级公开契约变化还应更新 `br/pkg/task/parity_test.rs`。若要接入现有 `meta_service_group_test.rs`，需显式增加测试模块声明。
- 兼容风险主要是根级 API 改名/消失和通配导出冲突；性能风险通常不在门面本身，而在无条件纳入的新模块及其下游依赖体积或初始化行为。

## 验证依据

- RustCodeGraph `status`：索引包含 7032 个 Rust 文件；查询时项目数据库可用。
- RustCodeGraph `files --filter br/pkg/task`：确认目标目录的生产模块、独立 Rust 测试与 Go 对照文件集合。
- RustCodeGraph `node --file br/pkg/task/lib.rs --offset 1 --limit 260`：读取完整 158 行源文件，核对 crate 属性、14 个生产模块、16 个 `cfg(test)` 模块、13 个通配再导出及 `restore_lifecycle` 的特殊边界。
- RustCodeGraph `query task --limit 30 --json`：确认 `br/cmd/br/backup.rs`、`cmd.rs`、`debug.rs`、`stream.rs`、`stubs.rs` 等对 `astersql_br_pkg_task` 的直接导入。
- RustCodeGraph 对 `br/cmd/br/backup.rs`、`restore.rs`、`stream.rs` 和 `br/pkg/task/parity_test.rs` 的文件节点读取：确认 CLI 到任务层的主要调用边，以及 parity 测试覆盖的是公共契约而非真实集群恢复。
- 直接读取 `br/pkg/task/Cargo.toml` 与 `br/cmd/br/Cargo.toml`：确认 crate 名称、`lib.rs` 入口、Go 包映射、依赖边界和上游路径依赖；Cargo 文件不在 RustCodeGraph 源码图覆盖范围内。
- `rg` 检索 `astersql_br_pkg_task`、`package task`、测试属性和模块声明：补充核对 Go 同目录包聚合、Rust 独立测试位置，以及 `meta_service_group_test.rs` 当前未在 `lib.rs` 接线的事实。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章节结构检查作为交付验证。
