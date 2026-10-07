# `pkg/expression/aggregation/agg_to_pb.rs` 逻辑说明

## 文件定位

`agg_to_pb.rs` 位于 `astersql-expression-aggregation` crate 内，是规划器聚合描述 `AggFuncDesc` 与 TiKV/TiFlash 下推协议 `tipb::Expr` 之间的转换边界。crate 由 `pkg/expression/aggregation/Cargo.toml` 定义，`lib.rs` 通过 `mod agg_to_pb` 装配并以 `pub use agg_to_pb::*` 导出本文件 API；其 Rust 生产调用主线之一是 `pkg/planner/core/operator/physicalop/physical_hash_agg.rs::to_pb`，该入口将每个物理聚合函数交给 `AggFuncToPBExpr`，再写入 `tipb::Aggregation.agg_func`。

本文件不执行聚合，也不发送 RPC。它只负责协议类型选择、表达式子树转换、字段类型编码以及聚合元数据的编解码。真正的聚合状态与更新逻辑位于同 crate 的 `aggregation.rs` 及各具体聚合实现中。

## 核心职责

- `baseFuncDesc::GetTiPBExpr`：把 AST 聚合/窗口函数名映射为存储端认识的 `tipb::ExprType`。
- `AggFuncToPBExpr`：把规划器侧 `AggFuncDesc` 序列化成可下推的 PB 表达式，携带参数、返回类型、`DISTINCT`、分布式聚合模式，以及 `GROUP_CONCAT` 专属字段。
- `AggFunctionModeToPB` / `PBAggFuncModeToAggFuncMode`：在本地 `AggFunctionMode` 与 TiPB 枚举之间双向映射。
- `PBExprToAggFuncDesc`：把 TiPB 聚合表达式重建为本地描述符，供协议反解和执行侧构建使用。

职责边界并非完全对称：名称的正向映射支持方差、标准差和 JSON 聚合等更多类型，而 `PBExprToAggFuncDesc` 当前只接受 `Count`、`ApproxCountDistinct`、`First`、`GroupConcat`、`Max/Min`、`MaxCount/MinCount`、`Sum/SumInt`、`Avg` 和三种位聚合。扩展者不能因为 `GetTiPBExpr` 支持某类型就推断反向路径也已支持。

## 主要符号

- `baseFuncDesc::GetTiPBExpr(try_window_desc)`：先查询聚合名称；命中即返回。只有结果为 `Null` 且 `try_window_desc` 为 `true` 时，才继续查询窗口函数名称。聚合下推调用它时传 `false`，`window_func.rs::WindowFuncToPBExpr` 传 `true`。
- `AggFuncToPBExpr(ctx, aggregate, store_type) -> Result<tipb::Expr, Error>`：本文件主要正向入口。依赖 `PushDownContext` 中的求值上下文、客户端和 `GROUP_CONCAT` 上限。
- `AggFunctionModeToPB(mode) -> tipb::AggFunctionMode`：穷举映射 `Complete/Final/Partial1/Partial2/Dedup`，不存在静默兜底。
- `PBAggFuncModeToAggFuncMode(mode: Option<_>) -> AggFunctionMode`：逆向穷举；PB 字段缺失时按 Go 约定返回 `Partial1Mode`。
- `PBExprToAggFuncDesc(ctx, aggregate, field_types) -> Result<AggFuncDesc, Error>`：主要反向入口，恢复名称、参数、返回类型和模式；明确把 `HasDistinct` 设为 `false`，并将 `OrderByItems` 清空、`GroupingID` 置零。

本文件没有模块级常量、独立类型、trait 或条件编译项。所有函数均通过 `lib.rs` 的通配再导出成为 crate 公共 API；`GetTiPBExpr` 是公开 inherent method。

## 执行流程

正向转换 `AggFuncToPBExpr` 的顺序是：

1. 从 `PushDownContext::PbConverter` 取得表达式转换器，以 `GetTiPBExpr(false)` 选择 PB 子类型。
2. 调用 `Client::IsRequestTypeSupported(ReqTypeSelect, expr_type)`；客户端不支持则立即返回错误。
3. 逐个调用 `PbConverter::ExprToPB` 转换 `AggFuncDesc.Args`。任一参数返回 `None`，整次聚合转换失败，不产生部分 PB。
4. 对已推断的 `RetTp` 调用 `expression::ToPBFieldTypeWithCheck`；该检查会按 `store_type` 约束字段类型，例如 TiFlash 拒绝非法 decimal 精度/小数位。
5. 设置 PB 的 `tp`、`children`、`field_type`、`has_distinct` 和 `agg_func_mode`。
6. 若类型是 `GroupConcat`，再逐项用 `SortByItemToPB` 转换 `OrderByItems`，并用 `codec::EncodeUint` 把 `ctx.GetGroupConcatMaxLen()` 写入 `val`。

