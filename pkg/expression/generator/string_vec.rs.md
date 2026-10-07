# `pkg/expression/generator/string_vec.rs`

## 文件定位

该文件属于独立 Rust crate `astersql-expression-generator` 的 `string_vec` 模块，由 [`lib.rs`](./lib.rs) 以 `pub mod string_vec` 暴露。它不是 SQL 表达式的运行时求值实现，而是 Go 源码生成器：在内存中构造 `FIELD` 函数的向量化 Go 实现及配套测试源码，并可将二者写入 `builtin_string_vec_generated.go` 与 `builtin_string_vec_generated_test.go`。

应用侧的原始生成入口仍是 [`../builtin.go`](../builtin.go) 中的 `//go:generate go run generator/string_vec.go`；生成后的代码落在 [`../builtin_string_vec_generated.go`](../builtin_string_vec_generated.go)，并为 [`../builtin_string.go`](../builtin_string.go) 定义的 `builtinFieldIntSig`、`builtinFieldRealSig`、`builtinFieldStringSig` 补充 `vecEvalInt` 与 `vectorized` 方法。因此，本文件位于“开发期代码生成”链路，而不是请求期 SQL 执行链路。

## 核心职责

- 用 `TypeContext` 描述模板展开所需的 Go 求值类型、列访问器名称以及是否为固定长类型。
- 按 `Int`、`Real`、`String` 的固定顺序生成三个 `builtinField*Sig.vecEvalInt` 实现；整数和浮点数使用列切片直接比较，字符串使用签名中的排序规则比较器 `b.ctor.Compare`。
- 生成 `ast.Field` 对应的三组向量化单元测试/基准测试用例声明。
- 尝试通过外部 `gofmt` 格式化生成文本；格式化不可用或失败时保留未格式化但完整的原始源码。
- 同时提供纯内存产物接口 `generate_dot_go`、`generate_test_dot_go`、`default_outputs` 和落盘接口 `generate_one_file`。

文件中保留的 `BUILTIN_STRING_VEC_TPL` 与 `BUILTIN_STRING_VEC_TEST_TPL` 是 Go `text/template` 正文的迁移对照物，但当前 Rust 生成路径实际由 `render_string_vec` 和 `generate_test_dot_go` 中的 Rust `format!` 展开；两个模板常量带有 `#[allow(dead_code)]`，不能把它们误认为当前执行入口。

## 主要符号

- `pub struct TypeContext`：单次类型展开的不可变描述。`et_name` 供测试用例生成 `types.ET*`；`type_name` 供实现生成 `VecEval*` 与 `builtinField*Sig`；`type_name_in_column` 供固定长列生成 `Int64s`/`Float64s`；`fixed` 决定走切片比较还是逐值比较。
- `TYPE_INT`、`TYPE_REAL`、`TYPE_STRING`：三个公开类型上下文。前两者 `fixed = true`，字符串 `fixed = false`。
- `types_map() -> [TypeContext; 3]`：确定生成顺序为 Int、Real、String；数组按值返回，元素因 `Copy` 可直接遍历。
- `render_string_vec(TypeContext) -> String`：展开单一类型的 `vecEvalInt` 和恒为 `true` 的 `vectorized` 方法，是实现文件生成的核心内部函数。
- `pub fn generate_dot_go() -> Result<Vec<u8>, String>`：拼接版权头、Go package/import 和三个类型实现，随后尝试 `gofmt`。当前函数内部没有产生 `Err(String)` 的路径；`Result` 主要保持生成器 API 的统一形状。
- `pub fn generate_test_dot_go() -> Result<Vec<u8>, String>`：生成 `vecGeneratedBuiltinStringCases`、两个测试入口和两个基准入口，也采用“格式化失败则返回原文”的策略。
- `format_go_source(&[u8]) -> io::Result<Vec<u8>>`：启动 `gofmt` 子进程，经管道写入 stdin，并根据退出状态返回格式化输出或携带 stderr 的 `InvalidData` 错误。
- `pub fn generate_one_file(&Path) -> io::Result<()>`：先写 `<prefix>.go`，成功后再写 `<prefix>_test.go`；第一步失败会阻止第二步。
- `pub fn default_outputs() -> Result<[(PathBuf, Vec<u8>); 2], String>`：以 `./builtin_string_vec_generated` 为默认前缀返回两个内存产物，不执行写盘。

