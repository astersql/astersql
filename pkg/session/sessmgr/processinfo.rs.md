# `pkg/session/sessmgr/processinfo.rs`

## 文件定位

该文件是 `astersql-session-sessmgr` crate 的进程快照与会话管理契约实现文件。crate 根 `pkg/session/sessmgr/lib.rs` 以 `pub mod processinfo; pub use processinfo::*;` 将这里的公开类型和函数暴露给执行器、服务器、会话运行时及 InfoSchema 协调代码；依赖边界由 `pkg/session/sessmgr/Cargo.toml` 声明。

它处于“连接/语句运行状态的生产者”和“SHOW PROCESSLIST、INFORMATION_SCHEMA、KILL 等消费者”之间：服务器实现 `Manager` 并生产 `ProcessInfo`，展示层读取快照并编码成结果行。RustCodeGraph 显示该文件被 `pkg/executor/show.rs`、`pkg/server/server.rs`、`pkg/domain/crossks/coordinator.rs` 等上游引用；其中可直接核验的主链是 `FetchShowProcessListRows -> Manager::ShowProcessList -> ProcessInfo::ToRowForShow`。

## 核心职责

- 用 `ProcessInfo` 表示连接当前语句、事务、计划、资源消耗及展示元数据的快照；用 `OOMAlarmVariablesInfo` 保存 OOM 告警所需的会话变量（`ProcessInfo`、`OOMAlarmVariablesInfo`）。
- 将快照编码为 SHOW PROCESSLIST 的 8 列或 INFORMATION_SCHEMA.PROCESSLIST 的 20 列，并处理 NULL、截断、时间、主机端口、状态位和资源统计（`ToRowForShow`、`ToRow`、`txnStartTs`、`serverStatus2Str`）。
- 维持 Go 指针、接口和切片字段的浅拷贝语义：Rust 用 `Arc` 表示共享对象，`Clone` 只增加引用计数（`Clone`、`clone_shallow`）。
- 定义会话管理器对展示、终止连接、TLS 更新、内部会话和 InfoSchema/MDL 协调的公共契约（`Manager`、`InfoSchemaCoordinator`、`NormalCloseKiller`）。
- 为带“正常关闭原因”的 kill 提供能力探测及普通 kill 回退（`KillWithNormalCloseMsg`）。

## 主要符号

- `OOMAlarmVariablesInfo`：三个按值保存的 OOM 告警字段；`SessionAnalyzeVersion: isize` 对齐 Go `int` 的目标指针宽度。
- `ErasedRunawayChecker`：`Send + Sync` 的类型擦除 trait；所有 `'static` 的 `resourcegroup::RunawayChecker` 自动实现，用于在 `ProcessInfo` 中存放异构 checker。
- `StatsInfoFn = fn(&dyn Any) -> HashMap<String, u64>`：从类型擦除的计划中提取统计计数的函数指针。
- `ProcessListValue`：结果单元格的内部表示，包含 `Null`、无符号/有符号整数、浮点数和文本；`Display` 将 `Null` 打印为 `<nil>`，供 `String` 调试输出使用。
- `ProcessInfo`：核心快照。时间和标量字段按值保存；计划、tracker、statement context、CPU 统计、运行时统计及切片等共享字段用 `Option<Arc<_>>` 或 `Arc<Vec<_>>` 保存。
- `ProcessInfo::default` / `go_zero_time`：构造与 Go 零值一致的快照；时间采用公元 0001-01-01 UTC 的 `SystemTime` 表示，其他字段取空值或零值。
- `ProcessInfo::Clone` / `clone_shallow`：返回一个新的结构体值；字符串按 Rust 值语义复制，所有 `Arc` 指向原对象，函数指针保持相同地址。
- `ProcessInfo::ToRowForShow(full)`：生成固定 8 列 `[ID, User, Host, DB, Command, Time, State, Info]`。
- `ProcessInfo::ToRow(tz)`：以 FULL 展示行开头，追加 digest、内存仲裁相关列、磁盘、事务时间、资源组、会话别名、影响行数和 TiDB/TiKV CPU 纳秒，共 20 列。
- `InfoSchemaCoordinator`：内部会话登记、计数、老事务/MDL 检查及 flashback 后连接清理契约。
- `Manager`：继承 `InfoSchemaCoordinator`，定义进程/事务读取、kill、TLS、server ID、连接属性和状态变量；`GetPerformanceSchemaAccountSummaries` 与 `as_normal_close_killer` 有空结果/不支持的默认实现。
- `PerformanceSchemaAccountSummary`：`performance_schema.accounts` 所需的用户、主机和当前/累计连接计数；匿名或后台身份可用 `None` 表示。
- `NormalCloseKiller`、`NormalCloseMsgKillStmt`、`NormalCloseMsgKillStmtFromRemote`、`KillWithNormalCloseMsg`：带关闭原因的可选扩展及两个标准原因字符串。