反向转换 `PBExprToAggFuncDesc` 的顺序是：先校验并映射 `ExprType`，再由 `expression::PBToExprs` 递归反解子表达式；随后通过 `FieldTypeFromPB` 恢复返回类型，调用 `baseFuncDesc::WrapCastForAggArgs` 补齐聚合参数所需 cast，最后恢复模式并构造 `AggFuncDesc`。该路径不会恢复 `DISTINCT`、`GROUP_CONCAT ORDER BY` 或 `GroupingID`。

## 数据与状态

输入状态来自 `AggFuncDesc`（定义于 `descriptor.rs`）：其内嵌 `baseFuncDesc` 保存 `Name`、`Args`、可选 `RetTp`，外层保存 `Mode`、`HasDistinct`、`OrderByItems` 和 `GroupingID`。本文件只读取输入，构造新的 PB 或新的描述符，不原地修改调用方对象。

协议输出 `tipb::Expr` 中的重要对应关系是：`Name -> tp`、`Args -> children`、`RetTp -> field_type`、`HasDistinct -> has_distinct`、`Mode -> agg_func_mode`。只有 `GROUP_CONCAT` 使用 `order_by` 和 `val`；其中 `val` 是无符号整数编码的最大结果长度，不是聚合数据本身。

`PushDownContext`（`pkg/expression/pushdown_context.rs`）以 `Arc` 持有构建上下文和客户端，但转换函数仅借用它；本文件没有缓存、全局变量或跨调用可变状态。反向路径的 `field_types` 是列引用解码的类型表，由 `PBToExprs` 使用。

## 依赖与调用关系

上游调用关系：

- `physical_hash_agg.rs::to_pb -> aggregation::AggFuncToPBExpr`：Rust 物理 HashAgg 生成 TiPB executor 的生产主链；转换出的函数列表写入 `tipb::Aggregation`。
- `window_func.rs::WindowFuncToPBExpr -> baseFuncDesc::GetTiPBExpr(true)`：窗口函数下推复用名称映射的窗口回退分支。
- `aggregation.rs::NewDistAggFunc -> PBAggFuncModeToAggFuncMode`：分布式运行时聚合构造恢复协议模式。
- `go_merge_44_test.rs` 同时覆盖 `GetTiPBExpr`、正向转换、`PBExprToAggFuncDesc` 和分布式聚合构造的串联，但它是测试证据，不应描述为生产入口。

下游依赖包括：`expression::PbConverter::ExprToPB`、`ToPBFieldTypeWithCheck`、`SortByItemToPB`、`PBToExprs`、`FieldTypeFromPB`，`kv::Client::IsRequestTypeSupported`，`codec::EncodeUint`，以及 `base_func.rs::WrapCastForAggArgs`。`Cargo.toml` 对应声明了 `expression`、`kv`、`codec`、`planner-util`、parser AST/MySQL、本地类型组件和带 `protobuf-codec` feature 的 `tipb` Git 依赖；本文件本身没有 feature gate。

RustCodeGraph 对 `AggFuncToPBExpr` 的节点 Trail 识别到 `GetTiPBExpr`、`AggFunctionModeToPB`、`GetGroupConcatMaxLen` 调用边；对 `PBExprToAggFuncDesc` 识别到 `PBAggFuncModeToAggFuncMode`、`baseFuncDesc` 构造和 `WrapCastForAggArgs`。对 trait 方法和通过 crate 再导出的跨 crate 边，本文再以精确源码引用补足。

## 错误处理与边界

- 客户端不支持选定的聚合 PB 子类型时，`AggFuncToPBExpr` 返回 `select request is not supported by client`。
- 任一普通参数或 `GROUP_CONCAT ORDER BY` 表达式不可转 PB 时，返回包含聚合格式化文本的错误；转换采用全有或全无语义。
- `ToPBFieldTypeWithCheck` 的存储类型校验错误以 `?` 原样传播；TiFlash 非法 decimal 是已知分支。
- `RetTp == None` 会触发 `expect("aggregate return type must be inferred")`，不是可恢复错误。这是调用前必须完成类型推断的不变量。
- `PushDownContext` 未注入客户端时，`ctx.Client()` 会 panic；生产调用者 `physical_hash_agg.rs::to_pb` 在构造上下文前先把缺失客户端转换为普通错误。
- 未知函数名由 `GetTiPBExpr` 映射为 `tipb::ExprType::Null`；之后是否被客户端能力检查拒绝取决于客户端实现，因此新增名称时应显式补映射，不能依赖 `Null`。
- `PBExprToAggFuncDesc` 对不在反向白名单内的 `ExprType` 返回 `unknown aggregation function type`；参数反解错误（非法载荷、列偏移越界等）由 `PBToExprs` 传播。
- PB 未携带聚合模式时默认 `Partial1Mode`；反向路径故意忽略 PB 的 `has_distinct`，这是与 Go 一致的当前契约，而不是字段遗漏。

## 并发与资源生命周期

本文件是同步、纯构建式代码：不创建线程、任务、通道、锁、事务、文件或网络连接，也不调用 `Client::Send`。`AggFuncToPBExpr` 只进行客户端能力查询；测试客户端的 `Send` 会 panic，用来证明转换不能产生 RPC。