## 执行流程

实现源码的内存生成从 `generate_dot_go` 开始：

1. 将 `HEADER`、`NEW_LINE` 和 `BUILTIN_STRING_IMPORTS` 拼成 Go 文件前缀。
2. `types_map` 依次返回 Int、Real、String，`render_string_vec` 为每种类型追加一个 `vecEvalInt` 和一个 `vectorized`。
3. 固定长类型生成 `buf0.Int64s`/`buf1.Int64s` 或 `Float64s` 切片并以 `==` 比较；字符串不取固定长切片，而是调用 `b.ctor.Compare(GetString(...), GetString(...)) == 0`。
4. `format_go_source` 尝试调用 `gofmt`；成功采用 stdout，失败则由 `unwrap_or_else` 回退到未格式化源码。

生成出的 `vecEvalInt` 对每个 chunk 批量执行 `FIELD(search, candidate...)`：先借出两个临时列缓冲区并计算第一个参数；结果列全部初始化为 `0`；再按候选参数下标从 `1` 向后求值。每一行若已经匹配、搜索值为空或当前候选为空则跳过，否则比较值，相等时把当前候选下标写入结果。因此它保留第一个匹配位置，无匹配或涉及空搜索值时保持 `0`。

测试源码由 `generate_test_dot_go` 构造：对三种类型各生成一个“四个同类型子表达式、返回 Int”的 `vecExprBenchCase`，再连接到 `testVectorizedEvalOneVec`、`testVectorizedBuiltinFunc` 及对应基准助手。

落盘时，`generate_one_file` 先调用 `generate_dot_go` 并写实现文件，再调用 `generate_test_dot_go` 并写测试文件。RustCodeGraph 显示它的直接生产调用仅为这两个生成函数；仓库内直接调用者是独立测试 `generation_writes_formatted_implementation_and_test_pair`。`default_outputs` 则直接调用两个内存生成函数，当前图中未发现上游调用者。

## 数据与状态

该模块没有可变全局状态。所有模板、文件头和导入块均为 `&'static str` 常量；`TypeContext` 只含静态字符串和布尔值，并实现 `Clone + Copy + Eq`。一次生成所需状态局限于局部 `String`、`Vec<u8>` 和路径值。

生成出的 Go 运行时代码维护两类逐批状态：来自 `bufAllocator` 的 `buf0`/`buf1` 临时列，以及长度为输入行数的 Int64 结果列。结果值 `0` 是“未找到”的哨兵，正整数是第一个匹配候选在参数列表中的位置。`i64s[j] > 0` 的提前跳过保证后续候选不会覆盖首个匹配。

`fixed` 是重要生成不变量：当前只有 Int 和 Real 为真，且其 `type_name_in_column` 必须分别与 chunk 的 `Int64s` 和 `Float64s` 访问器一致；String 必须为假，才能保留排序规则语义而不是按原始字节或 Rust 字符串相等性比较。

## 依赖与调用关系

模块自身只直接使用 Rust 标准库：`std::fs` 负责写文件，`std::path` 负责派生两个目标路径，`std::process` 与 `std::io::Write` 负责驱动 `gofmt`。[`Cargo.toml`](./Cargo.toml) 将其归入 edition 2024、`publish = false` 的 `astersql-expression-generator` crate；crate 级的 `gtmpl`、`tree-sitter`、`tree-sitter-go` 依赖由生成器集合共享，但本文件没有直接引用它们，`tempfile` 仅由独立测试使用。

RustCodeGraph 核对的主要内部边为：

