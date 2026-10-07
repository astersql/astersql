# [`pkg/executor/aggfuncs/builder.rs`](./builder.rs)

## 文件定位

本文件是 `astersql-executor-aggfuncs` crate 中的聚合/窗口函数“选择层”。crate 入口 `pkg/executor/aggfuncs/lib.rs` 以 `pub mod builder` 暴露它；`pkg/executor/aggfuncs/Cargo.toml` 则声明该 crate 对应 Go 包 `pkg/executor/aggfuncs`。它把轻量描述符 `AggFuncDesc` 转换为 `BuiltAggFunc`：后者记录应使用哪个 `AggImplementation`、结果列序号以及少量已解析的构建参数。

它不是完整的表达式描述符转换器，也不直接为所有分支构造 `Box<dyn AggFunc>`。当前 Rust 生产代码中可确认的下游包括 `aggfuncs.rs` 的 `BuiltAggFunc::spill_function`（为部分实现绑定 spill 状态/序列化器）和 `func_max_min_count.rs` 的 `BuiltAggFunc::instantiate_count_extrema`（实例化 MAX_COUNT/MIN_COUNT）。源码搜索未发现普通执行器直接调用本文件公开的 `build` 或 `build_window_function`；这两个入口的直接调用目前集中在 `builder_test.rs` 和 `func_max_min_count_test.rs`。因此“供哈希/流式聚合/窗口执行器选择实现”是该层的设计职责，而不是已经完整接入所有 Rust 执行路径的证明。

## 核心职责

- 用 `FunctionName`、`AggMode`、`has_distinct`、参数/返回字段类型决定 `AggImplementation` 变体，入口为 `build`。
- 为窗口专有函数以及滑动窗口 MAX/MIN、MAX_COUNT/MIN_COUNT 做二次分派，入口为 `build_window_function`；其他名称回退到 `build`。
- 用 `value_kind` 把 SQL 字段元数据压缩为运行时分派所需的 `ValueKind`，并保留整数符号、单/双精度、Enum/Set/Bit 等差异。
- 在构建期提取常量参数：近似百分位的百分数、NTILE 桶数、NTH_VALUE 序号、LEAD/LAG 偏移与默认值、GROUP_CONCAT 分隔符。
- 把会话参数 `windowing_use_high_precision` 和 `group_concat_max_len` 固化进选择结果，避免下游再次解释构建上下文。
- 对不支持的模式、类型或缺失参数返回 `None`，而不是创建占位实现。

## 主要符号

- `AggMode`：`Complete`、`Partial1` 消费原始行，`Partial2`、`Final` 消费/合并部分结果；`Dedup` 是多数 helper 明确拒绝的模式。
- `EvalType`、`FieldKind`、`FieldType`：本地字段类型模型。`FieldType` 同时保存 `kind`、求值类型、unsigned 标志和 collation；其中 collation 在本文件仅随描述符/排序项保存，实际比较器由下游实现创建。
- `ConstantValue`：构建期常量的受限表示。`as_u64` 对非整数或负 `Int` 返回 `0`；`as_i32` 对越界整数返回 `None`；`converted_to` 只实现本文件需要的 Int、Real、String 转换。
- `ArgDesc`、`OrderByItem`、`AggFuncDesc`：输入描述符。`AggFuncDesc` 汇总函数名、阶段、DISTINCT、参数、返回类型和 ORDER BY。
- `AggFuncBuildContext`：只有窗口高精度和 GROUP_CONCAT 最大长度两个构建参数，且可 `Default` 为零/false。
- `ValueKind`：实例选择使用的运行时值类别。`value_kind` 特判 Enum、Set、Bit；普通 Int 再按 `unsigned` 区分，Real 再按 `FieldKind::Float` 区分 `Float32`/`Float64`。
- `AggImplementation`：选择结果的枚举，编码普通/部分/DISTINCT、滑动窗口、高精度等实现差异；它本身不是可执行 trait 对象。
- `BuiltAggFunc`：输出元数据，包含 `implementation`、`ordinal`、有效参数数、排序项以及可选 separator/max_len/default_value。`built` 负责生成通用默认值。
- `build`、`build_window_function`：两个公开工厂。其余 `build_*` helper 均为文件私有，负责单个函数族的校验和枚举选择。

