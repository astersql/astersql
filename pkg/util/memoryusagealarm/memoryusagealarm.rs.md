# `pkg/util/memoryusagealarm/memoryusagealarm.rs`

## 文件定位

[`memoryusagealarm.rs`](./memoryusagealarm.rs) 是 `astersql-util-memoryusagealarm` crate 的业务实现文件；[`lib.rs`](./lib.rs) 通过 `#[path = "memoryusagealarm.rs"]` 装载该模块并重导出全部公开项。crate 边界和依赖由 [`Cargo.toml`](./Cargo.toml) 定义，未声明 feature：运行时依赖 `crossbeam-channel`、`backtrace`、`chrono`、`memory-stats`，并通过仓库内 crate 读取配置、磁盘、日志、内存与系统变量。

它承担周期性内存风险检测及诊断材料落盘，不执行内存回收、SQL 终止或全局内存仲裁。`Handle::Run` 是循环入口，`memoryUsageAlarm` 保存跨轮次状态。当前可确认的真实 Rust 生产接线是 [`br/pkg/utils/memory_monitor.rs`](../../../br/pkg/utils/memory_monitor.rs) 的 `spawn_memory_alarm`：构造 `Handle`、在独立线程执行 `Run`，并在 BR context 取消时发送退出信号。`cmd/tidb-server/main.rs` 虽有 `MemoryUsageAlarmHandle().SetSessionManager(...).Run()` 启动形状，但当前返回的是 `cmd/tidb-server/stubs.rs` 中的占位 `Handle`，不是本文件的实现；`pkg/domain/Cargo.toml` 声明了本 crate 依赖，但源码搜索未发现实际引用，因此不能据此宣称 TiDB server 已完成真实 Rust 接线。

## 核心职责

本文件的职责可以分为五层：

1. `ConfigProvider` 与 `TiDBConfigProvider` 把告警比例、保留数量、日志目录和组件名从具体运行环境中抽象出来，使 TiDB 与 BR 可共享告警器。
2. `Handle::Run` 每 100 ms 唤醒一次，响应退出通道，并把当前可选的 `SessionManager` 快照交给告警状态机。
3. `memoryUsageAlarm::{updateVariable, initMemoryUsageAlarmRecord, alarm4ExcessiveMemUsage, needRecord}` 完成配置刷新、记录目录发现、内存采样和触发判定。
4. `doRecord`、`recordSQL`、`printTop10SqlInfo`、`recordProfile` 把告警日志、运行中 SQL、堆信息和线程回溯写入一次性的 `record<timestamp>` 目录。
5. `tryRemoveRedundantRecords` 按配置保留最新记录，删除队首的旧目录。

关键不变量是：只有用量严格高于 `serverMemoryLimit * memoryUsageAlarmRatio` 才可能记录；正常情况下两次记录至少间隔 60 秒，但窗口内若相对上次记录增长量严格大于内存上限的 10%，仍立即记录。全局内存仲裁开启时，`alarm4ExcessiveMemUsage` 整体跳过，以避免两条治理路径重复工作。

## 主要符号

### 配置和输入边界

- `ConfigProvider: Send + Sync`：四个 getter 构成环境适配接口。`TiDBConfigProvider` 从 `task_vardef::{MemoryUsageAlarmRatio, MemoryUsageAlarmKeepRecordNum}`、`task_config::get_global_config()` 取值，组件名固定为 `tidb-server`。
- `SessionManager: Send + Sync`：只暴露 `ShowProcessList() -> Vec<Arc<ProcessInfo>>`，避免本 crate 反向依赖完整 session manager。
- `ProcessInfo`：告警所需的会话快照，包括开始时间、SQL 文本、峰值内存、事务时间戳、别名、影响行数、OOM 会话变量、简化执行计划行和可选的预生成日志字段。
- `OOMAlarmVariablesInfo`、`ProcessLogField`、`ProcessLogValue`：定义 running SQL 文本中的附加变量和受支持字段类型。字段值只支持字符串、无符号整数、有符号整数和布尔值。

### 驱动与状态机

