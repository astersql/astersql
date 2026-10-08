# [`pkg/sessionctx/vardef/runtime.rs`](runtime.rs)

## 文件定位

本文件属于 Cargo crate `astersql-sessionctx-vardef`（`pkg/sessionctx/vardef/Cargo.toml`），由 `pkg/sessionctx/vardef/lib.rs` 的私有模块 `runtime` 装入，并通过 `pub use runtime::*` 将全部公开函数暴露给 session、Domain、server 等上层 crate。它不是独立的调度器，而是 Go `pkg/sessionctx/vardef/runtime.go` 的 Rust 对照：集中保存进程级 lease/保留期，并提供 NextGen 系统变量只读集合的判定函数。

在完整应用中，`cmd/tidb-server/main.rs` 启动时通过三个 setter 写入配置中的 schema、stats 和 plan-replayer GC 周期；session 初始化与 DDL runtime 使用 `GetSchemaLease` 构造 schema validator、同步器及 DDL 运行环境；Domain 的 plan-replayer 文件 GC 使用当前文件保留期；session 的 `SET` 执行路径调用 `IsReadOnlyVarInNextGen` 拒绝 NextGen 下不允许修改的变量。

## 核心职责

1. 用四个进程级 `AtomicI64` 保存 schema lease、stats lease、plan-replayer GC lease 和非 capture plan-replayer 文件保留时间，并提供一一对应的公开 setter/getter。
2. 保留 Go 版默认值：schema 为 1 秒、stats 为 3 秒、plan-replayer GC 为 10 分钟、普通 plan-replayer 文件保留期为 7 天。
3. 通过 `IsReadOnlyVarInNextGen` 对变量名做不区分大小写的精确匹配，识别六个 NextGen 只读变量。

本文件只负责状态保存和集合判定，不负责解析配置、校验系统变量取值、定时调度、文件删除或判断当前 kernel 是否为 NextGen；这些动作分别位于调用方。

## 主要符号

- `SCHEMA_LEASE: AtomicI64`：纳秒数，默认 `1_000_000_000`。`SetSchemaLease(Duration)` 与 `GetSchemaLease() -> Duration` 是唯一公开访问接口。
- `STATS_LEASE: AtomicI64`：纳秒数，默认 `3_000_000_000`。由 `SetStatsLease`/`GetStatsLease` 访问。
- `PLAN_REPLAYER_GC_LEASE: AtomicI64`：纳秒数，默认 `600_000_000_000`，由 `SetPlanReplayerGCLease`/`GetPlanReplayerGCLease` 访问。
- `PLAN_REPLAYER_FILE_RETENTION_TIME: AtomicI64`：纳秒数，默认 7 天，由 `SetPlanReplayerFileRetentionTime`/`GetPlanReplayerFileRetentionTime` 访问；它对应普通（non-capture）dump 文件，而 capture 文件的固定保留期由 `pkg/domain/plan_replayer.rs` 的调用处单独传入。
- `IsReadOnlyVarInNextGen(name: &str) -> bool`：先调用 `str::to_lowercase`，再与 `TiDBEnableMDL`、`TiDBMaxDistTaskNodes`、`TiDBDDLReorgMaxWriteSpeed`、`TiDBDDLDiskQuota`、`TiDBEnableDistTask`、`TiDBDDLEnableFastReorg` 六个常量做精确匹配。

所有函数都是公开 API；四个原子变量是模块私有实现细节。文件没有类型、trait、`impl`、异步函数或条件编译项。

## 执行流程

lease/保留期读写流程一致：调用方把 `Duration` 传给 setter，setter 以 `as_nanos()` 取得纳秒数、转换为 `i64`，再用 `Ordering::SeqCst` 写入对应原子；getter 以相同内存序读取 `i64`、转换为 `u64`，最后由 `Duration::from_nanos` 重建值。该流程没有锁、回调或持久化步骤。

启动时，`cmd/tidb-server/main.rs` 的配置初始化路径调用 `SetSchemaLease`、`SetStatsLease` 和 `SetPlanReplayerGCLease`。随后 `pkg/session/runtime/session_factory.rs` 读取 schema lease 创建 `astersql_infoschema_isvalidator::Validator`，并把毫秒值交给 cross-keyspace schema syncer；`pkg/session/runtime/session.rs` 也把它传给 serving DDL runtime。因此 setter 改的是后续读取者观察到的进程全局值，已持有旧值的对象不会由本文件主动刷新。

