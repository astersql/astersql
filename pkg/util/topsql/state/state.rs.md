# `pkg/util/topsql/state/state.rs`

## 文件定位

该文件是 `astersql-util-topsql-state` crate 的状态实现，crate 入口 `pkg/util/topsql/state/lib.rs` 通过 `#[path = "state.rs"] mod state` 加载它并 `pub use state::*`。`pkg/util/topsql/state/Cargo.toml` 将库入口指定为 `lib.rs`，仅直接依赖 `log` 与 `thiserror`，并用 `package.metadata.porting.go-package = "pkg/util/topsql/state"` 标明 Go 对照目录。根 workspace 和 TopSQL 门面、reporter、collector、stmtstats 等 crate 都以路径依赖接入该 crate；`pkg/util/topsql/lib.rs` 又将它公开为 `topsqlstate`。

它不采集、聚合或上报数据，而是保存进程级 TopSQL/TopRU 开关和容量、时间参数，供这些执行组件快速判断是否工作。TopSQL 是按 SQL/计划摘要进行画像的路径；TopRU 是按 Resource Unit 汇总的路径。源码没有条件编译项，测试模块由 `lib.rs` 在 `cfg(test)` 下独立挂载，符合生产逻辑与测试分文件的布局。

## 核心职责

1. 定义与 Go 版本一致的默认值：TopSQL 默认关闭，精度为 1 秒，最多 100 条时间序列、5000 条元数据，上报周期为 60 秒；TopRU item interval 默认为 60 秒（`DefTiDBTopSQL*`、`DefTiDBTopRUItemIntervalSeconds`）。
2. 以 `GlobalState: State` 集中保存可跨线程访问的原子状态。TopSQL 使用布尔开关；TopRU 使用消费者引用计数，因此多个订阅者不会因其中一个退出而被整体关闭。
3. 提供 TopSQL、TopRU 和组合 profiling 的启停/查询 API（`EnableTopSQL`、`DisableTopSQL`、`TopSQLEnabled`、`EnableTopRU`、`DisableTopRU`、`TopRUEnabled`、`TopProfilingEnabled`）。
4. 校验、设置、读取和复位 TopRU 聚合窗口（`SetTopRUItemInterval`、`GetTopRUItemInterval`、`ResetTopRUItemInterval`），并保证非法输入不污染当前合法值。
5. 暴露 `PrecisionSeconds`、`MaxStatementCount`、`MaxCollect` 和 `TopRUItemIntervalSeconds` 原子字段，供 reporter/datamodel 等热路径直接读取；私有 `enable` 和 `ruConsumerCount` 则只能通过 API 维护不变量。

## 主要符号

- `State`：进程级配置容器。`enable: AtomicBool` 与 `ruConsumerCount: AtomicI64` 为私有字段；`PrecisionSeconds`、`MaxStatementCount`、`MaxCollect`、`TopRUItemIntervalSeconds` 是公开的 `AtomicI64`。该类型没有 `Clone`，全局使用点均通过静态单例访问。
- `GlobalState: State`：唯一的公开静态实例，按各 `DefTiDB*` 常量初始化。公开字段允许外层配置/测试用 `load`、`store` 更新；开关和 RU 计数不公开，避免绕开配对语义。
- `ItemInterval = i32`：TopRU interval 的线协议边界类型。Rust protobuf 枚举不能表达未知数值，保留 `i32` 可让 `1`、`99` 等输入仍进入与 Go 相同的运行时校验，而不是在类型转换时消失。
- `TopRUStateError::InvalidItemInterval(ItemInterval)`：当前唯一错误变体；其显示文本为 `invalid top ru item interval: <value>`。`ErrInvalidTopRUItemInterval` 是兼容 Go 错误前缀的字符串常量，测试用它校验错误分类文本。
- `EnableTopSQL()` / `DisableTopSQL()` / `TopSQLEnabled() -> bool`：以顺序一致原子操作设置或读取 TopSQL 布尔开关。
- `TopProfilingEnabled() -> bool`：返回 `TopSQLEnabled() || TopRUEnabled()`；它表达“任一 Top* 消费者存在即可运行公共 profiling hook”，并不改变状态。
- `EnableTopRU()` / `DisableTopRU()` / `TopRUEnabled() -> bool`：维护 RU 消费者数。开启执行 `fetch_add(1)`；关闭通过 CAS 循环安全减一，计数已为零时无操作，最后一个消费者退出时复位 interval。
- `normalizeTopRUItemIntervalSeconds(ItemInterval) -> Result<i64, TopRUStateError>`：私有纯校验函数。`0` 归一为默认 60，`15`、`30`、`60` 原样转为 `i64`，其余值报错。
- `SetTopRUItemInterval(ItemInterval) -> Result<(), TopRUStateError>`：先校验，再记录包含旧值、活跃订阅数和新值/错误的日志；仅成功分支写入全局 interval，因此遵循合法写入“后写覆盖”。
- `GetTopRUItemInterval() -> i64` / `ResetTopRUItemInterval()`：读取 interval，或恢复到默认 60 秒。

