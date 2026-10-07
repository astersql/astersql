# `pkg/expression/generator/time_vec.rs`

## 文件定位

本文件是 `astersql-expression-generator` crate 中的时间类 Go 源码生成器，模块由
`pkg/expression/generator/lib.rs` 的 `pub mod time_vec` 暴露。它不是 SQL 执行期的时间函数实现；它在 Rust
进程中组织模板和签名元数据，输出供 Go `pkg/expression` 使用的
`builtin_time_vec_generated.go` 与 `builtin_time_vec_generated_test.go`。

crate 边界由 `pkg/expression/generator/Cargo.toml` 定义：这是一个 `publish = false`、以 `lib.rs`
为入口的独立 workspace，运行时仅直接使用 `gtmpl`；`tree-sitter` 与 `tree-sitter-go` 虽在 crate
依赖中声明，但本文件没有使用。临时目录只出现在 `tempfile` 驱动的独立 Rust 测试中。

文件内虽保留私有 `main`，但 Cargo 只声明了 `[lib]`，没有二进制 target；因此它目前不会像 Go
原文件那样由 `go run` 自动成为生成入口。实际可调用入口是 crate 内可见的
`generate_one_file(&Path)`，当前生产模块之外只由独立 Rust 测试直接调用。

## 核心职责

1. 以 `ADD_OR_SUB_TIME_TEMPLATE` 和 `TIME_DIFF_TEMPLATE` 保存 Go 向量化实现模板，覆盖
   ADDTIME、SUBTIME、TIMEDIFF 的批量求值代码。
2. 用 `ADD_TIME_SIGS`、`SUB_TIME_SIGS`、`TIME_DIFF_SIGS`、`ADD_DATE_SIGS`、
   `SUB_DATE_SIGS` 描述 Go 生成器的类型特化矩阵，并用 `FUNCTIONS` 固定五类 SQL 函数的顺序。
3. 以 `TEST_FILE_TEMPLATE` 生成五类函数的向量化正确性测试与 benchmark，其中 ADDDATE、
   SUBDATE 的签名矩阵用于测试用例展开，而不进入 `generate_dot_go` 的生产实现模板。
4. 把 Rust 结构转换为 `gtmpl::Value`，注册区间单位辅助函数，渲染模板，调用外部 `gofmt`，并写出
   `.go`/`_test.go` 文件。

本文件生成的是 Go 源码而不是在 Rust 中计算日期时间。模板中的 `types.ParseDuration`、
`calculateTimeDiff`、`bufAllocator` 等名称属于生成后的 Go 编译环境，不能当作 Rust 下游调用。

## 主要符号

- `ADD_OR_SUB_TIME_TEMPLATE`：ADDTIME/SUBTIME 共用模板。按 `Function.func_name` 选择加法或减法，
  按 `Sig` 选择参数列类型、结果列类型、恒 NULL、零日期、二进制字符串和转换错误等分支。
- `TIME_DIFF_TEMPLATE`：TIMEDIFF 模板。根据 String、Time、Duration 的组合决定字符串判型、缓冲区
  复用以及调用 `calculateTimeDiff` 或 `calculateDurationTimeDiff`。
- `TEST_FILE_TEMPLATE`：生成 `vecBuiltinTimeGeneratedCases`、两个测试入口和两个 benchmark 入口；
  通过三个区间单位函数展开 ADDDATE/SUBDATE 用例。
- `Sig`：单个 Go 特化签名的紧凑描述，保存签名名、两种输入类型、输出类型、可选 Go field type
  以及 `all_null`。字段大多私有，防止它成为 crate 的通用类型系统；测试仅需读取 `all_null`。
- `Function`、`TmplVal`：分别表示一个 SQL 函数及其签名集合、整个 Time 分类及函数集合。
- 五个签名常量：数量依次为 11、11、8、32、32；ADDTIME 与 SUBTIME 各含 3 个恒 NULL 签名。
- `interval_units_for_duration_as_duration`、`interval_units_for_duration_as_datetime`、
  `interval_units`：分别返回 11 个 Duration 结果单位、9 个 Datetime 结果单位及按该顺序拼接的
  20 个完整单位。