plan-replayer 文件保留期的用户可见路径从 `pkg/sessionctx/variable/sysvar_builtins.rs` 开始：系统变量 getter 格式化当前 `Duration`，setter 先解析并约束为 `i64` 纳秒，再调用本文件 setter。`pkg/domain/plan_replayer.rs::DumpFileGcChecker::gc_with_current_retention` 每次 GC 获取当前值并传给实际删除逻辑。

NextGen 判定流程是：调用方先决定当前是否为 NextGen，再把规范化或原始变量名传入 `IsReadOnlyVarInNextGen`；本函数仅做小写化和白名单匹配。当前直接生产调用证据位于 `pkg/session/runtime/control.rs::execute_set`，用于拒绝 NextGen 下修改 `tidb_enable_metadata_lock`。

## 数据与状态

四个原子保存的是纳秒整数，与 Go 的 `time.Duration`（底层 `int64` 纳秒）保持表示层对齐。状态是整个进程共享的，而不是 session、tenant 或 keyspace 局部状态；任一 setter 都会影响之后所有 getter。源码没有版本号、观察者或回滚机制。

六个只读变量名来自同 crate 的 `pkg/sessionctx/vardef/tidb_vars.rs`，本文件不复制字符串字面量。这可让变量常量改名时匹配集合在编译期随引用更新。匹配前使用 Unicode 小写转换，且不做 trim、前缀剥离或别名解析，所以前后空白会导致不匹配；`runtime_1_aster_unit_test.rs` 明确覆盖了这一点。

## 依赖与调用关系

下游依赖很窄：标准库 `std::sync::atomic::{AtomicI64, Ordering}` 提供并发状态，`std::time::Duration` 提供时间值；crate 根再导出的六个 TiDB 变量常量提供 NextGen 集合。尽管 `Cargo.toml` 还声明 `chrono`、`kerneltype`、`sysinfo` 和 `nextgen` feature，本文件自身没有直接使用它们，也没有 `#[cfg(feature = "nextgen")]` 分支。

已核对的直接上游包括：

- `cmd/tidb-server/main.rs`：启动配置写入三个 lease。
- `pkg/session/runtime/session_factory.rs` 与 `pkg/session/runtime/session.rs`：读取 schema lease，连接 infoschema validator、cross-keyspace schema syncer 和 serving DDL runtime。
- `pkg/session/runtime/control.rs::execute_set`：NextGen `SET` 只读判断。
- `pkg/sessionctx/variable/sysvar_builtins.rs`：`tidb_plan_replayer_file_retention_time` 的全局 getter/setter 桥接。
- `pkg/domain/plan_replayer.rs::DumpFileGcChecker::gc_with_current_retention`：读取普通 dump 文件保留期。

此外，若干 server/session/RealTiKV 测试会临时设置 schema 或 stats lease。它们说明这些 API 也承担测试时缩短等待周期的用途，但测试调用不等于生产调度逻辑。

## 错误处理与边界

本文件 API 不返回 `Result`，也不校验业务范围。零时长能够正常往返；负数无法由 Rust `Duration` 表达。setter 中 `Duration::as_nanos()` 返回 `u128`，随后使用 `as i64`，因此超过 `i64::MAX` 纳秒的输入会发生截断；getter 再把可能的负 `i64` 用 `as u64` 转换，可能得到很大的正时长。生产系统变量路径在 `sysvar_builtins.rs` 先执行 `i64::try_from`，规避该路径的溢出，但直接 API 调用者仍必须保证范围。

`SetSchemaLease` 的 Go 注释强调它危险：过小会造成性能下降，而且启动后只影响非 local storage。本 Rust 函数只保存数值，没有编码这两项约束，调用者必须遵守同一契约。

`IsReadOnlyVarInNextGen` 不验证 kernel 类型，也不产生错误；未知名称、带空白名称及集合外变量都返回 `false`。是否把 `true` 转成用户可见的“read only in NextGen”错误由 session 层负责。

## 并发与资源生命周期

四个状态均使用 `AtomicI64` 和最强的 `SeqCst` 内存序，因此单次读写不会数据竞争，并在所有线程间参与同一全序。它们是进程生命周期静态值，无初始化锁、析构、后台任务或资源句柄；更新立即对后续原子读取可见。

