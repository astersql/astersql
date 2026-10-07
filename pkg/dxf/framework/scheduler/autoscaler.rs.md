# `pkg/dxf/framework/scheduler/autoscaler.rs`

## 文件定位

本文件属于 `astersql-dxf-framework-scheduler` crate，是 DXF（Distributed eXecution Framework）调度侧的资源估算模块。crate 入口 `pkg/dxf/framework/scheduler/lib.rs` 通过 `pub mod autoscaler` 声明模块，并用 `pub use autoscaler::*` 再导出其公共 API；因此调用方可以直接通过 `astersql_dxf_framework_scheduler` 使用这些符号。

它虽沿用 Go 文件名 `autoscaler.go` 的 “autoscaler”，但当前职责不是持续监控负载或直接增删节点，而是做无副作用的容量换算：根据数据量、单节点 CPU、索引体积比和调优因子，给出任务节点上限、槽位数及 DistSQL 扫描并发度。Rust 生产链中的直接使用者是 `pkg/executor/importer/production_resource.rs::calculate_go_import_resources`。

## 核心职责

- `ResourceCalc` 保存一次估算所需的输入，并统一计算放大后的有效数据量。
- `max_node_count_for_add_index` 与 `max_node_count_for_import_into` 按 8 核、200 GiB 基准折算最大节点数，并分别应用 30 与 32 的场景上限。
- `required_slots` 以每 25 GiB 一个槽位估算单节点线程/槽位需求，并限制到 CPU 核数。
- `calc_dist_sql_concurrency` 根据节点数在默认扫描并发和每核最大并发之间线性插值。
- `calc_max_node_count_by_store_count` 只把调用方已经取得的 store 数量换算为 DXF 节点数。
- Windows 构建下的 `GetExecCPUNode` 查询当前目标 scope 的执行节点 CPU 数，并保留 Go 测试环境的本机 CPU 回退语义。

本文件只计算建议值，不创建调度器、不预留槽位、不访问任务元数据，也不执行扩缩容动作；实际结果如何持久化和消费由上层负责。

## 主要符号

- `BASE_CORES: f64 = 8.0`：节点数折算的基准 CPU 核数。
- `BASE_DATA_SIZE: f64 = 200 GiB`：与 8 核基准配套的单节点数据量。
- `BASE_SIZE_PER_CONCURRENCY: f64 = 25 GiB`：槽位估算的数据量步长。
- `MAX_NODE_COUNT_FOR_ADD_INDEX = 30`、`MAX_NODE_COUNT_FOR_IMPORT_INTO = 32`：两类任务在 8 核基准下的节点上限；实际上限还乘 `AmplifyFactor` 和 CPU 比例。
- `MAX_DIST_SQL_CONCURRENCY_PER_CORE = 32`、`DEFAULT_DIST_SQL_SCAN_CONCURRENCY = 15`：DistSQL 插值的终点和单节点默认值。
- `ResourceCalc { data_size, node_cpu, index_size_ratio, factors }`：可克隆的值对象。`new` 克隆传入的 `TuneFactors`；`for_add_index` 固定 `index_size_ratio = 0.0`。
- `ResourceCalc::amplified_data_size`：计算 `AmplifyFactor × (1 + index_size_ratio) × data_size`，再转成 `i64`。
- `ResourceCalc::max_node_count_by_size`：内部公共算法；CPU 非正时返回 0，否则按 `8 / node_cpu` 缩放并夹取最小 1 和场景上限。
- `ResourceCalc::{max_node_count_for_add_index,max_node_count_for_import_into,required_slots}`：三个主要估算入口。
- `calc_max_node_count_by_store_count`、`calc_dist_sql_concurrency`：Rust 风格公共函数。
- `NewRCCalcForAddIndex`、`NewRCCalc`、`CalcMaxNodeCountByStoresNum`、`CalcDistSQLConcurrency`：为机械移植调用方保留的 Go 风格公共拼写；它们仅转发到 Rust 风格实现。
- `GetExecCPUNode`：仅 `cfg(target_os = "windows")` 编译的外部查询入口。