- `type_context_value`、`sig_value`、`function_value`：将静态 Rust 元数据转换成 Go 模板期望的
  对象字段。`type_context_value` 明确映射 ET 名、Go 列访问名、Go 类型名和定长属性。
- `TemplateData`：统一 `Function`、`[Sig]`、`TmplVal` 的模板值转换接口。
- `render_with_funcs`：模板解析/执行的共同底层；`render_template` 不注册辅助函数，
  `render_test_template` 注册三个区间单位函数。
- `format_go_source`：启动 `gofmt` 子进程，以管道传入源码并收集标准输出/错误。
- `generate_dot_go`、`generate_test_dot_go`：分别渲染实现文件和测试文件。
- `generate_one_file`：公开给 crate 内调用者的双文件写出入口；`main` 固定默认前缀并在错误时 panic。

文件没有条件编译项；条件编译只出现在 `lib.rs` 对独立测试模块的 `#[cfg(test)]` 挂载处。

## 执行流程

调用 `generate_one_file(prefix)` 时，流程如下：

1. `generate_dot_go(prefix.with_extension("go"))` 依次用 `ADD_OR_SUB_TIME_TEMPLATE` 渲染 AddTime、
   SubTime，再用 `TIME_DIFF_TEMPLATE` 渲染 TimeDiff。三个结果先聚合到内存中的 `String`；任一解析或
   渲染错误都会在首次写盘前返回。
2. `format_go_source` 将聚合源码送入 `gofmt`。格式化成功采用其标准输出；启动、管道写入或等待失败
   会返回错误，但 `gofmt` 自身以失败状态退出时，`generate_dot_go` 记录警告并保留未格式化源码。
3. `fs::write` 写实现文件。实现文件成功后，才进入测试文件生成，因此实现生成失败会短路。
4. `generate_test_dot_go` 用 `TMPL_VAL` 渲染 `TEST_FILE_TEMPLATE`。渲染时注册三个 FuncMap 函数，
   让模板按结果类别选择区间单位。格式化采用与实现文件相同的成功/回退规则，最后写 `_test.go`。

生成后的 ADDTIME/SUBTIME Go 方法先向量化求值参数列，尽可能复用固定宽度的第一参数结果列，合并
NULL 位图，再逐行处理零日期、Duration 字符串解析、加减运算和输出。全 NULL 签名只调整结果列并
返回。TIMEDIFF 会复用 Duration 参数对应的结果缓冲；字符串操作数先判定为 Duration 或时间，类型
类别不匹配时置 NULL，计算失败则返回错误。

双文件生成不是事务性的：如果实现文件已经写入而测试模板渲染或测试文件写入随后失败，调用者会看到
错误，但实现文件仍然保留。扩展调用方不能假定“错误即没有任何输出”。

## 数据与状态

签名和模板均是静态只读数据。`Sig` 中的类型是名称而非 Rust 类型，`type_context_value` 在渲染期把
`Int`、`Real`、`Decimal`、`String`、`Datetime`、`Duration` 映射为 Go helper `TypeContext`
对应字段。例如 Datetime 的模板类型名是 `Time`、列访问名是 `Time`，Duration 的列访问名是
`GoDuration`；String 被标记为非定长，其他已知值为定长。

`TMPL_VAL` 固定 `Category = "Time"`，函数顺序为 AddTime、SubTime、TimeDiff、AddDate、
SubDate。顺序会影响生成文本，因此属于兼容性数据。区间单位也保持 Go 原顺序：Duration 组从
MICROSECOND 到 DAY_MICROSECOND，Datetime 组从 DAY 到 YEAR_MONTH。

每次渲染新建局部 `Template` 和 `Context`，每次生成新建局部字符串、子进程及路径。文件中没有全局
可变状态、缓存或随机源；生成测试模板中的随机生成器表达式只是输出到 Go 文件的文本。

## 依赖与调用关系

上游关系：

- `pkg/expression/generator/lib.rs` 声明 `pub mod time_vec`，并在测试配置下把
  `time_vec_2_aster_unit_test.rs` 挂到同一 crate。
