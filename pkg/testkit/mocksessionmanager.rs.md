# `pkg/testkit/mocksessionmanager.rs`

## 文件定位

本文件属于 `astersql-testkit` crate；crate 入口由 `pkg/testkit/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `pkg/testkit/lib.rs`，后者通过 `pub mod mocksessionmanager;` 将本模块公开。它是测试基础设施中的内存态会话管理器，用简化的 Rust 数据结构复刻 Go `pkg/testkit/mocksessionmanager.go` 的主要可观察行为，供测试构造进程、事务、连接及内部会话状态。

它不是生产服务器会话管理器：当前文件没有实现 `pkg/session/sessmgr` 中的管理器 trait，也没有连接真实 `Session`、`Domain`、TLS 配置或 SQL 执行链。仓库 Rust 引用搜索显示，实际方法调用集中在独立测试 `pkg/testkit/mocksessionmanager_test.rs`；因此应把它理解为可独立使用的 test double，而非应用主链组件。

## 核心职责

- 维护显式进程 fixture，并在没有显式 fixture 时从连接快照回退生成进程列表（`StoreProcessInfo`、`ShowProcessList`、`GetProcessInfo`）。
- 维护显式事务 fixture，并在没有显式 fixture 时从连接快照回退生成事务列表（`SetTxnInfo`、`GetTxnInfo`、`ShowTxnList`）。连接回退事务只有在 `TxnInfo::has_process_info` 为真时才进入列表，与 Go 版要求 `TxnInfo.ProcessInfo != nil` 对齐。
- 保存连接属性、server ID 和内部会话的可选事务信息，支持测试身份、数量和 start TS 收集逻辑。
- 提供 Go mock 所需的空操作接口：`Kill`、`KillAllConnections`、`UpdateTLSConfig` 和始终为空的 `GetStatusVars`。
- 用预计算的 `SessionSnapshot` 字段模拟 flashback 连接筛选及 MDL job 清理，而不依赖完整 planner/session 类型。

## 主要符号

- `ProcessInfo`：公开进程快照，字段为连接 `id`、用户、数据库、命令、开始时间及兼容字段 `killed`。`killed` 只为早期 Rust 调用者保留，`Kill` 不会修改它。
- `TxnInfo`：公开事务快照，包含 `connection_id`、`start_ts`、可选 SQL digest，以及模拟 Go `ProcessInfo != nil` 的 `has_process_info`。
- `SessionSnapshot`：公开连接快照，聚合可选进程/事务、连接 ID、是否为 flashback-cluster 语句及持有的 DDL job ID 集合。
- `State`：私有聚合状态，保存显式事务、连接、连接属性、内部会话和 server ID；调用者不能绕过方法直接修改。
- `MockSessionManager`：公开、可克隆的管理器。克隆仅克隆两个 `Arc`，所以所有 clone 共享同一份状态。
- 写入/删除 API：`StoreProcessInfo`、`DeleteProcessInfo`、`StoreConnection`、`DeleteConnection`、`SetTxnInfo`、`SetConAttrs`、`SetServerID`、`StoreInternalSession`、`DeleteInternalSession`。
- 查询/协调 API：`ShowProcessList`、`GetProcessInfo`、`ShowTxnList`、`GetTxnInfo`、`GetConAttrs`、`ServerID`、`ContainsInternalSession`、`InternalSessionCount`、`GetInternalSessionStartTSList`、`KillNonFlashbackClusterConn`、`CheckOldRunningTxn`、`GetStatusVars`。

文件中没有模块级常量、trait、自由函数或条件编译项；全部行为位于 `MockSessionManager` 的固有 `impl` 中。方法名保留 Go 风格的大写开头，以便逐项对照迁移接口。

## 执行流程

1. 调用者通常用 `MockSessionManager::default()` 创建空管理器；`State`、进程向量和锁均取默认空值。
2. 测试可通过 `StoreProcessInfo` / `SetTxnInfo` 设置显式 fixture，或通过 `StoreConnection` 设置较接近活动连接的 `SessionSnapshot`。
3. `ShowProcessList` 先持有进程读锁；只要显式进程向量非空，就按 `ProcessInfo::id` 构造并立即返回 map。否则主动释放读锁，再锁住 `State`，从带 `process_info` 的连接快照构造结果。
4. `GetProcessInfo` 同样先查显式向量，再按连接 ID 查连接快照。两层都没有结果时返回 `None`。
5. `ShowTxnList` 在同一把 state 锁下检查显式事务；非空时整体 clone 返回，否则遍历连接事务，只保留存在事务且 `has_process_info` 为真的项目。`GetTxnInfo` 对单个 ID 也优先显式事务，再回退连接事务，但不会应用列表过滤条件。
6. 内部会话以 `u64` 身份作为 key、`Option<TxnInfo>` 作为 value；start TS 查询忽略没有事务的内部会话，返回值沿用 `HashMap` 遍历顺序，调用者不得依赖排序。
7. `KillNonFlashbackClusterConn` 遍历连接，对非 flashback 快照调用 `Kill`；由于 `Kill` 固定返回 `false` 且无副作用，该流程只复刻遍历/筛选行为。
8. `CheckOldRunningTxn` 逐个连接执行 `jobs.retain`，移除任一连接的 `lock_ddl_jobs` 中出现的 job ID，最终保留所有连接均未锁定的 job。

## 数据与状态

状态分成两个锁域：`processes: Arc<RwLock<Vec<ProcessInfo>>>` 模拟 Go 的 `PS`/`PSMu`；`state: Arc<Mutex<State>>` 管理其余数据。`StoreProcessInfo` 和 `SetTxnInfo` 都按连接 ID 执行 upsert，保留首次插入次序；重复 ID 会原位替换。`DeleteProcessInfo` 除删除进程外，还会删除相同连接 ID 的显式事务，但不会删除 `connections` 中的回退快照。

`StoreConnection` 以方法参数 `id` 作为 map key，而快照自身另有 `connection_id`。查询进程/事务按 map key 定位；`KillNonFlashbackClusterConn` 调用 `Kill` 时使用快照内的 `connection_id`。当前实现不校验两者相等，这是构造 fixture 时必须维持的调用者不变量。

所有返回的 `ProcessInfo`、`TxnInfo`、连接属性 map 都是克隆值；调用者修改返回值不会反向修改管理器。`started_at: SystemTime` 只作为数据承载字段，本文件不计算持续时间。默认 `server_id` 为 `0`，状态变量固定为空 map。

## 依赖与调用关系

直接语言依赖仅来自标准库：`HashMap`/`HashSet`、`Arc`/`Mutex`/`RwLock` 和 `SystemTime`。尽管 `pkg/testkit/Cargo.toml` 声明了 session、domain、planner、parser 等多个 workspace crate，本文件没有直接导入这些 crate；这是因为它使用本地简化快照，而非完整生产类型。

上游方面，`pkg/testkit/lib.rs` 公开模块并在 `#[cfg(test)]` 下挂载 `pkg/testkit/mocksessionmanager_test.rs`。RustCodeGraph 对目标文件的文件级关系报告 “used by” `pkg/testkit/mocksessionmanager_test.rs` 和 `pkg/testkit/mockstore_domain_ddl_test.rs`；但源码引用搜索只在前者发现 `MockSessionManager` 方法调用，后者没有直接导入或调用本模块，故不能把后者视为已确认的运行时调用者。当前仓库没有发现非测试 Rust 文件调用这些方法。

