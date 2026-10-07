# `pkg/planner/core/generator/shallow_ref/shallow_ref_generator.rs`

## 文件定位

本文件是 `astersql-planner-core-generator-shallow_ref` crate 的核心实现，crate 入口 `pkg/planner/core/generator/shallow_ref/lib.rs` 通过 `pub mod shallow_ref_generator` 声明模块并再导出其公开项。`pkg/planner/core/generator/shallow_ref/Cargo.toml` 将 `lib.rs` 设为库入口，没有声明运行时依赖，并用 `package.metadata.porting.go-package` 指向对应 Go 包。

它不是 SQL 规划运行时的一环，而是逻辑算子 Copy-on-Write 辅助方法的源码生成器。固定目录入口 `GenShallowRef4LogicalOps` 生成 Go 文件内容，对应的已提交产物是 `pkg/planner/core/operator/logicalop/shallow_ref_generated.go`。实际 Go 生成链由 `pkg/planner/core/operator/logicalop/logical_plans_misc.go` 中的 `//go:generate go run ../../generator/shallow_ref/shallow_ref_generator.go -- shallow_ref_generated.go` 驱动；仓库搜索只发现 Rust API 被同 crate 的独立测试调用，未发现 Rust 生产代码直接调用它。

## 核心职责

文件提供两条生成路径：

- `GenShallowRef4LogicalOps` 使用文件内显式维护的 `logical_operator_specs`，为 `LogicalJoin`、`LogicalProjection`、`LogicalAggregation` 和 `LogicalSort` 生成与 Go 反射生成器相同的完整 Go 源码。Rust 无法反射 Go 类型，因此 `TypeSpec`、`StructFieldSpec` 和 `OperatorSpec` 将 Go 字段类型、导出性以及 `shallow-ref:"true"` 标签结果固化为元数据。
- `gen_shallow_ref_for_logical_ops` 接收公开的 `LogicalOperator`/`Field` 元数据，为调用方描述的任意算子生成较简化的 `ShallowRef` 方法。它复制算子值，并按 `FieldKind` 对导出字段生成 slice、map 或嵌套 `ShallowRef` 处理；其他种类沿用浅复制结果。

固定目录路径的核心语义是：算子级方法只复制算子结构体；带 shallow-ref 标签的字段另有字段级方法。字段级方法为 slice 分配新 backing array，为嵌套 struct 先复制结构体再递归复制其导出字段中的容器，但保留 slice 元素里的指针或接口引用，从而允许重排、删除或追加容器元素而不改动原容器。

## 主要符号

- `pub enum FieldKind`：公开元数据 API 的字段分类。`Slice`、`Map`、`ShallowRef` 会生成额外操作；`Value`、`Pointer`、`Interface`、`Embedded` 只保留结构体浅复制。
- `pub struct Field`：描述字段名、Go 类型名、种类和是否导出。`exported == false` 的字段由公开生成器跳过。
- `pub struct LogicalOperator`：公开生成器的一项算子描述，包含算子名和字段列表。
- 私有 `TypeSpec`：固定目录递归生成器的类型树，分为 `Scalar`、递归 `Slice`、带字段列表的 `Struct` 和可报告错误的 `Unsupported`。
- 私有 `StructFieldSpec`/`OperatorSpec`：分别记录 Go 字段和算子元数据；`shallow_ref` 决定是否生成字段级方法，`exported` 控制 struct 递归时是否处理该字段。
- `CODE_GEN_LOGICAL_OP_COW_PREFIX`：固定生成结果的许可证、generated 声明、Go package 和 imports 前缀。
- `pub fn gen_shallow_ref_for_logical_ops(...) -> Result<Vec<u8>, String>`：公开、调用方驱动的简化生成入口。
- `fn generate`、`fn write_operator`、`fn write_field_method`：固定目录生成流水线，依次负责整文件、单算子方法和带标签字段方法。
- `fn write_shallow_ref_element(...) -> Result<Option<String>, String>`：递归发射 slice/struct Copy-on-Write 代码；返回的 `Option<String>` 表示递归层创建的临时变量名。
- `fn is_shallow_ref_field`、`fn line`、`field`、`scalar`、`slice`、`operator`：标签判断、缩进输出和元数据构造辅助函数。
- `fn logical_operator_specs() -> Vec<OperatorSpec>`：固定目录的事实来源，列出四个算子及需单独复制的字段，包括 `PossibleProperties.Orders` 的二维 slice。
- `pub fn refine_field_type_name`：仅删除类型名开头的 `logicalop.`，与 Go `refineFieldTypeName` 保持一致。
- `pub const SHALLOW_REF_STRUCTURES`：向测试公开固定目录的四个算子名。
- `pub fn GenShallowRef4LogicalOps() -> Result<Vec<u8>, String>`：固定目录公开入口；保留 Go 风格命名以便对应迁移来源。
- `fn validate_name`：接受 ASCII 字母或下划线开头、后续可含 ASCII 数字的 Go 标识符，拒绝空字符串和其他字符。

