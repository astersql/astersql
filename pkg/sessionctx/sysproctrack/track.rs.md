# `pkg/sessionctx/sysproctrack/track.rs`

## 文件定位

处在会话状态与系统进程管理的边界：被跟踪对象暴露 `SessionVars` 和 `ProcessInfo`，跟踪器负责按连接 ID 注册、注销、列举和发出终止请求。

## 核心职责

1. 用 `TrackProc` 规定一个可跟踪系统进程必须能提供共享的会话变量，以及可空、共享所有权的进程展示快照。
2. 用 `Tracker` 规定系统进程注册表的四个操作：`Track`、`UnTrack`、`GetSysProcessList`、`KillSysProcess`。
3. 用 `TrackProcRef = Arc<dyn TrackProc>` 表示具有稳定对象身份且可跨线程共享的进程句柄；`Arc::ptr_eq` 可用于复现 Go 接口值的“同一对象”判断。
4. 用 `TrackError = anyhow::Error` 承接 Go `error` 边界，使实现可以报告注册冲突等失败，而不在契约层固定错误枚举。

文件注释提到 `SHOW PROCESSLIST` 和 kill 管理路径，这是接口的设计用途；但本文件本身既不实现 SQL 展示，也不发送 kill 信号。对应的完整 Go 行为位于 `pkg/domain/domain.go` 的 `SysProcesses` 方法中。

## 主要符号

- `pub type TrackError = anyhow::Error`：`Tracker::Track` 的开放错误类型。只有注册操作返回错误；其余三个方法按 Go 接口保持无返回错误。
- `pub type TrackProcRef = Arc<dyn TrackProc>`：擦除具体进程类型并共享所有权。`TrackProc: Send + Sync` 使 trait object 可跨线程引用，`Arc` 同时为实现方提供稳定身份与引用计数生命周期。
- `pub trait TrackProc: Send + Sync`：被跟踪进程的最小接口。
  - `GetSessionVars(&self) -> &Mutex<SessionVars>` 返回对象内部长期持有的会话变量锁。调用方必须取得互斥锁后读取或修改 `ConnectionID`、`SQLKiller` 等状态。
  - `ShowProcess(&self) -> Option<Arc<ProcessInfo>>` 返回当前展示快照；`None` 表示当前没有可展示进程。返回 `Arc` 允许快照在离开进程对象后继续被列表持有。
- `pub trait Tracker: Send + Sync`：并发安全跟踪器的抽象边界。
  - `Track(&self, id, proc) -> Result<(), TrackError>` 注册进程，唯一可报告失败的入口。
  - `UnTrack(&self, id)` 注销指定 ID。
  - `GetSysProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>>` 生成以跟踪 ID 为键的快照集合。
  - `KillSysProcess(&self, id)` 请求终止已跟踪进程。

本文件没有常量、结构体、枚举、自由函数、`impl` 块或条件编译项；`#![allow(non_snake_case)]` 专门保留与 Go API 相同的方法名。

## 执行流程

本文件只约束调用形状，实际流程由实现方完成。根据同路径 Go 实现 `pkg/domain/domain.go::SysProcesses` 和独立 Rust 对齐测试 `migration_aster_unit_test.rs::MockTracker`，预期流程如下：

1. 上游取得一个 `TrackProcRef`，并以系统进程 ID 调用 `Tracker::Track`。
2. 实现方独占注册表；若 ID 已绑定不同对象则返回冲突错误，同一对象重复注册可幂等成功。成功后把 `SessionVars.ConnectionID` 设为 ID。Go 生产实现还重置 `SQLKiller`；trait 本身没有把这一副作用编码成类型约束。
3. 管理面调用 `GetSysProcessList` 时遍历注册表，向每个对象调用 `ShowProcess`，只保留非空且 `ProcessInfo.ID` 与注册键一致的快照。
4. kill 路径调用 `KillSysProcess(id)`；Go `SysProcesses` 在 ID 存在时通过该进程会话变量中的 `SQLKiller` 发送 `QueryInterrupted`，未知 ID 被静默忽略。
5. 生命周期结束时调用 `UnTrack(id)`。Go 实现删除映射、把 `ConnectionID` 清零并重置 `SQLKiller`；Rust 对齐测试覆盖删除和清零，但测试用实现没有模拟 `SQLKiller` 重置。