## 执行流程

1. 调用方先构造 `AggFuncDesc` 和 `AggFuncBuildContext`，再按普通聚合或窗口语境调用 `build`/`build_window_function`。
2. `build` 按 `FunctionName` 分派。COUNT/SUM/SUM_INT/AVG/FIRST_ROW/MAX/MIN/MAX_COUNT/MIN_COUNT、方差族和近似函数进入各自 helper；BIT、JSON 等简单分支可直接选择枚举；GROUP_CONCAT 因需补充元数据而直接返回 `build_group_concat` 的结果。
3. helper 先校验阶段和所需参数，再结合 DISTINCT、返回类型或参数类型选择 `AggImplementation`。例如 COUNT 在 `Complete|Partial1` 选择 original 路径，在 `Partial2|Final` 选择 partial 路径；单参数 DISTINCT 只有受支持类型才使用专用实现，否则回退到 multi-argument 实现。
4. `build_window_function` 直接选择 RANK、ROW_NUMBER、FIRST/LAST_VALUE、CUME_DIST、NTH_VALUE、NTILE、PERCENT_RANK；LEAD/LAG 交给 `build_lead_lag`。MAX/MIN 仅对 Int、Uint、Float32/64、Decimal、String、Time、Duration 改为 `SlidingMaxMin`，其他类型保留普通实现。
5. MAX_COUNT/MIN_COUNT 只有在基础结果不是 `reject_rows` 且类型不属于 Enum、Set、Json、VectorFloat32 时转成 `SlidingMaxMinCount`；否则保留普通 `MaxMinCount`。
6. 未命中窗口专用分支时，`build_window_function` 回退 `build`，因此 SUM、AVG 等聚合也可沿普通选择逻辑用于窗口语境，并读取 `windowing_use_high_precision`。
7. 成功时 `built` 把实现枚举、ordinal、参数数和 ORDER BY 复制进 `BuiltAggFunc`；GROUP_CONCAT 与 LEAD/LAG 随后补充专用字段。失败路径通过 `?` 或显式分支返回 `None`。

几个需要特别保持的不变量：GROUP_CONCAT 最后一个参数是分隔符，构建后 `argument_count` 必须减一；LEAD/LAG 缺省 offset 为 1、缺省 default 为 `Null`；MAX_COUNT/MIN_COUNT 在 `Final|Partial2` 且参数多于一个时设置 `reject_rows`，阻止把不支持的 row-based final 误当作普通或滑动实现。

## 数据与状态

本文件没有聚合运行时部分结果，也不读取输入行。所有状态都是不可变的构建期值：输入 `AggFuncDesc` 以共享引用读取；`built` 克隆 `order_by_items`；GROUP_CONCAT 克隆字符串分隔符；LEAD/LAG 复制或克隆常量默认值。返回的 `BuiltAggFunc` 拥有这些元数据，可在描述符生命周期结束后继续使用。

阶段与类型是选择状态的两条主轴。`AggMode` 决定读取原始行还是部分结果，`ValueKind` 决定具体值表示。`ordinal` 只透传给下游，表示结果写入的列；本文件不验证 ordinal 是否落在输出 chunk 范围。`argument_count` 通常等于 `desc.args.len()`，只有 GROUP_CONCAT 排除末尾分隔符常量。

构建上下文被选择性固化：浮点 SUM/原始 AVG 把高精度标志编码在枚举字段中；GROUP_CONCAT 保存最大长度。排序项中的 collation 和 `FieldType.collation` 不在本文件解析为比较器，属于实例化/执行层责任。

## 依赖与调用关系

crate 边界由 `pkg/executor/aggfuncs/Cargo.toml` 给出，`lib.rs` 将 `builder` 与各具体聚合模块并列公开。本文件自身只使用 Rust 标准库类型，没有直接 `use` 外部 crate；它以本地数据模型隔离了表达式/AST 类型，真实 SQL 描述符到这些本地类型的桥接不在本文件中。