## 执行流程

固定目录生成流程如下：

1. `GenShallowRef4LogicalOps` 调用 `logical_operator_specs` 构造四个 `OperatorSpec`，再交给 `generate`。
2. `generate` 先写入 `CODE_GEN_LOGICAL_OP_COW_PREFIX`，然后逐个调用 `write_operator`；非空目录最后删除一个多余换行，并返回 UTF-8 字节。
3. `write_operator` 先校验算子名，再生成 `LogicalXxxShallowRef`。方法体执行 `shallow := *op` 并返回 `&shallow`，即只复制顶层 Go 结构体。
4. `write_operator` 只对 `is_shallow_ref_field` 为真的字段调用 `write_field_method`。当前 `field` 构造器把固定目录字段统一标为导出且需要 shallow-ref。
5. `write_field_method` 校验字段名，生成 `FieldNameShallowRef`，调用 `write_shallow_ref_element` 发射递归复制代码，最后返回写回后的 `op.FieldName`。
6. `write_shallow_ref_element` 遇到 `Scalar` 时不生成深复制；遇到 `Slice` 时创建容量等于原长度的新 slice、遍历元素、递归处理元素并 append；遇到 `Struct` 时先复制结构体，只递归其导出字段，随后按所在层级写回；遇到 `Unsupported` 时返回错误。
7. 为避免嵌套 slice 循环变量重名，外层元素默认叫 `one`，当字段名已以 `one` 开头时追加 `e`；以 `one` 开头也被用作“当前处于 slice 元素内部”的约定，此时递归函数返回临时变量而不直接向父对象写回。

公开元数据生成流程独立于上述递归路径：它逐个验证算子名和字段名，先生成带 nil 接收者保护的顶层 `ShallowRef`，跳过未导出字段，再按 `FieldKind` 生成一层字段操作，最终返回字节。该入口不会调用 `logical_operator_specs` 或 `write_shallow_ref_element`。

## 数据与状态

所有生成状态都局限在函数栈和局部 `String` 中；`line` 以 tab 写入缩进并统一追加换行。固定目录是每次调用重新构造的 `Vec<OperatorSpec>`，没有可变全局状态或缓存。

当前固定数据覆盖：`LogicalJoin` 的五组条件 slice、`LogicalProjection.Exprs`、`LogicalAggregation` 的 `AggFuncs`、`GroupByItems` 和 `PossibleProperties`、以及 `LogicalSort.ByItems`。`PossibleProperties` 被描述为 struct，其中 `Orders` 是二维 slice，`HasTiFlash` 是 scalar，因此生成代码复制 struct 和两层 slice，但布尔值由结构体复制自然保留。

生成结果复制的是容器所有权边界而不是元素对象：新的 slice 与原 slice 不共享 backing array；其中的指针、接口值或 scalar 仍按 Go 赋值语义复制。固定生成器未描述 map 类型；公开 `FieldKind::Map` 路径会发射 `maps.Clone` 文本。

## 依赖与调用关系