- `generate_dot_go -> types_map -> render_string_vec`，并调用 `format_go_source`。
- `generate_test_dot_go -> types_map`，并调用 `format_go_source`。
- `generate_one_file -> generate_dot_go`，随后 `-> generate_test_dot_go`。
- `default_outputs -> generate_dot_go` 与 `-> generate_test_dot_go`。
- `string_vec_test.rs::generation_writes_formatted_implementation_and_test_pair -> generate_one_file`。

生成产物的运行时下游是 `pkg/expression`：`fieldFunctionClass.getFunction` 根据参数类型选择三种 `builtinField*Sig`，`distsql_builtin.go` 也按 `tipb.ScalarFuncSig_Field*` 重建相应签名；本文件生成的方法随后满足这些签名的向量化求值路径。

## 错误处理与边界

- `format_go_source` 可能因找不到 `gofmt`、无法启动进程、管道写入失败或非零退出而返回 `io::Error`。两个内存生成函数刻意吞掉该错误并返回原始源码，因此“生成成功”不等于“已由 gofmt 格式化”。
- `format_go_source` 对已配置为 piped 的 stdin 使用 `expect("piped gofmt stdin")`；在当前构造不变量下应存在 stdin，若标准库违反该不变量则会 panic。
- `generate_one_file` 保留真实文件 I/O 错误。实现文件写入失败时测试文件不会写；实现文件成功而测试文件失败时会留下不完整的文件对，函数不提供事务性回滚。
- 写文件使用 `fs::write`，会覆盖已有目标；文件权限由 Rust/操作系统创建语义与 umask 决定，不显式复制 Go 版本的 `0644` 参数。
- 生成代码假设至少存在搜索参数 `b.args[0]`。参数数量合法性属于 `fieldFunctionClass.verifyArgs` 的上游职责，不在生成的 `vecEvalInt` 内重复检查。
- 空值不参与匹配；结果保持 `0`。子表达式向量求值错误会立即向上传播，同时 `defer` 确保已经借出的 Go 列缓冲区归还。
- `TypeContext` 字段是公开的，因此外部可构造当前矩阵之外的上下文；但 `render_string_vec` 私有且 `types_map` 固定，公开生成入口不会接收任意上下文。

## 并发与资源生命周期

Rust 生成器没有共享锁、静态可变数据、异步任务或跨调用缓存；不同调用可各自构造内存文本。并发调用 `generate_one_file` 写同一前缀时没有协调或原子替换，可能互相覆盖，调用方必须保证目标路径互斥。

`format_go_source` 为每次格式化创建一个独立 `gofmt` 子进程。源码写完后 stdin 所有权被取走并在写入句柄离开作用域时关闭，`wait_with_output` 等待进程结束并收集 stdout/stderr，不会留下后台任务。若进程错误退出，stderr 被复制进错误消息，但上层生成函数仅用它决定回退，不对外暴露该诊断。

生成出的 Go 方法每次求值从 `bufAllocator` 获取两个缓冲区，并在获取成功后立即注册 `defer put`。第二个缓冲区获取失败时，第一个仍会归还；任一 `VecEval*` 错误也沿返回路径触发归还。文件本身只生成这段生命周期逻辑，不拥有运行时缓冲区。

## 与 Go 版本的对应关系

直接对照文件是 [`string_vec.go`](./string_vec.go)。二者共享相同的 `header`、import、Int/Real/String 类型顺序、`FIELD` 比较分支、测试用例矩阵和“格式化失败仍保留原始源码”的总体语义。仓库现有 [`../builtin_string_vec_generated.go`](../builtin_string_vec_generated.go) 与模板结构相符，确认生成目标确实接入 Go 表达式运行时。

已确认的实现差异如下：

