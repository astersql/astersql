# `pkg/expression/generator/control_vec.rs`

## 文件定位

本文件属于独立 crate `astersql-expression-generator`，由 [`pkg/expression/generator/lib.rs`](lib.rs) 以 `pub mod control_vec` 暴露。该 crate 的 `Cargo.toml` 指定 Rust 2024 edition、`publish = false`，并以 `[workspace]` 形成独立工作区；它是表达式向量化 Go 源码的生成/一致性维护工具，不进入 SQL 请求的运行时求值链。

文件面向的运行时产物是相邻上级目录中的 `pkg/expression/builtin_control_vec_generated.go` 和 `pkg/expression/builtin_control_vec_generated_test.go`。前者为 Go 表达式引擎提供 CASE、IFNULL、IF 的 `fallbackEval*`、`vecEval*` 和 `vectorized` 方法；后者提供类型矩阵的单元测试及基准测试入口。Rust 表达式运行时另外通过 `pkg/expression/lib.rs` 挂载 `builtin_control_vec_generated.rs`，不能把它与本文件返回的 Go 字节混为一谈。

## 核心职责

1. 用 `TypeContext`、`Sig`、`Function`、`TemplateKind` 和 `TemplateValue` 描述控制流内置函数的类型矩阵：CASE、IFNULL、IF 均覆盖 Int、Real、Decimal、String、Datetime/Time、Duration、JSON 七种返回类型。
2. 用 `FALLBACK_TEMPLATE`、`BUILTIN_CASE_WHEN_VEC`、`BUILTIN_IF_NULL_VEC`、`BUILTIN_IF_VEC` 表达向量化控制流的核心语义：向量计算会预先执行本应按行择一的分支，因此无关分支若产生错误或新增 warning，必须改走逐行标量求值。
3. 通过 `generate_dot_go` 和 `generate_test_dot_go` 原样读取已签入的 Go 生成物，通过 `generate_one_file`/`default_outputs` 返回目标路径和内容，供一致性检查或上层写盘逻辑使用。

需要特别注意当前实现边界：私有 `render` 和 `render_function` 能展开本文件内的简化模板，但公开生成入口并不调用它们，而是使用 `include_bytes!`。所以当前可执行契约是“返回仓库中已签入文件的精确字节”，不是“现场重新渲染模板”。

## 主要符号

- `HEADER: &str`：拟生成 Go 文件的头部，包含 package、`time`/`types`/`chunk` import，以及控制表达式因提前执行分支而需要标量回退的说明。
- `TypeContext`：静态类型描述，记录 EvalType 名、表达式求值后缀、chunk 列 API 后缀、Go 值类型及是否定长。`TYPE_INT` 至 `TYPE_JSON` 是七个预定义上下文；其中 String、JSON 为变长类型。
- `LocalTypeContext`：拥有 `String` 字段的公开结构体，注释称其对应 Go 局部 `typeContext`。当前文件内没有构造或消费它，是保留的迁移表面。
- `Sig { arg0 }`：单个返回/参数类型签名。`CASE_WHEN_SIGS`、`IF_NULL_SIGS`、`IF_SIGS` 构成三个七元素签名表；后两者直接别名到 CASE 表。
- `TemplateKind`：在 CaseWhen、IfNull、If 三套主模板之间分派。
- `Function` 与 `FUNCTIONS`：把生成名、签名表和模板种类绑定；名称为 `Case`、`Ifnull`、`If`。
- `TemplateValue` 与 `TMPL_VAL`：保存 `Control` 分类和完整函数表。它们对应 Go 生成器的 `tmplVal`，但当前 Rust 公开生成路径没有读取 `TMPL_VAL`。
- `render(template, builtin, t)`：替换类型、结果容量、标量存储和分支拷贝占位符。定长类型使用 `Resize*`/槽位写入，变长类型使用 `Reserve*`/顺序追加。
- `render_function(function)`：按 `TemplateKind` 选择模板，并为每个签名依次拼接 fallback 和 vectorized 实现。当前为私有且没有调用者。
- `generate_dot_go()` / `generate_test_dot_go()`：分别返回 `builtin_control_vec_generated.go` 与其 `_test.go` 的编译期嵌入字节；签名保留 `Result<_, String>`，但函数体当前没有失败分支。
- `generate_one_file(prefix)`：按固定顺序返回 `(prefix.go, 主文件字节)`、`(prefix_test.go, 测试文件字节)`，只组装数据，不执行文件 IO。
- `default_outputs()`：把默认前缀固定为 `./builtin_control_vec_generated`。

## 执行流程

当前实际公开流程如下：