## 执行流程

1. 服务器或系统会话构造并发布 `ProcessInfo`。`pkg/server/server.rs` 的 `Server` 将内部连接信息经 `to_session_process_info` 转换成该类型，并在 `ShowProcessList` 中合并 domain 的系统进程。
2. SHOW PROCESSLIST 路径中，`pkg/executor/show.rs::FetchShowProcessListRows` 取得 `Manager::ShowProcessList` 的快照；若调用者没有 PROCESS 权限，仅保留与登录用户名相同的连接，然后逐条调用 `ToRowForShow(full)`。
3. `ToRowForShow` 将空 `Info`、空 `DB` 转为 `Null`；非 FULL 模式按 Unicode `char` 截取前 100 个字符；有端口时通过 `join_host_port` 形成 `host:port`（IPv6 加方括号）；再查 `mysql::Command2Str`、计算运行整秒并格式化状态位。
4. INFORMATION_SCHEMA 路径调用 `ToRow(tz)`。函数先尝试增加 `RefCountOfStmtCtx`；成功且 `StmtCtx` 存在时读取内存、磁盘和影响行数，作用域退出时由 `ReferenceGuard::drop` 成对执行 `Decrease`。随后读取 CPU 快照，以 `ToRowForShow(true)` 为前 8 列并追加 12 列。
5. `txnStartTs` 将 StartTS 右移 18 位获得物理毫秒，按调用者时区格式化为 `MM-DD HH:MM:SS.mmm(StartTS)`；StartTS 为零或时间戳不可表示时返回空串。
6. kill 路径调用自由函数 `KillWithNormalCloseMsg`：只有关闭原因非空且 `Manager::as_normal_close_killer` 返回扩展接口时才调用扩展方法，否则无条件回退到 `Manager::Kill`。

## 数据与状态

`ProcessInfo` 是一次可共享的观察快照，不拥有会话执行循环。身份/展示状态包括 `ID`、`User`、`Host`、`Port`、`DB`、`Command`、`State`、`Info`、`Digest` 和 `SessionAlias`；事务状态包括 `CurTxnStartTS`、`CurTxnCreateTime`；资源状态包括 `MemTracker`、`DiskTracker`、`SQLCPUUsage`、`RuntimeStatsColl`、`ResourceGroupName` 和 OOM 变量；诊断状态包括 `Plan`、`StatsInfo`、`BriefBinaryPlan`、`IndexNames`、`TableIDs` 和脱敏 SQL。

SHOW 行的列顺序是公开契约。`ToRowForShow` 始终返回 8 项；`ToRow` 始终返回 20 项。`serverStatus2Str` 只输出 `ASC_SERVER_STATUS` 中已知且置位的状态，严格按常量表的升序以 `; ` 连接，未知位被忽略。命令字不在 `mysql::Command2Str` 时输出空文本。

Go 零时间与 Rust `SystemTime` 的差异由 `go_zero_time` 和 `elapsed_seconds` 显式桥接：持续时间先钳制到 Go `time.Duration` 的 `i64` 纳秒范围，再整除为秒并转换为 `u64`。因此默认快照的运行时长会饱和到约 292 年，而不是溢出或报错。

## 依赖与调用关系

