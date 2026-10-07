# `pkg/expression/expr_to_pb.rs`

## 文件定位

`expr_to_pb.rs` 是 `astersql-expression` crate 中的“表达式→TiPB”边界适配层。`pkg/expression/lib.rs` 将它声明为私有模块 `expr_to_pb_kernel`，再通过 `pub use expr_to_pb_kernel::*` 对外暴露公开 API。该文件把 root 端的 `Expression` 树、`FieldType` 以及 GROUP BY / ORDER BY 项编码为 `tipb` 协议对象，供 planner 构造 TiKV/TiFlash DAG 请求；它只在内存中构造消息，不发送 RPC。

crate 边界由 `pkg/expression/Cargo.toml` 定义：库入口为 `lib.rs`，本文件直接依赖 `protobuf` 2.8 的枚举转换、Git 固定 revision 的 `tipb`，并通过 crate 内再导出使用 `astersql-kv`、`astersql-types`、codec、collate 和下推黑名单实现。直接独立 Rust 测试在 `pkg/expression/expr_to_pb_test.rs`，由 `lib.rs` 中的 `#[cfg(test)]` 模块声明接入，没有把测试嵌入生产文件。

## 核心职责

1. `ExpressionsToPBList` 和 `ProjectionExpressionsToPBList` 批量转换表达式，将“不能下推”从 `Option` 升格为包含表达式文本的 `Error`。Projection 入口对顶层 `Column` 刻意跳过类型限制，因为直接投影列本身不表示计算。
2. `PbConverter::ExprToPB` 是单个表达式的类型分派中心，当前只接受 `Constant`、`CorrelatedColumn`、`Column` 和 `ScalarFunction`；其他 `Expression` 实现会返回 `None`。
3. `encodeDatum`、`columnToPBExpr` 和 `scalarFuncToPBExpr` 分别负责字面量 codec 编码、列引用的新旧协议选择，以及标量函数签名/子节点/元数据/返回类型的递归编码。
4. `IsPushDownEnabled` 和 `canFuncBePushed` 把存储类型映射到共享下推推断模块，同时检查函数名与“函数名.协议签名”两级黑名单键。
5. `ToPBFieldType`、`ToPBFieldTypeWithCheck` 和 `FieldTypeFromPB` 处理类型元数据的双向转换，并在 TiFlash 边界拒绝非法 Decimal。`GroupByItemToPB` / `SortByItemToPB` 则将已编码表达式包装为 `tipb::ByItem`。

## 主要符号

- `ExpressionsToPBList(ctx, exprs, client) -> Result<Vec<tipb::Expr>, Error>`：逐个调用 `ExprToPB`，任一元素失败即返回内部错误，不产生部分结果。
- `ProjectionExpressionsToPBList(...)`：与上者相同，但顶层 `Column` 调用 `columnToPBExpr(column, false)`；列位于标量函数内部时仍走常规类型检查。
- `PbConverter<'a> { client, ctx }`：借用 KV 能力查询接口与求值上下文，本身不拥有二者；`NewPBConverter` 只组装这两个引用。
- `PbConverter::ExprToPB`：按运行时类型下转分派；它也是标量函数子节点的递归入口。
- `conOrCorColToPBExpr` / `encodeDatum`：先在 root 用空 `chunk::Row` 求值常量或关联列，再把 `Datum::Kind` 映射到 `tipb::ExprType` 和字节。支持 Null、有/无符号整数、字符串/二进制字面量、Bytes、Float32/64、Duration、Decimal、Time、Enum 和 VectorFloat32。
- `columnToPBExpr(column, check_type)`：检查 `ColumnRef` 能力和类型开关；支持 DAG basic 的新协议编码 `column.Index` 并携带 `FieldType`，否则用 `column.ID` 的旧协议。
- `scalarFuncToPBExpr`：要求 `PbCode > Unspecified`、两级黑名单通过、每个参数都可递归编码，并要求 `RetType` 存在；最终写入 `ScalarFunc`、已序列化 metadata、签名、children 和返回类型。
- `pushdownStoreType` / `IsPushDownEnabled` / `canFuncBePushed`：内部存储枚举适配与下推黑名单入口。
- `ToPBFieldType` / `FieldTypeFromPB`：往返复制 type、flag、flen、decimal、charset、collation 和 elems；`ToPBFieldTypeWithCheck` 在此之前增加 TiFlash Decimal 有效性检查。
- `GroupByItemToPB` / `SortByItemToPB`：返回 `Option<tipb::ByItem>`，表达式不能编码时传播 `None`；后者额外保留 `desc`。

