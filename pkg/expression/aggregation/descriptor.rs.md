# `pkg/expression/aggregation/descriptor.rs`

## 文件定位

`descriptor.rs` 属于 `astersql-expression-aggregation` crate，是规划器侧聚合函数描述与行式聚合运行时之间的桥梁。crate 入口 `pkg/expression/aggregation/lib.rs` 以 `mod descriptor` 装配本文件，并通过 `pub use descriptor::*` 对外导出 `AggFuncDesc`、`NewAggFuncDesc` 和 `NewAggFuncDescForWindowFunc`。`pkg/expression/aggregation/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/expression/aggregation`；直接语义基准是同目录的 `descriptor.go`。

在完整 SQL 链路中，`pkg/planner/core/logical_plan_builder_runtime.rs` 的聚合构建流程先改写 SQL 聚合参数，再调用 `NewAggFuncDesc`，随后填充 `OrderByItems` 并把描述符放入逻辑聚合节点。描述符之后可用于计划等价判断、分布式/并行聚合拆分、外连接简化、下推转换和运行时聚合器实例化。它不是聚合状态本身；实际逐行状态及 `Update`/`GetResult` 契约位于 `aggregation.rs` 的 `Aggregation` 与 `AggEvaluateContext`。

## 核心职责

本文件承担五组职责：

1. 用 `AggFuncDesc` 汇总基础函数签名 `baseFuncDesc`、执行阶段 `Mode`、`DISTINCT`、聚合内排序项和 `GroupingID`。
2. 通过 `NewAggFuncDesc`/`NewAggFuncDescForWindowFunc` 建立类型已推断的规划描述符；普通入口下沉到 `base_func.rs::newBaseFuncDesc`，后者将函数名转为小写并立即调用 `TypeInfer`。
3. 通过 `Hash64`、`Equals`、`Equal`、`StringWithCtx`、`Clone` 和 `MemoryUsage` 支撑计划指纹、等价比较、解释输出、隔离式复制和内存估算。
4. 通过 `Split` 把完整或最终聚合改写为局部阶段和最终阶段的两个描述符，并按 AVG、近似去重计数、DISTINCT COUNT、GROUP_CONCAT、APPROX_PERCENTILE、MAX_COUNT/MIN_COUNT 等不同中间结果形状重建参数。
5. 通过 `EvalNullValueInOuterJoin`、`UpdateNotNullFlag4RetType` 和 `GetAggFunc` 分别处理外连接无匹配行的可折叠结果、返回类型可空性，以及描述符到具体行式 `Aggregation` 实现的分派。

本文件只为 `GetAggFunc` 明列的 SUM、SUM_INT、COUNT、AVG、GROUP_CONCAT、MAX、MIN、MAX_COUNT、MIN_COUNT、FIRST_ROW 和三种位聚合构造行式实现。`baseFuncDesc::TypeInfer` 能识别的其他聚合或窗口函数不等于能由本方法实例化；未知名称会触发 panic，而不是返回错误。

## 主要符号