所有输出对象由调用者取得所有权；参数、上下文和字段类型表均只借用到函数返回。`PushDownContext` 内部 `Arc` 的共享生命周期由上下文自身管理，转换期间不克隆客户端。临时 `Vec` 在任一 `collect::<Option<Vec<_>>>()` 失败时随错误路径释放；成功后所有权转入 PB repeated 字段。

性能成本与参数数量及 `ORDER BY` 项数量线性相关，主要成本是递归表达式转换和新 PB 节点分配。这里没有批间缓存；新增昂贵逻辑时应避免对每个参数重复遍历整个描述符或复制大载荷。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/aggregation/agg_to_pb.go`，Rust 保留了 Go 的五个对应符号及主分支：名称映射、客户端能力检查、子表达式转换、返回类型检查、`GROUP_CONCAT` 附加字段、模式双向映射和 PB 反解。

可见的语言表达差异包括：Go `AggFunctionModeToPB` 返回枚举指针，Rust 返回枚举值交给 protobuf setter；Go 以 `nil` 表示缺失模式，Rust 以 `Option` 表示；Go 循环遇到 `nil` 子表达式返回错误，Rust 用 `collect::<Option<Vec<_>>>()` 实现同样的全有或全无行为；Go 的 PB 描述符字段是指针，Rust generated API 通过 `get_*`/`set_*` 访问。

语义上需特别保留三点：模式字段缺失默认 `Partial1Mode`；`PBExprToAggFuncDesc` 不恢复 `HasDistinct`；`GROUP_CONCAT` 的最大长度编码进 `val`。Rust 独立测试 `agg_to_pb_test.rs` 对前两类正向行为和 DISTINCT 反解约定提供回归证据，Go 测试 `agg_to_pb_test.go` 还覆盖 `MaxCount/MinCount` 的正向映射；Rust 的对应补充覆盖位于 `go_merge_44_test.rs`。

## 扩展指南

新增一种聚合协议类型时，至少应按数据流检查：AST 名称常量、`GetTiPBExpr` 正向映射、`PBExprToAggFuncDesc` 是否需要反向映射、`aggregation.rs::NewDistAggFunc` 是否存在执行实现、模式和返回类型是否能被存储端接受。若只支持下推而不支持协议反解，应在设计和测试中明确该非对称性。

新增 `GROUP_CONCAT` 类专属元数据时，应在 `AggFuncDesc` 字段、正向 PB 编码、必要的反向恢复以及 Go 对照之间同步，而不能只添加 setter。修改模式枚举时，必须同时更新 `aggregation.rs::AggFunctionMode`、两个模式转换函数和 `aggregation_aster_unit_test.rs` 的往返/缺省测试。

测试应继续放在独立文件：优先扩展 `agg_to_pb_test.rs` 验证普通正向转换、失败边界与 PB 反解；Go 新增行为对齐可扩展 `go_merge_44_test.rs`；若涉及规划器接线，再覆盖 `physical_hash_agg.rs` 所在模块的独立测试。重点兼容风险是 TiPB 枚举值和缺省模式，正确性风险是正反向类型集合漂移、cast 未补齐或 `DISTINCT` 契约被误改，性能风险是参数树重复转换和大 `ORDER BY` 列表分配。

## 验证依据

- 目标源码：`pkg/expression/aggregation/agg_to_pb.rs`；确认 1 个公开 inherent method 和 4 个公开自由函数，无条件编译项。
- crate 与模块边界：`pkg/expression/aggregation/Cargo.toml`、`pkg/expression/aggregation/lib.rs`。
- 数据结构与运行时：`base_func.rs::baseFuncDesc`、`descriptor.rs::AggFuncDesc`、`aggregation.rs::AggFunctionMode`/`NewDistAggFunc`。
- 生产调用与依赖：`pkg/planner/core/operator/physicalop/physical_hash_agg.rs::to_pb`、`pkg/expression/aggregation/window_func.rs::WindowFuncToPBExpr`、`pkg/expression/pushdown_context.rs`、`pkg/expression/expr_to_pb.rs`、`pkg/expression/pb_to_expr_runtime.rs`。
- Go 对照：`pkg/expression/aggregation/agg_to_pb.go`。
- 测试证据：`pkg/expression/aggregation/agg_to_pb_test.rs`、`pkg/expression/aggregation/agg_to_pb_test.go`、`pkg/expression/aggregation/go_merge_44_test.rs`、`pkg/expression/aggregation/aggregation_aster_unit_test.rs`。
- RustCodeGraph 查询：`status`、`files --filter pkg/expression/aggregation`、`query agg_to_pb --json`、`node agg_to_pb.rs::AggFuncToPBExpr`、`callees` 查询；独立 `callers` 查询在当前索引上未及时返回，因此跨 crate 调用者由精确引用搜索核实。
- 本任务为纯文档分析，按任务约束未运行 Cargo；最终以固定 11 个二级标题的结构命令验证。
