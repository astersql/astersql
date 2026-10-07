# `pkg/expression/generator/other_vec.rs`

## 文件定位

该文件属于独立 library crate `astersql-expression-generator`。同目录 [`lib.rs`](lib.rs) 通过 `pub mod other_vec` 公开模块，并把 [`other_vec_test.rs`](other_vec_test.rs) 作为独立测试模块接入；[`Cargo.toml`](Cargo.toml) 声明 Rust 2024 edition、`publish = false`，并用 `package.metadata.porting.go-package = "pkg/expression/generator"` 标记 Go 移植来源。同目录不存在 `doc.go`。

它是离线的 Go 源码生成器移植件：把 SQL `IN` 的七种向量签名渲染成 Go 实现和生成式测试源码。它自身不执行 SQL 表达式，也不属于服务运行时求值链。当前真实的 Go 生成入口仍是 [`pkg/expression/builtin.go`](../builtin.go) 中的 `//go:generate go run generator/other_vec.go`，由 [`other_vec.go`](other_vec.go) 写出 `builtin_other_vec_generated.go` 与对应测试文件；Rust 模块只有 library API，不含 binary/main，也不写盘，尚未替换该入口。

## 核心职责

1. 用 `TypeContext`、`Sig`、`Function` 和 `TemplateValue` 固化 `IN` 的生成矩阵：Int、String、Decimal、Real、Datetime/Time、Duration、JSON 七种输入都返回 Int 布尔语义。
2. 为每个签名生成 `vecEvalInt` 和 `vectorized() bool`，表达批量参数求值、常量哈希快路径、动态参数逐列比较以及 SQL `IN` 的 TRUE/NULL(UNKNOWN)/FALSE 收束规则。
3. 按类型选择哈希键和相等比较：整数考虑 signed/unsigned，字符串考虑 collation，Decimal/JSON 使用哈希键，Time 使用 `CoreTime`，其余按类型比较。
4. 生成配套 Go 测试/benchmark 源码：每种类型一组四个动态参数用例，另加一组“左值 + 两个常量”的用例。
5. 把生成内容连同目标路径作为内存值返回，使调用者可以自行决定格式化、比较或写盘策略。

当前实现是迁移中的生成器，而非已验证可替换 Go 生成器的成品。独立测试证明输出能被 `gofmt` 解析并包含指定分支，但没有编译生成的 Go：固定长类型的常量哈希分支会在声明前使用 `buf0Values`，整数动态比较会引用 Go 树中没有定义的 `compareSignedAndUnsignedInts`。这两点必须在接入前修正并增加编译级验证。

## 主要符号

