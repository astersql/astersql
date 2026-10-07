# `pkg/expression/vs_helper.rs`

源文件：[`vs_helper.rs`](./vs_helper.rs)

## 文件定位

本文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs:361-362` 以私有模块 `vs_helper_kernel` 装配。它不计算向量距离，而是把一棵通用 `Expression` 表达式树识别为“受支持的向量距离函数 + 一个 `VECTOR FLOAT32` 列 + 一个 `VECTOR FLOAT32` 常量”，并产出便于规划侧消费的 `VSInfo`。

当前 Rust 接线需要特别区分“模块存在”和“生产路径使用”：RustCodeGraph 对 `InterpretVectorSearchExpr` 的 callers/callees 查询均为空，文本引用搜索也只找到 `pkg/expression/scalar_function_37_aster_unit_test.rs` 的直接调用。Rust 规划器 `pkg/planner/core/operator/physicalop/physical_topn.rs:588` 目前使用自己的 `interpret_vector_search` 实现相同识别流程，并未调用本文件。相对地，Go 生产路径在 `pkg/planner/core/operator/physicalop/physical_topn.go:348` 直接调用 `expression.InterpretVectorSearchExpr`。

## 核心职责

- `vsDistanceFnNamesLower` 建立四个允许识别的函数名集合：`VecL1Distance`、`VecL2Distance`、`VecCosineDistance`、`VecNegativeInnerProduct`。集合元素统一为小写，与 `ScalarFunction.FuncName.L` 的规范化名字匹配（`vs_helper.rs:28-38`）。
- `InterpretVectorSearchExpr` 先确认输入动态类型是 `ScalarFunction`，再确认函数名属于上述集合，最后从参数中找出恰好一个向量列和一个向量常量（`vs_helper.rs:54-88`）。
- 成功时把函数名、builtin 的 protobuf 标量函数码、查询向量和值所属列封装为 `VSInfo`（`vs_helper.rs:101-108`）；失败以 `None` 表示，不产生诊断错误。
- 本 helper 只判断表达式形态，不判断某个具体向量索引是否支持该距离函数；源码在 `VSInfo` 注释中明确要求调用方继续检查 `DistanceFnName`（`vs_helper.rs:40-42`）。

## 主要符号

- `static vsDistanceFnNamesLower: LazyLock<HashSet<String>>`：首次访问时构造、此后只读的允许函数名集合。使用 `LazyLock` 避免可变全局状态，同时保持 Go 包级初始化集合的效果。
- `pub struct VSInfo<'a>`：解释结果。`DistanceFnName: ast::CIStr` 保留原函数名对象；`FnPbCode: tipb::ScalarFuncSig` 是下推/执行所需的 protobuf 枚举；`Vec: types::VectorFloat32` 是查询向量；`Column: &'a Column` 借用原表达式树中的列节点（`vs_helper.rs:43-49`）。
- `pub fn InterpretVectorSearchExpr(expr: &dyn Expression) -> Option<VSInfo<'_>>`：唯一入口。虽然类型和函数声明为 `pub`，其父模块 `vs_helper_kernel` 在 `lib.rs` 中是私有且没有 `pub use`，因此当前 API 实际限于 crate 内部。
- 文件没有 trait、`impl`、条件编译项或可变模块状态；公开数据字段主要服务于调用方继续构造规划属性。

## 执行流程

1. 通过 `Expression::as_any()` 将输入下转为 `ScalarFunction`；列、常量或其他表达式节点立即返回 `None`（`vs_helper.rs:54-56`）。
2. 用 `x.FuncName.L` 查询惰性集合；不属于四种距离函数的标量函数返回 `None`（`vs_helper.rs:58-60`）。
3. 通过 `ScalarFunction::GetArgs()` 遍历 builtin 的参数。遇到 `Column` 或 `Constant` 时，要求其 `RetType` 存在且 `GetType()` 为 `mysql::TypeTiDBVectorFloat32`，否则立即返回 `None`（`vs_helper.rs:62-83`；`GetArgs` 定义于 `scalar_function.rs:145`）。
4. 分别记录最后一个匹配的列/常量并计数；遍历后要求计数严格为一对一，否则返回 `None`（`vs_helper.rs:64-88`）。其他动态类型的参数不会被计数，也不会单独导致失败；正常 builtin 的参数个数约束由上游函数构造保证，本 helper 自身没有显式检查 `args.len() == 2`。
5. 对常量 `Datum` 的实际 kind 执行 `intest::Assert`，预期必须是 `KindVectorFloat32`（`vs_helper.rs:90-99`）。这项内部一致性检查位于字段类型检查之后。
6. 调用 builtin 的 `PbCode()` 取得整数码，再用 `tipb::ScalarFuncSig::from_i32` 转为枚举；未知码返回 `None`（`vs_helper.rs:101`）。
7. 返回 `VSInfo`：函数名被 clone，向量由 `Datum::GetVectorFloat32()` 取得，列保持对原表达式节点的共享借用（`vs_helper.rs:103-108`）。参数次序不影响识别，`column, constant` 与 `constant, column` 都可通过。

