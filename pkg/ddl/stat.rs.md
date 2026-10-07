# `pkg/ddl/stat.rs`

## 文件定位

该文件属于 `astersql-ddl` crate：`pkg/ddl/Cargo.toml` 声明库入口为 `lib.rs`，`pkg/ddl/lib.rs` 通过 `pub mod stat;` 无条件公开本模块，并在 `cfg(test)` 下从独立文件 `pkg/ddl/stat_test.rs` 挂载测试。它位于 DDL 的状态观测边界，定义导出本节点标识与当前 schema version 的值模型，以及从会话池读取版本时必须遵守的资源归还协议。

当前接线状态必须与 Go 生产实现区分。仓库精确引用搜索表明，Rust 生产代码尚未实例化 `DdlStatistics`、`DdlStats` 或实现 `StatsSessionPool`；这些符号的直接 Rust 使用者只有 `pkg/ddl/stat_test.rs`。因此本文件已移植可独立验证的状态值、scope 和会话生命周期语义，但不能据此认定 Rust 的 `SHOW STATUS` 或 DDL 实例已经调用它。Go 的真实生产入口是 `pkg/ddl/stat.go:(*ddl).Stats`。

从 DDL 执行模型看，这里是只读、metadata-only 的状态查询，不创建或推进 DDL job，不触发 schema state 转换、reorg/backfill、schema diff 发布、owner 调度、取消/回滚或 delete-range GC。读取到的 schema version 是 DDL 元数据状态的观测值，不是由本文件递增。

## 核心职责

1. `SERVER_ID` 与 `DDL_SCHEMA_VERSION` 固定对外键名为 `server_id` 和 `ddl_schema_version`，与 Go 状态变量名称保持一致。
2. `StatusValue` 保留字符串与 `i64` 两种标量，使 schema version 不会为适配 map 而字符串化。
3. `StatsSessionPool` 把 Go 的 `ddl.sessPool.Get/Put` 与 `GetDDLInfoWithNewTxn` 压缩为最小可替换边界，便于验证获取失败、读取失败和归还会话的顺序。
4. `DdlStatistics::stats` 执行动态读取：获取会话、读取 schema version、归还会话，然后仅在读取成功时构造两项状态 map。
5. `DdlStats` 表示调用方已拥有 server ID 与 schema version 时的不可变快照；其 `scope` 和 `stats` 不访问会话池或元数据。

本文件不负责把统计提供者注册到 `pkg/sessionctx/variable/statusvar.rs` 的 Rust `StatisticsHandle`，也不实现 Go `GetDDLInfoWithNewTxn` 的事务、job 列表或 reorg handle 查询。

## 主要符号

- `pub(crate) const SERVER_ID: &str`、`DDL_SCHEMA_VERSION`：键名仅在 crate 内可见；两者分别选择 `StatusValue::String` 与 `StatusValue::I64`。
- `pub enum StatusValue { String(String), I64(i64) }`：本模块自己的强类型值枚举，派生 `Clone`、`Debug`、`Eq` 和 `PartialEq`。它与 `pkg/sessionctx/variable/statusvar.rs::StatusValue` 不是同一类型，当前也没有转换实现。
- `pub trait StatsSessionPool`：关联类型 `Session` 和 `Error` 由实现者决定。`get` 取得所有权，`put` 消耗并归还会话，`schema_version` 通过 `&mut Session` 读取 `i64` 版本。
- `pub struct DdlStatistics<P>`：持有拥有所有权的 `server_id: String` 与泛型 `session_pool: P`。其 impl 要求 `P: StatsSessionPool`。
- `DdlStatistics::stats(&self) -> Result<BTreeMap<String, StatusValue>, P::Error>`：动态查询入口。成功结果恰含两个键；错误类型原样沿用池实现的关联错误，没有在本层包装。
- `pub struct DdlStats`：由 `server_id: String` 和 `schema_version: i64` 组成的快照，派生可克隆、可比较和调试能力。
- `DdlStats::scope(&self, _status: &str) -> ScopeFlag`：忽略具体状态名，恒返回 `ScopeGlobal | ScopeSession`，对应 Go `variable.DefaultStatusVarScopeFlag`。
- `DdlStats::stats(&self) -> BTreeMap<String, StatusValue>`：不失败的快照导出；克隆 server ID，按值复制 schema version。

