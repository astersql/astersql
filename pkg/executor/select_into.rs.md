# [`pkg/executor/select_into.rs`](select_into.rs)

## 文件定位

本文件属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/executor/lib.rs` 以 `pub mod select_into` 对外公开。它实现 Rust 侧 `SELECT ... INTO OUTFILE` 的行序列化与本地文件写出能力：上游子执行器按 chunk 供给行，本执行器消费全部行并写入文件，自身不向调用者返回结果行。

构建层已经存在与该语句对应的抽象路径：`pkg/executor/builder.rs` 的 `Plan::SelectInto` 分支调用 `executorBuilder::buildSelectInto`，后者先构建目标计划，再向 `ExecutorBuilderDependencies::build_executor` 传入 `ExecutorKind::SelectInto` 和唯一子执行器。然而，全仓库 Rust 搜索只在本文件找到具体 `SelectIntoExec`，没有找到将该 kind 构造成此结构体的依赖实现。因此可以确认文件内执行逻辑和构建器分派均已存在，但不能据此声称二者已完成运行时接线。

## 核心职责

- `SelectIntoExec::Open` 校验只处理 `ast::SelectIntoType::Outfile`，以独占方式创建目标文件，初始化 sink、缓存 chunk、字段类型和复用缓冲区，然后打开子执行器。
- `SelectIntoExec::Next` 反复拉取子执行器，直到获得空 chunk；每个非空 chunk 交给 `dumpToOutfile`。传入的输出 chunk 被刻意忽略，因为 SQL 结果被写入文件。
- `SelectIntoExec::dumpToOutfile` 逐行逐列执行 NULL 表示、类型到文本/字节的转换、可选包围、转义、字段与行分隔，并在整批成功写出后累计 affected rows。
- `SelectIntoExec::Close` 无论前一步是否失败都依次尝试 Flush、关闭 sink、关闭子执行器，再按 Flush、sink Close、source Close 的顺序返回首个错误。
- `DumpRealOutfile` 保留 Go 版本的精度和量级分支：未指定精度时，绝对值大于等于 `1e15` 或非零且小于 `1e-15` 的数使用科学计数法，并移除指数中的 `+`。

## 主要符号

- `EXP_FORMAT_BIG` / `EXP_FORMAT_SMALL`：浮点数切换到科学计数法的上下阈值，分别为 `1e15` 和 `1e-15`。
- `SelectIntoContext(Arc<dyn Any + Send + Sync>)`：传给子执行器 `Open`/`Next` 的类型擦除上下文；`Clone` 只克隆 `Arc`，默认值装入空元组。
- `LineFieldsInfo`：承载 `FIELDS TERMINATED BY`、`ENCLOSED BY`、`ESCAPED BY`、`OPTIONALLY ENCLOSED`、`LINES STARTING BY` 与 `LINES TERMINATED BY` 的 Rust 数据。当前写出路径读取除 `LinesStartingBy` 外的字段；后者已保存但未被 `dumpToOutfile` 使用。
- `SelectIntoSource`：子执行器适配边界，要求真实实现提供 `Open`、`Next`、`Close`、缓存 chunk 分配、字段类型和 affected rows 记账，不含默认或占位实现。
- `SelectIntoSink`：把 `WriteAll`、`Flush`、`Close` 分开，使错误阶段可独立观测，也便于测试注入非文件 sink。
- `LocalOutfileSink`：默认本地文件 sink。`create_exclusive` 使用 `OpenOptions::create_new(true)`，Unix 下设置模式 `0640`；`Close` 取走 writer，二次 flush，经 `into_inner` 取得文件并调用 `sync_all`。
- `SelectIntoExec`：持有 source、AST `SelectIntoOption`、格式选项、四个复用缓冲区、当前包围状态、sink、缓存 chunk、字段类型缓存和生命周期标志 `started`。
- `SelectIntoExec::{Open, Next, considerEncloseOpt, escapeField, dumpToOutfile, Close}`：完整的执行与资源管理方法。它们是公开固有方法，但本结构体没有直接实现本文件外的统一 executor trait。
- `DumpRealOutfile`：独立公开浮点格式化函数，也是当前 Rust 独立测试唯一直接覆盖的生产符号。

## 执行流程

1. 构建阶段的抽象入口位于 `executorBuilder::buildSelectInto`：先递归构建 `SelectIntoPlanData::target_plan()`；若子计划失败或没有执行器，构建终止；否则把一个 child 交给 `ExecutorKind::SelectInto` 工厂。该工厂到本文件具体类型的桥接在当前 Rust 源码中未找到。
2. 对已构造的 `SelectIntoExec` 调用 `Open` 时，非 `Outfile` 类型立即返回 `unsupported SelectInto type`，不会创建文件。
3. `LocalOutfileSink::create_exclusive` 以读写、创建且不得已存在的模式打开 `intoOpt.FileName`。成功后先设置 `started` 与 writer，再从 source 获取缓存 chunk 和字段类型，预分配行/字段/转义缓冲，最后调用 `source.Open(ctx)`。
4. `Next` 每轮先重置缓存 chunk，再调用 `source.Next`。空 chunk 表示结束；非空 chunk 进入 `dumpToOutfile`。因此一次 `Next` 会消费整个 source，而不是只产生一批结果。
5. `dumpToOutfile` 为每行清空 `lineBuf`，在列间加入 `FieldsTerminatedBy`。NULL 使用“转义字符 + `N`”；若没有转义字符则使用 `NULL`。
6. 非 NULL 值先按字段类型转换。整数区分有符号/无符号 longlong；float/double 调用 `DumpRealOutfile`；decimal、字符串/blob、日期时间、duration、enum、set、JSON、向量分别使用对应 chunk getter。`BIT` 是特例：原始字节直接写入 `lineBuf`，不经过字段缓冲和转义。
7. 包围规则为：配置了包围符且不是 optional 时包围所有非 NULL 字段；optional 时只包围 `ETString`、`ETDuration`、`ETTimestamp`、`ETDatetime`、`ETJson`。只有 `ETString` 和 `ETJson` 会经过 `escapeField`。
8. `escapeField` 始终把 NUL 转成 `0` 并加转义前缀；也转义转义符自身和包围符。字段未被包围时还转义字段终止符首字节；无论是否包围都转义行终止符首字节。多字节终止符只以首字节参与判断，与 Go 实现一致。
9. 每行末尾追加 `LinesTerminatedBy`，通过 `SelectIntoSink::WriteAll` 完整写入。整批所有行均写成功后才调用 `source.AddAffectedRows(row_count)`。
10. `Close` 在 `started == false` 时幂等返回；否则收集三个关闭阶段的结果、清空 writer 并复位 `started`，最后按固定优先级返回错误。

## 数据与状态

`SelectIntoExec` 是有状态且可复用缓冲的 pull-to-sink 算子。`chk` 和 `fieldTypes` 在 `Open` 时从 source 建立；`lineBuf` 复用于整行，`fieldBuf` 复用于单字段，`escapeBuf` 复用于转义结果，`realBuf` 复用于科学计数法中间文本。`dumpToOutfile` 为解决 Rust 借用关系会对字段缓冲做局部 clone，也通过 `std::mem::take` 在 `DumpRealOutfile` 前后转移浮点缓冲所有权。

`enclosed` 是逐字段更新的瞬时状态，仅用于决定是否需要转义字段终止符。`started` 是资源生命周期门闩：创建 sink 成功后即置为 true，`Close` 完成三个阶段后恢复 false。值得注意的是，若随后 `source.Open` 失败，调用方仍须执行 `Close` 才能清理已创建文件句柄；本文件不会自动删除已经创建的 outfile。

输出数据的重要不变量是：affected rows 只在一个 chunk 全部写成功后递增；当前 chunk 中途写失败不会记入该批次，即使此前若干行已落入缓冲或文件。NULL 不加包围符；未知 MySQL 类型落入空分支，可能只写包围符/分隔符而没有字段内容，扩展类型时必须显式补充分支。

## 依赖与调用关系

直接 crate 依赖可由 `pkg/executor/Cargo.toml` 核对：`astersql-errors` 提供 `SharedError`/`New`，`astersql-parser-ast` 提供 `SelectIntoOption` 和类型枚举，`astersql-parser-mysql` 提供类型码与标志，`astersql-types` 提供 `FieldType`/`EvalType`，`astersql-util-chunk` 提供行批与类型化 getter。标准库负责文件、缓冲写、同步、类型擦除上下文和共享所有权。

已验证的上游结构边为 `Plan::SelectInto -> executorBuilder::buildSelectInto -> ExecutorBuilderDependencies::build_executor(ExecutorKind::SelectInto, ..., [child])`；模块边为 `pkg/executor/lib.rs -> pub mod select_into`。已验证的文件内调用边包括 `Next -> source.Next -> dumpToOutfile`、`dumpToOutfile -> DumpRealOutfile/escapeField/writer.WriteAll/source.AddAffectedRows`，以及 `Close -> writer.Flush -> writer.Close -> source.Close`。

RustCodeGraph 对文件给出的引用集合包含 crate 使用者，但精确符号搜索与全仓库 `rg` 均未发现本文件外构造或调用 `SelectIntoExec` 的 Rust 代码。因而“模块可见”和“builder 存在 SelectInto kind”是已证实事实，“具体执行器已被主 SQL 链实例化”则未验证，当前证据更接近缺少适配桥接。

## 错误处理与边界

- 不支持的 INTO 类型在文件创建前返回 `errors::New("unsupported SelectInto type")`。
- 目标文件使用独占创建；已存在、权限不足或路径无效等 `io::Error` 被转换为只保留文本的 `SharedError`。该设计避免覆盖已有文件，但丢失了结构化 I/O 错误类型。
- `Next` 使用 `?` 原样传播 source、序列化写入错误；它假设已经成功 `Open`，否则对 `chk` 或 writer 的 `expect` 会 panic。这是调用顺序契约，不是可恢复错误。
- `escapeField` 为空转义符时直接复制原字段，不处理 NUL 或分隔符；NULL 则输出 `NULL`。此行为由 Go 的 delimiter 测试明确覆盖。
- 字段/包围/行终止配置在本文件中只读取第一个字节用于逃逸判断；合法性检查（例如包围符必须单字符）应在更上游完成。Go 测试证明非法配置应在 outfile 留盘前失败，但本 Rust 文件没有该校验。
- `LocalOutfileSink::Close` 通过 `sync_all` 提供可报告的最终 I/O 阶段，但标准库 `File` 的 drop 本身无法报告 close 错误；因此它是对 Go `os.File.Close` 可观测性的近似，不是完全相同的系统调用语义。
- Rust 独立测试中的非有限数用例期望默认精度下正无穷输出 `Inf`；当前 `DumpRealOutfile` 的极端量级分支对正无穷显式写 `+Inf`。本任务不运行 Cargo，故这里只记录静态可见的不一致，不把该用例声明为已通过。

## 并发与资源生命周期

该类型没有内部锁，也没有声明 `Send`/`Sync` 边界；所有可变操作都要求 `&mut self`，设计上由单一执行流串行驱动。`SelectIntoContext` 内部对象要求 `Send + Sync`，但这只保证上下文可共享，不表示执行器或 sink 可以并发调用。

资源获取顺序为文件 sink、chunk/类型/缓冲、source；释放顺序为 Flush、sink Close、source Close。`Close` 会尝试全部阶段，即使较早阶段失败也不会跳过后续清理，并保留预定错误优先级。`writer.take()` 使本地 sink 自身的 Close 幂等；执行器层通过 `started` 使重复 Close 幂等。文件成功创建后没有删除回滚：失败可能留下部分或空文件，这是扩展错误恢复时必须明确决定的兼容行为。

每个 `WriteAll` 覆盖一整行，避免短写被当作成功，但 BufWriter 允许数据在 Close 前停留于用户态缓冲。`sync_all` 只在本地 sink 的 Close 阶段发生；自定义 sink 的持久化保证由其 `Close` 实现负责。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/executor/select_into.go`，Go 集成测试为 `pkg/executor/select_into_test.go`，Rust 独立测试为 `pkg/executor/select_into_test.rs`。Rust 保留了 Go 的主要结构和顺序：`Open/Next/dumpToOutfile/Close`、四类复用缓冲、包围状态、独占创建及 Unix `0640`、逐类型格式化、BIT 不转义、NULL 表示、affected rows，以及浮点阈值。

