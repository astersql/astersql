# [`pkg/planner/core/generator/hash64_equals/hash64_equals_generator.rs`](./hash64_equals_generator.rs)

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-generator-hash64_equals`，包入口是同目录的 `lib.rs`，并通过 `pub use hash64_equals_generator::*` 再导出本文件的公共 API。它是一个代码生成器：输入逻辑算子及参与结构哈希的字段描述，输出 `package logicalop` 下各算子的 Go `Hash64` 和 `Equals` 方法源码；输出最终用于 Cascades 优化器比较逻辑计划结构，而本文件本身不在 SQL 请求的运行时热路径上。

crate 边界由 `pkg/planner/core/generator/hash64_equals/Cargo.toml` 确认：该包没有声明外部 Rust 依赖，`[package.metadata.porting].go-package` 指向同路径 Go 包。固定清单入口 `GenHash64Equals4LogicalOps` 对应 Go 的同名函数，生成结果由 `pkg/planner/core/operator/logicalop/hash64_equals_generated.go` 承载。

## 核心职责

1. 用 `HashFieldKind`、`HashField`、`LogicalOperator` 提供显式的公共元数据 API，替代 Go 版本依赖 `reflect` 和 ``hash64-equals:"true"`` struct tag 的发现过程。
2. 用内部 `FieldType`、`FieldSpec`、`OperatorSpec` 表达生成阶段需要区分的标量、切片、值类型、指针和接口式 `HashEquals` 语义。
3. 为每个算子生成稳定的类型前缀哈希、字段哈希及结构相等比较代码，特别保留 nil 与空切片、nil 与非 nil 指针的差异。
4. 在 `logical_operator_specs` 中维护 Go `GenHash64Equals4LogicalOps` 当前覆盖的 21 个逻辑算子及其参与字段，在 `logical_op_name_to_plan_codec` 中维护算子名到 `plancodec` 常量的映射。
5. 通过 `validate_name` 拒绝会破坏生成语法的算子名；最终结果以 `Result<Vec<u8>, String>` 返回给调用方。

## 主要符号

- `HashFieldKind`：公共字段分类。`Bytes` 映射为无符号整数切片，`Pointer` 映射为需编码 nil 状态的 `HashEquals` 指针，`Slice` 的元素按直接 `HashEquals` 处理，`Interface` 也按直接调用处理，`HashEquals` 表示值类型并在 `Equals` 右侧取地址，`Ignored` 会被过滤。
- `HashField { name, kind }`：一个参与生成的字段描述；字段名目前不经 `validate_name` 校验，因此公共调用者必须保证其为合法且真实的 Go 字段选择器。
- `LogicalOperator { name, codec_name, fields }`：公共入口所需的单个算子元数据。`name` 会被校验；`codec_name` 被原样写入 `h.HashString(...)`，调用者需要提供合法 Go 表达式。
- `FieldType`、`FieldSpec`、`OperatorSpec`：私有生成模型。`HashEqualsValue`、`HashEqualsPointer`、`HashEqualsDirect` 的区分决定 `Equals` 是传 `&rhs` 还是直接传 `rhs`，以及顶层字段是否需要 nil 标志。
- `CODE_GEN_HASH64_EQUALS_PREFIX`：固定输出头，包含许可证、generated 标记、`logicalop` 包名及 `base`、`plancodec` 导入。
- `gen_hash64_equals_for_logical_ops`：公共的元数据驱动入口；校验算子名、过滤 `Ignored` 字段、转换类型，再调用 `generate`。
- `generate`：共享编排器，为每个 `OperatorSpec` 依次调用 `write_hash_method` 和 `write_equals_method`，并移除非空输出末尾多余的一个换行。
- `write_hash_method`、`write_hash_field`、`write_hash_element`：递归生成 `Hash64`；切片记录长度并逐元素递归，指针和切片顶层字段先写 `NilFlag`/`NotNilFlag`。
- `write_equals_method`、`write_equals_element`：递归生成 `Equals`；先做目标类型断言与接收者 nil 对称检查，再逐字段比较，遇到第一处差异立即返回 `false`。
- `nested_item_name`、`line`：分别生成嵌套切片循环变量名、按 tab 缩进追加一行；前者避免以 `one` 开头的当前表达式与新循环变量重名。
- `logical_operator_specs`、`LOGICAL_STRUCTURES`：固定迁移清单及其公开名称清单，两者都覆盖从 `LogicalJoin` 到 `LogicalLock` 的 21 个算子。
- `logical_op_name_to_plan_codec`：固定名称映射；未知名称返回空字符串，而不是错误。
- `GenHash64Equals4LogicalOps`：固定清单入口，调用 `generate(&logical_operator_specs())`。名称保留 Go 风格，crate 根的 `#![allow(non_snake_case)]` 允许该 API。
- `validate_name`：仅接受首字符为 ASCII 字母或下划线、后续为 ASCII 字母数字或下划线的非空 Go 标识符。

