# `pkg/expression/generator/compare_vec.rs`

## 文件定位

该文件属于独立 library crate `astersql-expression-generator`，由同目录 [`lib.rs`](lib.rs) 以公开模块 `compare_vec` 接入。[`Cargo.toml`](Cargo.toml) 声明该 crate 使用 Rust 2024 edition、`publish = false`，并以 `package.metadata.porting.go-package = "pkg/expression/generator"` 标明 Go 移植来源。它是离线的 Go 源码生成逻辑，不参与 SQL 表达式在 TiDB/AsterSQL 服务进程中的运行时求值。

文件的直接 Go 对照是 [`compare_vec.go`](compare_vec.go)。当前主工程真实的生成入口仍是 [`pkg/expression/builtin.go`](../builtin.go) 中的 `//go:generate go run generator/compare_vec.go`；该入口生成 `builtin_compare_vec_generated.go` 与 `builtin_compare_vec_generated_test.go`，二者由 `pkg/expression/BUILD.bazel` 纳入 Go 构建/测试。Rust 模块目前提供可测试的对等生成能力及返回内存生成物的 API，但没有替换这条 Go `go generate` 主链，也没有自行写盘的可执行入口。

## 核心职责

1. 用 `CompareContext` 与 `TypeContext` 描述“比较/合并函数 × 求值类型”的生成矩阵。比较项包括 `LT`、`LE`、`GT`、`GE`、`EQ`、`NE`、`NullEQ` 与 `Coalesce`，类型项包括 Int、Real、Decimal、String、Datetime、Duration 与 JSON。
2. 为普通比较生成 `vecEvalInt` 与 `vectorized() bool`：先向量化求值两个参数、合并 NULL 位图，再按类型选择正确的比较表达式并写入 0/1 结果。
3. 为 `NullEQ` 生成独立 NULL 语义：双 NULL 为 1、仅一侧 NULL 为 0、两侧非 NULL 时才比较值。
4. 为 `Coalesce` 生成固定长与变长两套向量路径，以及发生参数求值错误或新增 warning 时的逐行标量回退路径。
5. 生成配套 Go 测试/benchmark 用例表；普通比较是两个同类型参数并返回 Int，`Coalesce` 是三个同类型参数并返回该类型。
6. 通过外部 `gofmt` 做 Go 语法格式化，最终以 `(PathBuf, Vec<u8>)` 数组返回实现文件与测试文件，保持生成逻辑和写盘 IO 分离。

## 主要符号