Rust 为可移植和可测试性引入了三个显式边界：`SelectIntoSource` 代替 Go 的 `BaseExecutor`/Children/StmtCtx，`SelectIntoSink` 代替 `bufio.Writer + os.File`，`SelectIntoContext` 代替 `context.Context`。Rust 还缓存 `FieldTypes`，而 Go 在每行每列通过 child schema 取得列类型。

Go 测试覆盖文件已存在、point get、BIT/ENUM/JSON/unsigned/float、表与常量输出、NULL、全包围与可选包围、自定义多字节终止符、各种转义、YEAR、affected rows 和 `DumpRealOutfile`。Rust 测试当前仅覆盖浮点精度/阈值及非有限值拼写，没有覆盖 source/sink 生命周期、真实文件权限/独占性、全部类型序列化和转义矩阵。特别地，Rust 的 `LinesStartingBy` 当前未写出；是否与所对照 Go 版本一致只能确认两边该路径均未引用该字段，不能推导完整 SQL 层是否另有处理。

## 扩展指南

- 新增或修正输出类型时，修改 `SelectIntoExec::dumpToOutfile` 的 MySQL 类型分支，并同步确认 `EvalType` 是否应走 `escapeField`、是否属于 optional enclosure。优先扩展独立的 `pkg/executor/select_into_test.rs`，不要把测试嵌入生产源文件。
- 改变转义、NULL、包围或多字节 delimiter 行为时，集中修改 `escapeField` 与 `dumpToOutfile`，并逐项对照 `pkg/executor/select_into_test.go` 的 `TestDeliminators`、`TestEscapeType` 和表输出用例，避免只为 Rust 测试简化 Go 语义。
- 改变浮点输出时，修改 `DumpRealOutfile`，同步 Rust 与 Go `TestDumpReal` 的阈值、精度、指数符号和非有限值矩阵；应先解决当前正无穷默认精度的静态不一致。
- 若补齐运行时接线，应在 `ExecutorBuilderDependencies::build_executor` 的具体实现中把 `ExecutorKind::SelectInto`、计划中的 `SelectIntoOption`/`LineFieldsInfo` 和唯一 child 适配为本文件的 `SelectIntoExec`，并使其满足主链统一 executor trait。必须增加端到端 Rust 测试，不能把 builder kind 的存在视为已接线。
- 若增加非本地 sink 或异步写出，应保持 WriteAll/Flush/Close 的错误阶段、affected rows 提交点和关闭优先级，明确线程安全与持久化语义；不要隐式改变“已存在文件不覆盖”或失败后残留部分文件的兼容行为。

