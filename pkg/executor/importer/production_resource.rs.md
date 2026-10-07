# `pkg/executor/importer/production_resource.rs`

## 文件定位

本说明对应生产源码 [`production_resource.rs`](./production_resource.rs)。它属于 `astersql-executor-importer` crate；[`lib.rs`](./lib.rs) 以 `mod production_resource` 挂载模块并通过 `pub use production_resource::*` 公开两个资源计算适配器。crate 名、库入口以及 DXF handle/scheduler 直接依赖由 [`Cargo.toml`](./Cargo.toml) 声明，`package.metadata.porting.go-package = "pkg/executor/importer"` 表明其 Go 对照包。

该文件位于 IMPORT INTO 准备阶段的资源估算边界：上游 `pkg/dxf/importinto/scheduler.rs::ImportSchedulerServices::FromEncodeRuntime` 构造 `HostImportResourceCalculator`，`prepareImportTask` 将其注入 `LoadDataController`，随后 `LoadDataController::CalResourceParams` 查询目标节点 CPU、调优因子和索引体积比，再调用本文件的计算实现，最终把线程数、最大节点数和 DistSQL 扫描并发写回 `Plan`。

本文件不实现资源估算公式本身，也不负责发现数据文件或执行导入。公式位于 `pkg/dxf/framework/scheduler/autoscaler.rs`；控制器编排和结果落盘位于 `pkg/executor/importer/import.rs`。`HandleImportResourceCalculator` 是直接组合 DXF handle 与 `KVSizeSamplerService` 的适配器；当前仓库的生产装配明确使用 `HostImportResourceCalculator`，直接适配器在本次搜索中只被 `production_resource_test.rs` 构造，不能据此宣称它已进入生产主链。

## 核心职责

1. 把 DXF handle 的 `GetCPUCountOfNode` 与 `GetScheduleTuneFactors` 适配成 `ImportResourceCalculator` trait 所需的 CPU 和调优参数查询。
2. 为索引体积比提供两种可替换路径：`HandleImportResourceCalculator` 调用控制器的 `sampleIndexSizeRatio` 并使用专门的 `KVSizeSamplerService`；`HostImportResourceCalculator` 把采样委托给构造时保存的原始 `ImportResourceCalculator`。
3. 用私有函数 `calculate_go_import_resources` 统一两种适配器的最终计算，调用 DXF scheduler 的 Go 兼容入口 `NewRCCalc` 与 `CalcDistSQLConcurrency`。
4. 在 importer 的 `usize` 与 DXF scheduler 的 `i32` 边界做有界转换，并将调度器结果整理成 `ResourceParams`。

文件保持“查询/采样策略可注入、公式集中复用”的边界：两个公开结构体只决定外部资源如何取得，最终线程、节点与扫描并发始终走同一个私有计算函数。

## 主要符号

- `pub struct HandleImportResourceCalculator { Context, Sampler }`：直接 DXF 适配器。`Context` 供 CPU/调优查询使用；`Sampler: Arc<dyn KVSizeSamplerService + Send + Sync>` 供 `LoadDataController::sampleIndexSizeRatio` 构造 parser/encoder 并执行 KV 采样。
- `pub struct HostImportResourceCalculator { Handle, SampleService }`：生产主链使用的组合适配器。`Handle` 来自 scheduler context 的 cancellation flag；`SampleService: Arc<dyn ImportResourceCalculator>` 保存被包装的原始计算服务，仅把索引比例采样委托给它。
- `ImportResourceCalculator for HostImportResourceCalculator`：CPU 与调优因子改走 DXF handle，采样保留 host/controller 原实现，计算走 `calculate_go_import_resources`。这是 `FromEncodeRuntime` 实际装配的实现。
- `ImportResourceCalculator for HandleImportResourceCalculator`：CPU、调优因子和最终计算与 host 适配器相同；区别是采样直接调用 `controller.sampleIndexSizeRatio(keyspace_codec, self.Sampler.as_ref())`。
- `fn calculate_go_import_resources(total_real_size, target_node_cpu_count, index_size_ratio, factors) -> ResourceParams`：文件内唯一私有函数。它将 CPU 转成 `i32`，把 importer 的 `ScheduleTuneFactors` 映射为 DXF `TuneFactors`，再依次取得 required slots、import 最大节点数和 DistSQL 并发。
- `ImportResourceCalculator`、`ScheduleTuneFactors`、`ResourceParams`：定义在 `import.rs` 的公共契约。控制器只依赖该 trait，因此生产适配、测试替身和 host 采样服务可以替换而无需改变 `CalResourceParams`。