文件没有条件编译项、宏、异步函数或 unsafe 代码。

## 执行流程

动态路径从 `DdlStatistics::stats` 开始。第一步调用 `session_pool.get()`；由于使用 `?`，获取失败会立即返回，既不会读取版本，也不会调用 `put`。成功后，会话存入局部可变变量，随后调用 `schema_version(&mut session)`，但暂不使用 `?` 提前返回，而是把 `Result<i64, Error>` 保存下来。接着无条件调用 `put(session)`，最后才对保存的结果执行 `map`：读取成功时构造 `BTreeMap`，读取失败时原样返回该错误。

这一顺序是资源安全的核心：schema version 读取错误不能绕过会话归还。`pkg/ddl/stat_test.rs:ddl_stats_returns_session_on_success_and_read_error` 同时覆盖成功和读取失败，并断言两条路径都是一次 `get`、一次 `put`；`ddl_stats_does_not_put_when_session_acquisition_fails` 则锁定获取失败时没有可归还资源。

快照路径更短：调用方先构造 `DdlStats`，`scope` 对任意参数都返回 global/session 联合作用域；`stats` 直接把两个字段映射到固定键。`pkg/ddl/stat_test.rs:ddl_status_scope_and_value_types_match_go` 验证 scope、键名和值的具体枚举变体。

Go 生产路径为 `pkg/ddl/stat.go:(*ddl).Stats` → `d.sessPool.Get` → `GetDDLInfoWithNewTxn` → `GetDDLInfo` → metadata mutator 的 `GetSchemaVersionWithNonEmptyDiff`，并通过 `defer d.sessPool.Put(s)` 归还会话。Rust 的 `schema_version` trait 方法代表这整段下游边界，而不是在本文件中实现事务。

## 数据与状态

两个导出结果都使用 `BTreeMap<String, StatusValue>`。键集合固定为两项，排序由字符串字典序决定；API 没有依赖插入顺序的契约。每次调用都会分配新的 map 和拥有所有权的键。server ID 在写入结果时被克隆，schema version 是 `i64` 按值复制。

`DdlStatistics` 持有会话池，但 `stats` 只借用 `&self`；池实现必须自行提供 `get`/`put` 所需的内部可变性和同步。局部 `Session` 的所有权从 `get` 移入函数，再被 `put` 消耗。成功获取后，本函数在正常返回路径上恰归还一次。

`DdlStats` 是调用时刻的静态快照，不会观察后续 DDL 变化；`DdlStatistics` 则每次调用都重新请求 schema version。两者都不缓存、不递增版本，也不持有事务句柄。`StatusValue` 仅支持当前两个标量种类；新增状态若需要其他数值或布尔类型，不能在不扩展枚举或引入统一状态值类型的情况下表达。

## 依赖与调用关系

目标文件的直接依赖只有标准库 `std::collections::BTreeMap` 和 workspace crate `astersql-sessionctx-vardef` 的 `ScopeFlag`、`ScopeGlobal`、`ScopeSession`。`pkg/ddl/Cargo.toml` 以路径依赖 `../sessionctx/vardef` 引入后者；本文件不直接使用 manifest 中其他 DDL、meta、KV 或 session crate。

RustCodeGraph 对 `DdlStatistics::stats` 的 callee 关系显示它调用 `StatsSessionPool::{get,schema_version,put}` 并引用两个键常量及 `StatusValue::I64`；对目标结构的 trail 只找到 `pkg/ddl/stat_test.rs` 中的导入和实例化。精确仓库引用搜索也没有找到生产 Rust 调用方，说明模块当前公开但未接入运行主链。