文件没有 trait、enum、异步函数或本地条件 feature；唯一条件编译项是 Windows 下的 `GetExecCPUNode`。

## 执行流程

导入场景的已接线主流程如下：

1. `pkg/executor/importer/production_resource.rs` 中两个 `ImportResourceCalculator::Calculate` 实现都进入 `calculate_go_import_resources`。
2. 该函数把 `usize` CPU 数转换为 `i32`（溢出时取 `i32::MAX`），并把导入侧 `ScheduleTuneFactors` 转成 scheduler crate 的 `TuneFactors`。
3. `NewRCCalc(total_real_size, cpu, index_size_ratio, &tune)` 构造 `ResourceCalc`。
4. `required_slots` 先调用 `amplified_data_size`；有效数据量非正时返回兼容默认值 4，否则计算 `round(size / 25 GiB)`，经 `min(node_cpu)`、`max(1)` 夹取。
5. `max_node_count_for_import_into` 再计算有效数据量，将 `AmplifyFactor × 32` 作为上限传给 `max_node_count_by_size`。后者以 `8 / node_cpu` 同时缩放数据需求和上限，再四舍五入为节点数。
6. `CalcDistSQLConcurrency(threads, nodes, cpu)` 在单节点时返回 `threads × 15`；多节点时从 `15 × node_cpu` 起，按 `(min(nodes - 1, 31) / 31)` 插值到 `32 × node_cpu`。
7. 上层将三个结果写入 `ResourceParams::{ThreadCnt, MaxNodeCnt, DistSQLScanConcurrency}`；其中两个无符号字段在写入前用 `max(0)` 防止负值转换。

add-index 路径通过 `NewRCCalcForAddIndex` 固定索引比为零，再调用 `max_node_count_for_add_index`；RustCodeGraph 当前只找到该路径的本文件转发和独立测试调用，未找到 Rust 生产调用者，不能据此声称它已经接入 Rust 运行主链。

## 数据与状态

`ResourceCalc` 是一次计算的普通拥有型数据结构，没有内部可变性。构造时复制三个标量并克隆 `TuneFactors`，后续方法均借用 `&self`，不会修改输入状态。

核心不变量和单位如下：

- `data_size` 以字节计；正常业务输入应为非负值。
- `node_cpu` 表示单执行节点核数。节点数计算显式处理 `node_cpu <= 0` 并返回 0；槽位计算对该非法值没有单独校验。
- `index_size_ratio` 表示索引 KV 体积相对数据 KV 体积的比例；import-into 会使用采样值，add-index 固定为 0。
- `factors.AmplifyFactor` 同时放大有效数据量和节点数上限。`TuneFactors` 定义在 `pkg/dxf/framework/schstatus/tune.rs`，本文件只消费其中的 `AmplifyFactor`。
- 节点数算法保证在 CPU 为正时至少返回 1；槽位算法对非正有效数据量返回 4，否则按表达式夹取和舍入。
- store 算法是整数除法：`max(3, store_count / 3)`；例如 0 到 11 个 store 都至少得到 3，12 个 store 得到 4。

## 依赖与调用关系

直接类型依赖只有 `crate::schstatus::TuneFactors`；它由 scheduler crate 在 `lib.rs` 中从 `astersql-dxf-framework-schstatus` 再导出。`pkg/dxf/framework/scheduler/Cargo.toml` 声明了该路径依赖，并将 Go 包映射记录为 `pkg/dxf/framework/scheduler`。

Windows 专用的 `GetExecCPUNode` 还依赖 Cargo 的目标条件依赖：

- `astersql_dxf_framework_storage::GetDXFSvcTaskMgr` 取得 DXF 服务任务管理器；
- `astersql_dxf_framework_handle::GetTargetScope` 取得当前目标 scope；
- `GetCPUCountOfNodeByRole(ctx, scope)` 执行实际查询；
- `astersql_util_intest::InTest` 与 `astersql_util_cpu::GetCPUCount` 实现测试回退；
- `crate::interface::{Context, Result, SchedulerError}` 承载上下文和错误。