目标文件没有模块级常量、enum、宏或条件编译项；测试通过 `lib.rs` 中独立的 `#[cfg(test)] mod production_resource_test` 接入，没有把测试嵌入生产源文件。

## 执行流程

生产准备流程如下：

1. `ImportSchedulerServices::FromEncodeRuntime` 取得 encoder runtime 提供的 `ControllerServices`。它保留其中原有的 `ResourceCalculator`，并配置 `ResourceCalculatorWithContext` 工厂。
2. `prepareImportTask` 创建 controller services；若该工厂存在，就用 `HostImportResourceCalculator` 替换 `services.ResourceCalculator`。新适配器的 `Handle` 绑定当前 scheduler context 的取消标志，`SampleService` 则是被替换前的计算器。
3. 数据文件初始化和大小限制检查完成后，`prepareImportTask` 调用 `LoadDataController::CalResourceParams(&services.KVCodec)`。
4. 控制器先调用 `TargetNodeCPUCnt`。`HostImportResourceCalculator` 通过 `handle::GetCPUCountOfNode(&self.Handle)` 查询当前目标节点 CPU，把外部错误转成字符串，并把非负 `i32` 转为 `usize`。
5. 控制器再以 `Plan.Keyspace` 调用 `ScheduleTuneFactors`。适配器通过 `handle::GetScheduleTuneFactors` 取得有效 TTL 内的值；handle 层在配置缺失或过期时返回默认因子。本文件仅复制 `AmplifyFactor`。
6. 控制器用 `GetNumOfIndexGenKV` 判断是否存在需生成 KV 的二级索引。没有索引时直接使用 `0.0`；有索引时调用适配器的 `SampleIndexSizeRatio`。生产 host 适配器把调用委托给原始服务，直接 handle 适配器则使用其 `Sampler` 调用控制器采样。
7. 控制器把 `TotalRealSize`、CPU、索引比例和调优因子交给 `Calculate`。两个适配器都进入 `calculate_go_import_resources`。
8. 私有函数构造 DXF `ResourceCalc`，依次调用 `required_slots()`、`max_node_count_for_import_into()` 与 `CalcDistSQLConcurrency(threads, nodes, cpu)`，组成 `ResourceParams`。
9. `CalResourceParams` 把结果写入 `Plan.ThreadCnt`、`Plan.MaxNodeCnt` 和 `Plan.DistSQLScanConcurrency`；`prepareImportTask` 随后把准备结果持久化并继续生成 chunk map。

`HandleImportResourceCalculator` 的流程只在步骤 6 的采样来源上不同。当前独立测试绕过 handle 查询，直接调用其 `Calculate`，证明它与生产 host 适配器共用的计算路径得到 Go RCCalc 预期值。

## 数据与状态

两个适配器都只持有共享服务句柄，不维护可变计数器或缓存。`handle::Context` 按值保存在结构体中；采样服务以 `Arc` 共享。`HostImportResourceCalculator::SampleService` 的 trait 本身要求 `Send + Sync`，`HandleImportResourceCalculator::Sampler` 则在字段类型上显式要求 `Send + Sync`，使两个适配器都满足 `ImportResourceCalculator: Send + Sync` 并能作为 `Arc<dyn ImportResourceCalculator>` 注入。

输入数据的语义如下：`total_real_size` 是解压/估算后的真实数据字节数；`target_node_cpu_count` 是单个目标执行节点的 CPU 数；`index_size_ratio` 是 `IndexKVSize / DataKVSize`，数据 KV 为零时采样层返回 `0.0`；`AmplifyFactor` 同时放大有效数据量和节点数上限。DXF `ResourceCalc` 的有效大小为 `AmplifyFactor * (1 + index_size_ratio) * data_size`。