## 执行流程

公共自定义流程从 `gen_hash64_equals_for_logical_ops` 开始：

1. 为每个 `LogicalOperator` 调用 `validate_name`。
2. 逐个字段调用 `public_field_spec`，把公共字段类别降低为内部 `FieldType`，同时丢弃 `Ignored` 字段。
3. 构造 `OperatorSpec`，保留调用者提供的 `codec_name`，再交给 `generate`。
4. `generate` 先复制固定 Go 文件头，然后对每个算子再次校验名称，依次输出 `Hash64` 和 `Equals`。
5. `Hash64` 先用 `codec_name` 哈希算子类型。顶层指针或切片字段先哈希 nil/non-nil 标志；非 nil 切片继续哈希长度和每个元素。标量转换到稳定宽度后调用相应 hasher 方法，复合值调用其 `Hash64(h)`。
6. `Equals` 先把 `other` 断言成相同算子指针；类型不符返回 `false`。随后处理 nil 接收者与 nil 对端，再递归比较字段。切片必须同时满足 nil 状态一致、长度一致和逐元素相等；无参与字段时写出 `_ = op2` 以保持生成的 Go 可编译。
7. 输出转换为 UTF-8 字节向量返回；文件不写磁盘，也不调用 `gofmt`。

固定清单流程由 `GenHash64Equals4LogicalOps` 构建 `logical_operator_specs` 后进入同一个 `generate`。Go 版本的 `main` 才负责将生成字节写入 `hash64_equals_generated.go`；Rust 文件没有对应 I/O 入口。

## 数据与状态

所有输入描述和中间结构均为本次调用拥有的普通值。`gen_hash64_equals_for_logical_ops` 为算子预分配 `Vec` 容量，字段名、算子名和 codec 表达式在转换时克隆；`generate` 使用局部 `String` 累积完整 Go 文件，最后通过 `into_bytes` 转移所有权。

生成结果的关键不变量是“相等对象必须产生相同哈希”。算子 codec 总是哈希流的第一项；顶层可空字段把 nil 状态编码进哈希；切片把 nil 状态和长度都编码进哈希，并按顺序递归元素。相等比较使用同样的字段顺序与类型递归，因此不会把 nil 切片和长度为零的非 nil 切片视为相等。

`logical_operator_specs` 和 `LOGICAL_STRUCTURES` 是两份静态覆盖信息：前者驱动生成，后者供测试与外部检查枚举名称。修改清单时必须保持两者以及 `logical_op_name_to_plan_codec` 同步。未知名称映射为空字符串这一行为意味着自定义 API 应显式提供 `codec_name`，不能依赖该映射兜底。

## 依赖与调用关系