多字段更新不是事务：例如启动路径依次设置三种 lease，其他线程可能在中间观察到新旧混合值。本文件也不协调测试间的全局修改。`pkg/sessionctx/vardef/runtime_1_aster_unit_test.rs` 使用 `LeaseRestore` 的 `Drop` 恢复前三个 lease，体现测试必须清理共享状态；保留期测试则显式在断言后恢复原值。新增测试应继续放在独立测试文件中，并避免并行修改同一原子导致相互干扰。

## 与 Go 版本的对应关系

`pkg/sessionctx/vardef/runtime.go` 与本文件拥有相同的四组状态、八个 getter/setter 和一个只读判定函数；默认值也一致。Go 使用 `atomic.StoreInt64`/`atomic.LoadInt64` 保存 `time.Duration` 的底层纳秒值，Rust 使用 `AtomicI64` 和 `Duration` 做显式转换。

`IsReadOnlyVarInNextGen` 保留 Go 的 `strings.ToLower` 后 `switch` 的语义和六元素集合。`pkg/sessionctx/vardef/runtime_test.go::TestIsReadOnlyVarInNextGen` 覆盖未知值、常量值和大写的 metadata-lock 名称；`pkg/sessionctx/vardef/runtime_test.rs::test_is_read_only_var_in_next_gen` 镜像这些用例并保留 Classic kernel 的提前返回。

可见差异是 Go 的默认 plan-replayer 文件保留期引用 `DefTiDBPlanReplayerFileRetentionTime`，Rust 当前在本文件中直接写出 7 天表达式，尽管 `tidb_vars.rs` 也定义了对应常量；两者当前数值一致，但未来修改默认值时必须同步检查。另一个表示差异是 Go API 原生接受可为负的 `time.Duration`，Rust `Duration` 不能表示负值。

## 扩展指南

新增一种运行时 lease/时长时，应在本文件增加私有原子和成对 setter/getter，在 `pkg/sessionctx/vardef/lib.rs` 维持再导出，并在独立的 `pkg/sessionctx/vardef/runtime_1_aster_unit_test.rs` 增加默认值、往返写入和恢复全局状态的测试；若来自 Go 移植，还需同步核对 `runtime.go` 的默认值与注释。调用方若从配置或系统变量接受字符串，应在边界层完成解析、范围校验和错误生成，不应把不受限的 `u128` 纳秒直接交给 setter。

扩展 NextGen 只读集合时，应修改 `IsReadOnlyVarInNextGen` 的匹配分支，并同步 `runtime_test.rs`、`runtime_1_aster_unit_test.rs` 和 Go 对照测试；还应确认 session 的所有相关 `SET` 路径确实调用该判定。当前生产接线只直接展示 metadata-lock 分支，因此不能仅把名称加入集合就推断其他变量已经受到拒绝逻辑保护。

性能方面，getter/setter 是热路径友好的常数时间原子操作；扩大只读集合时保持无分配的常量比较较理想，但当前 `to_lowercase()` 会分配新字符串。任何改变大小写规则、修剪行为或内存序的优化都属于行为/并发契约变更，应以 Go 兼容测试和调用方需求为依据。

## 验证依据

- 源码与装配：`pkg/sessionctx/vardef/runtime.rs`、`pkg/sessionctx/vardef/lib.rs`、`pkg/sessionctx/vardef/Cargo.toml`。
- Go 对照：`pkg/sessionctx/vardef/runtime.go`、`pkg/sessionctx/vardef/runtime_test.go`。
- Rust 独立测试：`pkg/sessionctx/vardef/runtime_test.rs`、`pkg/sessionctx/vardef/runtime_1_aster_unit_test.rs`；后者验证四个默认值、setter/getter 往返、状态恢复、大小写不敏感和空白不匹配。
- 生产调用证据：`cmd/tidb-server/main.rs`、`pkg/session/runtime/session_factory.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/control.rs`、`pkg/sessionctx/variable/sysvar_builtins.rs`、`pkg/domain/plan_replayer.rs`。
- RustCodeGraph：`status` 显示本仓库索引包含 Rust/Go 文件；`query IsReadOnlyVarInNextGen --kind function` 定位 Rust、Go 定义及 Go 测试，`query GetSchemaLease --kind function` 定位 Rust/Go 对照定义。对 `callers`/`callees` 的查询未返回边，因此调用关系使用上述精确源码搜索补证，未把缺失的图边当作不存在调用。
- 本文只进行静态分析和结构检查，依任务约束未运行 Cargo 或运行时测试；关于实际调用范围的结论限于列出的直接源码证据。