RustCodeGraph 的文件查询识别出本文件 35 个符号。源码内部的关键调用边是 `GenShallowRef4LogicalOps → logical_operator_specs + generate`、`generate → write_operator`、`write_operator → validate_name + write_field_method`、`write_field_method → validate_name + write_shallow_ref_element`，而 `write_shallow_ref_element` 对 slice 元素和 struct 字段递归调用自身。

crate 入口 `lib.rs` 公开再导出这些 API，并仅在 `cfg(test)` 下挂载 `shallow_ref_test.rs` 与 `shallow_ref_generator_test.rs`。根 `Cargo.toml` 把目录列为 workspace member，并用 `facade_planner_core_generator_shallow_ref` 指向本地 package；目标 crate 自身没有第三方 Rust 依赖。

Go 侧对应调用边为 `logical_plans_misc.go` 的 `go:generate` 指令启动 `shallow_ref_generator.go::main`，随后 `main → GenShallowRef4LogicalOps → genShallowRef4LogicalOps → cc.shallowRefElement/cc.format`，最终写入 `shallow_ref_generated.go`。RustCodeGraph 对 Rust 精确 callers/callees 查询未输出边；仓库文本引用核验表明，Rust 入口当前仅被两个同目录独立测试文件调用，不能据此声称它已替代 Go 的 `go:generate` 执行入口。

## 错误处理与边界

`validate_name` 是两条 Rust 生成路径的显式输入边界。公开生成器验证每个算子名以及所有字段名（包括随后会因未导出而跳过的字段）；固定生成器验证算子名和实际生成方法的字段名。错误以 `Result<_, String>` 向上传播，内容为 `invalid Go identifier ...`。

`TypeSpec::Unsupported` 让递归生成器以 `Err("doesn't support element type...")` 报告不支持的种类，对应 Go `shallowRefElement` default 分支的 panic。当前 `logical_operator_specs` 只使用 `Scalar`、`Slice` 和 `Struct`，因此固定目录在现有数据下没有触发该错误的路径，但返回 `Result` 保留扩展后的失败传播。

固定目录生成的算子级 Go 方法直接解引用 `*op`，与 Go 原实现一致；nil 接收者会在生成代码运行时 panic。公开元数据 API 生成的另一种 `ShallowRef` 方法则包含 `if p == nil { return nil }`，两条 API 的 nil 行为不能混为一谈。

固定递归算法用字段名是否以 `one` 开头判断嵌套层级。这复刻了 Go 实现，但也意味着真实顶层字段若以 `one` 开头会被当作内部元素，扩展目录时必须检查该命名约束。公开 `FieldKind::Slice` 通过剥离 `type_name` 的 `[]` 前缀拼接元素类型，调用方若提供不匹配的类型名可能生成无效 Go；`FieldKind::Map` 会发射 `maps.Clone`，而该公开入口自己的短前缀没有加入 `maps` import，因此使用 map 路径的调用方还需补足生成前缀或验证输出可编译。文件本身只生成字节，不调用 `gofmt` 或 Go 编译器验证公开入口输出。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务、文件句柄或网络资源。每次调用独立分配元数据和输出 `String`，递归期间只借用并修改当前输出缓冲区，因此并发调用之间没有共享可变状态。