文件没有模块级常量、自定义 enum/trait 或条件编译分支。唯一自定义类型是 `PbConverter<'a>`。

## 执行流程

1. planner 的物理算子 `ToPB` 获取 `EvalContext` 和 `kv::Client`，然后调用批量入口或 `SortByItemToPB`。例如 `physical_projection.rs::ToPB` 使用 Projection 特例，`physical_hash_join.rs::ToPB` 编码 join keys/conditions，`physical_topn.rs::ToPB` 编码 order/partition items。
2. 批量入口构造一个 `PbConverter`，预分配结果 `Vec`，对每棵表达式调用 `ExprToPB`。
3. `ExprToPB` 按类型进入三条路径：
   - 常量/关联列：`Eval` 取得 `Datum` → `encodeDatum` 选择协议类型和 codec → 客户端检查该字面量类型能力 → 填入 `FieldType`。
   - 列：检查 `ColumnRef` 能力 → 可选检查 Bit/Set/Geometry/Unspecified/Enum 限制 → 根据 DAG basic 能力选择 Index+FieldType 或 ID 编码。
   - 标量函数：检查签名 → 检查下推黑名单 → 递归编码所有参数 → 获取已编码 metadata → 必要时把派生 collation 写回返回类型副本 → 构造 `tipb::Expr`。
4. 任一可选步骤失败都使单表达式返回 `None`。批量 API 将它转为 `Error`；`GroupByItemToPB` / `SortByItemToPB` 保留 `Option`，由算子上层补充场景错误。
5. 成功的协议对象被算子放入 `tipb::Executor`/DAG 结构，真正的请求发送发生在本文件之外。

## 数据与状态

`PbConverter` 只保存两个生命周期受限的共享引用：`&dyn kv::Client` 用于 `IsRequestTypeSupported`，`&dyn EvalContext` 用于求值、时区、错误上下文和类型/排序规则。转换器内无可变缓存、全局注册表或请求计数器；每次调用新建 `tipb` 值和所需 `Vec<u8>`/children `Vec`。

主要不变量是：

- 新协议的 `ColumnRef.val` 是列在输入 schema 中的 `Index`，且必须携带 `FieldType`；旧协议的 `val` 是真实列 `ID`，`0` 和 `-1` 不可下推。
- Float32/Float64 的 `-0.0` 不能使用会归一化符号位的 mem-comparable 编码，必须留在 root；`+0.0` 可以编码。
- Decimal 字面量的精度/小数位由 `MyDecimal` 自身参与 codec 编码，不用输出 schema 的值替代。
- `ToPBFieldType` 和 `FieldTypeFromPB` 保留七类协议元数据；collation 必须经 `CollationToProto` / `ProtoToCollation` 转换，不是直接拷贝字符串。
- ScalarFunction 必须“整棵子树可下推”：任一 child 失败就不会生成部分协议树。

## 依赖与调用关系

上游直接调用者包括：

- `pkg/expression/pushdown_context.rs::PushDownContext::PbConverter` 从 owned 下推上下文构造本文件的借用转换器。
- `pkg/planner/core/operator/physicalop/physical_projection.rs::ToPB` 调用 `ProjectionExpressionsToPBList`。
- `physical_hash_join.rs`、`physical_hash_agg.rs`、`physical_selection.rs`、`physical_table_scan.rs`、`physical_exchange_sender.rs` 等调用 `ExpressionsToPBList` 或类型转换 API。
- `physical_topn.rs`、`physical_sort.rs`、`physical_limit.rs`、`physical_window.rs` 调用 `SortByItemToPB`。
- `pkg/expression/aggregation/agg_to_pb.rs` 复用 `ExprToPB`、`SortByItemToPB`、`ToPBFieldTypeWithCheck` 和 `FieldTypeFromPB`；`aggregation.rs` / `window_func.rs` 复用下推开关。
- `pkg/expression/grouping_sets.rs` 直接调用私有模块的 `ExpressionsToPBList`；`pkg/session/runtime/relational_scan.rs` 通过 crate 公开再导出调用 `ToPBFieldType`。