`pkg/session/sessmgr/Cargo.toml` 将该 crate 连接到 `chrono`/`chrono-tz`、`rustls`，以及 auth、cursor、disk、execdetails、mdldef、memory、mysql、ppcpuusage、resourcegroup、stmtctx、txninfo 等本地 crate；`lib.rs` 将这些依赖重导出为本 crate 模块，供本文件统一引用。

主要下游调用为：`ToRowForShow -> join_host_port / command_name / elapsed_seconds / serverStatus2Str`；`ToRow -> ReferenceCount::TryIncrease / ReferenceGuard::drop -> Decrease / Tracker::BytesConsumed / StatementContext::AffectedRows / SQLCPUUsages::GetCPUUsages / txnStartTs / ToRowForShow`；`KillWithNormalCloseMsg -> NormalCloseKiller::KillWithNormalCloseMsg` 或 `Manager::Kill`。RustCodeGraph 明确给出了 `ToRow -> ToRowForShow`，源码及独立测试补齐了其余局部边。

主要上游包括 `pkg/executor/show.rs::FetchShowProcessListRows`（展示行消费者）与 `pkg/server/server.rs`（`Manager`、`InfoSchemaCoordinator`、`NormalCloseKiller` 的生产实现）。服务器的 `GetProcessInfo` 还在普通连接缺失时查询 domain 系统进程；`Kill` 在本地连接未命中时转交 domain 的系统进程终止逻辑。

## 错误处理与边界

本文件的公开编码函数不返回 `Result`，而通过确定的降级值处理缺失或不可表示数据：空 DB/SQL 为 `Null`，缺少 tracker 时资源用量为零，缺少或无法安全持有 statement context 时影响行数为 `Null`，缺少 CPU tracker 时 CPU 为零，非法物理毫秒或零 StartTS 得到空串，未知命令得到空文本，未知状态位被忽略。

`elapsed_seconds` 对“当前时间早于开始时间”的负持续时间沿用 Go 的有符号 duration 计算后转换为 `u64` 的语义；对超出 Go duration 范围的值先饱和。调用者不应把异常大的无符号秒数误认为正常墙钟时长。

`KillWithNormalCloseMsg` 的扩展能力不是强制要求：非空原因但实现未暴露 `NormalCloseKiller` 时仍执行普通 kill，不会因为无法记录原因而拒绝终止。`Manager` 的 performance schema 汇总默认返回空数组，因此“实现了 Manager”不代表一定提供账户汇总。

## 并发与资源生命周期

公开 trait 要求管理器及 InfoSchema 协调器实现 `Send + Sync`；`ErasedRunawayChecker` 也要求 `Send + Sync`。`ProcessInfo` 中跨线程共享的动态对象均受 `Arc` 管理，`Clone` 不复制底层 tracker/context/plan 状态，因此克隆后的快照可能观察到这些共享对象的后续内部变化；字符串和标量则是克隆时的独立值。

读取 `StmtCtx` 派生状态前必须先通过 `RefCountOfStmtCtx::TryIncrease`。成功后立即建立 `ReferenceGuard`，所有正常离开作用域的路径都会调用 `Decrease`，防止语句结束并发释放时产生悬空访问。若增加失败，函数不读取 statement context、内存/磁盘 tracker 或影响行数，而采用安全的零值/NULL。

`Manager` 只定义生命周期操作，不规定锁实现。直接实现证据位于 `pkg/server/server.rs`：服务器对 clients 使用读锁，对 TLS 配置使用写锁，对账户累计数据使用互斥锁；这些锁策略属于实现而非本 trait 的保证。

## 与 Go 版本的对应关系

直接对照 `pkg/session/sessmgr/processinfo.go`：Rust 保留了 `ProcessInfo` 字段、8/20 列布局、浅拷贝、100 字符展示截断、StartTS 物理时间解码、状态位顺序、InfoSchema 协调接口、Manager 方法以及 normal-close kill 的能力探测/回退。`Arc` 对应 Go 指针、接口或切片头共享语义；`Option` 对应 nil；`ProcessListValue` 替代 Go 的 `any` 结果单元格。

需注意以下已验证差异：

