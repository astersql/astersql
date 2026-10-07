# `pkg/expression/builtin_compare_vec_generated.rs`

## 文件定位

该文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），是从 TiDB Go 生成文件 `pkg/expression/builtin_compare_vec_generated.go` 迁移出的“向量比较 + `COALESCE`”语义内核。`pkg/expression/lib.rs` 通过 `#[path = "builtin_compare_vec_generated.rs"] mod builtin_compare_vec_generated_kernel;` 无条件编译它，但模块本身是私有的；仅在 `cfg(test)` 下由 `expression_compare_vec_generated` 门面重新导出其公开项。

因此，当前文件不是 SQL 表达式注册、签名选择或 `chunk::Column` 调度入口。仓库级符号搜索未发现两份专用 Rust 测试之外的调用者，说明它目前承担可独立验证的迁移内核角色，尚未接到 Rust 生产表达式执行主链。Go 文件中的 `builtin*Sig.vecEval*` 才是当前 Go 侧按具体签名接入运行时的生成代码。

## 核心职责

文件包含两组相互独立的能力：

1. 比较内核：以 `CompareOp` 表达 `<`、`<=`、`>`、`>=`、`=`、`!=` 和 NULL-safe equality `<=>`，由 `vec_compare_by` 统一完成等长校验、逐行 NULL 语义和 0/1 结果编码；六个公开类型入口注入各自的原生比较器。
2. `COALESCE` 内核：以 `VectorExpression<T>` 抽象参数表达式，`vec_coalesce` 先尝试整列求值并逐参数填充仍为 NULL 的结果行；若投机性向量求值报错、产生新警告或返回错误长度，则整式改为逐行短路求值，以保存 Go 标量语义。

这两组实现共享 `Option<T>` 表示 SQL NULL，返回值以 `Vec<Option<_>>` 保存逐行结果。文件不负责类型推导、表达式树构造、内存配额、物理 `Chunk` 缓冲区复用或函数注册。

## 主要符号

- `CompareOp`：七种 SQL 比较操作。`NullEq` 是唯一不传播 NULL 的操作：双 NULL 为 1，单侧 NULL 为 0。
- `EvalError`：当前仅有 `ColumnLengthMismatch { expected, actual }` 与 `Evaluation(String)`。前者由比较内核直接产生，后者供 `VectorExpression` 实现包装求值失败。
- `EvalContext`：最小警告上下文，保存 `Vec<String>`；公开 `append_warning`、`warning_count`、`warnings`，内部 `truncate_warnings` 用于撤销向量探测产生的警告。
- `bool_to_int64`：将谓词编码为 SQL 内建返回格式，真为 1、假为 0。
- `vectorized`：恒返回 `true`，对应 Go 每个生成签名的 `vectorized() bool { return true }`。
- `comparison_result`、`vec_compare_by<T>`：比较路径的内部公共算法；前者将 `Ordering` 映射到算子结果，后者处理列长度、NULL 与逐行收集。
- `compare_real`：复刻 Go `cmp.Compare` 的浮点全序约定，规定 NaN 等于 NaN，且 NaN 小于非 NaN。
- `vec_compare_real`、`vec_compare_decimal`、`vec_compare_string`、`vec_compare_time`、`vec_compare_duration`、`vec_compare_json`：六个公开类型入口，分别使用浮点特殊序、`MyDecimal::Compare`、collator、`Time::Compare`、有符号 `i64::cmp` 和 `CompareBinaryJSON`。
- `VectorExpression<T>`：同时要求整列 `vec_eval` 和单行 `eval_row`，使 `COALESCE` 可在两条路径之间切换。
- `fallback_coalesce<T>`：按行、按参数从左到右求值，遇到首个非 NULL 即停止该行后续参数。
- `vec_coalesce<T>`：通用向量实现。各类型包装 `vec_coalesce_{int,real,decimal,string,time,duration,json}` 只固定类型；其中 `vec_coalesce_time` 还对所有非 NULL 结果调用 `Time::SetFsp(result_fsp)`。

