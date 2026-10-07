# `lightning/pkg/errormanager/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-lightning-pkg-errormanager` 的 crate 根，而不是错误管理算法的实现文件。`Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "lightning/pkg/errormanager"` 把它定位为 Go 包 `github.com/pingcap/tidb/lightning/pkg/errormanager` 的 Rust 迁移入口。文件通过显式 `#[path = ...]` 装配 [`stubs.rs`](stubs.rs)、[`errormanager.rs`](errormanager.rs) 以及三个独立测试模块。

该 crate 当前被 [`lightning/pkg/importer/Cargo.toml`](../importer/Cargo.toml) 以路径依赖引入；[`lightning/pkg/importer/import.rs`](../importer/import.rs) 使用 `astersql_lightning_pkg_errormanager as errormanager`，构造 `errormanager::ErrorManager` 并保存在导入控制器中。因此，本文件处在 Lightning importer 与错误管理实现之间的包级 API 边界上。

## 核心职责

本文件只承担四项装配职责：

1. 先声明私有 `stubs` 模块并执行 `pub use stubs::*`，把迁移阶段使用的配置、SQL、KV、日志、编码等替代边界公开到 crate 根。
2. 再声明私有 `errormanager` 模块并执行 `pub use errormanager::*`，把真正的错误管理类型、常量和函数提升为包级 API。
3. 在 `cfg(test)` 下接入 `parity_test`、`errormanager_test` 和 `resolveconflict_test`，保持测试逻辑与生产源文件分离。
4. 用 crate 级 `#![allow(...)]` 暂时容纳 Go 风格命名和迁移代码产生的未使用项及 Clippy 告警。

它不创建错误表、不递减错误配额、不解析冲突，也不拥有线程或数据库资源；这些行为均在 `errormanager.rs` 或 `stubs.rs` 中实现。

## 主要符号

- `mod stubs`（`lib.rs:27-28`）：私有模块声明，源码固定为 `stubs.rs`。其内容不是第三方依赖，而是当前 crate 内的迁移期外围能力替身。
- `pub use stubs::*`（`lib.rs:30`）：把 `config`、`sql`、`kv`、`log`、`context`、`atomic` 等桩模块和类型再导出；`errormanager.rs` 也通过 `crate::...` 使用这些名字。
- `mod errormanager`（`lib.rs:32-33`）：私有实现模块声明，源码固定为 `errormanager.rs`。
- `pub use errormanager::*`（`lib.rs:35`）：公开真实实现的包级 API。主要导出包括 `ErrorManager`、`DataConflictInfo`、`New`，表/视图名常量 `ConflictErrorTableName`、`DupRecordTableName`、`ConflictViewName`，以及 `ErrorManager` 上的初始化、记录、冲突替换和汇总方法。
- `mod parity_test`、`mod errormanager_test`、`mod resolveconflict_test`（`lib.rs:37-47`）：仅测试构建存在的私有模块，不会进入普通依赖方的生产 API。

本文件自身没有常量、结构体、trait、函数或 `impl`，也没有 feature 条件；唯一条件编译项是三个 `#[cfg(test)]` 测试模块。

## 执行流程

普通构建时，Rust 从 `lib.rs` 进入，先解析 `stubs.rs` 并将其公开项提升到 crate 根，再解析 `errormanager.rs` 并将真实错误管理 API 提升到同一根命名空间。上层 importer 因而可以直接调用 `errormanager::New(...)`、引用 `errormanager::ErrorManager`，同时用 `errormanager::config::Config`、`errormanager::sql::DB` 和 `errormanager::log::Logger` 满足构造参数。

[`lightning/pkg/importer/import.rs`](../importer/import.rs) 的已接线流程是：从 importer 配置复制任务号、后端、冲突策略和阈值到本 crate 再导出的 `config::Config`，创建当前为内存替身的 `sql::DB`，调用 `New`，再把结果放入 `Controller.errorMgr`。实际运行逻辑随后进入 `errormanager.rs`：`New` 推导 V1/V2 冲突开关与配额，`Init` 按启用项创建 schema、表和视图，各 `Record*` 方法消耗配额并写入记录，`ReplaceConflictKeys` 处理 replace 冲突，`HasError`/`Output`/`LogErrorDetails` 汇总结果。上述业务步骤是再导出 API 的下游行为，不是 `lib.rs` 自身执行的算法。