- `AggFuncDesc`：公开描述符。`baseFuncDesc` 含小写 `Name`、表达式 `Args` 和可选 `RetTp`；`Mode` 为 `AggFunctionMode`；`HasDistinct` 表示 DISTINCT；`OrderByItems` 保存例如 GROUP_CONCAT 的排序表达式；`GroupingID` 区分 grouping/rollup 身份。`Deref`/`DerefMut` 让调用方直接访问基础字段。
- `NewAggFuncDesc(ctx, name, args, has_distinct) -> Result<AggFuncDesc, Error>`：标准构造入口。调用 `newBaseFuncDesc` 完成名称规范化和类型推断，默认 `CompleteMode`、空排序项、`GroupingID = 0`。
- `NewAggFuncDescForWindowFunc(ctx, desc, has_distinct)`：窗口函数复用聚合实现时的转换入口。若窗口描述的 `RetTp` 尚无结果，则重新进行基础构造与类型推断；否则深拷贝已有基础描述。
- `AggFuncDesc::from_runtime`：crate 内快速构造，刻意跳过类型推断并留下 `RetTp = None`。依赖它的代码必须在读取返回类型前补足或避开相关路径。
- `Hash64`/`Equals`：结构指纹及结构相等，纳入基础描述、`Mode`、`HasDistinct` 和有序 `OrderByItems`；二者都不纳入 `GroupingID`，与 Go 注释中“将弃用”的选择一致。
- `Equal`：带 `EvalContext` 的语义相等；比较 DISTINCT、排序项和基础表达式语义，但不比较 `Mode`、`GroupingID`。它与 `Equals` 的用途和字段集合不同，不能互换。
- `StringWithCtx`：生成 `name(distinct args order by items)`，并把参数上下文与脱敏模式传给表达式和排序项。
- `Clone`：深拷贝基础描述中的返回类型和参数表达式，并逐项克隆 `OrderByItems`；`Mode`、`HasDistinct`、`GroupingID` 按值复制。
- `Split(ordinal)`：返回 `(partial, final_desc)`；局部描述由深拷贝得到，完整模式转 `Partial1Mode`、最终模式转 `Partial2Mode`，最终描述固定为 `FinalMode`。
- `EvalNullValueInOuterJoin` 及四个私有 helper：在把内表列视为 NULL 后尝试把聚合化为常量，返回 `(Datum, valid)`；表达式求值错误通过 `Result` 传播。
- `GetAggFunc`：按函数名建立具体 `Box<dyn Aggregation>`。GROUP_CONCAT 读取上下文最大长度，MAX/MIN 与 MAX_COUNT/MIN_COUNT 按参数类型选择 collation。
- `UpdateNotNullFlag4RetType`：根据聚合种类、有无 GROUP BY、是否所有聚合都是 FIRST_ROW 决定是否删除 `RetTp` 的 `NotNullFlag`；不支持名称返回 `Error`。
- `MemoryUsage`：累计基础描述、`isize`、`bool` 和排序项的内存估算；它没有单独计入 `Vec` 容器容量或 `GroupingID` 之外的所有结构体布局填充，语义是局部估算而非分配器精确统计。

## 执行流程

标准规划流程如下：

1. 规划器改写聚合 AST 参数为 `ExprBox`；`logical_plan_builder_runtime.rs` 调用 `NewAggFuncDesc`。
2. `newBaseFuncDesc` 将名称小写化，保存参数并调用 `TypeInfer`；错误（例如不支持函数或参数不合法）原样向规划器传播。
3. 规划器补入 `OrderByItems`，并将描述符保存在逻辑/物理聚合节点。指纹与等价方法让这些节点参与计划比较及缓存。
4. 需要并行或分布式两阶段聚合时，调用 `Split(ordinal)`。局部端保留原参数与语义，只改阶段；最终端用中间输出列重建参数：AVG 读取 count 与 sum 两列，APPROX_COUNT_DISTINCT 读取字符串中间态，普通聚合读取一个局部结果列。DISTINCT COUNT 保留原表达式以满足最终聚合构建约定；GROUP_CONCAT 和 APPROX_PERCENTILE 还复制末尾的分隔符/百分位参数。
5. 执行侧需要本 crate 的行式聚合器时，`GetAggFunc` 把描述符封装进 `aggFunction` 并构造具体实现。运行时随后依据 `aggregation.rs::Aggregation` 创建每组上下文、逐行 `Update`、读取局部或最终结果并重置上下文。

`Split` 对 MAX_COUNT/MIN_COUNT 有特殊契约：最终描述符的单槽参数沿用原值参数类型，因为该路径用于执行器内部合并 `PartialResult`；行式 Final/Partial2 的两列形状则在 `GetAggFunc` 中以第二个参数作为比较值。`go_merge_44_test.rs` 同时验证单槽拆分类型和两列 Final 运行时行为。

## 数据与状态