独立测试由 `pkg/ddl/lib.rs` 的 `#[cfg(test)] mod stat_test;` 接线。Go 对照的上游使用证据包括 `pkg/ddl/restart_test.go:getDDLSchemaVer`，它通过 `ddl.DDL.Stats(nil)` 取出 `ddl_schema_version` 并断言值为 `int64`；Go 的通用状态变量框架则通过 DDL 接口消费 `GetScope`/`Stats`。Rust 尚不存在对应的适配边，因此不能把 `DdlStats::scope` 当作已经注册到状态变量系统。

## 错误处理与边界

`DdlStatistics::stats` 有两个可失败点。`get` 失败时错误通过 `?` 原样返回，且不调用 `put`；`schema_version` 失败时先归还已取得会话，再原样返回错误。与 Go 的 `errors.Trace` 不同，Rust 此层不增加上下文、堆栈或新的错误枚举。`put` 无返回值，因此归还失败无法被表达、记录或合并到查询错误中。

当前实现只保证显式 `Result` 路径的归还。如果 `schema_version` panic，控制流不会到达下一行 `put`；`StatsSessionPool::Session` 是否还能通过自身 `Drop` 恢复资源取决于具体实现，本文件没有保证。若生产适配器需要 panic/取消安全，应优先使用 RAII 会话守卫，不能只依赖当前的显式顺序。

server ID 允许空字符串，schema version 允许任意 `i64`，本文件不检查有效性或单调性。`scope` 忽略传入的状态名，意味着同一 `DdlStats` 导出的所有状态共享 scope。map 构造使用固定且不同的两个键，因此不会发生本文件内部的覆盖；如果未来新增重复键，`BTreeMap::from` 会只保留后一个值，需要测试明确协议。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、原子变量或通道。`stats(&self)` 是否可被多线程并发调用取决于 `P` 是否满足上层共享方式及其内部同步；trait 本身没有 `Send`/`Sync` 约束，因此模块不承诺跨线程能力。

动态路径的资源生命周期为“获取会话—可变借用读取—按所有权归还”。读取成功与普通错误都会在函数返回前归还，获取失败则不存在会话。与 Go 的 `defer Put` 相比，Rust 用保存 `Result` 后显式归还表达相同的普通错误语义，但没有通用 guard 覆盖 panic。

Go 的 `GetDDLInfoWithNewTxn` 会在会话上开始新事务、调用 `GetDDLInfo`，随后执行 rollback；Rust trait 把这一事务生命周期封装在 `schema_version` 实现责任内。本文件既不开始也不结束事务，因此未来适配器必须确保提交/回滚和会话归还的先后关系与 Go 一致，且不能把带未清理事务状态的会话放回池中。

## 与 Go 版本的对应关系

键常量逐字对应 `pkg/ddl/stat.go` 的 `serverID` 和 `ddlSchemaVersion`。`DdlStats::scope` 对应 `(*ddl).GetScope`，Rust 的 `ScopeGlobal | ScopeSession` 与 `pkg/sessionctx/variable/statusvar.go:DefaultStatusVarScopeFlag` 等价。`DdlStats::stats` 是 Rust 新增的纯快照表达；Go 没有独立的同名快照结构。

`DdlStatistics::stats` 对应 `(*ddl).Stats` 的主要控制流：获取池会话，读取当前 schema version，归还会话，返回 server ID 和整数版本。Rust 用泛型 trait 隔离真实 DDL/session/meta 依赖；Go 直接持有 `d.uuid`、`d.sessPool` 并调用 `GetDDLInfoWithNewTxn`。Go 返回 `map[string]any`，Rust 返回 `BTreeMap<String, StatusValue>`，因此 Rust 的值域更窄且顺序确定。

