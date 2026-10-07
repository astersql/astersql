# `pkg/expression/aggregation/base_func.rs` 逻辑说明

## 文件定位

`base_func.rs` 位于 `astersql-expression-aggregation` crate，是聚合函数与窗口函数共用的规划期基础层，而不是执行期逐行累加器。它定义 `baseFuncDesc`，统一保存函数名、参数表达式和推导后的返回类型；`AggFuncDesc`（`pkg/expression/aggregation/descriptor.rs`）和 `WindowFuncDesc`（`pkg/expression/aggregation/window_func.rs`）都内嵌并通过 `Deref` 暴露这份描述。

crate 边界由 `pkg/expression/aggregation/Cargo.toml` 与 `lib.rs` 确定：本文件作为私有 `mod base_func` 编译，但其公开项经 `pub use base_func::*` 导出。它直接依赖同 workspace 的 `expression`、字段/Datum 类型门面、parser 的 AST/MySQL 常量、planner util 和内存大小工具；crate 没有为本文件声明独立 feature。

## 核心职责

1. `newBaseFuncDesc` 规范化函数名并立即调用 `TypeInfer`，建立可供 planner 使用的完整描述。
2. `TypeInfer` 及 `typeInfer4*` 按函数族推导 MySQL 兼容的类型、长度、小数位、字符集、排序规则和 flag，必要时还会改写参数表达式以插入 cast。
3. `Hash64`、`Equals`、`equal`、`clone`、`StringWithCtx` 和 `MemoryUsage` 支持计划缓存身份、语义比较、计划复制、诊断输出和内存记账。
4. `GetDefaultValue` 给出空输入组的聚合默认值；`WrapCastForAggArgs` 使执行/下推阶段看到与返回求值类型匹配的参数。

本文件不实现聚合状态分配、行更新或最终结果输出；那些行为位于同 crate 的 `count.rs`、`sum.rs`、`avg.rs` 等实现文件以及执行器侧聚合实现中。

## 主要符号

- `baseFuncDesc { Name, Args, RetTp }`：唯一核心状态。`Name` 由正规构造入口转为 ASCII 小写；`Args` 是可克隆的动态表达式；`RetTp` 是可空字段类型，以兼容运行时/PB 等跳过常规推断的构造路径。
- `newBaseFuncDesc`：正规入口。先构造 `RetTp: None`，再调用 `TypeInfer`；推断失败时不返回半成品。
- `TypeInfer`：按 `ast::*` 名称分派。支持 COUNT/MAX_COUNT/MIN_COUNT、近似聚合、SUM/SUM_INT/AVG、GROUP_CONCAT、MAX/MIN/FIRST_ROW、位聚合、方差/标准差、JSON 聚合以及多个窗口函数；未知名称返回 `unsupported agg function`。
- `typeInfer4ApproxPercentile`：验证恰有两个参数、百分位是常量且可求值，并限制在 `[1, 100]`；返回类型跟随输入的整数、浮点、decimal、时间或其它类型分支。
- `typeInfer4Sum`、`typeInfer4Avg`、`TypeInfer4AvgSum`：实现数值聚合的精度扩展。AVG 的除法精度增量来自 `EvalContext`；物理聚合拆 AVG 时，SUM 对简单列重新推断，对复杂 decimal 表达式继续扩大长度。
- `typeInfer4GroupConcat`：推导字符串 collation；元数据不完整时回退到连接或默认 charset/collation，并仅对最后一个 separator 之前的 decimal 参数构建 cast。
- `typeInfer4MaxMin`、`typeInfer4LeadLag`：复用/合并输入类型，清除可能产生 NULL 的函数返回类型上的 `NotNullFlag`；scalar float 参数会被包装为 double cast。
- `GetDefaultValue`：COUNT/MAX_COUNT/MIN_COUNT/BIT_OR/BIT_XOR 返回 0，BIT_AND 返回 `u64::MAX`，通常可空的聚合返回 NULL Datum；非字符串 APPROX_COUNT_DISTINCT 返回 0。
- `noNeedCastAggFuncs`、`AggCastKind`、`castAggArg`、`WrapCastForAggArgs`：共同实现参数 cast 策略。LEAD/LAG/NTH_VALUE 的第二个偏移参数和 NULL 类型参数不会被 cast。
- `Hash64`/`Equals` 与 `equal`：前者包含返回类型并用于结构身份，后者只比较名称和参数的求值语义。`clone` 对返回类型和每个表达式做深拷贝。

本文件没有模块级业务常量、trait、异步函数或条件编译项；唯一枚举是内部 cast 分派载体 `AggCastKind`。

## 执行流程

典型聚合规划链为：`NewAggFuncDesc`（`descriptor.rs`）调用 `newBaseFuncDesc` → 名称小写化 → `TypeInfer` 选择函数族分支 → 写入 `RetTp`，部分分支同时改写 `Args` → `AggFuncDesc` 再附加执行 Mode、DISTINCT、ORDER BY 与 GroupingID。窗口链由 `NewWindowFuncDesc`（`window_func.rs`）先校验偏移/桶数参数，再走相同基础构造，随后按窗口语义调整返回类型的可空 flag。