输出 `ResourceParams` 是一次性值：`ThreadCnt` 来自 required slots，`MaxNodeCnt` 来自 import-into 节点估算，`DistSQLScanConcurrency` 来自线程数、节点数与单节点 CPU 的组合。控制器负责把它们写回 `Plan`；本文件不修改 controller 或 plan，也没有全局状态。

## 依赖与调用关系

已确认的生产上游链为：

`ImportSchedulerServices::FromEncodeRuntime` → 构造 `HostImportResourceCalculator` → `prepareImportTask` 注入 `LoadDataControllerServices::ResourceCalculator` → `LoadDataController::CalResourceParams` → `ImportResourceCalculator::{TargetNodeCPUCnt, ScheduleTuneFactors, SampleIndexSizeRatio, Calculate}`。

RustCodeGraph 对 `scheduler.rs::FromEncodeRuntime` 的节点轨迹明确给出对 `HostImportResourceCalculator` 的实例化边；对 `production_resource.rs::calculate_go_import_resources` 的节点轨迹明确给出两个 `Calculate` 实现对它的调用边。`import.rs::CalResourceParams` 的轨迹则确认它依次调用 trait 的四个方法。直接引用搜索还确认仓库生产代码只有 `pkg/dxf/importinto/scheduler.rs` 构造 `HostImportResourceCalculator`，没有生产代码构造 `HandleImportResourceCalculator`。

主要下游依赖为：

- `astersql-dxf-framework-handle`：`GetCPUCountOfNode` 经已安装 runtime 查询 CPU；`GetScheduleTuneFactors` 读取调优因子，并在缺失或 TTL 过期时采用默认值；其 `Context` 还承载取消标志。
- `astersql-dxf-framework-scheduler`：`NewRCCalc`、`ResourceCalc::required_slots`、`max_node_count_for_import_into` 和 `CalcDistSQLConcurrency` 实现实际估算公式。
- `pkg/executor/importer/import.rs`：定义 trait、输入/输出类型及调用编排，并决定无索引时跳过采样、采样失败时降级。
- `pkg/executor/importer/sampler.rs`：直接适配器所用的 `KVSizeSamplerService` 和 `LoadDataController::sampleIndexSizeRatio` 实现。

`Cargo.toml` 证明 handle 与 scheduler 均是 `astersql-executor-importer` 的路径依赖；本文件未使用 feature gate，也没有额外外部网络或存储依赖。

## 错误处理与边界

- `GetCPUCountOfNode` 与 `GetScheduleTuneFactors` 的错误都通过 `error.to_string()` 转为 importer 的 `Result<_, String>`。控制器使用 `?` 传播这两类错误，因此准备阶段立即失败，不会写入部分资源参数。
- DXF CPU 返回值从 `i32` 转成 `usize`；负数会返回 `invalid node CPU count: <value>`。零能成功转换，并继续交给调度公式；scheduler 对非正 CPU 的最大节点数返回 `0`，本文件不额外拒绝零值。
- 最终计算前，`usize` CPU 通过 `i32::try_from` 转回 scheduler 类型；超过 `i32::MAX` 时饱和为 `i32::MAX`，避免回绕或 panic。
- `required_slots` 与 DistSQL 并发在写入 `usize` 前用 `.max(0)` 夹住负值；`MaxNodeCnt` 保留 scheduler 的 `i32` 结果，包括 CPU 为零时的 `0`。
- 本文件的 `Calculate` 不返回 `Result`，公式计算不会在此处传播错误。浮点 `index_size_ratio` 与 `AmplifyFactor` 没有在本层校验有限性或非负性；其有效域由采样、handle 配置和 scheduler 公式共同约束，扩展时不能假定本文件已经拒绝 NaN、无穷或负值。
- `LoadDataController::CalResourceParams` 在无二级索引时完全跳过采样；有索引但采样失败时使用 `unwrap_or(0.0)` 降级。这个非致命策略发生在上游控制器，不是两个适配器吞掉错误。
- `handle::GetScheduleTuneFactors` 的“缺失/过期用默认值”发生在 handle 层；本文件只传播真正的 runtime 查询错误并复制 `AmplifyFactor`。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁或事务。两个适配器通过 `Arc` 共享采样服务，方法均以 `&self` 调用，因此资源计算本身是无状态、可并发调用的。是否可以安全并发最终由 `ImportResourceCalculator: Send + Sync`、`KVSizeSamplerService + Send + Sync` 以及 DXF handle runtime 的契约保证。