- `NewMemoryUsageAlarmHandle(exitCh, provider) -> Box<Handle>`：创建尚未绑定 session manager 的运行句柄。
- `Handle::{SetSessionManager, Run}`：用 `RwLock<Option<Arc<dyn SessionManager>>>` 安装/读取会话管理器；`Run` 维护单个 `memoryUsageAlarm`，每 100 ms 检测一次，通道收到值或断开时退出。
- `memoryUsageAlarm`：保存最近记录时间、最近变量刷新时间、最后错误、记录根目录、已知记录目录、上次记录用量、比例、保留数、有效内存上限及初始化标志。
- `updateVariable`：最多每 60 秒刷新一次。优先采用非零 `task_memory::tracker::ServerMemoryLimit`；否则调用可替换的 `MemTotal` probe，使用系统总内存。
- `initMemoryUsageAlarmRecord`：刷新变量，建立 `<log-dir>/oom_record`，按文件名排序扫描名称包含 `record` 的目录项，并将其加入保留队列。
- `alarm4ExcessiveMemUsage`：编排单次检测；`needRecord` 返回 `(bool, AlarmReason)`，原因有 `ExceedAlarmRatio`、`GrowTooFast`、`NoReason`。

### 诊断输出

- `doRecord`：写告警日志、创建记录目录、可选写 `running_sql`，随后写 profile；任一步骤失败时把字符串错误存入 `self.err` 并停止本次后续动作。
- `getTop10SqlInfoByMemoryUsage`：按 `max_consumed` 降序；`getTop10SqlInfoByCostTime`：按开始时间升序，即越早开始越靠前。两者都委托给 `getTop10SqlInfo`，最多输出十条。
- `recordSQL`：过滤 SQL 文本为空的进程，创建 `running_sql`，先输出内存 Top 10，再输出耗时 Top 10。
- `getPlanString`：把已准备好的 `brief_binary_plan_rows` 格式化为五列表格；它不在此 crate 内解码二进制计划。
- `recordProfile`、`write`、`recordGoroutineProfile`：分别编排 profile、写 `heap` 文件、把当前 Rust 线程的 `Backtrace` 写入兼容文件名 `goroutine`。
- `elapsed_since`、`duration_between`：遇到系统时钟回拨时返回零时长；`write_best_effort` 对 running SQL 的单次写失败只记录日志；`log_error` 统一写后台错误日志。

## 执行流程

1. 上游创建退出通道和 `Arc<dyn ConfigProvider>`，调用 `NewMemoryUsageAlarmHandle`；若能提供会话快照，再调用 `SetSessionManager`。
2. `Handle::Run` 创建一个未初始化的 `memoryUsageAlarm`。循环对退出接收端执行 100 ms 的 `recv_timeout`：收到值或发送端断开即返回；超时则短暂持有读锁、克隆 session manager 的 `Arc`，随后释放锁并调用 `alarm4ExcessiveMemUsage`。
3. 首轮检测调用 `initMemoryUsageAlarmRecord`；后续轮次调用 `updateVariable`。若全局内存仲裁开启、初始化失败、或比例不在开区间 `(0, 1)`，本轮直接结束。
4. `ReadMemStats` 总会提供实例 heap 统计。显式设置 `ServerMemoryLimit` 时，以 `heap_alloc` 作为受测用量；否则以 `MemUsed` 系统 probe 的结果作为受测用量，并以 `MemTotal` 作为上限。
5. `needRecord` 先检查阈值。超过阈值后，距上次记录超过 60 秒返回 `ExceedAlarmRatio`；否则把两次 `u64` 用量按 Go 语义转成 `i64` 后 wrapping 相减，增长严格超过上限的 10% 时返回 `GrowTooFast`。
6. 触发后先更新 `lastCheckTime` 和 `lastRecordMemUsed`，再调用 `doRecord`。目录名由本地时区 RFC 3339 秒级时间拼成 `record<timestamp>`。
7. `doRecord` 根据是否设置 server limit 选择告警字段。记录目录建立成功后，存在 session manager 才生成 `running_sql`；无论是否有 session manager，随后尝试生成 `heap` 与 `goroutine`。
8. 返回后无条件调用 `tryRemoveRedundantRecords`，只要已知目录数大于保留数就从队首移除并递归删除。删除失败会记日志，但目录名已经从内存队列移除。

## 数据与状态

`memoryUsageAlarm` 由 `Handle::Run` 所在线程独占并以 `&mut self` 更新，不依赖内部互斥锁。`lastCheckTime` 同时决定 60 秒冷却窗口、SQL 耗时计算和记录目录时间戳；`lastRecordMemUsed` 是增长过快判定基线。两者在真正开始记录前更新，因此后续文件写入失败也会占用冷却窗口。