已确认的调用/消费关系如下：

- `builder_test.rs` 直接调用 `build` 和 `build_window_function`，验证阶段、DISTINCT、GROUP_CONCAT、LEAD/LAG 和拒绝分支。
- `func_max_min_count_test.rs` 通过同一入口构造 MAX_COUNT/MIN_COUNT 元数据，再调用实例化路径。
- `aggfuncs.rs::BuiltAggFunc::spill_function` 匹配 `AggImplementation`，为 DISTINCT COUNT、近似计数、DISTINCT SUM/AVG、方差、GROUP_CONCAT、FIRST_ROW、百分位等部分实现绑定真实状态与 `Serializer`；未覆盖变体返回 `None`。
- `func_max_min_count.rs::BuiltAggFunc::instantiate_count_extrema` 消费 `MaxMinCount`/`SlidingMaxMinCount` 并按 `ValueKind` 创建具体 `CountExtrema<T>`；同文件的 `build_count_extrema_function` 是从真实表达式字段类型到该元数据/实例化层的局部桥接。

RustCodeGraph 将 `builder.rs` 标为被 `aggfuncs.rs`、`builder_test.rs`、`func_first_row.rs`、`func_max_min_count.rs` 等 13 个文件使用；精确源码搜索则没有找到普通聚合执行器对两个公开入口的直接调用。因此新增说明不应声称本文件已经替代 Go `Build` 的全部应用级调用链。

## 错误处理与边界

公开入口以 `Option<BuiltAggFunc>` 表达“不支持/描述符无效”，不区分错误类别。常见 `None` 原因包括：未知函数名；Dedup 模式；参数缺失；字段类型无法映射；百分位常量不是 i32；GROUP_CONCAT 末参不是字符串常量；AVG partial/final 返回物理类型不是 NewDecimal/Double；MAX_COUNT/MIN_COUNT 的 Real 类型不是 Float/Double。

边界转换偏保守但并非全部报错：`ConstantValue::as_u64` 把负整数、非整数和越界转换变为 0，所以 NTH_VALUE、NTILE、LEAD/LAG 的非法常量若未被更上游验证，可能被固化为 0；该文件依赖“描述符构造阶段已经校验”的 Go 语义。LEAD/LAG 默认值转换失败时保留原值，而不是返回 `None` 或替换成 NULL，`builder_test.rs::build_window_lead_lag_preserves_default_when_conversion_fails` 明确锁定此行为。

`build_count` 对单参数 DISTINCT 的不支持类型回退 multi-argument 状态，而不是拒绝。近似百分位对无法识别的求值类型选择 `PercentileNull`，表达“任何输入均返回 NULL”。JSON 聚合只拒绝 Dedup。BIT 聚合当前不按 mode 做额外校验。所有这些都是现有选择逻辑，扩展时不能用统一的“非法即 None”规则覆盖。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件、网络连接或事务，也不分配聚合 partial result。所有工厂都是同步纯计算；除克隆字符串/向量外没有外部资源生命周期。输入通过 `&AggFuncDesc` 借用，输出完全拥有其元数据，因此并发安全取决于这些普通值类型，构建过程没有共享可变状态。