- `time_vec_2_aster_unit_test.rs` 通过 `use crate::time_vec::*` 调用区间函数、渲染函数和
  `generate_one_file`。仓库搜索未发现其他 Rust 模块调用本文件入口。
- 私有 `main` 调用 `generate_one_file`，但在当前仅库 target 的 Cargo 配置中不是可执行程序入口。

Rust 下游关系：

- `gtmpl::{Template, Context, Value, Func, FuncError}` 承担 Go 风格模板的数据表达、函数注册、解析与执行。
- `std::process::Command`/`Stdio` 管理 `gofmt`，`std::io::Write` 写入其 stdin。
- `std::fs::write` 写最终文件，`Path`/`PathBuf` 构造 `.go` 与 `_test.go` 路径。

生成文本的 Go 下游包括 `pkg/parser/mysql`、`pkg/parser/terror`、`pkg/types`、`pkg/util/chunk`，以及
表达式包内部的 `typeCtx`、`isDuration`、时间加减辅助函数、缓冲分配器和向量化测试框架。这些是
模板产物的编译依赖，不是 Rust crate 的链接依赖。

RustCodeGraph 已索引本文件并识别 28 个符号，但对限定的 `generate_one_file`、
`render_template`、`render_test_template` 查询未返回静态 callers/callees；因此调用关系以上述
源码内直接调用和仓库引用搜索为准，不推断不存在的外部生成链。

## 错误处理与边界

- `template_error` 把模板解析/渲染错误转换为 `io::ErrorKind::InvalidData`，错误文本保留在消息中。
- `render_with_funcs` 对解析和执行都使用 `?` 向上传播；无效模板不会产生输出文件。
- `type_context_value` 对未知名称采用同名透传且标记为非定长。这便于模板扩展，但新增类型若需要特殊
  列访问名、Go 类型或定长语义，必须显式增加映射，不能依赖 fallback。
- 模板生成的 Go 代码区分可恢复 SQL 数据错误和致命求值错误：截断类 Duration 错误通常追加 warning
  并置 NULL；其他错误返回；零日期、类别不匹配及明确的恒 NULL 签名置 NULL。
- `format_go_source` 对无法启动 `gofmt`、无法写 stdin 或等待失败直接返回错误；对 `gofmt` 返回非零
  则由两个生成函数降级写原始源码。这延续 Go 版本“格式化失败仍保留原文供调试”的意图。
- `fs::write` 会覆盖已有目标且不是原子替换；父目录必须存在。Rust 版本也没有显式复制 Go
  `0644` 权限参数，文件权限由 Rust/平台创建语义决定。
- `format_go_source` 对 piped stdin 使用 `expect`。按当前构造 stdin 应存在；若标准库契约被破坏会
  panic，而不是返回 `io::Error`。
- 私有 `main` 在 `generate_one_file` 出错时 panic；库调用入口则返回 `io::Result<()>`。

## 并发与资源生命周期

本文件没有线程、async、锁、channel 或共享可变状态。单次渲染的数据都在栈和局部堆对象中，函数返回后
释放。`gofmt` 子进程由 `wait_with_output` 同步等待并回收，stdin handle 在写完并移交等待流程后关闭。

并发调用 `generate_one_file` 只有在输出前缀不同的情况下才自然隔离；若多个调用者使用同一前缀，
`fs::write` 没有锁、临时文件或 rename 协议，可能相互覆盖。实现文件与测试文件也没有跨文件原子性。
模板所生成的 Go 代码会使用表达式框架的列缓冲池，并通过 `defer b.bufAllocator.put` 归还临时列；这是
生成产物的资源生命周期，不是 Rust 生成器自身的共享资源。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/generator/time_vec.go`。对应关系如下：

- Go 的 `addOrSubTime`、`timeDiff`、`testFile` 对应三个 Rust 模板常量；主体保留 Go 模板分支。
- Go 的五个 `*SigsTmpl` 对应五个 Rust `*_SIGS` 切片；独立测试核对 11/11/8/32/32 的数量、
  两组 3 个恒 NULL 项，以及五个函数的顺序。
- Go 的 `sig`、`function`、匿名 `tmplVal` 对应 Rust 的 `Sig`、`Function`、`TmplVal`/`TMPL_VAL`。
- Go 从 helper 包取得 `TypeContext` 和区间函数；Rust 为避免依赖未迁移的 helper 所有权，使用字符串
  保存类型，并在 `type_context_value` 中恢复模板上下文，区间函数则在本文件直接实现并注册到 FuncMap。
- Go `format.Source` 是进程内格式化；Rust 通过外部 `gofmt` 子进程实现。两者都在格式化器拒绝源码时
  警告并写原始文本，但 Rust 还可能因找不到/无法执行 `gofmt` 而直接失败。
- Go `os.WriteFile(..., 0644)` 显式给出创建权限；Rust `fs::write` 没有显式权限参数。
- Go 文件有 `//go:build ignore` 且可作为生成程序运行；Rust crate 目前只有 library target，私有
  `main` 仅保留语义形状，实际覆盖来自独立测试调用 `generate_one_file`。