## 数据与状态

`vsDistanceFnNamesLower` 是进程级只读缓存，只在第一次查询时分配四个小写 `String` 和 `HashSet`。其内容在初始化后不会变化，因而不存在运行期注册或按会话变化的函数集合。

`VSInfo<'a>` 混合了拥有型和借用型数据：函数名与向量值随结果持有，`Column` 则受输入表达式生命周期 `'a` 约束。调用方不能让 `VSInfo` 中的列引用活得比原 `ScalarFunction` 更久，也不能通过该共享引用修改表达式树。这与 Go 的 `*Column` 指针表达同一“指向原树节点”的意图，但 Rust 在类型层面强制生命周期和只读借用。

函数本身不缓存解释结果，也不读取会话、事务、统计信息或存储状态。每次调用会线性扫描参数，时间复杂度为 `O(n)`（`n` 为 builtin 参数数）；固定函数集合查询平均为 `O(1)`。对正常的二元距离函数，扫描成本是常数级。

## 依赖与调用关系

- crate 内依赖通过 `use crate::*` 获得 `Expression`、`ScalarFunction`、`Column`、`Constant`、`ast`、`mysql`、`types`、`tipb` 与 `intest`；`protobuf::ProtobufEnum` 提供 `ScalarFuncSig::from_i32`（`vs_helper.rs:20-24`）。
- `pkg/expression/Cargo.toml` 声明该 crate、`protobuf = "=2.8.0"`、Git `tipb` 依赖，以及 parser/types/intest 等内部路径依赖；本文件没有独立 feature gate。
- Rust 模块入口是 `pkg/expression/lib.rs:361-362`。直接 Rust 测试入口是 `pkg/expression/scalar_function_37_aster_unit_test.rs:306-330`，测试模块由 `lib.rs:698-700` 在 `#[cfg(test)]` 下装配。
- 当前 Rust 生产规划链是 `PhysicalTopN` 候选生成 → `physical_topn.rs::interpret_vector_search` → `property::VectorSearchInfo` → `VectorProperty.VSInfo`（`physical_topn.rs:480-501,588-629`），与本 helper 并行而非调用本 helper。
- Go 生产规划链是 `PhysicalTopN` 候选生成 → `expression.InterpretVectorSearchExpr` → `property.VectorProperty.VSInfo`（`physical_topn.go:342-370`）。这条 Go 调用边说明了本文件设计上的应用位置，但不能作为 Rust 已接线的证据。

## 错误处理与边界

该 API 用 `Option` 表达“不符合向量检索形态”，没有 `Result`，也不区分具体失败原因。以下情况返回 `None`：输入不是 `ScalarFunction`；函数名不在白名单；列/常量缺少 `RetType`；列或常量的字段类型不是 `VECTOR FLOAT32`；匹配到的列或常量数量不是各一个；缺失最终记录；protobuf 整数码不是有效的 `ScalarFuncSig`。

需要注意三类边界：

- 未识别为 `Column`/`Constant` 的额外参数会被忽略。因而“恰好一个列和一个常量”是对这两类节点的计数约束，不是 helper 自己对总参数数的约束；上游距离函数工厂通常负责二元 arity。
- `RetType` 只验证 SQL 字段类型。常量 `Datum` 的实际 kind 由 `intest::Assert` 检查；这里没有把 kind 不一致转换为 `None` 的显式分支，因此扩展或构造测试表达式时必须保持 `RetType` 与 `Value.Kind()` 一致。
- 四种函数均可被识别，但“可被识别”不等于“可被具体索引实现”；调用方仍需按函数名、索引元数据和排序方向做兼容性检查。

## 并发与资源生命周期

`LazyLock<HashSet<String>>` 由标准库保证线程安全的一次性初始化，初始化后只执行并发只读查询。函数没有锁、异步任务、通道、文件句柄、网络连接或事务资源，也没有需要显式清理的对象。