`AggFuncDesc` 自身是可克隆的规划元数据，不保存某个分组的累计值。可变字段的语义边界如下：

- `Mode` 决定输入是原始行还是局部结果；`Split` 只对 `CompleteMode` 和 `FinalMode` 作阶段转换，其余模式保持原值。
- `Args` 的顺序是运行时协议。AVG Final 要求 `ordinal[0]` 为 count、`ordinal[1]` 为 sum；MAX_COUNT/MIN_COUNT 行式 Final/Partial2 的第二参数是极值比较对象。
- `RetTp` 通常在标准构造后为 `Some`，但 `from_runtime` 明确允许为空。`Split` 的 AVG 和普通分支会 `expect` 已推断类型，因此不能把未完成类型信息的描述符送入这些路径。
- `OrderByItems` 的顺序参与指纹和相等判断；它被深拷贝，避免计划重写时共享可变表达式。
- `GroupingID` 会被 `Clone` 保留，但新构造和 `Split` 产生的 final 描述符均初始化为 0；它不参与本文件的 hash/equality 协议。

真正的执行期资源在 `aggregation.rs::AggEvaluateContext`：包括求值上下文 `Arc`、DISTINCT 检查器、计数、当前值、GROUP_CONCAT 缓冲和 FIRST_ROW 标志。描述符只是每个运行时聚合器持有的配置。

## 依赖与调用关系

上游直接证据包括：

- `pkg/planner/core/logical_plan_builder_runtime.rs`：从 SQL 聚合 AST 创建 `AggFuncDesc`，补入 `OrderByItems`，是正常 SQL 规划主入口。
- `pkg/planner/core/operator/logicalop/logical_aggregation.rs`：逻辑聚合处理过程中创建描述符。
- `pkg/expression/aggregation/agg_to_pb.rs` 与 `aggregation.rs::NewDistAggFunc`：在 planner 描述、tipb 下推表达式和分布式行式聚合之间转换。
- `pkg/executor/typed_hash_agg.rs`、`pkg/executor/physical_plan_runtime.rs` 及其独立测试：消费描述符构建执行期聚合。

下游依赖包括：

- `base_func.rs`：`baseFuncDesc`、名称规范化、返回类型/参数类型推断、基础 hash/equality/clone/memory usage。
- `expression`：`BuildContext`、`EvalContext`、表达式克隆/求值、`Column`、`Schema` 和外连接 NULL 替换。
- `planner_util::ByItems`：聚合内 ORDER BY 的格式化、指纹、相等、克隆和内存估算。
- `types`、`mysql`、`ast`：Datum、FieldType、类型标志、函数名常量及中间列类型。
- `collate`：MAX/MIN 和 MAX_COUNT/MIN_COUNT 的字符串比较器选择。
- 同 crate 的 `sumFunction`、`avgFunction`、`concatFunction`、`maxMinFunction` 等：`GetAggFunc` 的具体运行时目标。

`Cargo.toml` 显示这些边界通过 workspace 内的 `astersql-expression`、`astersql-expression-exprstatic`、`astersql-planner-util`、字段/Datum 类型 crate、collation crate，以及 tipb Git 依赖组装；本文件没有 feature 条件编译项。

## 错误处理与边界

可恢复错误主要来自标准构造的类型推断、外连接表达式求值和可空性更新：它们都返回 `Result<_, Error>`。`UpdateNotNullFlag4RetType` 对未列举函数返回带函数名的错误。`EvalNullValueInOuterJoin` 对未支持函数则直接 panic，这反映它只允许调用方传入明确覆盖的聚合集合。

以下是调用方必须维护的前置条件：

