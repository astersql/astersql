# `pkg/statistics/handle/util/auto_analyze_proc_id_generator.rs`

## 文件定位

本文件属于 Cargo crate `astersql-statistics-handle-util`；crate 入口 `pkg/statistics/handle/util/lib.rs` 公开声明 `auto_analyze_proc_id_generator` 模块并通过 `pub use` 重导出其 API。它位于统计信息 handle 的工具层，负责把自动 ANALYZE 使用的“伪进程 ID”分配接口、进程内活动 ID 集合和进程跟踪回调抽象放在一个可复用边界中。

当前 Rust 主链中，`pkg/statistics/handle/autoanalyze/exec/exec.rs` 通过 Cargo 依赖 `astersql-statistics-handle-util` 使用 `AutoAnalyzeProcIdGenerator`、`TrackProc` 和 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST`；`pkg/util/expensivequery/expensivequery.rs` 读取全局集合，以便对自动分析进程应用最长运行时间限制。代码搜索未发现 Rust 生产代码调用 `new_generator` 或 `new_auto_analyze_tracker`，因此它们目前是已公开的兼容/注入 API，而不是现有 Rust 自动分析执行路径的装配入口。

## 核心职责

- `AutoAnalyzeProcIdGenerator` 将“申请 ID”和“释放 ID”定义为可跨线程共享的 trait，避免工具层规定 ID 的具体来源或复用策略。
- `Generator` / `new_generator` 把调用方提供的两个闭包包装为 trait 对象，支持依赖注入。
- `GlobalAutoAnalyzeProcessList` 与 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST` 保存本进程内当前被视为自动分析任务的 ID，供执行、窗口检查和昂贵查询检查共同查询。
- `TrackProc` 携带数据库、表和 SQL 语句上下文；当前 Rust 自动分析执行路径实际只填充 `statement`，数据库与表为空字符串（`RunAnalyzeStmt`）。
- `AutoAnalyzeTracker` / `new_auto_analyze_tracker` 规定“先更新全局集合、再调用外部 processlist 回调”的顺序，与 Go 实现保持一致。

该文件不生成 ID 数值、不注册真实会话、不执行 ANALYZE，也不负责在回调失败时自动回滚；这些行为由调用方及下游回调承担。

## 主要符号

- `pub trait AutoAnalyzeProcIdGenerator: Send + Sync`：公开分配协议。`auto_analyze_proc_id(&self) -> u64` 返回一个 ID；`release_auto_analyze_proc_id(&self, id)` 将该 ID 交还具体实现。trait 本身没有唯一性、非零值或复用时机校验。
- `pub struct Generator`：保存 `getter: Arc<dyn Fn() -> u64 + Send + Sync>` 与 `release: Arc<dyn Fn(u64) + Send + Sync>`。字段私有，调用只能经过 trait 方法。
- `Generator::new(...) -> Self`：接收 `'static`、`Send + Sync` 的分配和释放闭包。
- `new_generator(...) -> Arc<dyn AutoAnalyzeProcIdGenerator>`：构造 `Generator` 并擦除为共享 trait 对象；相较 `Generator::new`，这是更适合跨模块注入的返回类型。
- `pub struct TrackProc { database, table, statement }`：可克隆、可比较且具有空字符串默认值的跟踪上下文。
- `pub struct GlobalAutoAnalyzeProcessList`：内部为 `RwLock<HashSet<u64>>`；`track` 插入、`untrack` 删除、`all` 拷贝快照、`contains` 查询成员关系。
- `pub static GLOBAL_AUTO_ANALYZE_PROCESS_LIST`：通过 `LazyLock` 首次访问时建立的进程级单例集合。
- `type TrackCallback` / `type UntrackCallback`：私有动态回调类型。跟踪回调返回 `Result<(), StatsError>`，注销回调无返回值。
- `pub struct AutoAnalyzeTracker`：持有两个 `Arc` 回调；`track` 和 `untrack` 将全局集合更新与外部回调按固定顺序组合。
- `new_auto_analyze_tracker(...) -> AutoAnalyzeTracker`：`AutoAnalyzeTracker::new` 的便捷公开构造函数。