`lastUpdateVariableTime` 只在成功确定内存上限后更新；若 `MemTotal` 读取失败，本轮留下 `err` 并允许下一次检测再次尝试。初始化过程随后执行 `CheckAndCreateDir`，成功会把 `err` 清空，这一点与当前 Go 赋值行为一致，并由 Rust 回归测试专门覆盖。

`lastRecordDirName` 的初始内容来自按路径排序的目录扫描，之后按创建顺序追加。筛选条件是文件名“包含”`record`，不要求它是目录，也不要求前缀严格匹配；删除时使用 `remove_dir_all`。`memoryUsageAlarmKeepRecordNum` 未在本文件中校验为非负数：负值会使循环持续到队列为空。

`ProcessInfo` 是快照而非 live session 引用。Top 10 排序会原地重排传入的 `Arc` slice；`printTop10SqlInfo` 对同一向量先按内存排序、再按时间排序，输出内容不依赖原始顺序。若 `log_fields` 非空则完全使用调用者提供的基础字段；否则由 `default_process_fields` 生成 cost、txn、mem、SQL、别名和影响行数，之后统一追加 OOM action、当前全局 server limit、会话变量和计划。

## 依赖与调用关系

上游关系：

- `br/pkg/utils/memory_monitor.rs::spawn_memory_alarm` → `NewMemoryUsageAlarmHandle` → `Handle::Run`，这是源码中可确认的真实生产调用边。BR 的 `BRConfigProvider` 将组件名设为 `br`，context 取消后发送退出信号并 join 告警线程。
- `pkg/util/memoryusagealarm/lib.rs` → `memoryusagealarm` 模块，并重导出本文件公开 API；同一入口以 `#[cfg(test)]` 装入两个独立 Rust 测试模块。
- `pkg/domain/Cargo.toml` 和根 `Cargo.toml` 声明 crate 依赖/门面，但声明本身不是运行时调用证据。`cmd/tidb-server` 当前同名链路属于 stub，安全扩展时必须先确认真实 domain 接线状态。

文件内主调用链为：

`Handle::Run` → `memoryUsageAlarm::alarm4ExcessiveMemUsage` → `initMemoryUsageAlarmRecord`/`updateVariable` → `needRecord` → `doRecord` → `recordSQL`/`recordProfile`，完成后由 `alarm4ExcessiveMemUsage` 调用 `tryRemoveRedundantRecords`。`recordSQL` → `printTop10SqlInfo` → 两个排序入口 → `getTop10SqlInfo` → `getPlanString`/`append_process_field`；`recordProfile` → `write` + `recordGoroutineProfile`。

主要下游依赖：

- `task-memory`：全局仲裁开关、server limit、heap 统计、系统总量/已用量 probe、字节格式化。
- `task-vardef`：告警比例、保留数、OOM action。
- `task-config`：TiDB 日志文件路径；`task-disk`：建立目录；`task-logutil`：后台结构化日志。
- `crossbeam-channel`：退出/定时等待；`chrono`：本地时间目录名；`memory-stats` 与 `backtrace`：Rust 原生诊断输出。

## 错误处理与边界

- 比例 `<= 0` 或 `>= 1` 被视为禁用；恰好等于阈值不记录，恰好增长 10% 也不走快速增长分支。
- `SystemTime` 回拨不会 panic：时间差按零处理。这可能延长冷却窗口，并把未来开始时间的 SQL cost 显示为零。
- `RwLock` poison 在 `SetSessionManager`、`Run` 和内存 probe 锁读取处通过 `expect` 触发 panic；这不是可恢复错误。
- 初始化和 profile/文件创建错误保存在 `memoryUsageAlarm.err`，但 API 不把错误返回给 `Handle::Run`。`MemTotal`/`MemUsed` 错误同时写日志；目录和 profile 的部分错误只保存字符串。
- `printTop10SqlInfo` 的内容写入采用 best effort：单段写失败只记日志，因此 `recordSQL` 仍可能返回 `Ok(())`。相反，创建 `running_sql` 文件会直接返回 `io::Error`。
- `doRecord` 在记录目录创建后立即加入保留队列；若 SQL 或 profile 写入失败，会留下部分目录。随后保留策略仍运行，可能删除旧目录。
- `tryRemoveRedundantRecords` 无论磁盘删除是否成功都会先从队列移除路径，失败目录不会在当前进程内再次重试；下一次进程初始化扫描时才可能重新发现。
- `write` 对非 `heap` 名称只创建空文件；当前生产调用只传 `heap`。`memory_stats()` 失败时回退到 `ForceReadMemStats`，输出是文本快照而非 Go `runtime/pprof` 的 heap profile。
- `recordGoroutineProfile` 只捕获执行告警的当前 Rust 线程回溯，不是 Go 版本的全 goroutine 栈；文件名兼容不等于内容语义完全等价。