1. 调用方通过 `crate::control_vec` 调用 `generate_dot_go`、`generate_test_dot_go`、`generate_one_file` 或 `default_outputs`。
2. 两个底层生成函数分别执行 `include_bytes!("../builtin_control_vec_generated.go")` 和 `include_bytes!("../builtin_control_vec_generated_test.go")`，复制为拥有所有权的 `Vec<u8>`。
3. `generate_one_file` 先取得主文件，再取得测试文件；`?` 保留了“主文件失败则不继续、测试文件失败则向上传播”的顺序语义，随后返回两个路径/字节对。
4. `default_outputs` 仅补充 Go 生成器 `main` 使用的默认 basename，不写磁盘。

模板描述的生成流程是另一条尚未接入的路径：`render_function` 根据 `TemplateKind` 选主模板，遍历七种 `Sig`，对每种类型先展开 `FALLBACK_TEMPLATE`，再展开相应向量模板。CASE 逐行选择第一个真 WHEN，未命中则取 ELSE 或 NULL；IFNULL 选择第一个非 NULL 参数；IF 将 NULL/零条件视为 false。该流程只可用于理解或未来接线，不能作为当前输出产生方式的证据。

## 数据与状态

Rust 侧只有编译期常量、不可变静态切片和函数局部 `String`/`Vec<u8>`，没有全局可变状态。`TypeContext`、`Sig`、`TemplateKind` 实现 `Copy`，模板遍历不会转移共享配置的所有权。`generate_*` 每次都分配新的字节向量，因此调用者可独立修改返回值，不会改变编译期嵌入资源。

生成的 Go 代码才持有逐批次运行状态：`chunk.Column` 保存条件和分支向量，`beforeWarns` 保存求值前 warning 数，结果列的 NULL bitmap 与定长槽位或变长追加序列共同表示结果。CASE 的参数不变量是 WHEN/THEN 成对，奇数参数的最后一项为 ELSE；无 ELSE 且未命中时结果为 NULL。IF 的条件为 NULL 或零时选择 false 分支。

七类类型中 Int、Real、Decimal、Time、Duration 标记为定长，String、JSON 标记为变长。这个差异决定结果列是先 `Resize*` 后按索引覆盖，还是先 `Reserve*` 后每行恰好追加一个值/NULL；扩展类型时必须维持结果行数与输入行数相等。

## 依赖与调用关系

上游直接证据：

- `pkg/expression/generator/lib.rs` 公开本模块，并在 `#[cfg(test)]` 下挂载独立的 `control_vec_test.rs`。
- `control_vec_test.rs` 调用四个公开入口中的前三个，逐字节比较签入 Go 文件，并核对双输出路径和顺序。
- `builtin_threadsafe_1_aster_unit_test.rs` 调用 `generate_dot_go` 验证 3×7 签名覆盖，调用 `default_outputs` 验证成对路径、非空输出、占位符已展开，并把输出送入 `gofmt` 做语法检查。
- RustCodeGraph 对文件给出 9 个使用文件；对文件限定的 `generate_*`/`render_function` callers/callees 查询没有返回调用边，因此以上直接调用位置由索引后的精确 `rg` 核验补足。`render_function` 在仓库内没有调用点。

下游依赖分两类：公开生成路径只依赖标准库 `Path`/`PathBuf`、`include_bytes!` 和两个签入 Go 文件；模板文本所描述的生成结果依赖 Go 的 `EvalContext`、`chunk.Chunk`/`chunk.Column`、各 builtin signature 的 `args`/`bufAllocator`/标量 `eval*`，以及 warning 计数与截断函数。`Cargo.toml` 声明了 `gtmpl`、`tree-sitter`、`tree-sitter-go` 和测试依赖 `tempfile`，但本文件本身没有导入这些 crate；它们属于生成器 crate 的其他模块或测试范围。

## 错误处理与边界

当前 Rust 公开入口以 `Result<_, String>` 暴露错误通道，但 `include_bytes!` 的缺文件问题发生在编译期，成功编译后的 `generate_dot_go`/`generate_test_dot_go` 总是返回 `Ok`。`generate_one_file` 通过 `?` 保留接口层错误传播顺序，却不进行目录创建、权限检查、写盘或 Go 格式化；调用者不能把“返回路径”误认为文件已经生成。

模板/签入 Go 产物的关键语义边界是短路一致性。向量化会计算所有候选分支，而标量 CASE/IF/IFNULL 只计算被选分支：若向量求值出现错误或使 warning 数增加，代码截断新增 warning 并调用相应 `fallbackEval*`，防止未选分支改变可见行为。缓冲区分配失败或条件向量求值失败则直接返回错误。标量 fallback 按行传播首个错误并准确写入 NULL。

本文件内简化模板不能单独视为 Go 原版的完全复刻。例如公开输出来自完整签入文件，而私有模板的若干拷贝占位符统一使用较抽象的 `CopyConstruct`/`AppendCell` 文本。修改模板但不改公开输出不会改变生成结果；反之替换签入文件会立即改变公开返回字节。

## 并发与资源生命周期