- Go `ToRow` 会读取 `StmtCtx.MemTracker.MemArbitration()` 和 `WaitArbitrate()`；当前 Rust 所依赖的非仲裁器实现不暴露这些能力，所以索引 10、11、12 的三列固定为 `Null`。这不是“已完整支持内存仲裁”。
- Rust `Manager` 额外提供 `GetPerformanceSchemaAccountSummaries` 和 `as_normal_close_killer` 默认方法，`PerformanceSchemaAccountSummary` 也没有出现在同路径 Go 文件中；它们是 Rust 服务器接线所需的扩展。
- Go `Clone` 返回指针，Rust 返回新的结构体值；对共享字段的可观察语义通过 `Arc` 保持一致。
- Rust 非 FULL 截断使用 `.chars().take(100)`，现有迁移测试以 101 个中文字符验证按字符截断；这与同路径 Go 的格式化行为在该覆盖范围内一致。

## 扩展指南

新增或调整 PROCESSLIST 列时，应同时修改 `ProcessListValue`（若需新数据类型）、`ToRowForShow` 或 `ToRow` 的固定顺序、对应展示层转换，以及 `pkg/session/sessmgr/migration_aster_unit_test.rs` 的长度和索引断言；列顺序变化具有 SQL 兼容风险。若补齐内存仲裁能力，应接入 statement-context memory tracker 的真实接口并覆盖仲裁时长、等待时间/字节数及并发失效路径，不能仅填占位值。

给 `ProcessInfo` 增加共享字段时，必须同步 `default` 和 `clone_shallow`，并在独立测试文件中用 `Arc::ptr_eq` 验证共享语义；增加按值字段则验证克隆后的相等与相互独立。不要把测试内嵌进本生产文件，现有测试入口是 `processinfo_test.rs` 和 `migration_aster_unit_test.rs`。

扩展 `Manager` 时要检查 `pkg/server/server.rs`、`pkg/session/runtime_test/session.rs` 以及 executor 测试中的所有实现；没有安全通用语义的方法不应随意添加默认空实现。修改 kill 参数或能力探测时，需同步 `NormalCloseKiller`、自由函数及服务器实现，并测试非空原因、空原因和不支持扩展的三条分支。

性能方面应保持行编码为线性扫描：状态位表和命令表都很小；避免为每行引入全局锁或深拷贝计划/tracker。兼容方面需保持 NULL 与空字符串、纳秒单位、StartTS 格式、IPv6 括号及 100 字符边界。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件且目标文件含 44 个符号；`files --filter pkg/session/sessmgr` 确认源、模块入口、Go 对照和测试均已索引；`node --file pkg/session/sessmgr/processinfo.rs` 核对全部 545 行；`explore` 给出 `ToRow -> ToRowForShow`，并指出 executor、server、domain 等引用面。
- 源与 crate 边界：`pkg/session/sessmgr/processinfo.rs`、`pkg/session/sessmgr/lib.rs`、`pkg/session/sessmgr/Cargo.toml`。
- 直接上游证据：`pkg/executor/show.rs::FetchShowProcessListRows`；`pkg/server/server.rs::{to_session_process_info, impl InfoSchemaCoordinator for Server, impl NormalCloseKiller for Server, impl SessionManager for Server}`。
- Go 对照：`pkg/session/sessmgr/processinfo.go`；Go 浅拷贝测试 `pkg/session/sessmgr/processinfo_test.go::TestProcessInfoShallowCP`。
- Rust 独立测试：`pkg/session/sessmgr/processinfo_test.rs::test_process_info_shallow_cp`；`pkg/session/sessmgr/migration_aster_unit_test.rs` 覆盖 SHOW 行、Unicode/IPv6/NULL、Go 零时间饱和、浅拷贝、20 列资源/事务/CPU、状态位顺序和 normal-close kill 回退。
- 本任务为只读行为分析加文档产出，按计划不运行 Cargo；交付前使用任务给定的命令验证目标文件存在且固定二级标题恰为 11 个，并人工复核没有把非仲裁器占位或默认空能力写成已支持行为。