本文件没有条件编译项、异步函数或后台任务。

## 执行流程

ID 分配流程是：调用方构造或取得 `dyn AutoAnalyzeProcIdGenerator`，调用 `auto_analyze_proc_id`；`Generator` 仅转发到 `getter`。任务退出时调用 `release_auto_analyze_proc_id`，`Generator` 再原样把 ID 传给 `release`。文件本身不把“分配”和“释放”与全局集合绑定。

`AutoAnalyzeTracker::track(id, context)` 的顺序是：

1. 对 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST` 取写锁并把 `id` 插入集合。
2. 释放集合写锁。
3. 调用 `track_callback(id, context)`，把其 `StatsError` 原样返回。

若第 3 步失败，第 1 步不会自动撤销；源码注释明确要求调用方显式 `untrack`。现有 Rust 主链没有调用这个组合方法，而是在 `RunAnalyzeStmt` 中直接 `track` 全局集合、先安装 `ProcIdGuard`，再调用 `SysProcTracker::Track`；该 guard 在成功、执行错误和跟踪错误路径都会注销并释放 ID。

`AutoAnalyzeTracker::untrack(id)` 先从全局集合删除 ID，然后无条件调用 `untrack_callback(id)`。`KillAutoAnalyzeOutsideWindow` 会遍历 `all()` 的快照，杀掉窗口外任务并直接从集合移除；昂贵查询检查则通过 `contains(process.id)` 判定是否应用自动分析超时。

## 数据与状态

唯一的模块级可变状态是 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST` 中的 `HashSet<u64>`。集合语义带来以下不变量和边界：同一 ID 重复 `track` 仍只有一个元素；不存在的 ID 执行 `untrack` 是无操作；`all` 返回独立 `Vec<u64>` 快照，返回顺序不稳定，快照生成后可立即与真实集合发生偏离。

`TrackProc` 拥有三个 `String`，调用跟踪回调时按值移动。`Generator` 与 `AutoAnalyzeTracker` 使用 `Arc` 共享闭包所有权；它们没有实现额外的计数、缓存或 drop 清理。释放闭包、注销闭包是否幂等，以及一个 ID 是否只能释放一次，均由调用方保证。

全局集合仅在当前进程内有效，不是集群一致状态，也不持久化。进程退出后状态自然消失；多节点上的自动分析任务分别维护各自集合。

## 依赖与调用关系

直接标准库依赖为 `HashSet`、`Arc`、`LazyLock` 和 `RwLock`。唯一 crate 内类型依赖是 `crate::util::StatsError`，用于跟踪回调失败。`pkg/statistics/handle/util/Cargo.toml` 将本文件编入 `astersql-statistics-handle-util`；本文件自身没有使用该 manifest 中的外部 crate。

已验证的 Rust 上游关系如下：

- `pkg/statistics/handle/autoanalyze/exec/exec.rs` 的 `StatsHandleOps` 继承 `AutoAnalyzeProcIdGenerator`，并由 `RunAnalyzeStmt` 申请 ID、写入全局集合、向 `SysProcTracker` 登记；`ProcIdGuard::drop` 注销并释放 ID。
- 同文件 `KillAutoAnalyzeOutsideWindow` 通过 `GLOBAL_AUTO_ANALYZE_PROCESS_LIST.all()` 获取快照，逐个 kill 并删除。
- `pkg/util/expensivequery/expensivequery.rs` 的 `inspect_query` 用 `contains` 识别自动分析进程，超过 `MAX_AUTO_ANALYZE_TIME` 时请求 session manager 杀查询。
- `pkg/statistics/handle/types/interfaces.rs` 将本 trait 以 Go 风格名称 `AutoAnalyzeProcIDGenerator` 重导入，并作为统计 handle 接口的组成部分。