文件没有模块级常量、条件编译项或可变全局状态；唯一 trait 是 `VectorExpression<T>`，唯一 impl 是 `EvalError` 的 `Display` 与 `Error` 实现以及 `EvalContext` 的固有方法。

## 执行流程

比较调用的流程如下：

1. 类型入口接收操作符以及左右 `&[Option<T>]`；字符串入口还接收 collation 名称。
2. `vec_compare_by` 先比较切片长度；不一致立即返回 `ColumnLengthMismatch`，避免 `zip` 静默截断。
3. 每行按 NULL 组合分支：`NullEq` 的 `(None, None)` 返回 1、单侧 NULL 返回 0；普通操作任一侧 NULL 返回 `None`。
4. 双侧非 NULL 时调用类型比较器得到 `Ordering`，再由 `comparison_result` 计算 0/1。
5. 结果按输入顺序收集，成功时长度严格等于左右输入长度。

`COALESCE` 调用的流程如下：

1. `vec_coalesce` 记录进入时的警告数，并建立长度为 `rows`、初值全为 NULL 的结果。
2. 对每个参数调用一次 `vec_eval(context, rows)`。即使前面参数已覆盖某些行，后续参数仍会整列求值，这是产生额外错误或警告的根源。
3. 若本次或此前向量参数报错，或上下文警告数高于进入时基线，则截断新增警告（如有），并调用 `fallback_coalesce` 重算整个表达式。向量错误本身不会直接返回，因为逐行短路可能根本不会触达错误参数。
4. 若向量结果长度不是 `rows`，同样回退；当前分支不会修改警告。
5. 正常向量路径只把非 NULL 值写入尚为 NULL 的结果槽，保持“首个非 NULL 胜出”。
6. 回退路径逐行从左到右调用 `eval_row`；首个非 NULL 后立刻停止该行，实际触达的标量错误以 `EvalError` 向上传播。
7. 时间包装器在通用合并成功后统一设置结果 FSP；空参数列表或零行输入自然返回全 NULL/空向量。

## 数据与状态

- SQL NULL：比较输入、比较结果和 `COALESCE` 值均使用 `Option`；普通比较可输出 NULL，而 `NullEq` 的每行结果总是 `Some(0|1)`。
- 比较结果：使用 `i64` 的 0/1，而不是 Rust `bool`，与 SQL 内建函数的整数结果一致。
- Duration：以有符号 `i64` 纳秒值表示，对齐 Go `time.Duration` 的比较语义；该文件不携带显示精度或类型元数据。
- Decimal、JSON、Time：直接复用 `types-decimal`、`types-json-functions`、`types-time` 的领域类型；文件通过 `pub use` 暴露三者，主要便于当前测试门面使用。
- 字符串校对：`vec_compare_string` 每次调用先用 `collate::GetCollator(collation)` 获取比较器。collation 名称的合法性与回退策略由 `crate::collate` 决定，不在本文件验证。
- 警告状态：`EvalContext.warnings` 是本地拥有的字符串列表。向量 `COALESCE` 只以调用开始时的数量作为回滚点，不区分警告来源或类别。
- 输出所有权：所有入口新建并返回 `Vec`；没有借用输出缓冲区，也没有 Go 版 `bufAllocator` 的复用机制。

## 依赖与调用关系

上游装配关系为 `pkg/expression/lib.rs` → 私有模块 `builtin_compare_vec_generated_kernel`。测试关系为 `lib.rs` 的 `cfg(test)` 门面 → `pkg/expression/builtin_compare_vec_generated_test.rs` 与 `pkg/expression/builtin_compare_vec_generated_5_aster_unit_test.rs` → 本文件公开入口。RustCodeGraph 对 `vec_compare_*` 和 `vec_coalesce` 的调用边也只落在本文件包装函数和这两份测试中；进一步的全仓 `rg` 没有发现生产调用点。

下游直接依赖包括：

- `crate::collate::GetCollator` 与 collator 的 `Compare`：字符串排序规则。
- `types_decimal::mydecimal::MyDecimal::Compare`：DECIMAL 比较。
- `types_time::Time::{Compare, SetFsp}`：时间比较与 `COALESCE` 输出精度。
- `types_json_functions::{BinaryJSON, CompareBinaryJSON}`：Binary JSON 比较。
- Rust 标准库 `Ordering`、`fmt`、`Vec`、`Option`：排序、错误显示和列式容器。