- `Split` 的 AVG 至少需要两个 ordinal；其他读取中间列的分支至少需要一个，否则会数组越界。
- AVG、普通聚合及若干运行时分派要求 `RetTp` 或首个参数存在；违反约定会由 `expect`、索引或 `last().expect` panic。
- GROUP_CONCAT/APPROX_PERCENTILE 拆分要求最后一个参数是分隔符/百分位；MAX/MIN 运行时分派要求首参存在；两阶段 MAX_COUNT/MIN_COUNT 在存在第二参数时将其视为比较值。
- `GetAggFunc` 只覆盖明确列出的行式聚合，其他已能类型推断的名称仍会 panic。
- COUNT 外连接折叠只有当所有参数都化成常量时才给出有效答案；常量 NULL 返回 NULL 且 `valid = true`，非恒定表达式返回 `valid = false`。SUM/MAX/MIN/FIRST_ROW 只检查首参。BIT_AND 对无效或 NULL 采用 `u64::MAX`，BIT_OR/BIT_XOR 采用 0。
- `UpdateNotNullFlag4RetType` 在 `RetTp = None` 时不会修改任何内容但仍可返回成功；调用方不能因此假设返回类型已存在。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源。`AggFuncDesc` 的生命周期通常跟随逻辑/物理计划；`Clone` 通过深拷贝参数、返回类型和排序项隔离并行规划/重写阶段的修改。`GetAggFunc` 每次返回拥有描述符克隆的独立聚合对象，不在多个聚合器之间共享可变描述符。

运行期每个分组的可变累计状态由 `Aggregation::CreateContext` 创建，并由具体聚合器更新/重置；求值上下文通过 `Arc<dyn EvalContext>` 共享。GROUP_CONCAT 的字符串缓冲、截断标记和最大长度属于新建的 `concatFunction` 实例，不属于描述符。因而若要引入跨线程共享或缓存，不能只修改 `AggFuncDesc`，还必须审查具体 `Aggregation` 实现和执行器对每组/每 worker 实例的所有权。

## 与 Go 版本的对应关系

`pkg/expression/aggregation/descriptor.go` 是直接对照。Rust 保留了 Go 的字段、构造、hash/equality、格式化、深拷贝、拆分、外连接默认值、运行时分派、非空标志更新和内存估算主流程；函数名及字段命名也刻意维持 Go 风格。

需要注意的实现差异：

- Go 以指针和 nil 表示描述符/返回类型，Rust 返回拥有所有权的 `AggFuncDesc` 并以 `Option<FieldType>` 表示返回类型；`Deref` 模拟嵌入的 `baseFuncDesc` 字段访问。
- Rust 的窗口转换在已有返回类型时调用 `clone_desc`，深拷贝参数和类型；Go 直接以窗口描述里的字段组装基础描述。
- Rust `Split` 为 DISTINCT COUNT 显式克隆表达式；Go 复用表达式 slice。两者的协议目的相同，但 Rust 避免共享可变表达式对象。
- Rust 在 MAX_COUNT/MIN_COUNT 的拆分中用静态求值上下文取得首参类型；Go 用 `GetType(nil)`。两者都为执行器内部单槽局部结果保留原值类型。
- Rust 的 `EvalNullValueInOuterJoin` 把 Go 的三返回值 `(Datum, bool, error)` 表达为 `Result<(Datum, bool), Error>`。
- Rust `MemoryUsage` 以 `size_of::<isize>()`/`size_of::<bool>()` 计固定字段并累计排序项；Go 使用 `size.SizeOfInt`/`size.SizeOfBool`。两者都属于估算口径。

`pkg/expression/aggregation/go_merge_44_test.rs` 是当前最集中验证新增 MAX_COUNT/MIN_COUNT 对齐语义的 Rust 独立测试；其他具体聚合由 `aggregation_aster_unit_test.rs`、`avg_test.rs`、`count_test.rs`、`concat_test.rs`、`first_row_test.rs`、`bit_xor_test.rs` 等独立文件覆盖，符合测试不内嵌生产文件的仓库约束。

## 扩展指南

新增聚合或调整协议时，至少按以下接点同步：