- `HEADER`、`NEW_LINE`、`BUILTIN_COMPARE_IMPORTS`：生成文件的许可证、generated 标记、`package expression` 与 import 骨架。生成代码依赖 Go 的 `cmp`、`types` 和 `chunk`。
- `TypeContext`：类型模板上下文，字段分别描述 EvalType 名、表达式方法类型名、`chunk.Column` 方法后缀、Go 类型拼写及是否固定长。`type_name_go` 与 Go `helper.TypeContext` 对齐，但当前 Rust 渲染路径没有读取该字段。
- `TYPE_INT`、`TYPE_REAL`、`TYPE_DECIMAL`、`TYPE_STRING`、`TYPE_DATETIME`、`TYPE_DURATION`、`TYPE_JSON` 与 `TYPES_MAP`：七种类型的静态矩阵。`Datetime` 的表达式方法名是 `Time`，Duration 的列访问后缀是 `GoDuration`；String/JSON 标记为变长。
- `CompareContext` 与 `COMPARES_MAP`：保存生成签名名片段及普通比较相对零的操作符。`NullEQ`、`Coalesce` 的 `operator` 为空，因为它们走专用模板。
- `BUILTIN_COMPARE_VEC_TEMPLATE`：普通比较模板，生成两个参数缓冲区、NULL 合并、逐行比较及 `vectorized() == true`。
- `BUILTIN_NULL_EQ_COMPARE_VEC_TEMPLATE`：NULL-safe equality 专用模板，不调用 `MergeNulls`，而是逐行显式区分双 NULL、单 NULL和非 NULL 相等。
- `BUILTIN_COMPARE_VEC_TEST_HEADER` / `BUILTIN_COMPARE_VEC_TEST_TAIL`：生成 `vecGeneratedBuiltinCompareCases` 及两项测试、两项 benchmark 的外壳。
- `compare_expression`：按 `TypeContext.et_name` 选择 Go 比较表达式。JSON 使用 `types.CompareBinaryJSON`，String 使用带 `b.collation` 的 `types.CompareString`，Datetime/Decimal 调用类型自身的 `Compare`，Real/Duration 使用 `cmp.Compare`。
- `render`：为普通比较/`NullEQ` 替换五类显式占位符；固定长类型还生成 `buf0`/`buf1` 的切片访问，变长类型保持通过 getter 取值。
- `render_coalesce`：生成 Coalesce 的标量回退和向量实现。固定长结果先整体 resize，变长结果按行 append；Time 额外传播 FSP，Decimal/Duration 在回退赋值时做类型专用解包。
- `format_go_source`：私有 `gofmt` 子进程适配器。进程/管道错误返回 `Err(String)`；`gofmt` 语法失败则告警并返回未格式化原文。
- `generate_dot_go`：生产 Go 实现的公开纯生成入口。普通比较和 `NullEQ` 跳过 Int，`Coalesce` 覆盖全部七种类型。
- `generate_test_dot_go`：配套测试源码的公开生成入口；非 Coalesce 的 Int 用例全部跳过，Coalesce 每种类型构造三个 null 比例为 0.2 的默认数据生成器。
- `generate_one_file`：把指定前缀映射为 `<prefix>.go` 和 `<prefix>_test.go`，返回路径与内容但不写盘。
- `default_outputs`：以 `./builtin_compare_vec_generated` 为默认前缀，使用完整静态矩阵调用 `generate_one_file`。

文件没有 trait、impl、宏或条件编译项；公开面由常量、两个上下文结构体和四个生成函数构成，其余模板与渲染辅助函数保持私有。

## 执行流程

默认调用链是：

`default_outputs → generate_one_file → {generate_dot_go, generate_test_dot_go} → format_go_source`

实现文件的详细流程如下：

1. `generate_dot_go` 拼接 `HEADER`、空行与 import 块。
2. 外层遍历 `compares`，内层遍历 `types`。该顺序决定最终 Go 声明的稳定顺序。
3. `NullEQ + Int` 直接跳过；其余 `NullEQ` 进入专用模板。`Coalesce` 对全部类型调用 `render_coalesce`。普通比较遇到 Int 也跳过，否则调用 `render`。
4. `render` 根据类型补入切片访问和比较表达式。生成代码在调用两个子表达式的 `VecEval*` 后，将缓冲区交还 `bufAllocator` 的动作注册为 Go `defer`，再逐行写 Int64 布尔结果。
5. `render_coalesce` 先生成逐行 `fallbackEval*`。向量路径依次求值所有参数，并用 `warningCount` 比较求值前后的 warning 数；发生错误或新增 warning 时，新增 warning 会先被截断，然后回退到标量求值，以保留 Coalesce “只计算到首个非 NULL 参数”的可观察语义。
6. 固定长 Coalesce 为结果预分配带全 NULL 位图的列，每个参数只填仍为 NULL 的行；变长 Coalesce 先保留所有参数列，随后逐行选第一个非 NULL 值，否则 append NULL。
7. 完整文本传入 `format_go_source`，成功时使用 `gofmt` stdout，失败时按错误类别返回错误或保留原始文本。

测试文件生成链独立遍历同一矩阵：每个比较名形成 map 项；普通比较跳过 Int 并生成两个同类型参数，Coalesce 为七种类型各生成一个三参数用例；最后拼接测试与 benchmark 函数并调用 `gofmt`。

## 数据与状态

模块级数据均为只读静态常量，没有全局可变状态。`CompareContext` 和 `TypeContext` 都是 `Clone + Copy`，只持有 `'static` 字符串与布尔值，因此循环中按值复制，不涉及所有权共享或堆分配。调用者也可传入矩阵子集，公开生成函数并不强制使用 `COMPARES_MAP`/`TYPES_MAP`。