RustCodeGraph 显示，`gen_hash64_equals_for_logical_ops` 的仓库内直接调用者是独立测试 `pkg/planner/core/generator/hash64_equals/hash64_equals_test.rs::TestHash64Equals`；其直接被调用者为 `validate_name` 和 `generate`。`GenHash64Equals4LogicalOps` 被同一测试以及 `hash64_equals_generator_test.rs::generated_output_matches_go_generator_contract` 使用；图中还关联 Go `main` 的同名调用，这是跨语言对照关系，不应解释成 Rust 运行时调用。

内部调用链为：

`GenHash64Equals4LogicalOps → logical_operator_specs → generate → write_hash_method/write_equals_method`。哈希侧继续到 `write_hash_field → write_hash_element`，相等侧继续到 `write_equals_element`；两个递归元素写出器都通过 `line` 追加文本，并在嵌套切片时使用 `nested_item_name`。

下游生成代码依赖 Go 的 `pkg/planner/cascades/base.Hasher`、`NilFlag`、`NotNilFlag` 和各字段自身的 `Hash64/Equals` 实现，还依赖 `pkg/util/plancodec` 的类型常量。Rust crate 自身的 Cargo manifest 没有依赖项；`lib.rs` 仅负责模块声明、公共再导出及将两个独立测试文件挂入 `cfg(test)`。

## 错误处理与边界

可恢复错误仅来自 `validate_name`：空算子名返回 `invalid empty Go identifier`，非法首字符或后续字符返回 `invalid Go identifier {name}`。`gen_hash64_equals_for_logical_ops` 在构造内部规格前校验一次，`generate` 又在写出每个算子前校验一次，固定清单入口也因后一次校验受到保护。

字段名和 `codec_name` 不做语法校验，错误内容可能生成无效 Go；这是公共元数据 API 的调用方边界。`logical_op_name_to_plan_codec` 对未知名称返回空字符串，也不报告错误。Rust 实现通过封闭的 `FieldType` 枚举避免 Go 版本在遇到不支持的反射类型时 `panic`，代价是新增类别必须显式扩展枚举和两个递归写出器。

顶层只有 `HashEqualsPointer` 和 `Slice` 生成 nil 标志；`HashEqualsDirect` 不额外判空，而是委托该值自己的方法。切片比较显式区分 nil 与空切片，并先检查长度再索引右侧，因此正常生成路径不会越界。浮点标量沿用 Go 的直接 `!=` 相等语义和 `HashFloat64` 哈希语义，包括其对 NaN、正负零的既有处理约定；本文件不额外归一化。

## 并发与资源生命周期

本文件没有全局可变状态、锁、通道、异步任务、线程、事务或外部句柄。每次调用只读静态字符串/清单，并独占自己的 `Vec`、规格值和输出 `String`，因此不同线程使用彼此独立输入并发调用时不会在本文件内产生共享状态竞争。