## 执行流程

TopSQL 的主流程很短：注册侧调用 `EnableTopSQL` 后，采集侧用 `TopSQLEnabled` 门控工作；注销侧在自身订阅计数归零后调用 `DisableTopSQL`。直接证据包括 `pkg/util/topsql/reporter/datasink.rs`：`DefaultDataSinkRegisterer::register` 为需要 TopSQL 的 sink 开启全局状态，`deregister` 只在自己的 `top_sql_sink_count` 归零时关闭；`close` 也负责兜底关闭。消费侧如 `pkg/util/topsql/topsql.rs::{AttachAndRegisterSQLInfo, AttachSQLAndPlanInfo, AttachAndRegisterProcessInfo}` 仅在开关打开时设置 profiling 标签，`pkg/util/topsql/stmtstats/aggregator.rs::drain_and_push_stmt_stats` 则在关闭时停止向 collector 推送语句统计。

TopRU 注册遵循“先配置、后计数”：`DefaultDataSinkRegisterer::register` 先将订阅配置的 `item_interval` 交给 `SetTopRUItemInterval`；校验失败被映射为 `DataSinkError::InvalidTopRuInterval`，不会插入 sink 或增加消费者数；成功后才调用 `EnableTopRU`。多个有效订阅的 interval 采用最后一次写入，随后每个订阅各自增加一次计数。注销或 registerer 关闭时，每个启用 TopRU 的 sink 对应一次 `DisableTopRU`。

`DisableTopRU` 循环读取当前计数：若小于等于零立即返回；否则用 `compare_exchange(previous, previous - 1)` 竞争更新，失败就重新读取。成功将 `1` 改为 `0` 时调用 `ResetTopRUItemInterval`，其余成功减计数不改 interval。因而只有最后一个消费者离开会把窗口恢复为 60 秒，额外关闭不会下溢。

消费侧按查询时快照决定本次行为。`pkg/session/runtime/scan_adapter_runtime.rs::{TopSQLStart, TopSQLFinish}` 把 `TopRUEnabled()` 写入语句开始/结束信息；`pkg/util/topsql/stmtstats/aggregator.rs::drain_and_push_ru` 在 RU 汇总为空或 TopRU 关闭时不推送；`pkg/util/topsql/reporter/pubsub.rs` 也同时检查本地配置和全局 TopRU 状态。状态只控制是否进入这些路径，不拥有它们的工作线程或数据缓冲。

## 数据与状态

`GlobalState` 的初始状态为：`enable=false`、`PrecisionSeconds=1`、`MaxStatementCount=100`、`MaxCollect=5000`、`ruConsumerCount=0`、`TopRUItemIntervalSeconds=60`。常量 `DefTiDBTopSQLReportIntervalSeconds=60` 不存入 `State`，它是固定的上报周期；例如 `pkg/util/topsql/reporter/report_ticker.rs` 用它构造 ticker，`pkg/util/topsql/reporter/datamodel.rs::Record::new` 将它与动态 `PrecisionSeconds` 一起用于预分配时间序列容量。

公开容量字段由下游按需读取而非缓存：`pkg/util/topsql/reporter/reporter.rs::{RegisterSQL, RegisterPlan}` 读取 `MaxCollect` 限制新元数据，`processCPUTimeData` 和 `processStmtStatsData` 读取 `MaxStatementCount` 决定 TopN/others；这些读取使用 `Ordering::Relaxed`，因为只需单值原子性。`Record::new` 对 `PrecisionSeconds` 使用 `SeqCst` 读取并至少取 1，避免零或负精度造成除法问题。相反，本文件所有开关、引用计数和 interval 操作都使用 `SeqCst`，提供统一的全序观察。

关键不变量是：`ruConsumerCount` 不因本文件 API 降到零以下；`TopRUEnabled` 当且仅当观察到计数大于零；最后一次从 1 到 0 的成功 CAS 会复位 interval；非法 interval 不写状态；`0` 是“未指定”而非零秒窗口，并被归一为 60。多个合法 setter 没有所有权仲裁，明确是全局后写覆盖。