`HostImportResourceCalculator` 由 `prepareImportTask` 使用的 services 工厂按准备调用构造，生命周期至少覆盖该 `LoadDataController` 的 `CalResourceParams`。其 `Handle` 从当前 scheduler context 的 cancellation flag 派生，使下游 runtime 查询能观察同一取消上下文；本文件没有主动轮询或修改取消状态。

`Arc` 的析构只减少引用计数，本文件没有显式 `Close`。直接适配器的采样可能临时创建 parser/encoder，但这些对象的具体生命周期由 `sampler.rs` 与 `KVSizeSamplerService` 实现管理，不由本文件持有。最终 `ResourceCalc` 是栈上临时值，取得三个数值后即释放。

## 与 Go 版本的对应关系

主要 Go 对照位于 `pkg/executor/importer/import.go::LoadDataController.CalResourceParams` 与 `pkg/dxf/framework/scheduler/autoscaler.go`。Go 控制器同样按“CPU → 调优因子 → 有索引才采样 → RCCalc → 三个 Plan 字段”的顺序运行；Rust 把原来直接调用的外部服务抽成 `ImportResourceCalculator`，再由本文件提供生产适配器。

公式对应关系为：Rust `NewRCCalc` 对应 Go `scheduler.NewRCCalc`，`required_slots` 对应 `CalcRequiredSlots`，`max_node_count_for_import_into` 对应 `CalcMaxNodeCountForImportInto`，`CalcDistSQLConcurrency` 保持同名。`pkg/dxf/framework/scheduler/autoscaler_test.rs` 以与 Go 测试相同的边界表覆盖零/负数据量、CPU 为零、节点上限、舍入、索引比例、放大因子和 DistSQL 插值。

当前可见差异与限制：

- Go `CalResourceParams` 直接调用 handle 和 controller 采样；Rust 用 trait 拆出依赖，并在 DXF scheduler 装配 `HostImportResourceCalculator`，以保留原始采样服务同时替换 CPU、调优与最终 RCCalc。
- Go 在该方法末尾记录数据库、表、线程、节点、CPU、大小、索引比例、放大因子和耗时日志；Rust `CalResourceParams` 与本文件目前没有对应日志，不能宣称可观察性完全对齐。
- Go 的整数类型是平台 `int`；Rust 在 importer 边界用 `usize`、scheduler 边界用 `i32`，本文件明确做饱和/非负转换。这是 Rust 所有权与类型边界上的保护，不改变正常 CPU 范围内的公式。
- Rust `production_resource_test.rs` 当前只直接覆盖 `HandleImportResourceCalculator::Calculate` 的 200 GiB、16 CPU、零索引比例结果；完整公式边界由 scheduler 的独立 Rust 测试覆盖。host 装配和控制器降级语义主要由源码、RustCodeGraph 调用边及 Go 对照支持，不能把单个目标测试描述成全链路覆盖。

## 扩展指南