返回结果不延长整棵表达式树的所有权：`Column` 仅被借用，借用期由编译器绑定到 `expr`；函数名和向量值进入结果后按普通 Rust 所有权释放。若未来要把结果跨线程、放入长生命周期规划缓存或存入拥有型物理属性，应像当前 `physical_topn.rs::interpret_vector_search` 那样显式 clone 列并选择合适的共享所有权，而不能直接保存本结构中的借用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/vs_helper.go`。Rust 保留了 Go 的四函数白名单、动态类型判断、字段类型检查、列/常量计数、`intest.Assert`、protobuf 码和返回字段，参数顺序无关的语义也一致。

主要语言映射如下：Go 的包级 `map[string]struct{}` 对应 Rust 的 `LazyLock<HashSet<String>>`；Go 的 `nil` 失败返回对应 `Option::None`；Go 的类型断言对应 `Any` 下转；Go 的 `*Column` 对应 `&'a Column`。Rust 还对 `RetType` 缺失和无效 protobuf 数值安全返回 `None`，而 Go 代码直接解引用 `RetType` 并直接使用强类型 `PbCode()` 返回值。

迁移接线尚未完全对齐：Go `physical_topn.go` 直接使用该 helper；Rust `physical_topn.rs` 使用局部的拥有型版本。局部版本额外把列 clone、把向量包进 `Arc`，以适配 `property::VectorSearchInfo` 的拥有型字段。若未来消除重复实现，需要先决定本 helper 是继续返回借用结果，还是新增一个明确的拥有型转换边界，而不是简单替换调用。

测试对应关系也不完全对称：仓库内未发现直接调用 Go `InterpretVectorSearchExpr` 的同名单元测试；Rust 在 `scalar_function_37_aster_unit_test.rs:306-330` 有一个正向用例。Go 的端到端行为主要由 `pkg/planner/core/casetest/vectorsearch/vector_index_test.go` 覆盖，Rust 对应规划测试位于同目录的 `vector_index_test.rs`，但当前走的是规划器局部 helper。

## 扩展指南

- 新增可识别距离函数时，至少同步修改 `vsDistanceFnNamesLower`、Go `vsDistanceFnNamesLower`，并确认该函数的 builtin `PbCode()`、索引能力与 planner 白名单一致；当前 Rust `physical_topn.rs::interpret_vector_search` 也有独立白名单，必须同步或先统一实现。
- 改变可接受表达式形态时，应在独立测试 `pkg/expression/scalar_function_37_aster_unit_test.rs` 增加正反例，至少覆盖：非标量表达式、未知函数、非向量列/常量、重复列、重复常量、参数换序、缺失 `RetType`、无效 protobuf 码，以及额外非列/常量参数的既定策略。不要把测试嵌入 `vs_helper.rs`。
- 若要让外部 crate 调用，应同时评估 `lib.rs` 的模块可见性/再导出和 `VSInfo<'a>` 的生命周期 API；仅把函数标为 `pub` 并不足以形成公开 crate API。
- 若要替换规划器的局部实现，应增加或复用明确的 `VSInfo` → `property::VectorSearchInfo` 转换，处理 `Column` clone、向量共享所有权与函数名 `CIStr`/`String` 差异，并运行 expression 的独立单测及 vectorsearch 规划测试。
- 兼容风险集中在函数白名单与 Go/Rust 漂移；正确性风险集中在类型/kind 不一致和额外参数策略；性能风险较低，但不要在热路径反复构造白名单或无必要深拷贝大向量。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件，其中 Rust 文件 7,032 个；`files --filter pkg/expression/vs_helper.rs` 报告目标文件有 3 个符号。
- RustCodeGraph 符号证据：`query VSInfo` 与 `query InterpretVectorSearchExpr` 同时定位 Rust/Go 定义；`node pkg/expression/vs_helper.rs::InterpretVectorSearchExpr` 展示完整入口；对该 Rust 定义执行 `callers --file pkg/expression/vs_helper.rs` 与 `callees --file ...` 均返回空数组。
- 已读源码与配置：`pkg/expression/vs_helper.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`、`pkg/expression/expression.rs`、`pkg/expression/scalar_function.rs`、`pkg/expression/constant.rs`、`pkg/expression/column.rs`。
- 已读 Go 对照与规划接线：`pkg/expression/vs_helper.go`、`pkg/planner/core/operator/physicalop/physical_topn.go`。
- 已读 Rust 生产对照：`pkg/planner/core/operator/physicalop/physical_topn.rs`、`pkg/planner/property/physical_property.rs`；文本引用搜索确认生产 Rust 规划器使用局部 `interpret_vector_search`。
- 已读测试：`pkg/expression/scalar_function_37_aster_unit_test.rs` 的直接正向单测；并定位 `pkg/planner/core/casetest/vectorsearch/vector_index_test.go` 与 `vector_index_test.rs` 作为 Go/Rust 规划级覆盖面。未运行 Cargo，符合本纯文档任务约束。