下游依赖是 `Expression::{as_any, Eval, GetType, StringWithCtx, ExplainInfo}`、`ScalarFunction` / `builtinFunc` 的 `PbCode` 与 `metadata`、`kv::Client::IsRequestTypeSupported`、`types::Datum` / `FieldType`、`codec` 数值编码、`collate` 协议排序规则转换、`infer_pushdown_kernel` 黑名单查询，以及 `tipb::{Expr, FieldType, ByItem, ExprType, ScalarFuncSig}`。RustCodeGraph 已证实内部主边 `ExpressionsToPBList → NewPBConverter → ExprToPB`、`ExprToPB → conOrCorColToPBExpr/columnToPBExpr/scalarFuncToPBExpr` 和 `scalarFuncToPBExpr → ExprToPB/canFuncBePushed/ToPBFieldType`。图对跨 crate planner 调用返回不完整，上述跨模块边因此由 `rg` 的真实调用点补证。

## 错误处理与边界

- `ExprToPB` 把不可下推统一表示为 `None`，包括未知表达式类型、不支持的 Datum kind、客户端能力不足、黑名单命中、无效签名、缺失 `RetType`或任一 child 失败。这不是协议传输错误，而是“应留在 root”的可能性判定。
- 常量/关联列求值失败、Decimal codec 失败或未被上下文吸收的 MySQL Time 编码错误会写入后台日志，然后返回 `None`。Time 错误先经 `errCtx(ctx).HandleError`，只记录仍未处理的错误。
- `ExpressionsToPBList` 两个批量入口用 `StringWithCtx(..., RedactLogDisable)` 生成可诊断错误文本。因为这里明确禁用 redact，上层不应将该错误无审查地暴露给不可信日志消费者。
- `ToPBFieldTypeWithCheck` 只对 `StoreType::TiFlash` 的非法 Decimal 返回 `Err`；TiKV 和非 Decimal 类型保持 Go 兼容的直接编码。
- `GroupByItemToPB` / `SortByItemToPB` 不创建错误，调用者必须将 `None` 转为适合具体算子的错误或改走 root 计算。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或 RPC。`PbConverter<'a>` 的生命周期保证 client 和 context 在转换期间有效，输出 `tipb` 值则拥有自己的 bytes、children 和字符串，可脱离转换器存活。

该类型没有声明 `Send`/`Sync` 边界，因此是否可跨线程取决于引用的 trait object；常规用法是在单次计划 PB 构造阶段临时创建和丢弃。`PushDownContext` 虽用 `Arc` 持有 client/context，但 `PbConverter()` 只借用其中对象，不增加额外共享所有权。性能上，批量入口按输入长度预分配，标量函数按参数数量预分配 children；递归深度与表达式树深度一致。

## 与 Go 版本的对应关系

主要对照文件是 `pkg/expression/expr_to_pb.go`。Rust 的公开函数、`PbConverter`、四类表达式分派、Datum 映射、新旧 ColumnRef 协议、字段类型往返、TiFlash Decimal 检查、GROUP/ORDER BY 包装与 Go 的同名逻辑逐项对应。关键兼容行为包括：

- Projection 的顶层列不检查 Set/Geometry/Enum 等计算类型限制。
- Float 负零留在 root，避免 `ATAN2` 等函数因符号位丢失改变结果。
- MySQL Time 需要 DAG 类型能力，且按 EvalContext 时区编码。
- 新 collation 开启时，ScalarFunction 把派生 collation 强制写入返回类型副本。
- 标量函数同时检查函数名和完整签名键；Rust 用 TiPB 枚举的 `Debug` 名称复现 Go `String()` 用于第二级键。

实现形式的差异是：Go 使用指针/nil 和三元 `(tp, val, ok)`，Rust 用拥有值的 `Option`/`Result`；Go 在 `scalarFuncToPBExpr` 对无效 PbCode 包含测试 failpoint panic，Rust 生产文件只返回 `None`；Go 将 metadata protobuf 在此处 marshal，Rust 的 `builtinFunc::metadata() -> Option<Vec<u8>>` 已经交付序列化 bytes，因此这里直接写入 `Expr.val`。这些是 API/基础设施差异，不应在后续修改中误当成可删减的 Go 行为。

Rust 回归 `pkg/expression/expr_to_pb_test.rs` 覆盖字段类型往返、TiFlash Decimal、常量/负零、Projection 特例、新旧列协议、标量函数与 GROUP/ORDER BY。Go 的 `pkg/expression/expr_to_pb_test.go` 更广泛地覆盖函数签名、排序规则、下推开关和负零回归，是扩展 Rust 测试时的语义源。