`pkg/expression/Cargo.toml` 明确声明 `types-decimal`、`types-json-functions`、`types-time` 的本地 crate 依赖；collation 则通过 expression crate 内部模块接入。该文件未使用 feature gate，也未引入异步运行时或外部 IO。

## 错误处理与边界

- 左右比较列长度不一致是显式错误，`expected` 记录左列长度、`actual` 记录右列长度；不会部分计算。
- 类型比较器闭包本身不可失败，因此 Decimal、字符串、Time 和 JSON 底层比较在此 API 中没有错误通道。尤其是未知 collation 的具体行为必须以 `crate::collate` 为准，不能从本文件推断为错误。
- `VectorExpression::vec_eval` 的错误会触发标量回退而非直接传播；只有回退时实际访问到的 `eval_row` 错误才返回。此行为用于避免向量求值提前执行原本会被 `COALESCE` 短路掉的表达式。
- 向量新增警告也触发回退，并先恢复调用前警告列表。专用测试验证既有警告保留、投机性警告删除。
- 向量返回错误长度触发回退，不产生 `ColumnLengthMismatch`。实现没有捕获 `eval_row` 对越界行的错误实现或 panic；trait 实现者必须能覆盖 `0..rows`。
- `evaluated.expect(...)` 位于已检查 `is_err()` 之后，正常控制流不会因 `Err` 触发该 panic。
- `rows` 可为 0，参数可为空；空参数且 `rows > 0` 时结果保持全 NULL，符合当前泛型实现，但专用测试未单独覆盖该边界。

## 并发与资源生命周期

本文件没有线程、任务、锁、channel、事务、网络或文件资源。所有比较状态均局限于一次栈上调用及其返回 `Vec`。`EvalContext` 通过 `&mut` 独占借用传递，使同一次调用中的警告追加与回滚串行可见，但该类型未自行提供跨线程同步。

`VectorExpression` 没有 `Send`/`Sync` 约束；参数以共享 trait 对象引用传入，内部是否使用可变状态由实现者决定。测试 mock 使用 `Cell` 统计调用次数，也说明当前接口允许非线程安全实现。若未来在并行执行器共享表达式，必须在上层增加所有权隔离或同步约束，不能据此文件宣称线程安全。

与 Go 实现相比，Rust 内核每次分配结果和参数返回向量，没有 `bufAllocator.get/put` 的池化生命周期。`T: Clone` 是标量回退和测试表达式复制值的接口成本；JSON、字符串等较大值可能发生克隆，接入热路径前需评估。

## 与 Go 版本的对应关系

Go 生成文件把 7 个比较算子乘以 6 种类型展开为 42 个 `vecEvalInt` 方法，并为 7 种 `COALESCE` 返回类型分别生成向量方法与标量回退。Rust 用 `CompareOp + vec_compare_by` 和泛型 `vec_coalesce` 去重，保持的核心语义包括：

- 普通比较合并两侧 NULL；`NullEQ` 双 NULL 为真、单侧 NULL 为假。
- Real 使用 Go `cmp.Compare` 的 NaN 顺序；Decimal、字符串、Time、Duration、JSON 使用对应领域比较规则。
- 每个生成签名报告可向量化。
- `COALESCE` 向量求值可能观察到标量短路不会观察到的错误/警告，因此出现任一情况时撤销新警告并整式按行回退。
- 时间结果采用返回类型的 FSP。

结构上的差异也很重要：Go 方法直接持有 `b.args`、`b.tp`、`chunk.Chunk`、输出 `chunk.Column` 和缓冲区分配器，是已接线的表达式签名实现；Rust 文件只接受切片或 `VectorExpression` trait 对象，不含 builtin signature 类型、chunk 适配和注册。Go 的字符串/JSON `COALESCE` 会先保留每个参数列再逐行选择，其他类型复用单个临时列；Rust 对所有类型使用统一“逐参数填空”的拥有型向量算法，结果语义由现有测试验证，但内存布局并非逐句翻译。