slice 生成逻辑会让最终 Go 代码按原长度分配新容量并逐项 append，时间和额外空间均与被复制的 slice 元素数线性相关；二维 `Orders` 会对外层和每个内层 slice 分别分配。元素对象不做深复制，所以生命周期仍由生成代码中的 Go 引用关系管理。磁盘写入只存在于 Go `main`，不属于本 Rust 文件的资源生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/generator/shallow_ref/shallow_ref_generator.go`。`GenShallowRef4LogicalOps` 的四算子目录、算子级结构体复制、按 `shallow-ref:"true"` 选择字段、slice/struct 递归、跳过未导出 struct 字段、元素临时名规则、`refineFieldTypeName` 以及不支持类型的错误语义均在 Rust 中有对应实现。

关键实现差异来自语言能力和入口角色：Go 用 `reflect.TypeOf` 和真实 struct tag 动态发现字段，并通过 `go/format.Source` 格式化；Rust 固定路径用 `logical_operator_specs` 手工镜像这些事实，并由 `line` 直接生成预期格式。因此 Go 算子字段或 tag 变化不会自动更新 Rust 元数据，必须同步修改 `logical_operator_specs`。

`pkg/planner/core/generator/shallow_ref/shallow_ref_generator_test.rs::generator_matches_go_output` 将 Rust 固定入口的全部字节与已提交 Go 产物比较；`generator_preserves_field_level_and_recursive_copy_semantics` 还检查字段级方法、struct 临时副本和二维 slice。`shallow_ref_test.rs::TestHash64Equals` 验证两次生成的确定性、与已提交产物逐行及逐字节一致、四算子方法存在，并覆盖公开元数据 API 的 slice 分支。Go 的 `shallow_ref_test.go::TestHash64Equals` 同样将反射生成结果与已提交文件对比。测试名称中的 `Hash64` 是历史命名，实际测试对象是 shallow-ref 生成器。

## 扩展指南

若 Go 新增或修改带 `shallow-ref:"true"` 的字段，应先以真实 Go 类型和 tag 为准更新 `logical_operator_specs`：新增算子用 `operator`，新增字段用 `field`，slice 用递归 `slice`，struct 用 `TypeSpec::Struct` 明确列出字段。随后应同步扩展同目录独立测试，至少让全量产物比对继续通过，并为新的嵌套结构增加类似 `generator_preserves_field_level_and_recursive_copy_semantics` 的关键文本断言；不要把 Rust 测试内嵌进本源文件。

若要支持新的类型种类，应修改 `TypeSpec` 与 `write_shallow_ref_element`，并核对 Go `cc.shallowRefElement` 的同类分支；不能用 `Scalar` 规避实际需要独立容器的字段。特别要验证顶层写回、slice 元素返回临时值、未导出 struct 字段跳过以及指针保持共享这些不变量。新增 map 支持时还应分别处理固定生成前缀和公开 API 前缀中的 import，并用生成结果编译或与 gofmt 后基准比对。

若扩展公开元数据 API，应在 `shallow_ref_generator_test.rs` 或 `shallow_ref_test.rs` 增加无效标识符、未导出字段、nil 接收者输出和各 `FieldKind` 的独立用例。兼容风险主要是生成方法名或字节格式变化导致已提交 Go 产物漂移；正确性风险是容器复制层级不足而修改原 slice/map；性能风险是为不需要重排的嵌套容器增加额外线性分配。

## 验证依据

- RustCodeGraph：`status` 确认本地索引可用；`files --filter pkg/planner/core/generator/shallow_ref` 找到 Rust、Go、入口及测试；`node --file ...shallow_ref_generator.rs` 读取完整 446 行和 35 个符号；`query` 精确定位 Rust `GenShallowRef4LogicalOps`、`gen_shallow_ref_for_logical_ops`、`write_shallow_ref_element`。精确 `callers`/`callees` 未返回图边，因此调用关系进一步由函数体及仓库引用核验，未把空结果解释成生产接线。
- Rust 源与入口：`pkg/planner/core/generator/shallow_ref/shallow_ref_generator.rs`、`pkg/planner/core/generator/shallow_ref/lib.rs`。
- crate/工作区：`pkg/planner/core/generator/shallow_ref/Cargo.toml`，以及根 `Cargo.toml` 的 workspace member 和 facade dependency 条目。
- Go 对照与生成入口：`pkg/planner/core/generator/shallow_ref/shallow_ref_generator.go`、`pkg/planner/core/operator/logicalop/logical_plans_misc.go`、`pkg/planner/core/operator/logicalop/shallow_ref_generated.go`。
- 独立测试：`pkg/planner/core/generator/shallow_ref/shallow_ref_generator_test.rs`、`pkg/planner/core/generator/shallow_ref/shallow_ref_test.rs`、`pkg/planner/core/generator/shallow_ref/shallow_ref_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰好包含十一个固定二级标题，并人工复核本说明能回答文件用途、生成流程、扩展入口和已知边界。