## 并发与资源生命周期

`Handle` 可跨线程使用的基础来自 `Receiver<()>`、`RwLock<Option<Arc<dyn SessionManager>>>` 以及两个 trait 的 `Send + Sync` 约束。配置提供者由 `Arc` 共享；状态机只在 `Run` 线程中存在。`SetSessionManager` 需要 `&mut self` 并写锁替换 manager，`Run` 每轮只在克隆 `Arc` 时持有读锁，实际取进程列表和磁盘 I/O 均不持锁。

退出生命周期由通道控制：收到一个 `()` 或所有发送端断开都会终止；空闲时每 100 ms 唤醒。BR 上游额外负责 join 内层告警线程。本文件不创建线程、不负责 join，也不为多次并发调用同一个 `Handle::Run` 提供显式保护。

每次记录依次创建目录和普通文件。Rust `File` 通过 RAII 在函数结束时关闭；无显式 flush/fsync，因此成功表示写调用完成，不表示数据已经持久化到稳定介质。旧目录删除发生在本轮记录动作之后。由于时间戳只精确到秒，极短时间内的强制记录可能复用同一目录名并向队列重复加入同一路径；通常 10% 增长条件限制了频率，但代码未显式保证目录名唯一。

全局变量和 probe（`ServerMemoryLimit`、`OOMAction`、`MemTotal`、`MemUsed`）可能由其他线程修改。配置刷新以 60 秒为缓存窗口，但 Top SQL 输出中的 OOM action 与 server limit 在每次格式化时重新读取，因此它们可能与触发判定时的缓存值不同。

## 与 Go 版本的对应关系

直接对照文件是 [`memoryusagealarm.go`](./memoryusagealarm.go)，行为测试对照是 [`memoryusagealarm_test.go`](./memoryusagealarm_test.go)。Rust 保留了 Go 的核心结构和顺序：100 ms ticker/timeout、60 秒变量刷新、server limit 优先、阈值与 10% 增长判定、告警字段、`oom_record/record<RFC3339>` 目录、Top 10 SQL、旧记录清理以及 heap/goroutine 两类文件。

明确的适配差异如下：

- Go `Handle` 用 `atomic.Pointer[sessmgr.Manager]`；Rust 用 `RwLock<Option<Arc<dyn SessionManager>>>`，且允许 manager 未设置，此时仍记录 profile、跳过 SQL。
- Go 直接持有完整 `sessmgr.ProcessInfo` 并调用 `util.GenLogFields`、解码 `BriefBinaryPlan`；Rust 定义轻量 `ProcessInfo` 快照，使用预生成字段或本地默认字段，计划行要求上游预先解码。
- Go 用 `runtime/pprof` 写真正的 heap profile，并用 64 MiB buffer 捕获所有 goroutine 栈；Rust heap 文件是 `memory-stats`/`ForceReadMemStats` 文本，`goroutine` 文件是当前线程 `Backtrace`。这两项是平台实现差异，不应被表述为字节格式兼容。
- Go `os.ReadDir` 按文件名排序；Rust 显式 `sort()` 路径后复现该顺序。Rust 的用量差值用 `wrapping_sub` 复现 Go 两个 `uint64` 先转 `int64` 再相减的溢出语义。
- Go 的 `recordSQL` 显式关闭文件并记录 close 错误；Rust 依赖 RAII，无法在当前返回值中报告 close 错误。
- Go server 的真实 domain/session manager 接线存在于 Go 主链；当前 Rust `cmd/tidb-server` 的同形调用仍是 stub，已确认的本 crate 真实消费者是 BR。