生成期间的可变状态仅存在于局部 `String out` 和模板替换产生的临时 `String`。最终内容转换为拥有所有权的 `Vec<u8>`；`generate_one_file` 再与拥有所有权的 `PathBuf` 配对。该 API 不缓存结果、不读取环境配置，也不改动工作区文件。

生成出的 Go 代码使用 `chunk.Column` 作为列状态。普通比较把两个输入 NULL 位图合并进输出；`NullEQ` 不传播 NULL，而是总为每行写出非 NULL 的布尔整数。Coalesce 的输出初始状态取决于列布局：固定长先 `Resize*(n, true)`，变长通过 `Reserve*` 后按行 append。固定长向量路径重复复用一个参数缓冲区；变长路径必须同时保存每个参数缓冲区，空间复杂度随参数数增长。

## 依赖与调用关系

RustCodeGraph 与源码共同确认的文件内调用边为：

- `default_outputs → generate_one_file`
- `generate_one_file → generate_dot_go`、`generate_test_dot_go`
- `generate_dot_go → render`、`render_coalesce`、`format_go_source`
- `render → compare_expression`
- `generate_test_dot_go → format_go_source`

上游接线方面，[`lib.rs`](lib.rs) 公开 `compare_vec` 模块。Rust 生产代码中没有找到 `default_outputs` 或其他公开生成 API 的调用者；直接 Rust 调用者位于独立测试 [`compare_vec_test.rs`](compare_vec_test.rs) 与 [`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs)。后者还验证完整分派矩阵、49 个 `vectorized` 方法、默认双文件路径和 Go 语法。

下游依赖只直接使用 Rust 标准库：`Path`/`PathBuf` 建立输出名，`Command`/`Stdio` 和 `Write` 驱动外部 `gofmt`。虽然 crate 的 `Cargo.toml` 声明 `gtmpl`、`tree-sitter`、`tree-sitter-go` 与开发依赖 `tempfile`，本文件本身没有使用这些 crate；它以 Rust 字符串拼接和显式占位符替换实现模板生成。

Go 侧真实主链是 `pkg/expression/builtin.go` 的 `go:generate → generator/compare_vec.go → builtin_compare_vec_generated{,_test}.go`，生成物再依赖 `pkg/expression` 的签名类型、`EvalContext`、buffer allocator、`types` 与 `chunk`。RustCodeGraph 的 `callees` 查询因 `default_outputs`/`generate_one_file` 等常见名称产生了跨文件聚合和 `builtin_compare_vec.rs` 假边；这些噪声未作为本说明的调用事实。

## 错误处理与边界

Rust API 统一返回 `Result<_, String>`。`format_go_source` 启动 `gofmt`、取得 stdin、写入源码或等待子进程失败时，把底层错误文本转换为 `String` 并向上传播；`generate_one_file` 用 `?` 保证实现文件生成失败时不会继续构造成功结果，测试文件失败也不会返回部分数组。

需要注意的特殊边界如下：

- `gofmt` 非零退出不是 Rust API 错误：函数向 stderr 打印 warning，并返回未格式化的原始源码。这与 Go 对照中 `format.Source` 失败后保留原文用于调试的策略相同，但调用者仅检查 `Result::is_ok()` 无法区分“已格式化”和“语法失败后回退”。
- 比较矩阵没有输入验证。未知 `et_name` 会在 `compare_expression` 中落入 `cmp.Compare(arg0[i], arg1[i])`，但 `fixed`、列后缀或模板所需 import 可能不匹配；未知 `compare_name` 则按普通比较处理。扩展矩阵时必须成组维护字段和模板。
- 普通比较与 `NullEQ` 对 Int 的跳过是有意边界，因为 Int 向量比较已有手写实现；Rust 测试明确断言不生成 `builtinLTIntSig` 与 `builtinNullEQIntSig`。Coalesce Int 不跳过。
- `NullEQ` 的结果列不调用 `MergeNulls`，确保 SQL NULL-safe equality 自身不返回 NULL；模板依赖 `ResizeInt64(n, false)` 将未命中相等分支的行保持为 0。
- Coalesce 不能简单保留所有向量化副作用：后续参数本不应在已有非 NULL 值时被观察。新增错误或 warning 会触发标量回退，并通过 `truncateWarnings` 撤销本轮新增 warning。
- 变长 Coalesce 在参数数为零时不会 append 任何行；正常内置函数构造应保证至少一个参数，但本生成器不验证该运行时前置条件。
- 本模块不写盘，因此没有文件覆盖、权限或部分提交错误；真正写出返回内容的调用者必须自行提供一致性/原子性策略。

## 并发与资源生命周期

Rust 生成过程同步且无共享可变状态，没有线程、锁、异步任务、通道或事务。每次调用 `format_go_source` 都启动独立 `gofmt` 子进程，将完整源码写入其 stdin，再用 `wait_with_output` 等待并收集 stdout/stderr。stdin 句柄在写完并离开表达式后释放；子进程被同步等待，不会由本函数故意留在后台。

路径和生成字节均由返回值拥有，离开局部作用域后其余缓冲区按 RAII 释放。多个线程可以各自调用纯生成 API；代码没有内部缓存或锁，但会各自启动 `gofmt`，并发量大时主要风险是进程启动和内存开销。`eprintln!` warning 可能交错，但不会改变返回数据。

生成出的 Go 比较函数按调用获得两个 buffer，并用 `defer b.bufAllocator.put(...)` 保证函数返回时归还；第二个 buffer 获取失败时，第一个已经注册归还。Coalesce 固定长路径只持有一个复用 buffer，变长路径为每个参数持有一个 buffer，均在函数退出时通过 defer 归还。生成代码本身没有启动 goroutine，也没有跨行共享锁；其线程安全性依赖 `builtin*Sig`、参数表达式、`EvalContext` 与 allocator 的既有调用契约。

## 与 Go 版本的对应关系

Rust 文件逐段移植 [`compare_vec.go`](compare_vec.go)：

- `TypeContext` 七个常量对应 Go helper 包的 `TypeInt` 至 `TypeJSON`；`CompareContext`、`COMPARES_MAP`、`TYPES_MAP` 对应 Go 的同名概念与相同顺序。
- 三个 Rust 模板对应 Go 的 `builtinCompareVecTpl`、`builtinNullEQCompareVecTpl` 与 `builtinCoalesceCompareVecTpl`。Rust 将 Go `text/template` 的分支提前落实到 `compare_expression`、`render` 和 `render_coalesce`，生成语义保持为类型专用比较、NULL-safe equality 与 Coalesce 回退。
- `generate_dot_go` 对应 Go `generateDotGo`，两者都跳过普通比较/NullEQ 的 Int，并让 Coalesce 覆盖七种类型。
- `generate_test_dot_go` 对应 Go `generateTestDotGo`，保留每个比较函数分组、非 Coalesce 跳过 Int，以及 Coalesce 三参数/0.2 NULL 比例的数据生成器。
- `format_go_source` 对应 Go `format.Source` 加失败回退。差异是 Rust 依赖 PATH 中的外部 `gofmt`；无法启动/通信会返回错误，而 Go 版本调用进程内标准库。两者遇到 Go 语法格式化失败都会保留原文。
- Go `generateOneFile` 立即依次写入 `.go` 与 `_test.go`；Rust `generate_one_file` 只返回两份路径/内容，不执行 IO。Rust `default_outputs` 是便于测试/接线新增的默认前缀封装。
- Go 文件有可由 `go run` 执行的 `main`；Rust crate 只有 library target，本文件无 `main`，所以当前不能直接承担 `go:generate` 命令。

独立 Rust 测试 [`compare_vec_test.rs`](compare_vec_test.rs) 验证 Coalesce 固定长/变长/FSP/warning 回退文本及测试数据生成器；[`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs) 验证完整分派矩阵、49 个签名、路径配对与 `gofmt` 可解析性。Go 的生成测试文件 [`pkg/expression/builtin_compare_vec_generated_test.go`](../builtin_compare_vec_generated_test.go) 则是生成结果的运行时测试面；手写 [`pkg/expression/builtin_compare_vec_test.go`](../builtin_compare_vec_test.go) 和 [`pkg/expression/builtin_compare_test.go`](../builtin_compare_test.go) 补充普通比较、NullEQ 与 Coalesce 行为覆盖。