真正的部分结果所有权、spill 生命周期和执行期内存记账位于 `aggfuncs.rs` 及具体 `func_*.rs`：例如 `PartialResult = Box<dyn Any + Send>`，`spill_function` 才会分配模板状态。本文件只决定下游应选择哪一种状态/算法；高精度和滑动窗口选择可能影响 CPU、内存与数值行为，但资源管理不在这里发生。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/aggfuncs/builder.go`。Rust `build` 对应 Go `Build` 的函数名分派，Rust `build_window_function` 对应 Go `BuildWindowFunctions`；各私有 `build_*` 基本沿用 Go 的阶段、DISTINCT 和类型选择。COUNT、SUM、AVG、FIRST_ROW、MAX/MIN、GROUP_CONCAT、方差/标准差、JSON、近似计数/百分位以及 LEAD/LAG 的关键分支均能在 Go 同名 helper 中找到依据。

两者的结构差异很重要：Go 工厂接收真实 `expression`/`aggregation.AggFuncDesc` 和 `exprctx.ExprContext`，立即返回具体 `AggFunc` 对象；Rust 文件定义了独立的简化描述符并返回 `AggImplementation` 元数据，只有部分实现另有实例化或 spill 绑定。Go 窗口 RANK/CUME_DIST/PERCENT_RANK 接收 `orderByCols` 并创建 row comparer，Rust `build_window_function` 没有该参数，因此这里只能记录实现种类，不能证明排序比较器已经构造。

具体语义差异/压缩包括：Go 从真实字段类型获取 collator，Rust 只保存 collation 字符串；Go GROUP_CONCAT 对不可能的分隔符求值错误会 panic，Rust 对缺失或非字符串常量返回 `None`；Go 近似百分位求值错误会记录日志并返回 nil，Rust 只接受 `ConstantValue`；Rust LEAD/LAG 仅实现有限常量转换，但与 Go 一样在转换失败时保留原默认表达式/值。Rust `builder_test.rs` 目前覆盖这些移植后的关键契约，而 Go 的更广泛行为仍由同目录 Go 测试及执行器测试承担。

## 扩展指南

新增聚合或窗口函数时，至少要同步检查以下位置：

1. 在 `FunctionName` 增加名称，在 `AggImplementation` 增加足以区分阶段、类型、DISTINCT/滑动等行为的变体。
2. 在 `build` 或 `build_window_function` 接入分派；若有复杂校验，新增私有 `build_*`，保持 `None` 边界与 Go 同名逻辑一致。
3. 若有构建期常量或会话变量，扩展 `AggFuncDesc`/`AggFuncBuildContext`/`BuiltAggFunc`，并明确所有权与默认值；不要把运行时可变状态塞进 builder。
4. 为新变体补真实消费者：通常是具体 `func_*.rs` 的实例化逻辑、`aggfuncs.rs::spill_function` 的状态绑定，以及必要的序列化/反序列化支持。只新增枚举并不能形成可执行实现。
5. 在独立测试 `pkg/executor/aggfuncs/builder_test.rs` 增加分派、非法模式、缺参、类型边界测试；具体算法测试仍放在相应 `*_test.rs`，不得嵌入生产源文件。同步核对 `builder.go` 及相关 Go 测试，尤其是部分聚合阶段和转换失败语义。

兼容性风险集中在阶段映射、字段物理类型、unsigned/Float32、collation、DISTINCT 状态格式和默认值转换；性能风险集中在误选高精度、滑动窗口或 multi-argument DISTINCT 实现。修改时还应检查 spill 是否支持新变体，否则内存执行可能可用而落盘恢复失败。

## 验证依据

- RustCodeGraph `status`：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/executor/aggfuncs` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph `node --file pkg/executor/aggfuncs/builder.rs`：完整读取 832 行，核对所有公开/私有符号、枚举分支、常量转换与返回边界；`query build_window_function` 和 `query build_count` 确认入口及测试符号。
- RustCodeGraph `node` 读取 `pkg/executor/aggfuncs/lib.rs`、`aggfuncs.rs`、`func_max_min_count.rs`、`builder_test.rs` 和 `builder.go`；Cargo 原文读取 `pkg/executor/aggfuncs/Cargo.toml`。目标目录不存在 `doc.go`。
- 源码调用搜索：`rg` 核对 `build_window_function`、`BuiltAggFunc`、`AggImplementation` 的 Rust 引用，确认公开入口的直接测试调用和两处生产消费/桥接，并确认未发现普通执行器的直接入口调用。
- `builder_test.rs` 的六组测试证实 COUNT 阶段/DISTINCT、GROUP_CONCAT 元数据、LEAD/LAG 参数及转换失败保留、JSON/百分位拒绝分支、MAX_COUNT/MIN_COUNT 普通/滑动/reject_rows 行为。
- 本任务仅生成文档，未运行 Cargo。已运行任务指定的 11 章节结构命令（退出码 0），并人工复核本文区分了设计职责、当前接线事实与验证限制。