- `HEADER`、`NEW_LINE`、`BUILTIN_OTHER_IMPORTS`：生产生成物的文件头、分隔符和 Go import 块。依赖 `cmp`、`mysql`、`types`、`chunk`、`collate`。
- `TypeContext`：类型模板上下文。`et_name` 服务于生成测试中的 `types.ET*`；`type_name` 服务于 `VecEval*`/getter 分派；`type_name_in_column` 是固定长列切片访问器后缀；`type_name_go` 记录 Go 类型名但当前渲染逻辑未读取；`fixed` 区分切片访问与 getter 访问。
- `TYPE_INT`、`TYPE_REAL`、`TYPE_DECIMAL`、`TYPE_STRING`、`TYPE_DATETIME`、`TYPE_DURATION`、`TYPE_JSON`：七个静态类型描述。String/JSON 是变长类型；Datetime 的表达式方法片段是 `Time`，Duration 的列访问器片段是 `GoDuration`。
- `Sig` 与 `IN_SIGS_TMPL`：签名名、输入和输出类型的静态表。顺序与 Go `inSigsTmpl` 一致，七个 `builtinIn*Sig` 全部输出 `TYPE_INT`。
- `Function`、`FUNCTIONS`、`TemplateValue`、`TMPL_VAL`：保留 Go 测试模板的“类别 → 函数 → 签名”层次；当前只有类别 `Other` 和函数 `In`。Rust 的手写测试生成函数直接遍历 `IN_SIGS_TMPL`，因此 `TMPL_VAL` 当前未参与渲染。
- `BUILTIN_IN_TEMPLATE`：生成函数主体。它申请两个 Go `chunk.Column` buffer，先查常量集合，再逐个求值动态列表项，最后只对尚未命中的行传播 `hasNull`。
- `hash_lookup`：按 `TypeContext.type_name` 返回常量哈希命中片段。Int 附带符号位兼容判断；Decimal/JSON 调用 `ToHashKey`；String 使用 collator key；Time 使用 `CoreTime`。
- `compare_expression`：返回动态参数比较表达式。Decimal/Time 调用类型方法，Duration/Real 使用 `cmp.Compare`，JSON/String 使用 `types` 专用比较；Int 当前返回未定义的 `compareSignedAndUnsignedInts` 调用。
- `render_in_sig`：把一个 `Sig` 展开为 Go 源码。它生成固定长/变长读取方式、NULL 初始化、哈希准备和比较准备，再逐项替换模板占位符；JSON 会通过字符串区间删除整个常量哈希块。
- `TEST_FILE_HEADER`：生成测试文件的头部、`inGener` 随机数据生成器和用例 map 起始文本。
- `generate_dot_go`：公开的生产源码生成入口，按 `IN_SIGS_TMPL` 的稳定顺序拼接七个特化实现。
- `generate_test_dot_go`：公开的测试源码生成入口，为每种类型生成动态与常量两类用例，并追加两项测试和两项 benchmark。
- `generate_one_file`：把调用者给定的前缀映射为 `<prefix>.go` 与 `<prefix>_test.go`，返回两个 `(PathBuf, Vec<u8>)`，不执行文件 IO。
- `default_outputs`：使用默认前缀 `./builtin_other_vec_generated` 调用 `generate_one_file`。

本文件没有 trait、impl、宏或条件编译项。公开面包括上下文结构体、静态矩阵/模板值以及四个生成函数；具体渲染与比较选择函数保持私有。

## 执行流程

默认调用链为：

`default_outputs → generate_one_file → {generate_dot_go, generate_test_dot_go}`

生产实现生成流程如下：

1. `generate_dot_go` 拼接 `HEADER`、换行和 `BUILTIN_OTHER_IMPORTS`。
2. 函数按 `IN_SIGS_TMPL` 顺序逐个调用 `render_in_sig`，所以输出顺序固定为 Int、String、Decimal、Real、Time、Duration、JSON。
3. `render_in_sig` 根据 `fixed` 决定通过 `buf*Values[i]` 还是 `buf*.Get*(i)` 读取值，并通过 `hash_lookup`、`compare_expression` 选择类型专用片段。
4. 生成的 `vecEvalInt` 取得两个 buffer；第一个保存左值列，第二个循环复用以求值列表项。任何 buffer 获取或 `VecEval*` 错误立即向上返回，已取得的 buffer 由 Go `defer` 归还。
5. 结果列初始化为全 NULL，并为每行维护 `hasNull`。非 JSON 类型还把构造阶段发现的常量 NULL (`b.hasNull`) 预置到所有行。
6. 若存在常量哈希集合，生成代码先尝试常量命中；命中行写 1 并清除 NULL。随后把 `nonConstArgsIdx` 指向的参数重建为动态参数列表。JSON 特化会删除整个哈希块，保持 Go 原模板的例外。
7. 动态参数逐列求值。已经命中的行直接跳过；左值或当前参数为 NULL 时只记录 `hasNull`；相等时把结果设为 1 且非 NULL。
8. 最后只对结果仍为 NULL 的行写入 `hasNull`：有 NULL 候选且无命中时保留 UNKNOWN，否则落为非 NULL 的 0；已经为 TRUE 的行不被 NULL 覆盖。

测试源码生成独立遍历相同签名表。每个类型先生成四个动态参数及 0.2 NULL 比例的数据生成器，再根据 EvalType 选择两个常量字面量；未知 `et_name` 返回错误。最后闭合 map 并追加统一测试/benchmark 入口。

## 数据与状态