后续有三类消费路径：

1. planner 构造窗口表达式后调用 `WrapCastForAggArgs`（`pkg/planner/core/logical_plan_builder_runtime.rs`），使参数类型符合描述符返回求值类型。
2. `PBExprToAggFuncDesc`（`agg_to_pb.rs`）从 tipb 恢复名称、参数和 `RetTp` 后调用 `WrapCastForAggArgs`，再交给执行阶段。
3. AVG 被拆成 COUNT 与 SUM 时，`pkg/planner/core/operator/physicalop/base_physical_agg.rs` 调用 `TypeInfer` 和 `TypeInfer4AvgSum`，保证局部 SUM 的 decimal 精度足以支持最终 AVG。

描述符还被 `AggFuncDesc` 的哈希、相等、克隆、拆分、内存记账及 PB 下推逻辑复用；`GetTiPBExpr` 定义在 `agg_to_pb.rs` 的同一个 `baseFuncDesc` impl 上。

## 数据与状态

`baseFuncDesc` 自身是普通所有权对象，没有全局可变状态。`Args` 拥有 `Vec<ExprBox>`；类型推断可能就地替换其中的表达式，例如位聚合、JSON_OBJECTAGG、GROUP_CONCAT decimal 参数和 scalar-float MAX/MIN 路径。`RetTp` 在正规构造成功后应为 `Some`，但 `AggFuncDesc::from_runtime` 以及显式结构体构造允许其为 `None`，所以只有完成推断的描述符才能安全进入要求返回类型的流程。

`clone` 先复制描述符，再用 `FieldType::Clone` 和 `CloneExpr` 重建嵌套对象，防止后续参数 cast 或类型 flag 修改污染原计划。`MemoryUsage` 统计字符串容量常量、名称字节、`FieldType` 固定大小及其 charset/collation/enum 文本，再累加表达式的 `MemoryUsage`；这是估算值，不是分配器级精确追踪。

## 依赖与调用关系

- 上游构造者：`NewAggFuncDesc`、`NewAggFuncDescForWindowFunc`（`descriptor.rs`）和 `NewWindowFuncDesc`（`window_func.rs`）调用 `newBaseFuncDesc`。
- planner 上游：`logical_plan_builder_runtime.rs`、`planner/util/coreusage/cast_misc.rs`、`planner/cascades/old/transformation_rules.rs` 调用 `WrapCastForAggArgs`；`base_physical_agg.rs` 调用 `TypeInfer4AvgSum`。
- 下推/恢复：`agg_to_pb.rs` 为 `baseFuncDesc` 扩展 `GetTiPBExpr`，并在 PB 反解后补 cast。
- 下游表达式能力：类型查询和常量求值依赖 `Expression::{GetType, EvalInt, ConstLevel}`；比较/复制/显示依赖 `Equal`、`Equals`、`CloneExpr`、`StringWithCtx`；cast 依赖 `WrapWithCastAs*` 与 `formal_registry::BuildCastFunction`。
- 下游类型能力：依赖 `FieldType` 的长度、小数位、flag、charset/collation、EvalType 与 clone/equality API，以及 `Datum` 构造函数。

RustCodeGraph 的文件节点显示 `base_func.rs` 被 14 个文件引用；精确标识符搜索确认生产调用集中在上述 aggregation、planner 和 PB 转换路径。图查询对 Go/Rust 同名 `baseFuncDesc` 的 callers/callees 无法消歧，因此调用边又用精确源码搜索核验。

## 错误处理与边界

- `newBaseFuncDesc`、`TypeInfer` 和需要校验/合并的推断函数使用 `Result<_, Error>` 传播错误。APPROX_PERCENTILE 将底层整数转换错误收敛成稳定的公开错误文本；空值、非常量、参数数目和值域分别报错。
- SUM_INT 要求一个整数参数；未知函数名直接失败。GROUP_CONCAT 的 collation 推导和 LEAD/LAG 的控制函数类型合并错误会向上传播。
- 多个分支直接索引 `Args[0]`，调用者必须先保证相应函数的参数个数；本文件不是完整的 SQL arity 校验层。
- `WrapCastForAggArgs` 在参数为空或函数位于免 cast 集合时返回；否则要求 `RetTp` 已推断。`RetTp == None`、未知 `EvalType` 会 panic，这是内部不变量检查而非用户错误。
- `TypeInfer4AvgSum` 仅接受名称为 SUM 的描述符；复杂 decimal 表达式分支只在现有 `RetTp` 为 `Some` 时扩展，调用链必须先从 AVG 描述复制出已推断类型。
- `GetDefaultValue` 对未列举函数保留默认 NULL Datum；这不是对所有窗口函数默认值的统一定义。

