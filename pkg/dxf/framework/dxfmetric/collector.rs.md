# `pkg/dxf/framework/dxfmetric/collector.rs`

源文件：[`collector.rs`](collector.rs)

## 文件定位

该文件属于 `astersql-dxf-framework-dxfmetric` crate，是 DXF（Distributed eXecution Framework）任务与子任务状态的 Prometheus 自定义采集器实现。DXF 的整体职责和 owner/follower 分工由 [`pkg/dxf/framework/doc.go`](../doc.go) 定义；本采集器对应 owner 侧周期读取任务元数据后暴露聚合指标的观测面。

crate 根模块 [`lib.rs`](lib.rs) 公开 `collector` 模块并再导出其公共 API。[`Cargo.toml`](Cargo.toml) 表明实现直接依赖 `prometheus`、DXF `proto`、`metricscommon` 和 `uuid`。当前 Rust 仓库中，`Collector` 的直接使用只在 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 得到验证；虽然 scheduler crate 声明了 dxfmetric 依赖，但未检索到 Rust 生产代码调用本文件的 `NewCollector` 或 `UpdateInfo`。相应的完整生产接线可在 Go 的 [`scheduler_manager.go`](../scheduler/scheduler_manager.go) `Manager.collectLoop`/`Manager.collect` 中看到，不能据此声称 Rust 侧已完成相同接线。

## 核心职责

- `Collector` 保存最近一次任务和子任务快照，并实现 `prometheus::core::Collector`，供 registry 在 scrape 时动态生成指标族。
- `collectTasks` 按 `(task_type, status)` 聚合任务数，生成 `tidb_disttask_task_status` Gauge。
- `collectSubtasks` 按 `(task_id, exec_id, status)` 聚合子任务数，附加任务类型标签，生成 `tidb_disttask_subtasks` Gauge。
- 对 `pending` 子任务从 `CreateTime` 起算、对 `running` 子任务从 `StartTime` 起算持续秒数，生成逐子任务的 `tidb_disttask_subtask_duration` Gauge；其他状态不输出 duration。
- 采用“快照重算”而非长期维护每组标签的 Gauge，从而使已经从当前任务表迁走、执行节点变化或状态变化的旧标签不会在后续 scrape 中残留。这个设计动机在 Go 对照文件 [`collector.go`](collector.go) 的 `Collector` 注释中有明确说明。

## 主要符号

- `type Labels = HashMap<String, String>`：Prometheus 常量标签集合。它是文件内别名，不对外公开。
- `pub struct Collector`：公共采集器。`snapshot` 用 `RwLock<Snapshot>` 保护可替换快照，`descriptors` 保存三个稳定的 `Desc`，`constLabels` 保存创建 GaugeVec 时复用的常量标签。
- `struct Snapshot`：内部值对象，同时持有 `Vec<TaskBase>` 和 `Vec<SubtaskBase>`，保证一次更新把两组数据作为一个锁保护单元发布。
- `read_lock`/`write_lock`：内部锁辅助函数；锁中毒时通过 `PoisonError::into_inner` 继续使用内层状态。
- `strings`/`descriptor`：将标签名转为拥有所有权的字符串并调用 `metricscommon::NewDesc`。描述符非法时通过 `expect` 终止，而不是把构造错误延迟到 scrape。
- `Collector::new(in_test: bool) -> Self`：真实构造入口。`in_test=true` 时生成随机 `server_id` 常量标签，避免同一进程内多个测试 registry/domain 的同名指标冲突；公开兼容入口 `NewCollector()` 固定传入 `false`。
- `Collector::UpdateInfo(&self, tasks, subtasks)`：取得写锁并以新 `Snapshot` 完整替换旧值。
- `Collector::gauge_vec`：为一次 scrape 创建带相同名称、help、动态标签和 `constLabels` 的临时 `GaugeVec`。
- `Collector::collectTasks`/`Collector::collectSubtasks`：分别生成任务指标族与子任务计数、时长指标族。
- `seconds_since(SystemTime) -> f64`：计算当前时间与起点的秒差；起点在未来时保留负数。
- `impl PrometheusCollector for Collector`：`desc` 返回构造期的三个描述符引用；`collect` 在一个读锁周期内从同一快照生成全部指标族。

## 执行流程

1. 调用 `Collector::new` 时建立空快照，随后以固定名称、help 和标签顺序创建三个 `Desc`。测试模式额外生成一次 UUID 并作为三个指标共享的 `server_id` 常量标签。
2. 数据提供方调用 `UpdateInfo`，写锁覆盖整个赋值操作；旧任务和旧子任务一起被替换，不做增量合并。
3. Prometheus registry 调用 `desc` 获取预声明描述符；实际 scrape 调用 `collect`。
4. `collect` 持有快照读锁，先执行 `collectTasks`。后者遍历任务，使用 `(task.Type.to_string(), task.State.to_string())` 作为键累计计数，再写入临时 GaugeVec 并收集为 `MetricFamily`。
5. `collectSubtasks` 单次遍历子任务：按 `(TaskID, ExecID, State)` 累计数量，同时记录 `TaskID -> TaskType`；遇到 `pending` 或 `running` 时立即计算持续时间并写入逐子任务 Gauge。
6. 第二次遍历聚合结果，为每个键补上任务类型字符串并写入子任务计数 Gauge。最后依次收集计数和 duration 两组指标族。
7. `collect` 合并任务与子任务指标族并返回；读锁随后释放。因此一次 scrape 内的三个指标视图来自同一个 `Snapshot`。