## 依赖与调用关系

下游依赖只有标准库原子类型、`thiserror::Error` 派生和 `log::{info!, warn!}`；该 crate 不依赖 reporter、collector、protobuf crate 或异步运行时，因此能作为 TopSQL 子系统共同依赖的低层状态边界。`ItemInterval` 以 `i32` 表达 protobuf 数值，避免引入具体生成枚举依赖。

主要上游接线如下：

- `pkg/util/topsql/reporter/datasink.rs` 是开关生命周期的核心生产入口：注册/注销 sink 设置 interval 并启停 TopSQL/TopRU。
- `pkg/util/topsql/topsql.rs` 和 `pkg/util/topsql/collector/cpu.rs` 根据 `TopSQLEnabled` 决定标签或采集行为。
- `pkg/util/topsql/stmtstats/{aggregator.rs,kv_exec_count.rs}` 分别用 TopSQL/TopRU 状态门控统计推送与执行计数。
- `pkg/session/runtime/scan_adapter_runtime.rs` 在语句开始和结束时读取 `TopRUEnabled`，把状态快照带入 RU 统计。
- `pkg/util/topsql/reporter/{reporter.rs,datamodel.rs,report_ticker.rs,pubsub.rs}` 消费公开容量、精度、固定周期和开关。
- `pkg/util/topsql/lib.rs` 将 crate 作为 `topsqlstate` 再导出；根 `pkg/lib.rs` 也通过 facade 路径再导出其 API。

RustCodeGraph 对 `state.rs` 识别出 14 个符号并报告该文件被多个文件使用，但对 `TopProfilingEnabled`、`SetTopRUItemInterval` 等精确符号的 `callers`/`callees` 查询返回空集合。因此以上具体边来自对图未覆盖引用的精确源码检索与相邻文件读取，不能把空调用边解释为“无调用者”。

## 错误处理与边界

本文件唯一可返回失败的公开操作是 `SetTopRUItemInterval`。非法数值生成保留原始 `i32` 的 `TopRUStateError::InvalidItemInterval`，写 warn 日志后原样返回；写入发生在校验成功之后，所以先前 interval 保持不变。合法设置写 info 日志并覆盖旧值。日志中的 `current_interval_seconds` 和 `active_subscribers` 是独立原子快照，仅用于诊断，不承诺与最终写入构成事务性视图。

`DisableTopRU` 将重复关闭视为幂等无操作，并用 CAS 防止并发下溢；它不返回“关闭次数不匹配”错误。`EnableTopRU` 使用 `AtomicI64::fetch_add`，源码未对极端整数溢出增加保护，调用方必须维持合理的订阅生命周期。TopSQL 是布尔开关而非本文件内的引用计数；多订阅协调由 `DefaultDataSinkRegisterer.top_sql_sink_count` 完成，绕过该注册器直接配对不当会使全局开关提前关闭。

公开数值字段允许写入零或负数，本文件不统一验证它们；各消费者自行做边界处理，例如 reporter 对计数上限 `.max(0)`，`Record::new` 对精度 `.max(1)`。因此新增写入口时不能假设 `State` 自动强制所有参数合法。

## 并发与资源生命周期

`GlobalState` 是整个进程生命周期内常驻的静态对象，没有显式析构。所有字段都是原子值，不持有锁、通道、任务、堆分配或外部资源；本文件的公开操作不会阻塞等待 I/O。`SeqCst` 让启停、RU 引用计数和 interval 在不同线程间以最强标准原子顺序可见，但“设置 interval + 增加引用计数”仍是调用方执行的两个独立操作，不是一个跨字段事务。

RU 生命周期以订阅者引用计数建模。CAS 循环保证并发关闭时只有成功看到 `previous == 1` 的线程执行最终复位；其它线程要么成功从更大计数减一，要么重试后看到零并返回。注册器还用自身互斥状态把 sink 集合变更与这些调用排序，并在 `Drop` 中 `close`，但该锁属于 `datasink.rs`，不属于 `State`。