模块级表和模板均为不可变 `const`/静态切片，没有全局可变状态。`TypeContext` 与 `Sig` 都是 `Clone + Copy`，内部只有 `'static` 字符串和布尔值；`Function`/`TemplateValue` 只借用静态切片。生成期间的可变状态仅是局部 `String`，最终转为调用者拥有的 `Vec<u8>`，路径由 `PathBuf` 拥有。

运行时语义存在于生成的 Go 文本中：`result` 是每行 Int64 结果与 NULL 位图，`hasNull: []bool` 暂存 UNKNOWN 候选，`r64s[i] != 0` 表示该行已命中并允许跳过后续比较。`b.hashSet`、`b.hasNull`、`b.nonConstArgsIdx` 和 `b.collation` 来自手写 `builtinIn*Sig` 构造逻辑，本生成器只消费这些约定，不建立它们。

内存复杂度方面，Rust 生成阶段与输出文本长度成正比；生成的 Go 求值阶段持有两个复用列缓冲区以及一个长度为行数的 `hasNull` 数组。动态参数按参数数依次扫描行，已命中行可提前跳过后续值比较，但每个动态表达式仍以整列方式先求值。

## 依赖与调用关系

源码确认的文件内调用关系是：

- `default_outputs → generate_one_file`
- `generate_one_file → generate_dot_go`、`generate_test_dot_go`
- `generate_dot_go → render_in_sig`
- `render_in_sig → hash_lookup`、`compare_expression`