## 并发与资源生命周期

文件没有锁、线程、异步任务、通道、事务或 I/O 资源。所有改写都需要 `&mut self`，共享读取使用 `&self`，并发隔离依靠 Rust 所有权和上层计划对象的生命周期。

表达式和字段类型由描述符拥有；深拷贝用于计划分支之间的隔离。临时 `HashSet` 由每次 `noNeedCastAggFuncs()` 调用创建，并在检查后释放；`AggCastKind` 只在一次参数包装期间持有克隆的目标类型。这里不维护执行期聚合状态，因此也没有跨行或跨分区资源清理职责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/aggregation/base_func.go`。Rust 保留了 Go 的结构字段、函数分派、类型宽度/精度规则、默认值表、cast 跳过规则和 planner-only 定位，并以 `Option<FieldType>` 对应 Go 的可空 `*FieldType`、以 `ExprBox` 对应接口表达式、以 `AggCastKind` 对应 Go 的局部 cast 函数变量。

需要注意的现状差异：

- Rust `clone` 能处理 `RetTp: None`，而 Go `clone` 直接解引用 `RetTp`，正常 planner 路径仍都假定已推断。
- Rust `typeInfer4LeadLag` 返回并传播 `InferType4ControlFuncs` 的错误；当前 Go 实现忽略该错误。这使 Rust 在冲突类型上更早失败。
- Rust `Hash64` 显式逐字段哈希 `FieldType`，目标是覆盖 Go `FieldType.Hash64` 的身份字段；对应共享测试专门验证 charset、collation、enum 元素、二进制字面量标记和 array flag。
- Rust `MemoryUsage` 手工估算 `FieldType` 及变长字段，而 Go 调用 `FieldType.MemoryUsage()`；两者意图一致，但不应假定字节值跨语言完全相等。
- Rust `typeInfer4GroupConcat` 和 Go 一样跳过最后一个 separator，并保留对 decimal 参数使用其原类型构建 cast 的当前行为。

## 扩展指南

新增聚合或窗口函数时，至少检查以下接入点：在 `TypeInfer` 增加名称分派并实现准确的类型/flag/collation 规则；决定空输入是否需要加入 `GetDefaultValue`；决定是否应加入 `noNeedCastAggFuncs`，否则确认 `RetTp.EvalType()` 已被 `AggCastKind` 覆盖；若需 PB 下推，同步 `agg_to_pb.rs::GetTiPBExpr` 及双向转换；若需完整聚合描述，同步 `descriptor.rs` 的拆分/等价逻辑和具体执行实现。

修改现有推断时应同步 `pkg/expression/aggregation/base_func_test.rs` 的独立入口及其委托的 `aggregation_aster_unit_test.rs` 用例，并与 `base_func_test.go` 的类型、clone、collation 与 AVG/SUM 精度断言对照。窗口参数及可空性还应同步 `window_func_test.rs`，PB 行为同步 `agg_to_pb_test.rs`。

兼容风险集中在返回类型元数据（会影响协议、表达式求值和下推）、decimal 精度、collation、NULL flag 和参数 cast；性能风险主要是额外表达式包装、重复构造免 cast 集合和深克隆。扩展时不要把 SQL 参数合法性完全依赖于数组索引前提，并应保留错误文本或明确评估兼容性。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件且目标目录已索引；`node --file pkg/expression/aggregation/base_func.rs` 分四段核对了全部 797 行、47 个文件符号及“被 14 个文件使用”的文件级关系；`query` 核对了 Rust/Go 的 `baseFuncDesc` 与 `newBaseFuncDesc`。同名跨语言符号的 `callers/callees` 未产生可用输出，故未据此臆造边。
- 生产源码：完整读取 `pkg/expression/aggregation/base_func.rs`；调用边核对 `descriptor.rs`、`window_func.rs`、`agg_to_pb.rs`、`pkg/planner/core/logical_plan_builder_runtime.rs` 和 `pkg/planner/core/operator/physicalop/base_physical_agg.rs`。
- crate 边界：读取 `pkg/expression/aggregation/Cargo.toml` 与 `lib.rs`，确认包名、依赖、模块声明、再导出和测试模块接线。
- Go 对照：读取 `pkg/expression/aggregation/base_func.go` 与 `base_func_test.go`，逐项核对构造、类型推断、默认值、cast、内存估算和测试意图。
- Rust 测试：读取 `base_func_test.rs`；其 clone/hash、返回类型与 AVG/SUM 用例委托到 `aggregation_aster_unit_test.rs`，另直接断言 APPROX_PERCENTILE 的错误屏蔽契约。还通过精确搜索确认 `aggregation_aster_unit_test.rs`、`go_merge_44_test.rs` 和 `window_func_test.rs` 对默认值、免 cast 集合及窗口描述有覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。结构校验命令及最终退出状态在任务交付时记录。