独立测试必须串行修改全局单例。`pkg/util/topsql/state/test_util.rs` 用静态 `Mutex<()>` 提供 `lock_global_state`，并以 `reset_global_state` 关闭 TopSQL、循环关闭所有 TopRU 消费者、复位 interval。新增测试应继续放在独立 `*_test.rs` 文件中并复用该守卫，避免并发测试互相污染；不要把测试嵌入 `state.rs`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/topsql/state/state.go`，Rust 默认常量、`State` 字段、全局初值、TopSQL 布尔开关、TopRU 引用计数、profiling 组合判断、CAS 关闭流程、合法 interval 集合、最后消费者退出时复位，以及“合法后写覆盖、非法不覆盖”均与 Go 版本一致。

主要语言差异如下：Go 用 `go.uber.org/atomic` 的堆指针字段，Rust 将原子值直接嵌入 `State`；Go setter 接收 `tipb.ItemInterval`，Rust因生成枚举不能保留未知数值而在边界使用 `type ItemInterval = i32`；Go 通过 `fmt.Errorf("%w: %d", ErrInvalidTopRUItemInterval, value)` 保留可供 `errors.Is` 判断的哨兵错误，Rust通过结构化枚举和一致文本表达同一错误类别；Go 用 `logutil`/`zap` 结构化日志，Rust用 `log` 宏写等价字段文本。

`pkg/util/topsql/state/state_test.go` 与 `state_test.rs` 对应验证引用计数、最后退出复位、额外关闭不下溢、合法 interval 后写覆盖、`0` 归一化及非法值不覆盖。Rust 另有 `migration_aster_unit_test.rs` 验证 TopSQL/TopRU 组合开关、精确错误文本和默认常量/原子初值。当前对照证据未显示 Rust 版有意删减这些核心语义。

## 扩展指南

- 新增全局参数时，在 `State`、`GlobalState` 初始器和对应 `DefTiDB*` 常量中同步定义，并核对 Go `state.go`。决定字段公开性时优先保护跨字段/引用计数不变量；若公开原子字段，还要检查所有消费者对非法值的处理。
- 新增 TopRU interval 枚举值时，修改 `normalizeTopRUItemIntervalSeconds`，保持 `ItemInterval=i32` 的未知值拒绝能力，并同步 `state_test.rs`、`state_test.go` 以及 `migration_aster_unit_test.rs`。还应检查 `reporter/datasink.rs` 的错误映射和实际 sink 配置来源。
- 修改启停语义时，至少联查 `reporter/datasink.rs` 的注册/注销/关闭、`stmtstats/aggregator.rs` 的门控和 `session/runtime/scan_adapter_runtime.rs` 的语句快照。TopSQL 当前在本文件只是布尔值，引用计数在注册器中；若把计数下沉到这里，必须避免形成双重计数。
- 修改内存顺序前应逐字段证明只需哪种同步保证。公开容量读取已有 `Relaxed` 与 `SeqCst` 混用，而生命周期状态统一用 `SeqCst`；不要仅为性能改成弱顺序而忽略订阅线程与采集线程的观察关系。
- 新增测试应放在同目录独立测试文件，通过 `lib.rs` 的 `cfg(test)` 模块挂载，并使用 `test_util::{lock_global_state, reset_global_state}`。涉及生产集成时可扩展 `reporter/datasink_test.rs`、`reporter/pubsub_test.rs` 或 `stmtstats/aggregator_test.rs`，但不要依赖测试执行顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/topsql/state` 找到 `lib.rs`、`state.rs`、Rust/Go 测试及 Go 实现；`node --file pkg/util/topsql/state/state.rs` 读取完整 187 行并识别 14 个符号；对 `TopProfilingEnabled`、`SetTopRUItemInterval`、`EnableTopRU`、`TopSQLEnabled` 执行 `query`，再对精确 Rust 符号执行 `callers`/`callees`，后者为空，故没有据此虚构调用边。
- crate 与装配证据：`pkg/util/topsql/state/{Cargo.toml,lib.rs}`、根 `Cargo.toml`、`pkg/util/topsql/{Cargo.toml,lib.rs}`，以及 reporter/collector/stmtstats 的 Cargo 路径依赖。
- 生产调用证据：`pkg/util/topsql/reporter/{datasink.rs,reporter.rs,datamodel.rs,pubsub.rs,report_ticker.rs}`、`pkg/util/topsql/stmtstats/{aggregator.rs,kv_exec_count.rs}`、`pkg/util/topsql/{topsql.rs,collector/cpu.rs}`、`pkg/session/runtime/scan_adapter_runtime.rs`。
- Go 对照与测试证据：`pkg/util/topsql/state/state.go`、`pkg/util/topsql/state/state_test.go`。
- Rust 独立测试证据：`pkg/util/topsql/state/state_test.rs`、`migration_aster_unit_test.rs`、`test_util.rs`；这些测试证明组合开关、引用计数、默认值、interval 校验/复位和全局状态串行隔离意图。本任务依计划为纯文档分析，未运行 Cargo 或代码测试。