Rust 入口没有锁、线程、异步任务、channel 或文件句柄；所有返回数据均为调用内新分配并由调用者拥有，静态配置只读，因此模块本身可被并发调用而没有共享可变状态。编译期嵌入使资源生命周期与生成器二进制/测试制品一致，而不是与源 Go 文件的运行时读取一致。

生成的 Go 向量函数从 `bufAllocator` 取得临时 `chunk.Column`，每次成功取得后立即注册 `defer put`，覆盖正常返回、向量错误和 fallback 返回。CASE 为每组 WHEN/THEN 以及可选 ELSE 分配缓冲；IFNULL 最多持有两个参数缓冲；IF 持有条件和两分支缓冲（定长 true 分支可直接复用结果列，具体以签入 Go 产物为准）。这些缓冲只在一次 `vecEval*` 调用期间有效。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/generator/control_vec.go`。映射关系为：Go `typeContext` ↔ Rust `TypeContext`/未使用的 `LocalTypeContext`，三个 `*Sigs` ↔ 三个静态签名切片，Go `function`/`tmplVal` ↔ Rust `Function`/`TMPL_VAL`，Go `generateDotGo`/`generateTestDotGo`/`generateOneFile`/`main` ↔ Rust 四个公开输出函数（其中 `default_outputs` 对应 `main` 的默认路径选择）。

两版共同覆盖 Case、Ifnull、If 与七种返回类型，并保留 warning/错误触发标量回退的意图。差异是实质性的：Go 版运行 `text/template`、`go/format.Source` 并以 0644 写盘，格式化失败时记录 warning 后写原始内容；Rust 版当前不解析模板、不格式化、不写盘，而是返回签入 Go 文件的精确字节和建议路径。Go 版的完整模板细分 Decimal 指针解引用、Duration 内部值、定长/变长列访问；Rust 私有简化模板没有成为输出来源，因此不能据此宣称已独立复现 Go 模板引擎。

`pkg/expression/builtin_control_vec_generated_test.go` 进一步确认测试矩阵：CASE 每类型覆盖单组/双组 WHEN-THEN 以及有无 ELSE，IFNULL 每类型覆盖两个同类型参数，IF 每类型覆盖 Int 条件和两个同类型分支；`defaultControlIntGener` 以约 0.3 比例生成零条件，以增加分支覆盖。

## 扩展指南

- 新增返回类型时，先在 `TypeContext` 常量中准确设置 EvalType、Go 方法后缀、列方法后缀、Go 值类型和 `fixed`，再同步三个签名表、Go `control_vec.go` 的三个签名表、两个签入生成物及独立测试。尤其要复核 Decimal、Duration、变长列的存储差异。
- 新增控制函数时，需要扩展 `TemplateKind`、`FUNCTIONS`/`TMPL_VAL`、`render_function` 分派、Go `function`/`tmplVal`、测试用例 map 和生成产物；同时决定它的未选分支错误/warning 是否必须 fallback。
- 若要让 Rust 真正重新生成而非回放签入文件，应把 `render_function`/测试模板接入公开入口，并补齐 Go 模板的所有类型特例、format.Source 等价验证和清晰的 IO 层。接线前不要删除逐字节对抗测试。
- 修改输出命名或写出顺序时，同步 `control_vec_test.rs::generate_one_file_preserves_go_write_order_and_paths` 和聚合测试 `every_generator_returns_go_and_test_output_paths`。
- 测试应继续放在独立的 `control_vec_test.rs` 或其他独立 `*_test.rs` 中，不要嵌入生产源文件。性能风险主要来自生成 Go 代码的额外缓冲分配、全分支提前计算和不必要 fallback；兼容风险主要来自 warning 截断、NULL/零条件、无 ELSE CASE 以及定长/变长写入语义漂移。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/expression/generator` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/expression/generator/control_vec.rs --offset 1 --limit 500` 读取目标文件 381 行及 35 个符号；对文件限定的 `generate_dot_go`、`generate_one_file`、`default_outputs`、`render_function` 执行 callers/callees 查询，未返回边。
- Rust 源与模块证据：`pkg/expression/generator/control_vec.rs`、`pkg/expression/generator/lib.rs`、`pkg/expression/generator/Cargo.toml`。
- Rust 测试证据：`pkg/expression/generator/control_vec_test.rs`；相关聚合测试 `pkg/expression/generator/builtin_threadsafe_1_aster_unit_test.rs`。
- Go 对照与产物证据：`pkg/expression/generator/control_vec.go`、`pkg/expression/builtin_control_vec_generated.go`、`pkg/expression/builtin_control_vec_generated_test.go`。
- 应用边界证据：`pkg/expression/lib.rs` 挂载 Rust 运行时的 `builtin_control_vec_generated.rs` 及其独立测试，证明生成器 crate 与表达式运行时模块是不同层次。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文档恰含 11 个固定二级标题，并人工检查所有“当前路径”和“模板设计”表述均有上述文件或符号依据。