上游方面，[`lib.rs`](lib.rs) 公开 `other_vec`。直接 Rust 使用点在独立测试 [`other_vec_test.rs`](other_vec_test.rs) 和 [`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs)：前者检查本文件的类型分支和测试生成物，后者检查跨生成器签名覆盖、默认双文件路径、占位符消除与 `gofmt` 语法。没有发现非测试 Rust 代码调用这四个生成 API。

下游 Rust 依赖仅为标准库 `Path`/`PathBuf`；本文件没有使用 crate manifest 中声明的 `gtmpl`、`tree-sitter`、`tree-sitter-go` 或开发依赖 `tempfile`。输出的 Go 文本则依赖 `expression` 包内的签名类型、`EvalContext`、`Expression`、buffer allocator，以及导入的 `mysql`、`types`、`chunk`、`collate` 和标准库 `cmp`。

真实 Go 主链是 `pkg/expression/builtin.go` 的 `go:generate → generator/other_vec.go → builtin_other_vec_generated{,_test}.go`。生成实现被 `pkg/expression/BUILD.bazel` 纳入 Go 构建；Rust 主工程另有 [`builtin_other_vec_generated.rs`](../builtin_other_vec_generated.rs) 运行时移植，但它不是本生成器的输出，也没有调用本文件。

RustCodeGraph 的文件视图确认目标文件被测试引用；精确 `callers`/`callees` 命令未为这些重名生成函数返回可靠符号边，因此上述边由已索引的完整源码和 `rg` 调用点交叉核对，不把缺失图边推断为生产接线。

## 错误处理与边界

四个公开生成函数返回 `Result<_, String>`。`generate_dot_go` 当前没有可达错误分支，但保留统一接口；`generate_test_dot_go` 遇到签名表中未知 `et_name` 时返回 `unsupported IN EvalType`。`generate_one_file` 用 `?` 保证任一子生成失败时不返回部分双文件结果。模块不启动 `gofmt`、不写文件，因此不处理格式化、权限、覆盖或部分写入错误。

重要边界与当前限制包括：

- `render_in_sig` 对 JSON 通过 `find(...).unwrap()` 定位并删除哈希块。模板标记若被改名或重新缩进而未同步这段逻辑，生成过程会 panic，而不是返回 `Err`。
- 固定长类型的 `{ReadArg0}` 是 `buf0Values[i]`，但模板只在动态比较循环内部通过 `{PrepareDynamicCompare}` 声明 `buf0Values`。常量哈希块位于该声明之前，因此 Int、Decimal、Real、Time、Duration 的生成代码存在未声明标识符；`gofmt` 不做类型检查，现有测试无法发现它。
- Int 动态比较生成 `compareSignedAndUnsignedInts(...)`，但仓库 Go 源码没有该函数定义；Go 对照模板在原位内联四种 signed/unsigned 组合。现有测试反而断言该字符串存在，因此同样没有编译级保障。
- Rust `HEADER` 比 Go 原版头部精简，生成函数也不调用 Go `format.Source` 或外部 `gofmt`；输出不是字节级复刻。调用者若要发布文件，必须自行格式化并检查编译。
- `type_name_go`、`FUNCTIONS` 和 `TMPL_VAL` 当前未参与生产渲染。仅修改这些字段不会改变 `generate_dot_go`，扩展时不能误以为它们已经驱动全部模板。
- 未知 `TypeContext.type_name` 会在哈希与比较选择中落入通用分支，但 getter、import、哈希键可比性和 Go 类型约束未被验证；新增类型必须显式审查，而不能依赖默认分支。
- SQL NULL 收束的不变量是“TRUE 优先于 UNKNOWN”：最终循环只修改仍为 NULL 的结果。改变结果初始 NULL 状态、`hasNull` 更新或命中时的 `SetNull(false)` 都可能破坏 `IN` 三值逻辑。

## 并发与资源生命周期

Rust 生成过程是同步纯内存计算，没有线程、异步任务、锁、通道、事务、文件句柄或子进程。所有字符串、字节和路径由当前调用拥有，离开作用域后按 RAII 释放；模块没有缓存，所以多个线程分别调用时不会共享可变状态。并发生成只会各自承担字符串复制和多次 `replace` 的成本。

生成出的 Go 函数每次求值申请两个 buffer，并在成功取得后立刻注册 `defer b.bufAllocator.put(...)`；后续任何求值或哈希键错误返回时都会归还已取得资源。第二个 buffer 获取失败时，第一个也已注册归还。函数不启动 goroutine，`hasNull` 和参数切片只在本次调用内存活；实际并发安全仍依赖 `builtinIn*Sig`、参数表达式、`EvalContext` 和 allocator 的外部契约。

## 与 Go 版本的对应关系

直接对照文件是 [`other_vec.go`](other_vec.go)：

- 七个 Rust `TYPE_*`/`IN_SIGS_TMPL` 项对应 Go helper 的类型上下文和 `inSigsTmpl`，名称与顺序一致；`FUNCTIONS`/`TMPL_VAL` 对应 Go 测试模板的 `function`/`tmplVal`。
- `BUILTIN_IN_TEMPLATE` 对应 Go `builtinInTmpl`，保留双 buffer、常量哈希优化、`nonConstArgsIdx` 重建、逐参数比较、NULL 收束及 `vectorized() == true` 的整体结构。
- `hash_lookup` 对应 Go 模板的 `$InputInt`、`$UseHashKey`、`$InputString`、`$InputTime` 分支；`compare_expression` 对应 Go `Compare` 子模板。
- `generate_dot_go`/`generate_test_dot_go` 对应 Go 同名函数，`generate_one_file` 对应成对生成顺序，`default_outputs` 则是 Rust 为默认路径增加的便捷封装。
- Go 版本使用 `text/template` 渲染、`format.Source` 格式化并以 0644 权限写盘；格式化失败时告警并写原始内容。Rust 使用字符串替换，只返回内存字节和路径，不格式化、不写盘，也没有 Go `main` 对应的可执行入口。
- Go 模板在固定长值进入哈希分支前声明 `args0`，整数比较在模板中内联；Rust 当前分别漏掉哈希前的 `buf0Values` 声明并改成未定义 helper，因此不能声称行为完全对齐或可直接替换。
- Go 生成物 [`builtin_other_vec_generated.go`](../builtin_other_vec_generated.go) 是当前可构建的真实实现证据；Rust 的测试只检查文本特征和 `gofmt` 解析，未把 Rust 输出与该文件做完整 diff 或编译。

独立 [`other_vec_test.rs`](other_vec_test.rs) 验证七个签名各生成两个方法、JSON 不含哈希/`b.hasNull` 分支、错误传播文本、类型比较表达式、测试常量和四个测试/benchmark 入口。跨模块 [`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs) 还验证默认 `.go`/`_test.go` 路径、非空内容、无残留 `{{` 以及 `gofmt` 可解析。它们是生成结构回归，不等价于 Go 编译或 SQL 行为回归。

