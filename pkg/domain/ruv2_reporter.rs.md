# `pkg/domain/ruv2_reporter.rs`

## 文件定位

本文件属于 `astersql-domain` crate，并由 `pkg/domain/lib.rs` 以公开模块 `ruv2_reporter` 导出。它位于资源组消费上报实现与 `Domain` 生命周期之间：`Domain` 只保存一个对象安全的 RU v2 上报接口，而不直接依赖某个具体资源组控制器类型。crate 边界和依赖分别由 `pkg/domain/Cargo.toml` 的 `[lib] path = "lib.rs"` 与 `astersql-resourcegroup = { path = "../resourcegroup" }` 证明。

文件仅定义边界接口和适配器，不计算 RU、不选择资源组，也不拥有发布时机。实际发布判断位于 `pkg/executor/statement_ru_result.rs::StatementRUContextSink::consumption`，session 侧转发位于 `pkg/session/runtime/scan_adapter_runtime.rs::ReportRUV2Consumption`；DDL job 的 RU 另由 `pkg/session/runtime/system_session.rs::report_ddl_job_ru` 走同一 `Domain` 槽位。

## 核心职责

1. `RUV2ConsumptionReporter` 把 `Domain` 所需的最小上报能力抽成 `Send + Sync` 的对象安全 trait，使 `Domain` 可以保存 `Arc<dyn RUV2ConsumptionReporter>`。
2. `ResourceGroupReporterBridge<R>` 持有 `Arc<R>`，把实现了 `astersql_resourcegroup::ConsumptionReporter` 的具体对象适配到上述 Domain 边界。
3. 桥接调用原样保留资源组名和 TiKV、TiDB、TiFlash 三个 `f64` 分量；本文件不校验、归一化、聚合或吞并数值。

当前仓库事实是：`ResourceGroupReporterBridge` 只有定义和 trait 实现，`rg` 未找到生产或测试构造点。现有调用方直接向 `Domain::bind_ruv2_consumption_reporter` 注入实现 `RUV2ConsumptionReporter` 的对象。因此桥接器是可用但尚未接线的适配层，不能据此声称资源组控制器已自动绑定到 Domain。

## 主要符号

- `pub trait RUV2ConsumptionReporter: Send + Sync`：公开、对象安全的 Domain 边界。唯一方法 `report_ruv2_consumption(&self, resource_group: &str, tikv: f64, tidb: f64, tiflash: f64)` 使用共享引用且无返回值，允许通过 trait object 并发共享。
- `pub struct ResourceGroupReporterBridge<R>(pub Arc<R>)`：公开元组结构体。字段公开，调用方可直接用 `ResourceGroupReporterBridge(Arc::new(...))` 构造或访问内部共享对象；文件未提供额外构造函数。
- `impl<R> RUV2ConsumptionReporter for ResourceGroupReporterBridge<R>`：当 `R: astersql_resourcegroup::ConsumptionReporter + Send + Sync` 时提供适配实现。方法体只调用 `self.0.report_ruv2_consumption(...)`。

文件没有常量、枚举、类型别名、条件编译项或内部私有函数。

## 执行流程

常规 SQL 的已接线流程如下：

1. `pkg/executor/statement_ru_result.rs::StatementRUContextSink::consumption` 获取资源组名，并要求 `RUV2ReporterAvailable()` 为真且资源组名非空。
2. executor 通过 adapter context 调用 `ReportRUV2Consumption(group, tikv, tidb, tiflash)`。
3. `pkg/session/runtime/scan_adapter_runtime.rs::SessionBoundAdapterOwner::ReportRUV2Consumption` 先记录运行时 effect，再从 `Domain::ruv2_consumption_reporter()` 取出可选 reporter。
4. reporter 存在时，调用本文件 trait 的 `report_ruv2_consumption`；不存在时直接结束，不产生上报。
5. 若槽位中存放的是 `ResourceGroupReporterBridge<R>`，其实现把四个参数不变地委托给 `R: astersql_resourcegroup::ConsumptionReporter`。

DDL job 的路径更短：`pkg/session/runtime/system_session.rs::report_ddl_job_ru` 从 job 元数据选择资源组（空值回退到默认组），然后把 `job.ru` 作为 TiKV 分量上报，TiDB 与 TiFlash 分量均传 `0.0`。这个入口同样先读取 Domain 的可选 reporter。

## 数据与状态