## 扩展指南

- 新增 EvalType 时，同时扩展 `TypeContext` 常量、`TYPES_MAP`、`compare_expression` 和 Coalesce 的固定长/变长、取值、存值规则，并同步 Go helper 与 `compare_vec.go`。尤其核对 `type_name` 与 `type_name_in_column` 不同的情况、是否需要额外 import、NULL 表示和返回值所有权。
- 新增比较算子时扩展 `COMPARES_MAP`，确认它能否使用普通“比较结果与零做运算”模板；若像 `NullEQ`/`Coalesce` 有特殊 NULL 或求值语义，应在 `generate_dot_go` 中使用独立分派，不能只填写操作符。
- 修改普通比较时，优先落在 `compare_expression` 或两个比较模板；同步验证 JSON、collation-sensitive String、Decimal 借用、Datetime 与 NaN/Real 比较语义。Int 的手写实现位于生成范围外，不能因矩阵扩展意外覆盖。
- 修改 Coalesce 时重点维护“首个非 NULL”和“后续参数副作用不可观察”的契约。任何 warning/error 策略、Time FSP、Decimal 解引用、Duration 解包或变长 append 调整，都应同时更新 [`compare_vec_test.rs`](compare_vec_test.rs) 以及 Go 运行时回归测试。
- 调整输出命名或写盘接线应在 `generate_one_file`/`default_outputs` 的边界完成；若增加 Rust 写盘函数，应测试双文件顺序、失败时部分更新和原子发布，不要把测试逻辑放进生产源文件。
- 若计划让 Rust 生成器替换 `go:generate`，需要另行提供 Cargo binary/稳定启动命令，并修改 `pkg/expression/builtin.go`。还应比较 Rust 与 Go 生成物，而不能仅验证包含若干字符串；外部 `gofmt` 的可用性也必须成为工具链前置条件。
- 性能风险主要在大模板字符串的反复 `replace`、变长 Coalesce 对全部参数列的保留及每次生成启动子进程；兼容风险集中在 Go 模板格式、类型矩阵、NULL/warning/FSP 语义和生成签名名保持一致。