## 扩展指南

- 新增可编码的 `Datum` kind：在 `encodeDatum` 增加精确的 `ExprType` 和 codec 映射，核对远端 TiKV/TiFlash 解码支持及 `IsRequestTypeSupported` 类型号，并在 `expr_to_pb_test.rs` 增加正常值、边界值和拒绝路径。若 Go 已有行为，必须与 `expr_to_pb.go::encodeDatum` 保持一致。
- 新增表达式种类：在 `ExprToPB` 增加显式分派和私有转换器，不要把不能下推的类型伪装为空或常量。同步独立 Rust 测试和 Go 对照测试意图。
- 新增标量函数下推：首先在 builtin 实现中提供正确 `PbCode` 和 metadata bytes，再确认 `canFuncBePushed` 两级键与存储引擎 mask。需回归签名、所有 children、返回 FieldType 及 new-collation 路径，可参照 `builtin_registry_aster_unit_test.rs` 中直接 `ExprToPB` 的测试。
- 修改列协议：必须同时保持 modern `Index + FieldType` 和 legacy `ID` 分支，并继续拒绝 legacy ID `0/-1`。Projection 的 `check_type=false` 是可见语义，不能在普通 `ExprToPB` 中全局关闭类型检查。
- 扩展 `FieldType`：必须对称更新 `ToPBFieldType` 与 `FieldTypeFromPB`，并增加往返测试；若是 TiFlash 特有限制，放在 `ToPBFieldTypeWithCheck` 并保留其他 store 的 Go 兼容行为。
- 性能审查重点是大型表达式树的递归深度、metadata/Datum bytes 拷贝和批量 `Vec` 分配；正确性审查重点是能力检查、排序规则、时区、Decimal 精度和负零。测试仍应放在独立 `pkg/expression/expr_to_pb_test.rs`，不应放回生产文件。

## 验证依据

- 生产源：`pkg/expression/expr_to_pb.rs` 全文（符号、分支、错误和资源生命周期）；`pkg/expression/lib.rs` 的模块声明、再导出与独立测试接线。目标包下无 `doc.go`，因此没有可读的包级 Go 契约文件。
- crate 边界：`pkg/expression/Cargo.toml`，包括 `[lib] path = "lib.rs"`、`protobuf = "=2.8.0"`、固定 revision 的 `tipb` 和本地 kv/types/codec/collate 相关依赖。
- RustCodeGraph：`status` 显示索引可用（11,467 files / 307,296 nodes）；`query` 找到 Rust/Go 同名符号；`node expr_to_pb.rs::ExpressionsToPBList`、`node expr_to_pb.rs::ExprToPB`、`node expr_to_pb.rs::scalarFuncToPBExpr`、`node expr_to_pb.rs::encodeDatum`、`node expr_to_pb.rs::ToPBFieldTypeWithCheck` 和 `node expr_to_pb.rs::SortByItemToPB` 核对了主要调用边。独立 `callers` 命令未返回完整跨 crate 边，故用下列真实源文件调用点补齐。
- 上下游源：`pkg/expression/pushdown_context.rs`、`pkg/expression/aggregation/agg_to_pb.rs`、`pkg/expression/aggregation/{aggregation,window_func}.rs`、`pkg/expression/grouping_sets.rs`、`pkg/planner/core/operator/physicalop/{physical_projection,physical_hash_join,physical_topn,physical_sort,physical_limit,physical_window,physical_table_scan,physical_selection,physical_exchange_sender}.rs` 以及 `pkg/session/runtime/relational_scan.rs`。
- Go 对照：`pkg/expression/expr_to_pb.go` 的全部同名实现；`pkg/expression/expr_to_pb_test.go` 中的常量/函数签名、GROUP/ORDER BY、collation、下推开关和负零用例。
- Rust 测试：`pkg/expression/expr_to_pb_test.rs` 的 7 个独立用例，覆盖 FieldType 往返、TiFlash/TiKV Decimal 分歧、Decimal/非 Decimal 边界、字面量与负零、Projection/协议版本、ScalarFunction/列表错误以及 GROUP/ORDER BY。`pkg/expression/builtin_registry_aster_unit_test.rs` 额外验证注册 builtin 的 `ExprToPB` 签名编码。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 shell 命令校验文件存在且恰有 11 个固定二级标题，并人工复核不含源码整段复制或无依据的支持性声明。