独立 Rust 测试并非内嵌在源文件：[`memoryusagealarm_test.rs`](./memoryusagealarm_test.rs) 覆盖阈值/冷却/增长、Top 10 完整输出、60 秒刷新、`MemTotal` 错误后的 Go 对齐初始化行为和线程回溯；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 额外覆盖转换溢出语义、日志目录、目录排序与保留、最多十条输出、heap/线程文件和创建错误。Go 测试还证明预期基线来自相同的阈值、排序、刷新窗口和 goroutine 输出场景。

## 扩展指南

- 新增配置项：先扩展 `ConfigProvider` 及 `TiDBConfigProvider`，同步所有实现者（至少 BR 的 `BRConfigProvider` 和两个 Rust 测试 mock），再决定是否应受 `updateVariable` 的 60 秒缓存约束。
- 修改触发策略：集中修改 `needRecord`，保持“阈值、冷却、快速增长”的优先级，并在 `memoryusagealarm_test.rs` 与 `migration_aster_unit_test.rs` 增加边界用例；若目标是 Go 对齐，还应同步或解释 `memoryusagealarm_test.go` 的差异。
- 新增诊断文件：接入 `doRecord`/`recordProfile`，明确失败是否应阻断后续文件，并为部分失败、保留清理和资源关闭增加独立测试。不要把测试写回生产 `.rs` 文件。
- 扩充 SQL 字段：优先修改 `ProcessInfo` 快照适配层、`default_process_fields` 或 `getTop10SqlInfo`；同时确认预生成 `log_fields` 路径是否也需要该字段，避免两条输出路径漂移。
- 接入 TiDB server：不能只依赖 `pkg/domain/Cargo.toml` 的依赖声明；需要用真实 domain handle 构造本文件的 `Handle`、实现 `SessionManager` 快照转换、管理退出通道和线程生命周期，并替换 `cmd/tidb-server/stubs.rs` 的占位链。该工作超出本文档任务范围。
- 改进 profile 兼容性：`write` 和 `recordGoroutineProfile` 是明确入口，但替换实现时要评估分配峰值、暂停时间、磁盘体积和敏感 SQL/栈信息暴露风险；不能假定 Rust 可直接生成 Go pprof 格式。
- 调整目录命名或保留策略：同时检查 `initMemoryUsageAlarmRecord` 的发现/排序规则、`doRecord` 的追加顺序和 `tryRemoveRedundantRecords` 的失败语义，并覆盖同秒重复记录、负保留数及非目录名称包含 `record` 的情况。

## 验证依据

本说明以以下直接证据为准：

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录中的 6 个 Go/Rust 文件均被索引；`files --filter pkg/util/memoryusagealarm` 确认文件集合。
- RustCodeGraph `node --file pkg/util/memoryusagealarm/memoryusagealarm.rs --offset 1 --limit 500` 及 `--offset 499 --limit 400`：读取完整 757 行实现；文件级结果报告该文件被 16 个文件使用。
- RustCodeGraph `query`：确认 Rust 与 Go 两侧的 `NewMemoryUsageAlarmHandle`、`alarm4ExcessiveMemUsage`、`recordSQL`、`recordProfile` 同名符号。精确 `callers/callees` 查询在本次会话 30 秒窗口内未返回结果，因此具体跨文件调用边由下述局部源码搜索补证，不将超时视作不存在调用。
- crate/入口：`pkg/util/memoryusagealarm/{Cargo.toml,lib.rs}`；反向声明：根 `Cargo.toml`、`pkg/domain/Cargo.toml`、`br/pkg/utils/Cargo.toml`。
- 真实调用与占位边界：`br/pkg/utils/memory_monitor.rs::{StartMemoryMonitor,spawn_memory_alarm}`、`cmd/tidb-server/main.rs` 的启动片段、`cmd/tidb-server/stubs.rs::{Domain::MemoryUsageAlarmHandle,Handle}`。
- Go 对照与独立测试：`pkg/util/memoryusagealarm/{memoryusagealarm.go,memoryusagealarm_test.go,memoryusagealarm_test.rs,migration_aster_unit_test.rs}`。
- 人工复核结论：该文件存在是为了在接近 OOM 时保留可诊断现场；运行方式是定时采样、阈值/增长判定、顺序落盘和保留清理；安全扩展必须维持配置缓存、错误后的部分状态、独立测试和 Go 语义边界。

本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付检查仅验证文档结构、路径引用、源码事实和工作区差异。