## 数据与状态

`Snapshot` 拥有传入的两个 `Vec`，因此调用者交出数据后不能在外部继续修改同一批元素。初始快照为空；尚未调用 `UpdateInfo` 时 scrape 会返回空的临时 GaugeVec 指标族，而不是旧值或错误。连续更新是替换语义，[`migration_collector_replaces_snapshot_atomically`](migration_aster_unit_test.rs) 验证第二批快照不会保留第一批的 `task_type` 标签。

任务计数键拥有 `String`；子任务计数键包含 `i64` 任务 ID、克隆的执行器 ID 和静态状态字符串。`task_types` 假设同一 `TaskID` 的子任务具有一致 `Type`；若输入违反这一上游不变量，最后遇到的类型会覆盖前值，并用于该任务 ID 的所有聚合行。指标值统一转成 `f64`，符合 Prometheus Gauge 的数据模型。

duration 不存入快照外的累计状态，而在每次 scrape 时根据 `SystemTime::now()` 重新计算，所以 pending/running 指标会随时间增长。每条 duration 带 `subtask_id`，而计数只聚合到 task、exec 和 state 维度。

## 依赖与调用关系

下游依赖如下：

- `proto::task::{TaskBase, TaskType}` 和 `proto::subtask::{SubtaskBase, SubtaskStatePending, SubtaskStateRunning}` 提供快照字段及状态常量。
- `metricscommon::NewDesc` 合并调用者常量标签与 metrics-common 的包级常量标签，再创建 `prometheus::core::Desc`；实现位于 [`pkg/metrics/common/wrapper.rs`](../../../metrics/common/wrapper.rs) 的 `NewDesc`。
- `prometheus::{GaugeVec, Opts}` 用于 scrape 期构建临时 Gauge，`PrometheusCollector::collect` 将它们转换为 `MetricFamily`。
- `uuid::Uuid::new_v4` 仅用于 `Collector::new(true)` 的测试隔离标签；公开的 `NewCollector` 不启用它。

上游方面，RustCodeGraph 将 `NewCollector -> Collector::new` 识别为直接调用边；[`lib.rs`](lib.rs) 将 `Collector`、`NewCollector` 再导出给 crate 使用者。仓库文本检索只发现 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 调用 `Collector::new` 和 `UpdateInfo`，没有发现 Rust 生产调用。Go 生产链则是 `Manager.collectLoop -> dxfmetric.NewCollector -> metrics.Register`，定时进入 `Manager.collect -> GetAllTasks/GetAllSubtasks -> UpdateInfo`，详见 [`scheduler_manager.go`](../scheduler/scheduler_manager.go)。

## 错误处理与边界

- 描述符或 GaugeVec 定义错误被视为编程错误：`descriptor` 和 `gauge_vec` 使用 `expect`，会 panic。指标名称、标签名或 help 调整必须同时保证构造期 Desc 与 scrape 期 GaugeVec 完全一致。
- `RwLock` 中毒不会直接让后续更新或 scrape 失败；辅助函数恢复内层状态。恢复保证可观测性继续工作，但中毒前若写操作曾部分修改内部值，调用方仍需把异常视为潜在数据可信度风险。当前更新是一次整值赋值，缩小了部分更新窗口。
- 空快照、空任务列表或空子任务列表产生零个对应时间序列；实现不会主动输出值为零的所有可能标签组合。
- 只有 pending/running 产生 duration。failed、succeed、canceled 等状态仍参与子任务计数，但不产生 duration；测试显式验证 failed 子任务 ID 不出现在 duration 指标中。
- `seconds_since` 对未来时间返回负值，与 Go `time.Since` 的符号语义一致；它不钳制为零，也不校验时间戳合理性。
- `task_types[&task_id]` 使用索引访问。正常循环中每次插入 count 时都会同步插入 task type，因此内部生成的键不会缺失；若未来拆分这两个步骤，必须保持该不变量或改为显式错误处理。

## 并发与资源生命周期

`UpdateInfo` 需要独占写锁；scrape 的 `collect` 在任务和子任务两段聚合期间一直持有读锁，所以多个 scrape 可以并发读取，但更新会等待所有活跃读者完成。其优点是一次 scrape 不会混合新任务与旧子任务；代价是快照越大、聚合越慢，写入阻塞时间越长。

三个持久资源是快照、描述符和常量标签，均随 `Collector` 生命周期释放。每次 scrape 都重新分配三个 `GaugeVec`、聚合 HashMap 及输出指标族，不保留跨 scrape 的标签实例，因此无需显式删除陈旧时间序列。文件自身不启动线程、异步任务、通道或定时器；周期调度与 registry 注册/注销属于上游生命周期。Go `Manager.collectLoop` 展示了注册、ticker 和退出时注销的参考生命周期，但 Rust 生产侧尚未找到对应接线。