RustCodeGraph 的直接生产证据为：`production_resource.rs::calculate_go_import_resources` 调用 `scheduler::NewRCCalc`，随后调用返回值的 `required_slots`、`max_node_count_for_import_into`，最后调用 `scheduler::CalcDistSQLConcurrency`。`lib.rs` 的再导出使这些调用无需写 `scheduler::autoscaler::...`。

`calc_max_node_count_by_store_count` / `CalcMaxNodeCountByStoresNum` 在当前 Rust 图中没有生产调用者；其输入已是 store 数量，因此 PD 客户端获取应由外层完成。

## 错误处理与边界

纯计算函数没有 `Result`，通过约定返回边界值：

- `max_node_count_by_size` 遇到 `node_cpu <= 0` 返回 0，避免除以零。
- `required_slots` 遇到放大后数据量 `<= 0` 返回 4。这一兼容默认值优先于 CPU 上限，所以非法 CPU 与非正数据量组合也可能返回 4。
- 正数据量的槽位表达式没有显式拒绝非正 CPU、负索引比、负/非有限放大因子；调用方应提供业务有效参数。新增校验会改变 Go 兼容行为，必须先补齐两端测试。
- `calc_dist_sql_concurrency` 对 `max_node_count <= 1` 直接使用 `thread_count × 15`；多节点分支只将步数上限限制为 31，没有验证负线程数或非正 CPU。
- store 数量换算本身不会失败。与之不同，Go 的 `CalcMaxNodeCountByStoresNum(ctx, store)` 还负责类型断言、取得 PD client 和 `GetAllStores`，这些步骤失败会记录警告并返回 0；Rust 函数并未移植这部分 I/O 和错误路径。
- Windows 的 `GetExecCPUNode` 在任务管理器不可用且处于测试环境时回退本机 CPU 数；非测试错误、目标 scope 获取错误和按角色查询错误均转成 `SchedulerError` 返回。非 Windows 构建根本不导出该函数。

整数乘法（例如单节点并发）没有显式饱和处理；极端输入下应考虑 Rust 构建配置对应的溢出行为。浮点乘法后再转整数也意味着精度、非有限数和越界输入不属于当前显式契约。

## 并发与资源生命周期

本文件不持有锁、原子变量、通道、任务、事务、连接或缓存，也不启动线程/异步任务。`ResourceCalc` 的计算只读取自身字段，克隆后可由调用方独立使用；文件本身不提供共享同步保证。

唯一外部资源访问是 Windows 的 `GetExecCPUNode`：任务管理器由 `GetDXFSvcTaskMgr` 获取，函数仅在调用期间借助它完成一次 CPU 查询，不负责初始化、关闭或长期持有管理器。上下文的取消和超时语义取决于 `Context` 与下游 `GetCPUCountOfNodeByRole`，本文件不额外重试。

## 与 Go 版本的对应关系

`pkg/dxf/framework/scheduler/autoscaler.go` 是直接语义基线。Rust 保留了相同的 8 核、200 GiB、25 GiB、30/32 节点和每核 32 并发常量，并用 `f64::round` 对齐 Go 的 `math.Round`。`ResourceCalc` 的有效数据量、两种节点估算、槽位估算和 DistSQL 插值公式均逐项对应；`pkg/dxf/framework/scheduler/autoscaler_test.rs` 的表驱动边界也与 `autoscaler_test.go` 对齐。

已确认的接口差异：