代码搜索仅发现 `new_generator`、`new_auto_analyze_tracker` 的定义，没有找到 Rust 调用点；`AutoAnalyzeTracker::track/untrack` 也没有当前生产调用点。新增装配前不能假定这些包装器已经参与主链。

## 错误处理与边界

ID getter、release 回调和 untrack 回调的签名都不返回错误；若闭包 panic，文件不捕获 panic。`AutoAnalyzeTracker::track` 只传播外部跟踪回调产生的 `StatsError`，不添加上下文，也不会在错误时回滚已插入的 ID。调用方必须确保失败路径执行 `untrack`，否则 `contains` 可能把残留 ID 误判为仍在运行的自动分析。

所有 `RwLock` 加锁都调用 `unwrap()`。若持锁线程 panic 导致锁中毒，后续 `track`、`untrack`、`all` 或 `contains` 会继续 panic，而不是返回可恢复错误。`HashSet` 不验证 ID 是否属于本模块，也不阻止已释放 ID 被保留或重新插入。

跟踪和注销分别更新集合后才调用外部回调，因此集合与外部 processlist 之间不存在原子事务：回调执行期间以及回调失败后，两侧可能短暂或持续不一致。这一顺序是与 Go 版本一致的明确兼容行为，修改时需评估所有错误清理路径。

## 并发与资源生命周期

`AutoAnalyzeProcIdGenerator: Send + Sync` 以及所有闭包的 `Send + Sync + 'static` 约束允许生成器和跟踪器被多个线程共享。`RwLock<HashSet<u64>>` 允许并发只读查询，写操作互斥；锁只覆盖集合操作，外部回调在锁释放后执行，避免在未知回调中持有全局锁和由此引起的重入死锁。

`LazyLock` 保证全局集合只初始化一次。`all` 在读锁内完成复制，随后释放锁；遍历期间新加入或移除的 ID 不会改变该快照。`Arc` 只管理闭包对象的引用计数，不管理进程 ID 生命周期；`Generator` 和 `AutoAnalyzeTracker` 被 drop 时不会自动释放或注销任何 ID。

现有 `RunAnalyzeStmt` 用 `ProcIdGuard` 把已分配 ID 的释放绑定到栈作用域，这是当前 Rust 主链的实际资源清理保证；该保证来自调用方文件，而非本文件的类型系统。

## 与 Go 版本的对应关系

同路径 Go 文件 `pkg/statistics/handle/util/auto_analyze_proc_id_generator.go` 是直接语义基准：Go 的 `AutoAnalyzeProcIDGenerator` / `generator` / `NewGenerator` 分别对应 Rust trait、`Generator` 和 `new_generator`；Go 的 `globalAutoAnalyzeProcessList`、全局变量、`Tracker`、`Untracker`、`All`、`Contains` 对应 Rust 的集合类型、单例及四个方法；Go 的 `AutoAnalyzeTracker` 和 `NewAutoAnalyzeTracker` 对应 Rust 同名类型与 snake_case 构造函数。

主要保持项是：集合采用读写锁保护；重复 ID 按集合去重；跟踪时先加入全局集合再调用外部回调；注销时先从集合删除再调用外部回调；跟踪回调错误不会在工具层自动回滚。

可见差异是：Go 的跟踪上下文使用 `pkg/sessionctx/sysproctrack.TrackProc`，Rust 在本文件定义了拥有 `database/table/statement` 字符串的 `TrackProc`；Go 全局值由显式构造函数创建，Rust 用 `LazyLock` 延迟创建；Go 返回指针/接口，Rust 用 `Arc<dyn ...>` 或按值返回跟踪器；Rust 的锁中毒会因 `unwrap` panic，而 Go 的 `sync.RWMutex` 没有锁中毒概念。Go 的 `pkg/statistics/handle/handle.go` 已调用 `util.NewGenerator` 完成真实 handle 装配，当前 Rust 搜索未找到等价的 `new_generator` 装配点。