测试构建在相同装配流程后额外编译三个测试模块。它们使用 `crate::*` 或 `crate::config` 等路径，直接验证 crate 根公开面，因此也验证了本文件的模块接线和再导出是否完整。

## 数据与状态

`lib.rs` 不声明或持有运行时数据。经它公开的主要状态位于 `errormanager.rs::ErrorManager`：可选 SQL 数据库句柄、任务 ID、信息 schema 名、初始与剩余错误配额、冲突配置与两类剩余计数、V1/V2 开关、日志器、一次性记录原子标志，以及供冲突替换路径使用的共享编码映射。

配额和一次性标志使用 `stubs.rs` 提供的原子包装；编码映射为 `Arc<Mutex<...>>`。SQL、KV、日志、上下文、表元数据与编码器等边界来自 `stubs.rs` 的公开模块。由于 `pub use stubs::*` 与 `pub use errormanager::*` 都是 glob 再导出，新增同名公开项可能造成根命名空间冲突，扩展时必须检查名称唯一性。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 没有外部 `[dependencies]` 条目；注释说明 macOS arm64 下暂未接入 KV/domain/kvproto/grpcio 等真实依赖，相关边界由本地 stubs 承担。
- 上游生产调用者：[`lightning/pkg/importer/Cargo.toml`](../importer/Cargo.toml) 依赖本 crate；[`lightning/pkg/importer/import.rs`](../importer/import.rs) 导入 crate、声明 `Option<errormanager::ErrorManager>`，并调用 `config::Config::NewConfig`、`sql::DB::new_memory`、`log::Logger::L` 和 `New`。
- 下游实现：[`errormanager.rs`](errormanager.rs) 是包级业务实现；[`stubs.rs`](stubs.rs) 提供它当前需要的外围接口及内存实现。
- 测试调用者：[`parity_test.rs`](parity_test.rs)、[`errormanager_test.rs`](errormanager_test.rs)、[`resolveconflict_test.rs`](resolveconflict_test.rs) 由本文件直接声明，分别覆盖公共契约、错误管理主体和 replace 冲突场景。
- RustCodeGraph 对 `lib.rs` 的文件查询只报告一个外部索引引用 `tools/tazel/parity_test.rs`；针对实际业务接线，仓库文本搜索给出了 importer 的直接依赖与调用。图工具对返回的 Rust 符号 ID 进一步执行 `node/callers/callees` 时未能解析，因此这里不把缺失的图边推断为“没有调用者”。

## 错误处理与边界

本文件没有 `Result`、错误分支或恢复逻辑；模块装配或名称冲突属于编译期错误。运行期错误由再导出的实现负责：`Init` 和记录/冲突处理方法返回 `Result`，数据库缺失时 `Init` 是无操作成功，配额耗尽、SQL 执行、解码及冲突处理错误按 `errormanager.rs` 的分支传播。

当前最重要的能力边界是 stubs：Cargo 注释明确它们替代尚未接入的真实依赖，importer 也显式使用 `sql::DB::new_memory()`。因此不能仅凭 crate 根 API 完整或测试通过，宣称该 crate 已连接真实 TiDB SQL/KV/日志基础设施。另一个边界是 crate 级宽泛 `allow`：它避免迁移期告警阻断构建，也可能掩盖死代码、命名和 Clippy 问题；收紧时应逐项处理，不宜一次删除全部属性。

## 并发与资源生命周期

`lib.rs` 没有启动任务、创建线程、加锁、开事务或关闭资源，模块声明只在编译期生效。普通构建不包含三个测试模块；测试进程结束时测试夹具随进程释放。