- Go 构造函数返回 `*ResourceCalc`，Rust 返回拥有型 `ResourceCalc`，并克隆 `TuneFactors`。
- Go 方法名为 `CalcMaxNodeCountForAddIndex`、`CalcMaxNodeCountForImportInto`、`CalcRequiredSlots`；Rust 的方法采用 snake_case，但构造函数和两个独立函数保留了 Go 风格兼容入口。
- Go 的 store API 接收 `context.Context` 和 `kv.Storage` 并自行查询 PD；Rust API 接收 `usize store_count`，只保留最终公式。因此二者不是端到端等价接口。
- Go 的默认扫描并发来自 `vardef.DefDistSQLScanConcurrency`；Rust 在本文件固定为 15。若 Go 常量变化，Rust 不会自动同步。
- Go 的 `GetExecCPUNode` 普遍参与 Go 构建；Rust 版本受 `target_os = "windows"` 限制。现有 Rust 测试仅以 `include_str!` 检查所需源码片段，未执行管理器成功/失败路径。

## 扩展指南

- 调整容量公式或基准值时，应同时修改对应常量/方法，并同步 `autoscaler_test.rs` 与 Go 基线 `autoscaler.go`、`autoscaler_test.go`；尤其保留 0 数据量、0 CPU、舍入临界点、上限和超上限用例。
- 新增资源维度时，优先扩展 `ResourceCalc` 和唯一的 `amplified_data_size`/`max_node_count_by_size` 汇合点，避免 add-index 与 import-into 公式分叉；随后检查 `production_resource.rs::calculate_go_import_resources` 的构造参数和 `ResourceParams` 映射。
- 修改 DistSQL 策略时，应覆盖单节点分支、2/5/32/超 32 节点及 8/16/32 核场景，并评估固定常量 15 与 Go `vardef` 的同步风险。
- 若要让 Rust store API 完整对齐 Go，PD 查询、日志和失败返回语义应在合适的 I/O 层实现，不能假设当前纯函数已经包含这些行为。
- 若扩展 `GetExecCPUNode` 到非 Windows，必须先核对目标平台 Cargo 依赖是否可用，并新增独立运行测试；不要只扩大 `cfg` 范围。
- 测试逻辑应继续放在同目录独立文件 `pkg/dxf/framework/scheduler/autoscaler_test.rs`，不要嵌入生产源文件。
- 性能上这些公式均为常数时间；扩展时应避免把 PD/元数据网络访问偷偷引入频繁调用的纯计算方法。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 Rust、Go 和测试文件均已索引。
- RustCodeGraph `files --filter pkg/dxf/framework/scheduler`：确认目标、模块入口、Rust/Go 对照和独立测试的目录关系。
- RustCodeGraph `node --file pkg/dxf/framework/scheduler/autoscaler.rs`：核对全部常量、`ResourceCalc`、方法、包装函数和 Windows 条件编译项。
- RustCodeGraph `callers/callees` 与聚焦 `explore`：确认 `required_slots → amplified_data_size`、`CalcDistSQLConcurrency → calc_dist_sql_concurrency`，以及 `production_resource.rs::calculate_go_import_resources` 的生产调用链；图中未发现 add-index 和 store 换算的 Rust 生产调用者。
- `pkg/dxf/framework/scheduler/lib.rs`：确认模块声明、公共再导出及 `#[cfg(test)] mod autoscaler_test` 的独立测试接线。
- `pkg/dxf/framework/scheduler/Cargo.toml`：确认 crate 名、`lib.rs` 入口、Go 包映射、`schstatus` 直接依赖和 Windows 专用依赖边界。
- `pkg/executor/importer/production_resource.rs`：确认 import-into 资源估算如何消费本文件结果并生成 `ResourceParams`。
- `pkg/dxf/framework/scheduler/autoscaler.go` 与 `autoscaler_test.go`：核对原始公式、PD store 查询、CPU 查询错误语义及表驱动期望。
- `pkg/dxf/framework/scheduler/autoscaler_test.rs`：核对节点数、槽位、索引比、放大因子、DistSQL 插值和 `GetExecCPUNode` 源码契约覆盖；该测试文件没有嵌入生产源文件。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行任务指定的 11 章节结构验证。