- Go 版本通过 `text/template.Execute` 展开，模板解析在全局初始化时由 `template.Must` 保证；Rust 当前通过 `format!` 手工展开，保留的模板常量不参与执行，因此不存在模板执行错误路径。
- Go 的 `generateDotGo`/`generateTestDotGo` 接收文件名和类型切片并直接写盘；Rust 将“生成字节”和“写盘”拆开，公开生成函数使用固定的三类型矩阵，`generate_one_file` 只接收前缀。
- Go 的 `format.Source` 是进程内格式化；Rust 启动外部 `gofmt`。两者失败时都写/返回原文，但 Go 会记录 warning，Rust 不记录格式化失败详情。
- Go `main` 解析命令行并以固定默认前缀执行生成；Rust 模块没有二进制 `main`，用 `default_outputs` 表达默认文件名，用 `generate_one_file` 供调用者落盘。
- Go 显式以 `0644` 写新文件；Rust 使用 `fs::write` 的平台默认创建权限。该差异不改变生成内容，但可能影响新文件权限。

## 扩展指南

新增或修改类型时，优先保持 Go 生成器与 Rust 生成器同改：更新对应 `TypeContext` 常量和 `types_map` 顺序，并检查 `render_string_vec` 的访问器与比较分支。尤其不能把需要排序规则、时区或其他上下文语义的类型简单标为 `fixed`。同时扩展 [`string_vec_test.rs`](./string_vec_test.rs) 中的类型矩阵和生成文本断言；Rust 单元测试必须继续放在独立测试文件，不应嵌入本源文件。

修改 `FIELD` 算法时，应同步核对三处行为：本文件的手工渲染正文、同目录 Go 模板 `builtinStringVecTpl`、以及实际生成文件/标量实现中的首匹配、NULL 和字符串排序规则语义。测试生成结构变化还要同步 `generate_test_dot_go`、Go 的 `builtinStringVecTestTpl` 与 `builtin_string_vec_generated_test.go` 所依赖的测试助手契约。

若要把 Rust 生成器接入自动生成流程，需要在本文件之外增加明确的二进制或构建入口；当前 `builtin.go` 仍调用 Go 生成器，不能仅凭 `default_outputs` 推断 Rust 已替代 `go generate`。若改进写盘可靠性，可考虑临时文件加原子替换或成对提交，但需明确处理并发写、权限和部分失败兼容性。

性能风险主要在生成产物：候选数乘输入行数的双层循环是 `FIELD` 的既有线性搜索语义；固定长切片访问避免逐值 getter，字符串必须承担 collator 比较成本。生成器自身的主要成本是字符串构造和每次调用启动一个 `gofmt` 进程。

## 验证依据

- Rust 源码与符号：[`string_vec.rs`](./string_vec.rs) 中的 `TypeContext`、`types_map`、`render_string_vec`、`generate_dot_go`、`generate_test_dot_go`、`format_go_source`、`generate_one_file`、`default_outputs`。
- 模块与 crate 边界：[`lib.rs`](./lib.rs) 的 `pub mod string_vec` 及独立 `#[path = "string_vec_test.rs"]` 测试模块；[`Cargo.toml`](./Cargo.toml) 的 crate、依赖和移植元数据。
- Rust 独立测试：[`string_vec_test.rs`](./string_vec_test.rs) 验证三类上下文顺序/形状、双文件写出、关键 Int/String 生成片段以及模板占位符已完全展开。
- Go 对照与接线：[`string_vec.go`](./string_vec.go)、[`../builtin.go`](../builtin.go) 的 `go:generate`、[`../builtin_string.go`](../builtin_string.go) 的签名选择与标量语义、[`../builtin_string_vec_generated.go`](../builtin_string_vec_generated.go) 及 [`../builtin_string_vec_generated_test.go`](../builtin_string_vec_generated_test.go) 的现有产物。
- RustCodeGraph：索引包含目标文件 21 个符号；`generate_dot_go` 的调用边指向 `types_map`、`render_string_vec`、`format_go_source`，调用者为 `generate_one_file` 与 `default_outputs`；`generate_test_dot_go` 的调用者相同；`generate_one_file` 的仓库内调用者为 `string_vec_test.rs::generation_writes_formatted_implementation_and_test_pair`；`render_string_vec` 仅由 `generate_dot_go` 调用。
- 文档任务按要求不运行 Cargo；验收采用固定十一章节结构检查，并人工复核本文区分了生成期与运行期事实、当前执行代码与保留模板、以及已验证差异与未接线边界。