本文件自身只有 `ResourceGroupReporterBridge<R>` 内的一个 `Arc<R>`。它不缓存资源组、RU 数值、错误或时间信息，也没有全局状态。`Arc` 使桥接器与外部持有者共享同一个底层 reporter，克隆持有关系不会复制 reporter 状态。

真正的可变绑定状态位于 `pkg/domain/domain.rs`：字段 `ruv2_consumption_reporter` 是 `RwLock<Option<Arc<dyn RUV2ConsumptionReporter>>>`，初始化为 `None`；`bind_ruv2_consumption_reporter` 整体替换该选项，`ruv2_consumption_reporter` 在读锁内克隆 `Arc` 后返回。因此一次调用取得 reporter 后，即使其他线程随后解绑，当前克隆仍保持对象存活并可完成调用。

## 依赖与调用关系

上游直接依赖包括：

- `pkg/domain/domain.rs`：持有、绑定并读取 `Arc<dyn RUV2ConsumptionReporter>`。
- `pkg/session/runtime/scan_adapter_runtime.rs`：常规 statement RU 的 Domain 转发点。
- `pkg/session/runtime/system_session.rs`：DDL job RU 的 Domain 转发点。
- `pkg/session/runtime/scan_adapter_runtime_test.rs` 与 `pkg/executor/statement_ru_plan_walk_test.rs`：定义测试 reporter 并注入 Domain。

下游依赖只有 `astersql_resourcegroup::ConsumptionReporter::report_ruv2_consumption`，且只在 `ResourceGroupReporterBridge<R>` 的实现中发生。`pkg/resourcegroup/checker.rs` 定义了该 trait；它还包含另一项 `report_consumption` 能力，但本桥接器不会调用该方法。

RustCodeGraph 将目标文件列为 6 个符号，并显示它被 session runtime 文件使用；精确 `callees report_ruv2_consumption` 查询确认桥接实现委托到同名 trait 方法。因为同名符号较多，调用图无法区分所有动态派发边，本文对 Domain 字段、executor/session 调用点和桥接器是否被构造的结论同时用精确 `rg` 结果核验。

## 错误处理与边界

接口返回 `()`，没有 `Result`，所以本文件没有可传播的业务错误。底层 reporter 若 panic，桥接层不会捕获；传入的空资源组名、负数、NaN、无穷值或异常大的 RU 也不会在这里拒绝或修正。合法性与容错策略必须由上游发布条件或具体 `ConsumptionReporter` 实现承担。

Domain 槽位为 `None` 时，上游使用 `if let Some(...)` 跳过上报。Domain 的读写锁若中毒，`bind_ruv2_consumption_reporter` 和 `ruv2_consumption_reporter` 会以 `expect("RUv2 reporter lock poisoned")` panic；这属于相邻 Domain 存储边界，而非本文件处理的错误。

对象安全边界只包含 RU v2 的四参数方法，无法经由 `dyn RUV2ConsumptionReporter` 调用 `ConsumptionReporter::report_consumption`。扩展时不应误以为该桥接器覆盖了原始 consumption 的完整接口。

## 并发与资源生命周期

`RUV2ConsumptionReporter: Send + Sync` 要求注入 Domain 的实现可跨线程转移并通过共享引用并发调用；桥接实现还显式要求 `R: Send + Sync`。`Arc<R>` 管理引用计数，最后一个强引用释放时才销毁底层 reporter，本文件没有显式关闭、flush 或后台任务生命周期。

调用过程不持有 Domain 的 `RwLock`：getter 在锁内克隆 `Arc`，随即释放读锁，之后才调用 reporter。这避免慢上报长期占用 Domain 锁，也允许并发重新绑定；代价是重新绑定与在途调用之间没有“立即停止旧 reporter”的强同步保证。

本文件没有内部锁，也不序列化调用。具体 reporter 若记录可变状态，必须自行使用锁、原子量或其他同步机制；测试中的 reporter 使用 `Mutex<Vec<...>>` 正是这一契约的示例。

## 与 Go 版本的对应关系

Go 的直接语义来源是 `pkg/resourcegroup/checker.go::ConsumptionReporter`。其 `ReportRUV2Consumption(resourceGroupName string, tikvRUV2, tidbRUV2, tiflashRUV2 float64)` 与 Rust 的资源组名加三个 `f64` 参数一一对应；`pkg/resourcegroup/checker.rs::ConsumptionReporter` 已移植这两个 Go 方法形状。

