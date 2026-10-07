# `pkg/executor/internal/builder/builder_utils.rs`

## 文件定位

本文件属于 workspace crate `astersql-executor-internal-builder`，crate 入口 `pkg/executor/internal/builder/lib.rs` 将 `builder_utils` 的公共项全部再导出。`pkg/executor/internal/builder/Cargo.toml` 的 `package.metadata.porting.go-package` 指向同目录 Go 包，说明这里是 `builder_utils.go` 的 Rust 移植边界。根 `Cargo.toml` 把它登记为 workspace member 和 facade 依赖，`pkg/lib.rs` 再通过 `facade_executor_internal_builder::*` 暴露其 API；`pkg/executor/Cargo.toml` 与 `pkg/executor/internal/mpp/Cargo.toml` 也声明了路径依赖。

当前接线必须区分“Cargo 已装配”与“生产逻辑已调用”：仓库 Rust 源码搜索只找到 `pkg/lib.rs` 的再导出和本 crate 的独立测试，未找到 Rust 生产代码调用这些构造函数。相反，Go 实现在 `pkg/executor/builder.go`、`pkg/executor/distsql.go` 和 `pkg/executor/internal/mpp/local_mpp_coordinator.go` 中有真实调用。因此本文件目前是可编译、可测试的迁移 API/数据模型，不应被描述成已承接 SQL 执行主链。

## 核心职责

该文件把抽象物理计划转换成存储侧执行器，并组装携带会话元数据的 DAG 请求，职责集中在三层：

1. 通过 `PhysicalPlan::ToPB` 把计划节点转换为 `Executor`，并根据目标选择 `StoreType::TiKV` 或 `StoreType::TiFlash`。
2. 为 TiKV 生成顺序列表，为 TiFlash 生成单根树形表示；对“非自然顺序”列表额外写入 child→parent 的 `ParentIdx`。
3. `ConstructDAGReq` 从 `SessionContext` 复制时区、runtime statistics 开关、下推标志和除法精度增量，并无论计划转换成功与否都调用 `SetEncodeType`，保持 Go 版本的可观察副作用。

文件使用本地 `Executor`、`DAGRequest`、`BuildPBContext` 等轻量类型表达接口。`Cargo.toml` 中计划使用的 distsql、kv、planner、sessionctx 等依赖全部位于 `target.'cfg(any())'.dependencies`；`cfg(any())` 恒为 false，所以当前实现并未直接使用真实下游 crate 类型或真实 protobuf。

## 主要符号

- `DefDivPrecisionIncrement: i32 = 4`：默认除法精度增量；只有会话值不同于该常量时才写入请求。
- `StoreType::{TiKV, TiFlash}`：决定 `ToPB` 的目标和 DAG 的列表/树形分支。
- `Executor { ParentIdx, Payload }`：本地执行器表示。`ParentIdx` 仅用于非自然顺序列表，`Payload` 保存抽象序列化载荷。
- `EncodeType::{TypeDefault, TypeChunk}`：请求编码枚举，默认值为 `TypeDefault`，实际选择委托给 `SessionContext::SetEncodeType`。
- `DAGRequest`：聚合时区、执行摘要、flags、除法精度、树根、执行器列表和编码类型。
- `BuildPBContext`：零大小占位类型；源码注释明确完整迁移后才承载 pushdown 配置。
- `BuilderError(String)`：实现 `Display` 和 `std::error::Error` 的字符串错误包装。
- `PhysicalPlan: Send + Sync`：计划转换边界，唯一方法 `ToPB(&BuildPBContext, StoreType) -> Result<Executor, BuilderError>`。
- `SessionVars`：构造请求需要的最小会话变量集合；`Default` 产生 UTC、零偏移、关闭统计、零 flags 和默认精度增量。
- `SessionContext`：提供 `GetSessionVars`、`GetBuildPBCtx` 与有副作用的 `SetEncodeType`。
- 五个公共构造函数：`ConstructTreeBasedDistExec`、`ConstructListBasedDistExec`、`ConstructListBasedDistExecForUnNatureOrderPlans`、`ConstructDAGReq`、`ConstructDAGReqForUnNatureOrderPlans`。

文件没有条件编译项；测试模块的条件编译在 `lib.rs`，生产类型与函数均为公共 API（经 crate 根再导出）。

## 执行流程

`ConstructTreeBasedDistExec` 只调用一次 `plan.ToPB(context, TiFlash)`，成功后把结果包装成长度为 1 的向量。它是 `ConstructDAGReq` 的 TiFlash 分支基础。

`ConstructListBasedDistExec` 按切片顺序遍历所有计划，以 `TiKV` 调用 `ToPB` 并依次追加结果。任一节点失败时 `?` 立即返回，后续计划不会再转换；独立测试 `list_error_stops_before_later_plans` 对调用序列作了验证。

`ConstructListBasedDistExecForUnNatureOrderPlans` 先复用普通列表构造，再遍历 `HashMap<usize, usize>`，把每个 `(child_index, parent_index)` 写成 `executors[child_index].ParentIdx = Some(parent_index as u32)`。映射外节点保持 `None`。