## 扩展指南

若新增 ID 分配策略，优先实现 `AutoAnalyzeProcIdGenerator`；只有策略天然由闭包表达时才使用 `new_generator`。需要保证并发唯一性、与释放端配对及复用安全，不能依赖本文件替调用方校验。若把 `new_generator` 接入正式 Rust handle，应同时检查 `StatsHandleOps`、所有成功/失败退出路径和昂贵查询中的 ID 可见周期。

若扩展跟踪上下文，需同步修改 `TrackProc`、`RunAnalyzeStmt` 的构造点、下游 `SysProcTracker` 适配以及 Go `sysproctrack.TrackProc` 对照；新增字段应明确所有权和是否允许为空。若改变跟踪/回调顺序或增加失败回滚，必须同时验证 Go 兼容语义以及 `ProcIdGuard`，避免双重注销、泄漏或窗口检查误杀。

测试应保持与源文件分离。可在同目录新增独立 `auto_analyze_proc_id_generator_test.rs` 并由 `lib.rs` 的 `#[cfg(test)]` 模块接入，覆盖闭包转发、重复插入、删除不存在 ID、并发访问、无序快照比较以及 track 回调失败后的显式清理；跨模块行为继续在 `pkg/statistics/handle/autoanalyze/exec/exec_test.rs` 验证。不要把测试内嵌进本生产源文件。

兼容性风险集中在公开 trait/字段与回调顺序；正确性风险集中在失败清理、ID 重用和双重释放；性能风险主要是所有自动分析任务共享一个 `RwLock<HashSet<_>>`，以及 `all` 每次复制全部 ID。除非有实际争用证据，不应为了优化改变简单且与 Go 对齐的同步模型。

## 验证依据

- RustCodeGraph `status`：本仓库索引可用，包含目标 Rust 文件；`files --filter pkg/statistics/handle/util` 显示同路径 Rust、Go、Cargo 和相邻测试文件。
- RustCodeGraph `node --file pkg/statistics/handle/util/auto_analyze_proc_id_generator.rs --offset 1 --limit 260`：读取了目标文件完整 154 行，核对 20 个符号及实现顺序。
- RustCodeGraph `query`：分别定位 `AutoAnalyzeTracker`、`new_auto_analyze_tracker`、`new_generator`、`AutoAnalyzeProcIdGenerator`，并确认 trait 在 `exec.rs` 与 `types/interfaces.rs` 的导入。精确 `callers` 查询在 30 秒内未返回，因此调用点改用下述源码搜索核验，未把不完整图结果当作事实。
- 源码与配置：`pkg/statistics/handle/util/Cargo.toml`、`pkg/statistics/handle/util/lib.rs`、`pkg/statistics/handle/util/util.rs`（`StatsError`）、`pkg/statistics/handle/autoanalyze/exec/Cargo.toml`、`pkg/statistics/handle/autoanalyze/exec/exec.rs`、`pkg/statistics/handle/types/interfaces.rs`、`pkg/util/expensivequery/expensivequery.rs`。
- Go 对照：`pkg/statistics/handle/util/auto_analyze_proc_id_generator.go` 与 `pkg/statistics/handle/handle.go`。
- 相关独立测试：同目录没有该文件的专用 Rust 测试；`pkg/statistics/handle/autoanalyze/exec/exec_test.rs::test_run_analyze_stmt_releases_process_id_when_tracking_fails` 验证跟踪失败时当前主链仍会释放、注销并清除全局集合。代码搜索未发现直接覆盖 Go 工具文件的 `*_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅运行任务指定的十一章节结构检查，并人工复核“定位、运行、扩展”均由上述路径或符号支持。