Go 的实际上游例子是自动分析：`pkg/statistics/handle/autoanalyze/exec/exec.go::RunAnalyzeStmt` 将 `sysProcTracker.Track`/`UnTrack` 包装进 `ExecOptionWithSysProcTrack`，而 `pkg/domain/domain.go::CheckAutoAnalyzeWindows` 会对越过时间窗口的进程调用 `KillSysProcess`。当前 Rust 自动分析代码定义的是本地 `SysProcTracker` trait，尚未直接实现或使用本文件的 `Tracker`。

## 数据与状态

本文件自身无全局变量和可变字段。状态所有权被刻意留给两侧：

- 被跟踪对象拥有 `Mutex<SessionVars>`；接口借用该锁而不转移所有权。`SessionVars` 是可变的会话级状态，注册 ID 和 kill 状态均可能由具体跟踪器通过它更新。
- `TrackProcRef` 的 `Arc` 强引用保证对象至少活到所有注册表、回调和调用者释放句柄为止，并保留可比较的分配身份。
- `ShowProcess` 的 `Option` 表达快照暂不可用，内部 `Arc<ProcessInfo>` 表达快照共享所有权；`GetSysProcessList` 返回新的 `HashMap`，因此结果容器不借用实现方注册表。
- `Tracker` 没有规定映射类型或锁类型。Go 使用 `map[uint64]TrackProc` 与 `sync.RWMutex`；Rust 测试实现使用 `RwLock<HashMap<u64, TrackProcRef>>`，这只是符合契约的一种实现。

关键不变量来自 Go 对照与迁移测试，而不是 trait 类型系统自动保证：注册键应与展示快照 ID 一致；同一 ID 不应被不同进程对象覆盖；注销后会话的 `ConnectionID` 应恢复为 0。

## 依赖与调用关系

直接依赖由 `pkg/sessionctx/sysproctrack/Cargo.toml` 给出：

- `anyhow` 提供 `TrackError` 的具体类型。
- `astersql-session-sessmgr` 以 crate 名 `sessmgr` 提供 `ProcessInfo`。
- `astersql-sessionctx-variable` 以 crate 名 `variable` 提供 `variable::session::SessionVars`。
- Rust 标准库提供 `HashMap`、`Arc`、`Mutex`。

模块入口 `lib.rs` 是公开出口，并在 `#[cfg(test)]` 下单独挂载 `migration_aster_unit_test.rs`，满足测试与生产源文件分离。工作区根 `Cargo.toml` 将该 crate 纳入 members，并定义 `facade_sessionctx_sysproctrack` 别名；`pkg/util/sqlexec/lib.rs` 还把依赖再导出为 `sysproctrack`，供 `restricted_sql_executor.rs::TrackSysProcFn` 接收 `TrackProcRef`。

RustCodeGraph 精确查询确认 `TrackProc` 位于第 48 行、`Tracker` 位于第 63 行；其文件级反向索引列出 `pkg/distsql/context/context_test.rs`、`pkg/distsql/context/migration_aster_unit_test.rs` 和 `pkg/executor/test/unstabletest/main_test.rs`，但这些是粗粒度依赖/测试边，源码搜索未显示它们调用本文件 trait。可核实的直接语义消费者是 `pkg/sessionctx/sysproctrack/migration_aster_unit_test.rs`，以及 `pkg/util/sqlexec/restricted_sql_executor.rs` 中使用 `TrackProcRef` 的回调类型。不要依据同名 `Track`/`Tracker` 的全仓搜索结果推断调用关系，因为内存跟踪器、游标跟踪器等存在大量同名符号。

## 错误处理与边界

`Track` 以 `Result<(), anyhow::Error>` 暴露失败；Go 生产实现的明确失败条件是同一 ID 已被不同 `TrackProc` 占用，错误文本为 `The ID is in use: <id>`，Rust 迁移测试也验证这一文本。trait 没有限制其他实现只返回这一种错误，也没有稳定错误码供调用者匹配，因此上游宜传播或记录错误，避免依赖字符串作控制流。

`UnTrack` 与 `KillSysProcess` 不返回结果，接口因而无法区分“不存在该 ID”和“操作成功”；Go 实现对未知 ID 均静默无操作。`GetSysProcessList` 通过过滤 `None` 和 ID 不一致快照处理竞态或已返还会话，而不是报错。