下游方面，方法只调用标准集合、锁和本 `impl` 的 `Kill`；没有网络、存储、SQL、Domain 或后台任务调用。Go 对照实现则作为大量 Go planner/executor/infoschema 测试的 `sessmgr.Manager` test double，这一广泛接线尚不能外推为 Rust 版本现状。

## 错误处理与边界

本 API 不返回 `Result`。所有锁获取都使用 `expect("process list poisoned")` 或 `expect("session manager poisoned")`；若持锁线程 panic 导致锁中毒，后续访问会 panic，而不是恢复或向上传播可处理错误。

边界行为包括：空管理器返回空列表/map 或 `None`；删除未知 ID 静默成功；重复存储进程/事务执行替换；连接快照中没有 `process_info` 时不会出现在进程列表；连接事务的 `has_process_info == false` 时不会出现在事务列表；内部会话 value 为 `None` 仍计入数量和 contains 查询，但不贡献 start TS。

显式进程或事务只要集合非空，就完全遮蔽连接回退，而不是合并两类来源。`GetProcessInfo` 找不到时返回 `None`，不同于 Go 版返回零值指针加 `false`。`GetConAttrs` 没有 Go 版的 user 参数和过滤入口。`KillAllConnections`、`UpdateTLSConfig`、`GetStatusVars` 是有意的空实现，不应据此声称对应生产能力已支持。

## 并发与资源生命周期

`Arc` 让 clone 跨线程共享状态；`RwLock` 允许并发读取显式进程，`Mutex` 串行化其余状态访问。文件没有线程、异步任务、通道、事务句柄或外部资源，生命周期完全由最后一个 `MockSessionManager` clone 的 drop 决定。

实现避免在 `ShowProcessList` 的回退路径同时持有 process 读锁和 state mutex：在锁 state 前显式 `drop(processes)`。其他跨锁方法中，`DeleteProcessInfo` 先释放临时 process 写锁再取得 state mutex。`KillNonFlashbackClusterConn` 在持有 state mutex 时调用当前无锁、无副作用的 `Kill`；若未来让 `Kill` 获取同一 state 锁，这里会产生自死锁风险，扩展时必须先复制目标 ID 并释放锁再逐个处理。

集合遍历顺序不稳定：`ShowProcessList` 的返回类型本身是 `HashMap`，连接回退事务、内部 session start TS 及 job 处理也不提供排序保证。测试如需确定性，应按 key/TS 排序后断言，而不是依赖当前哈希实现。

## 与 Go 版本的对应关系