Go 的常规 SQL 路径在 `pkg/session/session.go::GetDistSQLCtx` 中从 Domain 的资源组控制器取得 `resourcegroup.ConsumptionReporter`，放入 `DistSQLContext.RUConsumptionReporter`；`pkg/executor/statement_ru_result.go` 检查 context、reporter 和非空资源组名后直接调用它。Rust 因 `Domain` 与资源组具体类型之间需要对象安全边界，增加了本文件 trait、Domain 可选槽位和桥接器；这是 Rust 特有接线，不是 Go 同路径文件的逐字结构。

Rust 当前只在显式调用 `bind_ruv2_consumption_reporter` 后具备该能力，且仓库内没有 `ResourceGroupReporterBridge` 的构造点。相较 Go 从 controller 自动取得 reporter 的路径，这一自动绑定仍是未验证/未接线状态。`pkg/executor/statement_ru_plan_walk_test.rs::go_merge_20_187_195_197_production_ru_unbridged_sql_has_no_estimated_publication` 还刻意保留并验证部分 canonical dispatch 分支不发布伪造 RU 的现状。

## 扩展指南

- 若要把真实资源组 controller 接入 Domain，优先在 controller 创建/安装的生命周期位置构造 `ResourceGroupReporterBridge`，再调用 `Domain::bind_ruv2_consumption_reporter(Some(...))`；关闭或替换时应对称传入 `None` 或新实例。需要先确认 controller 的 `ConsumptionReporter::Consumption` 关联类型与生命周期，不要复制 reporter 状态。
- 若要增加上报字段或错误反馈，需要同步修改本文件 trait、`astersql_resourcegroup::ConsumptionReporter`、Domain trait object 的所有实现、executor adapter 接口及 Go 对照契约；这是破坏性接口变更，需评估动态派发调用方和跨语言语义。
- 若只改变 RU 计算、资源组选择或发布时机，应修改 executor/session 对应符号，而不是在本适配层加入计算逻辑。保持桥接器为无状态、原样委托可减少重复策略。
- 测试应继续放在独立文件。适配器本身新增行为时，宜新增同目录独立 `pkg/domain/ruv2_reporter_test.rs` 并在模块入口按现有测试组织方式接入；跨层行为则同步扩展 `pkg/session/runtime/scan_adapter_runtime_test.rs` 和 `pkg/executor/statement_ru_plan_walk_test.rs`。至少覆盖四参数原样转发、`None` 分支、重新绑定/解绑，以及所需的并发语义。
- 性能上，每次 Domain 获取都会克隆一次 `Arc`，实际 reporter 调用可能位于 statement 完成热路径。新增锁、分配或同步 I/O 前应评估尾延迟；兼容性上必须保留 Go 的 TiKV/TiDB/TiFlash 参数顺序。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/domain/ruv2_reporter.rs` 显示目标文件含 6 个符号；`node --file ... --offset 1 --limit 400` 读取完整 21 行并列出 session runtime 使用点；`query`/`node` 核对 `RUV2ConsumptionReporter`、`ResourceGroupReporterBridge` 和 `report_ruv2_consumption`；`callees report_ruv2_consumption` 核对委托边。
- 源码与配置：`pkg/domain/ruv2_reporter.rs`、`pkg/domain/lib.rs`、`pkg/domain/Cargo.toml`、`pkg/domain/domain.rs`、`pkg/resourcegroup/checker.rs`、`pkg/session/runtime/scan_adapter_runtime.rs`、`pkg/session/runtime/system_session.rs`、`pkg/executor/statement_ru_result.rs`。
- Go 对照：`pkg/resourcegroup/checker.go`、`pkg/session/session.go`、`pkg/executor/statement_ru_result.go`。
- 独立测试：`pkg/session/runtime/scan_adapter_runtime_test.rs::canonical_adapter_ruv2_reports_through_domain_consumption_service` 验证可用性和四参数转发；`pkg/executor/statement_ru_plan_walk_test.rs::go_merge_20_187_195_197_production_ru_canonical_sql` 验证已接线路径单次发布，`go_merge_20_187_195_197_production_ru_unbridged_sql_has_no_estimated_publication` 验证未接线路径不伪造上报；`pkg/resourcegroup/migration_aster_unit_test.rs::consumption_reporter_forwards_both_go_report_shapes` 验证下游 trait 的 Go 对齐形状。另参考 `pkg/executor/statement_ru_reporting_test.go` 的 Go reporter 测试。
- 本任务仅新增文档，按计划不运行 Cargo。最终使用任务指定的结构命令验证恰有 11 个固定二级标题，并人工复核“文件为何存在、如何运行、如何安全扩展”均有直接证据支撑。