`ConstructDAGReq` 的顺序是：

1. 读取 `SessionVars`，建立请求并复制时区、offset 和 flags。
2. 仅在 `RuntimeStatsEnabled` 为真时设置 `CollectExecutionSummaries = Some(true)`；false 保持 `None`，不是 `Some(false)`。
3. 仅在除法精度增量不是 4 时把它转换成 `u32` 写入；默认值保持 `None`。
4. TiFlash 分支读取 `plans[0]`、构造唯一执行器并放入 `RootExecutor`；其他存储类型（本枚举当前只有 TiKV）构造列表并写入 `Executors`。
5. 保存构造结果后调用 `context.SetEncodeType(&mut request)`，随后才用 `build_result?` 传播错误。因此计划构造失败时编码设置仍执行一次。

`ConstructDAGReqForUnNatureOrderPlans` 在普通 DAG 成功返回后才设置 `request.Executors` 的父下标；若普通构造失败，不执行映射写入。

## 数据与状态

所有请求数据按值拥有：时区名称被克隆，执行器 payload 是 `Vec<u8>`，列表和树根也归 `DAGRequest` 所有。本文件没有全局可变状态；唯一常量是默认精度增量。

可选字段承担“未显式下发”的语义：runtime statistics 关闭时摘要字段为 `None`，默认除法精度时精度字段为 `None`，普通执行器的父索引也为 `None`。TiFlash 成功请求设置 `RootExecutor` 且列表为空；TiKV 成功请求填充 `Executors` 且根为空。独立测试分别覆盖这两组形态和默认/非默认元数据。

非自然顺序映射约定为 `{child_index => parent_index}`。`HashMap` 的遍历顺序不影响结果，因为每项只覆盖对应 child 的独立字段；若同一 child 只能存在一个键值，这是 `HashMap` 自身保证。

## 依赖与调用关系

内部下游调用边为：

- `ConstructDAGReq` → `SessionContext::{GetSessionVars, GetBuildPBCtx, SetEncodeType}`，并按分支调用 `ConstructTreeBasedDistExec` 或 `ConstructListBasedDistExec`。
- 两种基础构造器 → `PhysicalPlan::ToPB`。
- `ConstructListBasedDistExecForUnNatureOrderPlans` → `ConstructListBasedDistExec`。
- `ConstructDAGReqForUnNatureOrderPlans` → `ConstructDAGReq`。

RustCodeGraph 对限定到本 Rust 文件的调用者查询只识别到 `builder_utils_aster_unit_test.rs` 中的直接测试，以及两个包装函数之间的内部调用；同名 Go 符号也会出现在名称匹配结果中，不能据此宣称跨语言调用。原始代码搜索进一步确认 Rust 生产调用尚不存在。

Go 生产链的直接证据是：`pkg/executor/builder.go` 的 table/index reader 构建路径调用 `ConstructDAGReq`，index lookup 调用非自然顺序版本；`pkg/executor/distsql.go` 为 correlated index side 构造列表；`pkg/executor/internal/mpp/local_mpp_coordinator.go` 为 TiFlash fragment sink 构造树形请求。这些说明移植目标在完整应用中的预期位置，但不是当前 Rust 接线事实。

## 错误处理与边界

计划转换错误原样以 `BuilderError` 返回。列表构造采用 fail-fast：失败节点之前的局部向量被丢弃，失败节点之后不会调用。`ConstructDAGReq` 特意在传播错误前调用 `SetEncodeType`，与 Go 在返回 `err` 前执行 `distsql.SetEncodeType` 的顺序一致。

当前 API 对索引前置条件没有运行时校验：TiFlash 分支直接访问 `plans[0]`，空计划会 panic；两个非自然顺序函数直接索引 `executors[*child_index]`，越界 child 会 panic。`parent_index as u32` 和非默认的 `DivPrecisionIncrement as u32` 都是未经范围检查的转换；过大的 `usize` 会截断，负的 `i32` 会按 Rust `as` 规则转换。Go 对照同样直接索引并转换到 `uint32`，但扩展时应先判断是否必须严格保留该兼容行为。

TiFlash 的非自然顺序包装没有防护：`ConstructDAGReqForUnNatureOrderPlans` 总是修改 `request.Executors`，而 TiFlash 请求的执行器位于 `RootExecutor`，非空映射会越界。因此当前调用契约实际上要求该函数用于 TiKV 列表形态；Go 的实际调用 `buildIndexLookUpPushDownDAGReq` 正是传入 TiKV。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`PhysicalPlan` 要求 `Send + Sync`，使 trait object 具备跨线程共享/传递能力，但这些构造函数本身同步、串行执行，也不保存传入引用。

构造期间的资源生命周期仅限栈上请求和拥有型向量：成功时所有权随返回值移交；错误时已生成的局部执行器随栈展开释放。`SessionContext::SetEncodeType` 接收请求的短期可变借用，context 本身可通过内部可变性产生副作用；测试用 `Arc<Mutex<usize>>` 记录调用次数只是验证手段，不代表生产实现存在锁。