## 扩展指南

新增或调整时间函数签名时，应先修改对应 `*_SIGS`，同时核对 Go `*SigsTmpl` 的顺序、字段类型提示和
`all_null`。若新增类型名称，还必须同步 `type_context_value` 的 ET 名、列名、Go 类型与定长属性。
遗漏映射可能让模板成功渲染，却生成错误的列 API。

修改运行语义应落在相应模板分支：ADDTIME/SUBTIME 修改 `ADD_OR_SUB_TIME_TEMPLATE`，TIMEDIFF
修改 `TIME_DIFF_TEMPLATE`；ADDDATE/SUBDATE 在本文件当前只参与测试用例矩阵，不应误以为
`generate_dot_go` 会生成它们的生产方法。新增第六类函数时还需同步 `FUNCTIONS`、`TMPL_VAL` 和
`TEST_FILE_TEMPLATE` 的分派条件，并判断是否应加入实现生成流程。

修改区间分类时必须保持三个区间函数、FuncMap 注册名和 Go helper 语义一致。修改写盘逻辑时应明确
是否要改善双文件原子性、并发覆盖或权限差异，避免无意改变当前 Go 对齐行为。

测试应继续放在独立文件 `pkg/expression/generator/time_vec_2_aster_unit_test.rs`，不要内嵌进生产源文件。
至少同步覆盖签名数量/顺序、恒 NULL 数、关键模板分支、区间边界值、两文件路径和格式化失败策略。
若模板文本变化，还应检查生成的 Go 文件可格式化并与 Go 原生成器的关键行为一致。

主要风险是：签名矩阵重复名称依赖输出类型或 field type 区分，按名称去重会破坏 ADDDATE/SUBDATE
覆盖；模板是大段 Go 源码，Rust 编译无法验证其中标识符；调用外部 `gofmt` 又使生成结果依赖工具环境。

## 验证依据

- Rust 源码：`pkg/expression/generator/time_vec.rs`。RustCodeGraph 状态显示仓库索引包含该文件，
  `files --filter` 报告 28 个符号；通过分段 `node --file` 检查了模板、签名矩阵、值转换、渲染、
  格式化和写盘入口。
- 模块与 crate：`pkg/expression/generator/lib.rs` 第 39–40、57–60 行；
  `pkg/expression/generator/Cargo.toml` 的 `[lib]`、依赖、开发依赖和 porting metadata。
- Go 对照：`pkg/expression/generator/time_vec.go`，重点核对模板主体及第 788–979 行的签名、模板值、
  `generateDotGo`、`generateTestDotGo`、`generateOneFile`、`main`。
- 独立 Rust 测试：`pkg/expression/generator/time_vec_2_aster_unit_test.rs`，覆盖区间分组和顺序、签名
  数量和恒 NULL 数、关键实现分支渲染、测试模板辅助函数以及 `.go`/`_test.go` 双文件写出。
- 仓库引用搜索：除本文件私有 `main` 外，实际调用入口只在上述独立测试中出现；目标目录不存在
  `doc.go`，因此无额外包契约可读取。
- 本任务是纯文档分析，按任务约束未运行 Cargo；结构验证用于确认文件存在且恰有规定的 11 个二级标题。