Rust 类型边界还有三项需要实现者显式处理：`Mutex<SessionVars>` 可能因 panic 中毒；trait 没有规定锁中毒策略；`ShowProcess` 可能在任何时刻返回 `None`；`anyhow::Error` 不保证可克隆或可按结构比较。当前迁移测试大量使用 `unwrap`，它证明正常路径和约定语义，不代表生产实现可以忽略锁中毒或恢复策略。

## 并发与资源生命周期

`TrackProc` 与 `Tracker` 都要求 `Send + Sync`，因此其对象允许在线程间移动和共享；但 trait 不自动保证复合操作原子性。实现方必须自行同步注册表，并使“检查 ID、设置会话状态、插入/删除映射”等步骤具有与 Go `sync.RWMutex` 临界区相当的一致性。

锁顺序是扩展时的主要风险。Go `SysProcesses` 先持有注册表锁，再访问进程会话变量；Rust 测试实现也先持有 `RwLock<HashMap<...>>`，再锁 `SessionVars`。新实现或 `TrackProc` 方法若反向持有会话锁再进入跟踪器，可能形成死锁，因此必须统一顺序并避免在注册表写锁内执行阻塞或可重入回调。

`Arc<dyn TrackProc>` 使注册表持有强引用，`UnTrack` 删除映射后对象仅在其他强引用也释放时销毁。`GetSysProcessList` 返回的 `Arc<ProcessInfo>` 与进程对象生命周期解耦，是调用时快照而非实时视图。trait 不创建后台任务、不拥有通道，也没有 RAII 注销守卫；调用者若漏掉 `UnTrack`，注册项和强引用会持续存在。Go 自动分析链通过执行选项成对传递 Track/UnTrack，说明成对清理是调用侧责任。

## 与 Go 版本的对应关系

为表达 Go 指针/接口语义，Rust 增加了显式同步和所有权包装：`*SessionVars` 变为 `&Mutex<SessionVars>`，可空 `*ProcessInfo` 变为 `Option<Arc<ProcessInfo>>`，接口值变为 `Arc<dyn TrackProc>`。Rust 还给两个 trait 加上 `Send + Sync`，把 Go 实现依靠锁达到的跨线程使用要求前移到类型边界。

当前 Rust `pkg/domain/domain.rs::SysProcesses` 是独立 API：它以 `SystemProcess`、自定义 `ProcessInfo` 和 `BTreeMap` 工作，拒绝所有重复 ID，`kill` 对不存在 ID 返回 `DomainError`，且没有实现本文件 `Tracker`。这些差异说明迁移接线尚未完成，不能声称 Rust 已完整替代 Go 系统进程主链。

## 扩展指南

- 新增真正的 Rust 生产跟踪器时，应优先为 `pkg/domain/domain.rs::SysProcesses` 或明确的适配器实现本文件 `Tracker`，统一两套 `ProcessInfo`/`SystemProcess` 模型，而不是再建立第三套注册表。
- 修改 `TrackProc` 方法前，要同步检查 `TrackProcRef`、`pkg/util/sqlexec/restricted_sql_executor.rs::TrackSysProcFn`、同 crate 迁移测试，以及 Go `track.go`/`domain.go` 的兼容语义。新增必需方法会破坏所有 trait 实现。
- 修改注册规则时，应保持 Go 的对象身份幂等语义、`ConnectionID` 生命周期、列表 ID 校验和未知 ID 行为；若有意偏离，必须在 API 文档与独立测试中写明兼容性影响。
- 需要稳定错误处理时，可引入结构化错误再由 `TrackError` 包装，但应保留现有调用者可传播任意错误的能力，并评估是否会破坏字符串兼容。
- 并发实现应记录锁顺序，避免在注册表锁内执行外部阻塞逻辑；高频 `GetSysProcessList` 场景还应评估快照分配、`Arc` 克隆和持读锁时间。
- 测试必须继续放在独立文件中。直接契约回归应扩展 `pkg/sessionctx/sysproctrack/migration_aster_unit_test.rs`；生产接线后，还应在具体实现同目录的独立测试中覆盖 SQLKiller 重置/发送、并发 Track/UnTrack、注销清理和端到端 process-list 可见性。