语义差异与缺口包括：Rust 尚无生产实现或状态框架适配；`schema_version` 只返回版本，未携带 Go `Info` 中的 `Jobs` 与 `ReorgHandle`；Rust 错误不经过 `errors.Trace`；Rust 显式 `put` 不覆盖 panic；本模块 `StatusValue` 与 Rust 通用状态变量模块的同名类型相互独立。Go 回归测试 `pkg/ddl/restart_test.go:getDDLSchemaVer` 证明生产 `Stats` 可用于 DDL worker 重启场景，而当前 Rust 测试只验证局部模型和 mock 池。

## 扩展指南

若新增 DDL 状态字段，应先确认 Go 的键名、scope 和动态值类型，再扩展 `StatusValue` 与两个 `stats` 路径，避免动态查询和快照导出产生不同键集。测试继续放在独立文件 `pkg/ddl/stat_test.rs`，至少覆盖键名、具体值变体、成功路径与每个错误分支；不要把 Rust 测试嵌入生产源文件。

若要接入生产 Rust 状态主链，最小接线应包括：为真实 DDL 会话池实现 `StatsSessionPool`；在 `schema_version` 内以新事务读取等价于 `GetSchemaVersionWithNonEmptyDiff` 的值并可靠回滚；把本模块值转换到 `pkg/sessionctx/variable/statusvar.rs` 的统一 `StatusValue`/`StatisticsHandle`；注册 provider 并验证 global/session scope。还应补集成级独立测试证明状态查询实际穿过注册框架，而不能用当前 mock 单测替代接线证据。

会话生命周期扩展应优先引入 RAII guard，使读取错误、早返回和 panic 都不会泄漏池资源，并定义 `put` 本身失败时的处理。如果 trait 增加 `Send`/`Sync`、异步方法或借用式 session，需同步评估对象安全、并发池语义和事务跨 await 的风险。

修改 schema version 来源时，要保持它是持久元数据中“带非空 diff”的当前版本，而不是本地缓存或某个 job 的临时版本；否则多节点观测和 Go 兼容性会偏离。该文件仍应保持只读状态查询，不应在此接入 job 推进、schema state 转换或版本发布副作用。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/stat.rs` 确认目标文件被索引且含 17 个符号；`node --file pkg/ddl/stat.rs --offset 1 --limit 400` 核对全部 96 行；`query` 精确定位 `StatsSessionPool`、`DdlStatistics`、`DdlStats`；`callees stat.rs::DdlStatistics` 显示动态 `stats` 对三个 trait 方法、键常量和值变体的调用/引用；精确 `explore` 找到测试实例化及 `get`/`put`/`schema_version` 调用边。
- Rust 源与装配：`pkg/ddl/stat.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。装配证据是生产模块无条件公开、测试模块仅在 `cfg(test)` 下接入；Cargo 证据是 crate 名为 `astersql-ddl`、入口为 `lib.rs`、scope 类型来自 `astersql-sessionctx-vardef` 路径依赖。
- Rust 独立测试：`pkg/ddl/stat_test.rs` 中的 `ddl_status_scope_and_value_types_match_go`、`ddl_stats_returns_session_on_success_and_read_error`、`ddl_stats_does_not_put_when_session_acquisition_fails`。同文件后半的 DDL job fixture 测试不直接调用本文件 API，未被当作状态导出行为证据。
- Go 对照：`pkg/ddl/stat.go` 的 `GetScope`/`Stats`，`pkg/ddl/ddl.go:GetDDLInfoWithNewTxn`/`GetDDLInfo`，以及 `pkg/sessionctx/variable/statusvar.go:DefaultStatusVarScopeFlag`。Go 使用证据来自 `pkg/ddl/restart_test.go:getDDLSchemaVer`。
- 包契约与 DDL 边界：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`。本文件读取元数据状态但不驱动 job、schema state、reorg、持久化 checkpoint、schema sync、MDL 或回滚状态机。
- 交付按任务约束不运行 Cargo；使用任务指定命令验证目标文档存在且恰有十一个固定二级标题，并用精确引用搜索人工确认未把测试专用、尚未接线的 Rust API 描述为生产现状。