## 验证依据

- RustCodeGraph 状态：项目索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/executor/select_into.rs --offset 1 --limit 500` 完整读取了 500 行目标源码。
- RustCodeGraph/代码搜索读取：`pkg/executor/builder.rs` 的 `ExecutorKind::SelectInto`、`SelectIntoPlanData`、`Plan::SelectInto` 分派与 `buildSelectInto`；`pkg/executor/lib.rs` 的模块公开和独立测试挂载。
- crate 边界：`pkg/executor/Cargo.toml` 的 package、lib path、直接依赖和 dev-dependencies。
- Go 对照：`pkg/executor/select_into.go` 全部 256 行；`pkg/executor/select_into_test.go` 全部 304 行。
- Rust 测试：`pkg/executor/select_into_test.rs` 全部 62 行。测试仅直接调用 `DumpRealOutfile`，其中默认精度正无穷的期望与当前实现静态不一致；按任务要求未运行 Cargo。
- 全仓库精确搜索：除目标文件和其 Rust 测试外，未找到 `SelectIntoExec` 或 `select_into::` 的使用；只在 `builder.rs` 找到抽象 SelectInto 构建路径。这是本文对运行时接线保持保留结论的依据。
- 文档完成后以任务指定命令校验：目标文件存在，且固定二级标题恰好 11 个；另人工检查本文没有把 Go 集成测试覆盖或 builder 抽象分派误报为 Rust 运行时已接线。