## 扩展指南

- 修复现有生成器时，先让固定长输入切片在哈希块之前声明，并将整数比较恢复为 Go 对照中的内联逻辑，或同时在 Go 生产包增加并验证等价 helper。随后为 Rust 输出增加 `go test`/最小 Go 编译验证；仅保留 `gofmt` 检查不足以证明可用。
- 新增 EvalType 时同步维护 `TypeContext` 常量、`IN_SIGS_TMPL`、`hash_lookup`、`compare_expression`、固定长/变长读取、测试常量分支和 Go `other_vec.go`。重点核对哈希键是否与相等比较同一等价关系、所需 import、NULL 表示和 chunk accessor。
- 新增 Other 类函数时，当前不能只扩展 `FUNCTIONS`/`TMPL_VAL`，因为生产与测试生成函数都手写为 `IN_SIGS_TMPL` 循环。应先重构分派或为新函数提供独立模板，并保持 Go 对照的函数分类与输出命名。
- 修改 NULL 逻辑时同步覆盖：左值 NULL、常量列表含 NULL、动态参数 NULL、前序 TRUE 后遇 NULL、全不匹配且无 NULL。测试应位于独立 [`other_vec_test.rs`](other_vec_test.rs) 或生成结果的 Go 测试中，不要嵌入生产 Rust 文件。
- 修改 Int 时同时覆盖四种左右 signed/unsigned 组合与负值边界；修改 String 时验证 collation key 和动态 `CompareString` 一致；修改 Decimal/JSON 时验证 `ToHashKey` 错误传播；修改 Time 时验证 `CoreTime` 与 `Compare` 的一致性。
- 若将 Rust 生成器接入 `go:generate`，需要另行提供稳定的 Cargo binary/命令、格式化和原子写盘策略，并修改 [`builtin.go`](../builtin.go)。接入前应与 Go 生成物做语义/编译对比，而不是只检查若干字符串。
- 性能风险集中在 Rust 模板的连续全串 `replace` 和生成 Go 代码的 `行数 × 动态参数数` 扫描；兼容风险集中在签名名称、三值逻辑、signed/unsigned、collation、哈希键与比较等价性以及生成文件格式。

## 验证依据

- 目标源码：[`other_vec.rs`](other_vec.rs)，通过 RustCodeGraph `node --file` 完整核对 451 行中的静态矩阵、模板、三个私有渲染函数和四个公开生成函数。
- crate/模块边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)，确认独立 library crate、公开模块、依赖清单和独立测试接线；同目录没有 `doc.go`。
- Go 对照与真实入口：[`other_vec.go`](other_vec.go)、[`pkg/expression/builtin.go`](../builtin.go)、[`builtin_other_vec_generated.go`](../builtin_other_vec_generated.go) 和 `pkg/expression/BUILD.bazel`，确认 Go 模板、写盘/main、当前 `go:generate` 主链和构建中的生成物。
- Rust 测试：[`other_vec_test.rs`](other_vec_test.rs) 与 [`builtin_threadsafe_1_aster_unit_test.rs`](builtin_threadsafe_1_aster_unit_test.rs)，确认文本分支、生成用例、默认路径和 `gofmt` 检查的实际覆盖范围。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/generator` 确认目标、Go 对照、模块入口和测试均已索引；`query` 定位 `generate_dot_go`、`render_in_sig`、`hash_lookup`、`compare_expression`、`default_outputs`；`node --file` 提供源码和文件级引用证据。精确 callers/callees 未返回可靠边，故调用关系另以完整源码与 `rg` 调用点验证。
- 迁移缺口证据：仓库范围 `rg` 显示 `compareSignedAndUnsignedInts` 只出现在本 Rust 生成器及其测试中，没有 Go 定义；模板顺序显示固定长 `buf0Values` 仅在常量哈希块之后声明。两项均明确记录为未通过 Go 编译验证的限制。
- 本任务是纯文档分析，依照总计划不运行 Cargo。交付只运行任务规定的 11 章节结构检查、变更范围检查与人工事实复核。