## 验证依据

- 目标源码：[`compare_vec.rs`](compare_vec.rs)，完整核对 510 行中的 4 个公开生成函数、2 个上下文结构体、静态矩阵、模板、渲染辅助函数和 `gofmt` 边界。
- crate 与模块入口：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)，确认独立 library crate、公开模块、依赖清单及两个独立 Rust 测试模块的接线；同目录没有 `doc.go`。
- Go 对照：[`compare_vec.go`](compare_vec.go) 与 [`helper/helper.go`](helper/helper.go)，确认模板矩阵、类型字段、跳过 Int、Coalesce 回退、生成顺序、格式化失败策略和 Go 写盘入口。
- 主工程入口与生成物：[`pkg/expression/builtin.go`](../builtin.go)、`pkg/expression/BUILD.bazel`、[`pkg/expression/builtin_compare_vec_generated.go`](../builtin_compare_vec_generated.go) 和 [`pkg/expression/builtin_compare_vec_generated_test.go`](../builtin_compare_vec_generated_test.go)，确认当前实际 `go:generate` 与构建/测试位置。
- 测试证据：[`compare_vec_test.rs`](compare_vec_test.rs) 验证 Coalesce 与生成测试用例；[`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs) 验证分派矩阵、49 个生成签名、默认输出路径及 Go 语法；Go 的手写/生成 compare 测试提供运行时语义参照。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/generator` 确认目标及直接关联文件被索引；`query`/`node --file` 定位 `CompareContext`、`generate_dot_go`、`generate_test_dot_go`、`generate_one_file`、`default_outputs` 并读取全文件；`callees` 验证正文列出的文件内调用链。常见符号名导致的跨文件聚合噪声已用精确源码和 `rg` 调用点排除。
- 本任务是纯文档分析，按总计划不运行 Cargo；交付仅执行任务规定的 11 章节结构检查与人工事实复核。