Go 权威对照为 `pkg/testkit/mocksessionmanager.go`。Rust `processes` 对应 `PS` + `PSMu`，`State::explicit_transactions` 对应 `TxnInfo`，`connections` 对应 `Conn`，`connection_attributes` 对应 `ConAttrs`，`server_id` 对应 `SerID`，`internal_sessions` 对应 `internalSessions`。进程/事务“显式集合非空则优先，否则连接回退”、kill/TLS/status 空操作，以及内部会话 start TS 收集的总体意图一致。

当前 Rust 版有明确简化：

- 用自定义 `ProcessInfo`、`TxnInfo`、`SessionSnapshot` 代替 Go 的 `sessmgr.ProcessInfo`、`txninfo.TxnInfo` 和 `sessionapi.Session`，因此不会动态读取真实 session 状态。
- 没有 Go `Dom` 字段，`ShowProcessList` / `GetProcessInfo` 不会合并 `Domain.SysProcTracker()` 的系统进程。
- Rust `GetConAttrs()` 没有 `*auth.UserIdentity` 参数；`Kill` 也没有 Go 接口中的 query/connection/server-side 标志，且额外以 `bool` 暴露固定失败结果。
- Rust 内部会话以数值 ID 和存储时的可选事务快照建模，而 Go 以对象身份为 key、查询时动态调用 `TxnInfo()`。
- Rust `CheckOldRunningTxn` 使用 `HashSet<i64>`，只表达 job ID 删除；Go 使用 `map[int64]*mdldef.JobMDL` 并委托 `variable.RemoveLockDDLJobs`。
- Rust 已增加显式的 Store/Delete/Set/Get 辅助方法，便于独立测试；这些不是 Go mock 的同名 API。

因此本文件属于行为导向的测试替身迁移，并非 Go `sessmgr.Manager` 的签名级完整移植。

## 扩展指南

新增功能前先决定目标是保持轻量 fixture，还是让它真正实现 Rust session-manager trait。若只是扩展 fixture，应优先在 `SessionSnapshot` / `State` 增加最小字段并保持“显式数据优先、连接数据回退”的现有不变量；相应测试放在独立文件 `pkg/testkit/mocksessionmanager_test.rs`，不要内嵌到生产源文件。

修改 `ShowProcessList` / `GetProcessInfo` 时要同时考虑显式与连接两条路径，以及 Go 的 Domain 系统进程语义是否需要移植。修改事务列表时应保持 `has_process_info` 过滤，并为 `GetTxnInfo` 与 `ShowTxnList` 的有意差异添加测试。修改内部会话时要覆盖 `None` 事务、重复 ID、删除未知 ID 和非确定遍历顺序。

若实现真实 kill 或 TLS 更新，不能只填充现有空方法：需要先对齐目标 trait 签名、锁顺序和调用者预期，特别要消除 `KillNonFlashbackClusterConn` 持 state 锁调用 `Kill` 的潜在重入死锁。若引入 workspace crate 类型或新依赖，还需同步 `pkg/testkit/Cargo.toml`，并检查它是否改变 testkit crate 的依赖边界和编译成本。

兼容性风险主要来自公开结构字段和大写方法名；正确性风险集中在显式/回退优先级、map key 与 `SessionSnapshot::connection_id` 不一致，以及 Go/Rust 类型语义差异；性能风险较低，但所有连接和事务查询均为线性遍历或整集合 clone，大 fixture 下会放大锁持有时间和分配量。

## 验证依据

- 源文件：`pkg/testkit/mocksessionmanager.rs`，核对 4 个结构体、`MockSessionManager` 固有实现的全部方法、锁域和分支；文件没有条件编译项。
- crate 与模块边界：`pkg/testkit/Cargo.toml`、`pkg/testkit/lib.rs`。
- Rust 独立测试：`pkg/testkit/mocksessionmanager_test.rs`，覆盖 kill 空操作、显式 fixture 覆盖连接回退、无进程信息事务过滤、内部会话身份/计数/start TS、server ID、连接属性、MDL job 清理及空 status vars。
- Go 对照：`pkg/testkit/mocksessionmanager.go`；仓库引用搜索还确认其用于 Go testkit/store 及 planner、executor、infoschema 等测试，但这些 Go 调用关系不等同于 Rust 已接线。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/testkit/mocksessionmanager.rs` 报告 33 个符号；`node --file ...` 返回完整 288 行源码及两个文件级 “used by” 关系；`query` 确认 Rust `MockSessionManager`、`State`、`SessionSnapshot`、`ProcessInfo`、`TxnInfo` 及方法节点。精确 `callers`/`callees` 命令在本次执行中超时且未返回边，故调用事实以图的文件级关系和 `rg` 实际引用结果为准，未将未返回的图边推断为事实。
- 人工复核：本文明确回答该文件为何存在（测试用会话管理替身）、状态如何写入和查询、显式/回退流程、锁与 clone 生命周期、Go 差异，以及安全扩展时的测试和死锁注意点。