## 与 Go 版本的对应关系

Rust 文件直接对照 [`collector.go`](collector.go)：三个指标名、help、动态标签顺序、任务/子任务聚合维度以及 pending/running duration 起点均保持一致。Rust 的 `NewCollector` 保留 Go 风格名称，`lib.rs` 通过 `#![allow(non_snake_case)]` 允许迁移期 API 命名。

主要实现差异是并发存储：Go 用两个 `atomic.Pointer` 分别发布任务和子任务切片，Rust 用一个 `RwLock<Snapshot>` 同时发布两组数据。Rust 因而明确提供跨两组数据的 scrape 一致性，但 scrape 聚合期间会阻塞更新；Go 的两个原子 load/store 无锁，不过任务和子任务可能来自相邻发布时刻。Rust 初始值是空快照，Go 在任一指针尚为 `nil` 时让对应收集函数直接返回，最终空数据行为相同。

另一个差异是测试标签入口：Go `NewCollector` 读取全局 `intest.InTest`，Rust 把该选择变成 `Collector::new(in_test)` 参数，而兼容函数 `NewCollector()` 固定为非测试模式。Rust 独立测试直接调用 `Collector::new(false)`；当前测试没有覆盖 `true` 时 UUID `server_id` 的存在或隔离效果。

Go 文件中 `setDistSubtaskDuration` 是独立方法；Rust 将分支内联到 `collectSubtasks` 并抽出 `seconds_since`。这是结构差异，不改变已测试的状态选择和标签语义。更重要的迁移状态差异是：Go scheduler manager 已注册并定时更新此 collector，而 Rust 生产源码中未找到同等调用，故 Rust 当前能力应描述为“实现并通过迁移单测覆盖”，而非“已在完整应用主链启用”。

## 扩展指南

- 新增指标时，应同时更新 `Collector::new` 的持久 `Desc`、scrape 期 `gauge_vec` 定义、`desc`/`collect` 返回组合以及 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 的 registry 文本断言；名称、help、动态标签和常量标签必须一致。
- 新增聚合维度时，修改 `collectTasks` 或 `collectSubtasks` 的 HashMap 键，并评估标签基数。特别是 `subtask_id` 只应继续用于逐子任务 duration，避免把高基数扩散到聚合计数。
- 支持新 duration 状态时，在 `collectSubtasks` 的 `start` 匹配中明确选择时间字段，并同步 Go `setDistSubtaskDuration`（若要求双语义对齐）及 failed/终止状态的负向测试。
- 若要减少写锁等待，可在读锁下克隆或以 `Arc<Snapshot>` 快速取得快照后释放锁，但应先保持任务/子任务原子发布和单次 scrape 一致性，并增加并发更新测试；不要退化为两个独立锁而不说明一致性语义。
- 若接入 Rust 生产调度链，应在 scheduler manager 的生命周期中完成创建、registry 注册、定时查询、`UpdateInfo` 与退出注销，并增加独立 scheduler 集成测试；不能只依赖本 crate 的聚合单测证明接线有效。
- 测试逻辑应继续放在独立的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，不要内嵌到生产文件。建议补充 `Collector::new(true)` 的常量标签测试、未来时间产生负 duration 的边界测试、同一 task ID 出现不一致类型时的契约测试，以及更新与 scrape 并发时的一致性测试。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/dxf/framework/dxfmetric` 确认目标 Rust/Go 文件、crate 入口和迁移测试均已索引。
- RustCodeGraph `node --file pkg/dxf/framework/dxfmetric/collector.rs --offset 1 --limit 260`：读取完整 237 行源文件及其 16 个索引符号；`node NewCollector` 确认本文件 `NewCollector` 调用 `Collector::new`。
- 直接读取：[`collector.rs`](collector.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`collector.go`](collector.go)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`pkg/dxf/framework/doc.go`](../doc.go)、[`scheduler_manager.go`](../scheduler/scheduler_manager.go) 和 [`pkg/metrics/common/wrapper.rs`](../../../metrics/common/wrapper.rs)。
- 仓库引用检索：Rust 侧 `Collector::new`/`UpdateInfo` 仅命中迁移测试与定义；Go 侧命中 scheduler manager 的创建、注册和快照更新。Cargo 检索确认 scheduler、taskexecutor、metering、importinto、DDL 和 metrics 等 crate 依赖 dxfmetric，但依赖声明本身不等于调用本采集器。
- [`migration_collector_aggregates_go_dimensions_and_durations`](migration_aster_unit_test.rs) 覆盖任务计数、子任务计数、pending/running duration 及 failed duration 排除；[`migration_collector_replaces_snapshot_atomically`](migration_aster_unit_test.rs) 覆盖整快照替换。按任务约束未运行 Cargo，测试结论来自已有测试代码审阅而非本轮执行。
- 文档交付使用任务文件指定的结构命令检查目标存在且恰有 11 个固定二级章节，并在交付前检查仅新增本说明、未修改 Rust/Go/Cargo 或只读的总体计划。