1. 在 `base_func.rs::TypeInfer` 增加名称、参数校验和返回类型推断；标准构造依赖这里建立 `RetTp` 不变量。
2. 若支持并行/分布式执行，在 `AggFuncDesc::Split` 定义精确的局部结果列数、类型、ordinal 顺序和需保留的常量参数。不要把单槽内部 `PartialResult` 协议与行式 Final 的多列协议混为一谈。
3. 若使用本 crate 的行式运行时，在 `GetAggFunc` 添加具体实现，并确认 collation、上下文配置、DISTINCT 和阶段模式所需参数索引。
4. 若外连接简化可触达该函数，在 `EvalNullValueInOuterJoin` 明确定义空输入/NULL 填充结果；若返回类型可空性有变化，同步 `UpdateNotNullFlag4RetType`。
5. 评估新字段是否必须纳入 `Hash64`、`Equals`、`Equal`、`Clone`、`StringWithCtx` 和 `MemoryUsage`。当前 `GroupingID` 刻意不进指纹；新增字段不能默认照此处理。
6. 同步 Go `descriptor.go` 的真实语义，或清楚记录有证据的移植差异；同时审查 tipb 转换 `agg_to_pb.rs` 和分布式构造 `aggregation.rs::NewDistAggFunc`。
7. 测试放在独立文件。描述符协议优先扩展 `go_merge_44_test.rs` 或新增同目录独立 `*_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 模块接入；具体聚合运行时扩展相应的 `avg_test.rs`、`count_test.rs` 等。规划调用变化还应同步逻辑聚合/计划构建的独立测试。

兼容性风险集中在中间结果列形状、模式转换和返回类型标志；这些变化可能影响 MPP/下推、并行 HashAgg、计划缓存等价性以及 spill/restore。性能风险主要是无意的深拷贝扩大、指纹遗漏造成错误复用，以及 GROUP_CONCAT 缓冲限制未从上下文传递。

## 验证依据

本说明基于以下直接证据，未运行 Cargo（任务明确为纯文档分析）：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录的文件清单包含 `descriptor.rs`、Go 对照及独立测试。
- RustCodeGraph `node --file pkg/expression/aggregation/descriptor.rs --offset 1 --limit 420` 与 `--offset 420 --limit 100`：核对目标文件全部 472 行、27 个索引符号及被规划器/执行器/测试引用的事实。
- RustCodeGraph `query AggFuncDesc`、`query NewAggFuncDesc`、`query EvalNullValueInOuterJoin`、`query UpdateNotNullFlag4RetType`：核对 Rust/Go 同名符号和直接对照位置。通用名称 `Split` 的 explore 结果存在跨仓库同名噪声，因此调用点另以文件限定节点和 `rg` 交叉核对，没有把无关结果当作证据。
- RustCodeGraph 文件节点：`base_func.rs` 核对标准构造、深拷贝和 `TypeInfer`；`aggregation.rs` 核对分布式构造、`Aggregation` trait 与 `AggEvaluateContext`；`logical_plan_builder_runtime.rs` 核对 SQL 规划主入口。
- 原始文件读取：`pkg/expression/aggregation/Cargo.toml`、`lib.rs`、`descriptor.go`、`go_merge_44_test.rs`；并用 `rg` 核对 `logical_aggregation.rs`、`typed_hash_agg.rs`、`physical_plan_runtime.rs` 以及同目录独立测试中的构造、拆分、外连接、非空标志和运行时分派引用。
- Go/Rust 测试证据：`go_merge_44_test.rs` 验证 MAX_COUNT/MIN_COUNT 的返回类型、Split、Complete/Final 聚合、外连接常量结果和 NotNullFlag；`aggregation_aster_unit_test.rs` 验证普通 COUNT 拆分及多种 `GetAggFunc` 分派；具体聚合测试验证对应运行时行为。
- 人工复核结论：本文能回答该文件为何存在、标准构造及两阶段执行如何运行、状态由谁拥有、哪些路径返回错误或 panic，以及新增聚合时必须同步的符号和独立测试。