经门面公开的 `ErrorManager` 使用原子计数器和原子布尔值处理可并发观察的配额/一次性状态，并使用 `Arc<Mutex<_>>` 共享编码映射；`ReplaceConflictKeys` 的测试还构造 `WorkerPool` 来覆盖并行冲突处理。SQL 数据库句柄的所有权和关闭语义位于 `stubs.rs`/调用方，门面不代管资源。`RecordErrorOnce` 的注释还明确指出其读取与 `RecordDuplicateOnce` 不是一个复合原子操作，调用者不能把两次调用视作原子事务。

## 与 Go 版本的对应关系

Go 的 [`errormanager.go`](errormanager.go) 直接在 `package errormanager` 中声明 `ErrorManager`、`DataConflictInfo`、构造函数 `New`、记录/替换/汇总方法和 SQL 常量；Go 不需要一个等价的 `lib.rs`。Rust 用本文件模拟 Go 的包级命名空间：私有实现模块加 `pub use errormanager::*` 让调用者仍以扁平包 API 使用符号，`pub use stubs::*` 则补齐尚未迁移成真实依赖的外围包能力。

Rust 测试与 Go 测试保持独立文件对应关系：`errormanager_test.rs` 对照 `errormanager_test.go`，覆盖初始化、冲突替换、`HasError` 和 `Output`；`resolveconflict_test.rs` 对照 `resolveconflict_test.go`，覆盖非聚簇主键、唯一键和 varchar 主键等 replace 场景；额外的 `parity_test.rs` 汇总正常、边界、错误与资源清理公共契约。Rust 还增加了“现有空行值保留 key”“缺失行后复用 keep set”等针对移植实现的回归场景，Go 同目录测试中没有同名用例。

## 扩展指南

- 新增错误管理业务符号时，应实现于 `errormanager.rs`；只要符号为 `pub`，现有 glob 再导出会自动把它暴露到 crate 根。先检查是否与 `stubs.rs` 的公开名冲突。
- 新增外围依赖适配时，应明确它是迁移期 stub 还是已接入真实 crate；修改 `stubs.rs`/Cargo 接线后同步检查 importer，不能在 `lib.rs` 中伪装业务实现。
- 新增生产模块时，只有确实需要形成公共包 API 才在这里声明并再导出；内部辅助模块应保持私有。新增测试必须继续放在独立 `*_test.rs` 文件，并用 `#[cfg(test)]`/`#[path]` 接入，不能把测试逻辑写进生产源文件。
- 修改公共导出面时同步扩展 `parity_test.rs`；修改初始化、配额、记录和摘要逻辑时扩展 `errormanager_test.rs`；修改冲突替换算法时扩展 `resolveconflict_test.rs`，并对照相应 Go 测试保护原始语义。
- 若未来以真实 SQL/KV/日志依赖替换 stubs，应重点复核错误类型、取消传播、事务/连接生命周期和并发语义；当前内存替身测试不能独立证明这些兼容性或性能特征。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter lightning/pkg/errormanager` 列出目标、实现、stubs 及 Rust/Go 测试；`node --file lightning/pkg/errormanager/lib.rs --offset 1 --limit 240` 返回完整 47 行源码；`query ErrorManager` 定位 Rust/Go 两个结构体，`query errormanager` 定位主要 Rust/Go API。精确 `node/callers/callees` 对查询结果 ID 无法解析，此限制已明确记录，调用关系另以 Cargo 和源码引用核验。
- crate 与调用链：`lightning/pkg/errormanager/Cargo.toml`、`lightning/pkg/importer/Cargo.toml`、`lightning/pkg/importer/import.rs`。
- Rust 生产实现：`lightning/pkg/errormanager/lib.rs`、`errormanager.rs`、`stubs.rs`。
- Go 对照：`errormanager.go`。
- Rust 独立测试：`parity_test.rs`、`errormanager_test.rs`、`resolveconflict_test.rs`。
- Go 独立测试：`errormanager_test.go`、`resolveconflict_test.go`。
- 人工复核结论：该文件存在的原因是建立与 Go 包级 API 对齐的 Rust crate 门面；其运行方式是编译期模块装配与公开再导出；安全扩展点是业务实现、stub 边界及三类独立测试，而不是在此文件内加入运行时逻辑。