Go 测试 `builtin_compare_vec_generated_test.go` 声明了全部算子、类型及三参数 `COALESCE` 的通用向量测试/基准矩阵；Rust 的 `builtin_compare_vec_generated_test.rs` 覆盖全部比较组合，`builtin_compare_vec_generated_5_aster_unit_test.rs` 进一步覆盖 NULL、NaN、collation、列长错误、警告回滚、短路错误和时间 FSP。Rust 目前没有对等的基准，也没有证明其已替代 Go 运行时路径。

## 扩展指南

- 新增比较算子：先扩展 `CompareOp` 和 `comparison_result`，明确 NULL 规则；同步更新两个 Rust 测试中的算子矩阵，并核对 Go 生成器 `pkg/expression/generator/compare_vec.rs` 的 `COMPARES_MAP`/输出语义。若算子不是二值排序谓词，不应强塞入 `Ordering` 映射。
- 新增比较类型：增加薄类型入口并复用 `vec_compare_by`，选择领域类型的权威比较器；同步补齐 `builtin_compare_vec_generated_test.rs` 的全算子覆盖及 `_5_aster_unit_test.rs` 的类型边界。字符串类类型必须显式处理 collation。
- 新增 `COALESCE` 类型：通常只需增加固定类型包装；若有结果后处理（如 Time FSP），放在通用合并成功之后，并同时验证向量与回退结果。测试仍应保存在独立 `*_test.rs` 文件，不能内嵌进生产源文件。
- 接入生产执行链：需要在现有 builtin signature/chunk 接口与这些拥有型向量 API 之间建立适配，并解决错误上下文、警告类型、输出缓冲复用及表达式注册；不能仅把私有模块改为公开就视为完成接线。
- 性能修改：重点衡量 `Vec<Option<T>>` 分配、`T: Clone`、所有参数整列求值和字符串/JSON 克隆。优化必须保留遇错/遇警告时的全式标量重算和警告回滚不变量。
- 兼容性修改：优先与 Go 生成文件和生成器模板对齐，尤其不可改用 Rust `f64::partial_cmp` 的默认 NaN 行为，也不可让普通比较把 NULL 编码为 0。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；随后使用 `explore`、`node --file`、`query`、`callers`/`callees` 核对本文件符号及调用关系。
- `pkg/expression/builtin_compare_vec_generated.rs`：完整 404 行实现，重点为 `CompareOp`、`EvalError`、`EvalContext`、`vec_compare_by`、六个比较入口、`VectorExpression`、`fallback_coalesce`、`vec_coalesce` 与七个类型包装。
- `pkg/expression/lib.rs`：生产期私有模块装配、测试期 `expression_compare_vec_generated` 重导出，以及两份测试模块声明。
- `pkg/expression/Cargo.toml`：crate 名称、`lib.rs` 入口及 `types-decimal`、`types-json-functions`、`types-time` 等依赖边界。
- `pkg/expression/builtin_compare_vec_generated.go`：42 个展开比较方法、7 组 `COALESCE` 向量/回退方法、警告回滚、buffer 生命周期和 Time FSP 逻辑。
- `pkg/expression/generator/compare_vec.rs`：默认生成目标及 Go 主文件/测试文件生成入口。
- `pkg/expression/builtin_compare_vec_generated_test.go`：Go 全算子/类型测试与基准用例矩阵。
- `pkg/expression/builtin_compare_vec_generated_test.rs`：Rust 全部 7 算子 × 6 类型的基本结果与 NULL 组合覆盖。
- `pkg/expression/builtin_compare_vec_generated_5_aster_unit_test.rs`：Rust 细化边界与 `COALESCE` 回退行为覆盖。
- 全仓精确符号搜索：排除目标源文件及两份专用 Rust 测试后，`vec_compare_*` 和 `vec_coalesce_*` 均无使用点，支持“尚无生产调用者”的判断。

本任务是纯文档分析，未运行 Cargo 或代码测试。完成检查以任务指定的 11 章节结构命令为准，并人工复核上述源码、装配、Go 对照与测试证据；生产链实际执行覆盖仍未由运行时测试证明。