内存生命周期局限于单次生成：规格和输出在返回前创建，临时规格随函数结束释放，字节结果的所有权交给调用者。生成器不打开文件、不刷新缓冲区、不执行 Go 格式化器；落盘、原子替换和 I/O 错误处理属于外层工具。输出规模与算子数、字段数及嵌套切片深度线性相关，递归深度由 `FieldType::Slice` 的嵌套层数决定。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/planner/core/generator/hash64_equals/hash64_equals_generator.go`。两者共享算子清单、codec 映射、生成文件头、Hash64/Equals 控制流、nil 标志、切片长度/顺序递归和复合字段调用规则。Go 的 `GenHash64Equals4LogicalOps` 以真实 `logicalop` 零值列表为输入，`genHash64EqualsForLogicalOps` 通过反射和 struct tag 自动发现字段类型；Rust 无法反射 Go 类型，因此 `logical_operator_specs` 手工保存同一事实，公共入口则要求调用者传入 `LogicalOperator` 元数据。

Go 的 `EqualsElement`/`Hash64Element` 对不支持的反射类型会 panic，并用接口实现检查决定复合字段调用；Rust 在建模阶段就把支持集合封闭为 `FieldType`。Go 使用 `go/format.Source` 格式化输出，Rust 写出器直接生成与已提交文件相同的 gofmt 形状。Rust 接收者名为 `p`，Go 为 `op`；两个 Rust 对照测试只归一化这一词法差异后进行完整内容、行数和字节长度检查。

Go `main` 调用生成器并以 `0644` 写入目标文件；Rust `GenHash64Equals4LogicalOps` 只返回字节，不负责落盘。Go 注释声称新增 tagged 字段可由反射自动更新；Rust 固定元数据不会自动发现字段变化，所以迁移版本必须人工同步 `logical_operator_specs`，测试中的全文件漂移比较承担漏同步检测。

## 扩展指南

- 新增逻辑算子时，在 `logical_operator_specs` 添加精确字段规格，在 `LOGICAL_STRUCTURES` 添加名称，并在 `logical_op_name_to_plan_codec` 添加 codec 映射；同时核对 Go `GenHash64Equals4LogicalOps` 清单、真实 struct tag 和生成文件。
- 给现有算子新增或修改参与字段时，先从 Go 结构体的 ``hash64-equals:"true"`` 标签确认字段顺序与真实类型，再选择 `value`、`pointer`、`direct`、`slice` 或标量 `field`。值类型与指针/接口的 `Equals` 参数形状不同，选错会生成无法编译或语义错误的 Go。
- 新增公共字段类别时，同时扩展 `HashFieldKind`、`public_field_spec`、`FieldType`、`write_hash_element` 和 `write_equals_element`；明确它是否可空、是否需要地址化右值，以及哈希和相等是否保持同一递归结构。
- 若强化公共输入校验，最可能修改 `gen_hash64_equals_for_logical_ops`、`validate_name`，并新增对字段名和 codec 表达式的独立测试；需注意避免拒绝当前合法的 Go 选择器/表达式。
- 测试应继续放在独立文件 `hash64_equals_test.rs` 或 `hash64_equals_generator_test.rs`，不要嵌入生产源文件。至少覆盖固定输出与已提交 Go 文件一致、每个清单项同时生成两种方法、codec 非空、指针/切片 nil 分支、嵌套切片变量与索引、所有标量类别、`Ignored` 过滤、空字段算子以及非法名称错误。
- 性能风险主要来自重复构造完整输出与深层递归；兼容风险主要来自清单、字段顺序、codec 或 nil 语义偏离 Go。任何变更都应同时查看生成文件 diff，而不能只以 Rust API 返回成功作为完成证据。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源和测试均在索引中。
- RustCodeGraph 源码读取：`hash64_equals_generator.rs` 第 1–591 行；确认公共/私有类型、常量、21 项规格、codec 映射、写出器和校验器。
- RustCodeGraph 查询：`query gen_hash64_equals_for_logical_ops`、`query write_hash_method`、`query write_equals_method`；调用查询确认 `gen_hash64_equals_for_logical_ops → generate/validate_name`、`generate → write_hash_method/write_equals_method/validate_name`、`write_hash_method → write_hash_field/line`、`write_equals_method → write_equals_element/line`。
- crate 与模块证据：`pkg/planner/core/generator/hash64_equals/Cargo.toml`、`pkg/planner/core/generator/hash64_equals/lib.rs`。
- Go 对照证据：`pkg/planner/core/generator/hash64_equals/hash64_equals_generator.go`、`pkg/planner/core/generator/hash64_equals/hash64_equals_test.go`。
- 独立 Rust 测试证据：`pkg/planner/core/generator/hash64_equals/hash64_equals_test.rs`、`pkg/planner/core/generator/hash64_equals/hash64_equals_generator_test.rs`；前者覆盖清单、codec、生成文件漂移和字段级 nil 分支，后者归一化接收者名后逐行、行数和字节长度对照已提交 Go 文件。
- 任务是纯文档分析，按计划不运行 Cargo；结构验证命令及最终退出状态在交付记录中报告。