- 新增资源输入（例如内存或 I/O 配额）时，先扩展 `import.rs::ImportResourceCalculator`、`ScheduleTuneFactors`/`ResourceParams` 与 `LoadDataController::CalResourceParams` 的数据流，再让两个适配器保持同形实现；不要只修改其中一个适配器，否则测试/备用路径与生产 host 路径会分叉。
- 修改 CPU 或调优查询策略时，入口是两个实现的 `TargetNodeCPUCnt`/`ScheduleTuneFactors`，同时检查 `pkg/dxf/framework/handle/handle.rs` 的默认值、TTL 和取消语义。应在独立的 `production_resource_test.rs` 增加负 CPU、handle 错误和默认调优因子用例，测试不得内嵌生产文件。
- 修改采样策略时，区分两条边界：直接适配器在 `controller.sampleIndexSizeRatio` 使用 `Sampler`；host 适配器委托 `SampleService`。同时回归“无索引不采样”“采样错误降级为 0”“DataKVSize 为 0 返回 0”和 keyspace codec 透传。
- 修改估算公式时，应优先改 `pkg/dxf/framework/scheduler/autoscaler.rs` 及其独立 `autoscaler_test.rs`，保持 Go `autoscaler.go` 的语义；本文件只负责类型映射与组装。若 `ResourceCalc` API 改变，再同步 `calculate_go_import_resources`。
- 扩展 `ScheduleTuneFactors` 时，必须在本文件两个 `ScheduleTuneFactors` 实现和 `calculate_go_import_resources` 的 DXF 类型映射中同步字段，避免配置已读取却在边界丢失。
- 修改整数转换时要保留“超大 `usize` 不回绕、负 scheduler 结果不转成巨大 `usize`”的不变量，并增加独立测试。兼容风险是计划字段变化导致任务调度不同；性能风险是线程/节点/扫描并发过大或过小；正确性风险主要来自 CPU/比例/放大因子错误传播，而非本文件的共享状态。
- 若要让 `HandleImportResourceCalculator` 进入新的生产路径，应先找到明确装配点并增加端到端准备阶段测试；当前证据只支持 host 适配器已在 DXF import scheduler 接线。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/importer` 确认目标、独立测试和模块入口均已索引。
- RustCodeGraph 符号查询：`query HandleImportResourceCalculator`、`query HostImportResourceCalculator`、`query calculate_go_import_resources`、`query TargetNodeCPUCnt`、`query ScheduleTuneFactors`、`query SampleIndexSizeRatio`、`query CalResourceParams`。
- RustCodeGraph 节点/调用证据：`node production_resource.rs::calculate_go_import_resources` 显示两个 `Calculate` 调用者；`node import.rs::CalResourceParams` 显示四个 trait 方法调用；`node scheduler.rs::FromEncodeRuntime` 显示 `HostImportResourceCalculator` 的生产实例化和字段来源。`explore` 与按文件 `node` 本次未产生输出，因此源码全貌和未接线结论用精确 `query/node` 加直接引用搜索补证。
- 生产 Rust 源：`pkg/executor/importer/production_resource.rs`、`import.rs::{ImportResourceCalculator, LoadDataController::CalResourceParams}`、`sampler.rs::sampleIndexSizeRatio`、`lib.rs`、`pkg/dxf/importinto/scheduler.rs::{FromEncodeRuntime, prepareImportTask}`、`pkg/dxf/framework/handle/handle.rs::{GetCPUCountOfNode, GetScheduleTuneFactors}`、`pkg/dxf/framework/scheduler/autoscaler.rs`。
- crate 边界：`pkg/executor/importer/Cargo.toml`，核对 crate 名、`lib.rs` 入口、Go 来源元数据以及 handle/scheduler 路径依赖。目标包没有 `doc.go`，模块级 Rust 说明由 `lib.rs` 提供。
- Rust 测试：`pkg/executor/importer/production_resource_test.rs::handle_resource_calculator_uses_go_import_rccalc`；公式边界由 `pkg/dxf/framework/scheduler/autoscaler_test.rs` 覆盖。`importer_testkit_test.rs::TestCalResourceParams` 当前包含移植测试形状，但其局部桩实现自带 `CalResourceParams`，不作为生产控制器行为的直接运行证据。
- Go 对照与测试：`pkg/executor/importer/import.go::LoadDataController.CalResourceParams`、`pkg/dxf/framework/scheduler/autoscaler.go`、`pkg/executor/importer/importer_testkit_test.go::TestCalResourceParams`，用于核对调用顺序、公式和 200 TiB/300 GiB 场景预期。
- 本任务仅新增说明文档，按计划不运行 Cargo。交付检查使用任务文件指定的结构命令验证恰有 11 个固定二级标题，并人工复核文档能够回答文件存在原因、生产接线、失败边界和安全扩展位置。