## 与 Go 版本的对应关系

五个 Rust 构造函数逐一对应 `pkg/executor/internal/builder/builder_utils.go` 的同名函数，主要控制流一致：TiFlash 使用单根、TiKV 顺序转换、非自然顺序写 `ParentIdx`、DAG 复制会话元数据，并在结尾设置 encode type。

关键语义对齐包括：列表遇首个错误立即停止；runtime statistics 只有启用时才显式写 true；默认除法精度不下发；`SetEncodeType`/`distsql.SetEncodeType` 即使在计划转换失败时仍执行；非自然顺序映射方向为 child→parent。

当前差异主要来自迁移阶段的数据边界。Go 直接使用 `tipb.Executor`、`tipb.DAGRequest`、planner/sessionctx/distsql 的真实类型，并从 `timeutil.Zone(Location())` 计算时区；Rust 使用本地简化结构，由 `SessionVars` 直接提供时区名称和偏移，`BuildPBContext` 还是占位，`Payload` 也只是字节向量。Go 树形函数即使 `ToPB` 报错也返回包含 `execPB` 的切片，而 Rust `Result` 在错误时不返回执行器；其上层目前只消费错误，因此独立测试只固定了错误与 encode-type 副作用，未证明错误时部分执行器的等价性。

Go 的直接相关测试搜索只找到 `pkg/executor/table_readers_required_rows_test.go` 通过 `buildMockDAGRequest` 间接使用普通 TiKV DAG 构造器；细粒度等价性由独立 Rust 测试 `builder_utils_aster_unit_test.rs` 覆盖。

## 扩展指南

若把该模块接入真实 Rust 执行链，优先替换类型边界而不是改变控制流：让 `BuildPBContext`、物理计划、存储类型和 DAG/Executor 对接真实 planner、kv、sessionctx、distsql 与 tipb 类型，并把当前 `cfg(any())` 下的依赖转为实际依赖。每次接线都应重新搜索 Rust 生产调用，不能仅凭 Cargo 依赖声明判断已接入。

新增会话字段时应在 `SessionVars`、`DAGRequest` 和 `ConstructDAGReq` 三处同步，并在 `builder_utils_aster_unit_test.rs` 增加默认省略与非默认下发两类测试。新增存储类型或编码规则时，应显式重审当前 `if TiFlash { ... } else { ... }`，避免把新类型静默归入 TiKV 分支。

修改非自然顺序逻辑时，需保持 child→parent 方向，并为越界、TiFlash 误用、`usize`→`u32` 范围策略补独立测试；测试逻辑继续放在 `builder_utils_aster_unit_test.rs`，不要嵌入生产源文件。修改错误流程时必须保留或有意更新“构建失败仍调用一次 `SetEncodeType`”的回归测试，并核对 Go 的返回顺序。

兼容风险集中在 optional 字段的“缺省”和显式 false/default 的差异、整数转换，以及错误时副作用顺序；性能风险主要是执行器 payload/时区字符串的拥有型复制和列表预分配是否仍适用于真实 protobuf。当前顺序循环是线性的，贸然并行化会改变 `ToPB` 调用顺序和首错语义。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点，目标 Rust、Go 与独立 Rust 测试均在索引内。
- RustCodeGraph `files --filter pkg/executor/internal/builder`：确认目录内索引到 `builder_utils.rs`、`builder_utils.go`、`builder_utils_aster_unit_test.rs` 和 `lib.rs`。
- RustCodeGraph `node --file`：完整读取 `builder_utils.rs`（1–210 行）、Go 对照（1–105 行）和独立 Rust 测试（1–285 行）。
- RustCodeGraph `query/callers/callees`：核对 `ConstructDAGReq`、`ConstructDAGReqForUnNatureOrderPlans`、`ConstructListBasedDistExecForUnNatureOrderPlans` 的包装边、trait 方法边与测试调用者；结果中的同名 Go 节点按语言分别解释。
- crate/装配证据：`pkg/executor/internal/builder/Cargo.toml`、`lib.rs`、根 `Cargo.toml`、`pkg/lib.rs`、`pkg/executor/Cargo.toml`、`pkg/executor/internal/mpp/Cargo.toml`。
- Go 生产入口证据：`pkg/executor/builder.go`、`pkg/executor/distsql.go`、`pkg/executor/internal/mpp/local_mpp_coordinator.go`。
- 测试证据：`pkg/executor/internal/builder/builder_utils_aster_unit_test.rs` 覆盖存储类型、顺序/首错、父索引、元数据、默认缺省和错误时编码副作用；`pkg/executor/table_readers_required_rows_test.go` 提供 Go 侧间接使用证据。
- 未运行 Cargo 或代码测试：任务是纯文档分析，计划明确禁止运行 Cargo；验证采用源码/调用图事实复核与任务规定的 Markdown 结构检查。
