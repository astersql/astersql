# `pkg/executor/trace.rs`

## 文件定位

该文件属于 `astersql-executor` crate；crate 根在 `pkg/executor/Cargo.toml` 中指定为 `lib.rs`，并由 `pkg/executor/lib.rs` 的 `pub mod trace;` 公开模块。它承载 `TRACE` SQL 的 Rust 侧核心算法：执行被包裹的语句、收集 basictracer 风格日志或 appdash 风格树、把轨迹写入结果 Chunk，以及生成 optimizer trace 归档名。

当前实现采用泛型 trait 隔离 TiDB 会话、SQL 执行器、tracer、Chunk 与外部存储。仓库搜索只找到本文件中的 `TraceBackend` 和 `OptimizerTraceStorage` 定义，没有生产实现或 Rust 侧 `TraceExec` 构造点。因此，本文件已经实现可复用的 TRACE 控制流和格式化逻辑，但尚未像 Go 的 `executorBuilder.buildTrace` 那样接入完整生产执行链；现有 Rust 接线是 `lib.rs` 的模块导出及独立单元测试模块 `trace_test.rs`。

## 核心职责

1. `TraceExec::Next` 提供一次性拉取接口：清空输出、检查 `exhausted`、保存并恢复 statement context，然后按 `optimizerTrace` 和 `format` 分派。
2. `nextTraceLog` 使用 basic trace 边界收集 `RawSpan`，由 `generateLogResult` 生成四列日志结果。
3. `nextRowJSON` 使用树形 trace 边界收集 `TraceNode`：非 JSON 格式经 `dfsTree` 深度优先输出三列树，JSON 格式序列化后按 4096 字节分行。
4. `executeChild` 在 restricted SQL 与内部 TRACE source 上下文中执行真实语句，排空可选结果集、记录执行/关闭错误和影响行数，并恢复 restricted SQL 标志。
5. `generateOptimizerTraceFile` 通过存储抽象生成随机、带纳秒时间戳的 ZIP 文件名并创建归档 writer；它和已经废弃的 `optimizerTrace` 执行分支是两个不同层面的能力。

## 主要符号

- 常量 `TRACE_FORMAT_LOG`、`TRACE_FORMAT_JSON`、`TRACE_EVENT_KEY` 分别定义格式分派值与日志字段过滤键；私有 `MAX_JSON_ROW_LEN = 4096` 保持 Go 的单行切分边界。
- `TraceChunk` 是结果行写入接口。`append_string` 用于 row/log，`append_bytes` 保留 JSON 原始字节语义，`append_time` 避免本文件绑定具体时间类型。
- `TraceLogField`、`TraceLog<T>`、`RawSpan<T>` 是 log 路径消费的数据模型。`RawSpan::formatted_tags` 已是后端按 Go `%v` 语义格式化后的可选字符串。
- `TraceTimespan` 和 `TraceNode` 是树路径数据模型。`start_order` 用于排序；展示用时间和时长已由后端预格式化。
- `TraceRecordSet` 抽象结果集的 Chunk 创建、逐批 `next` 与 `close`。
- `TraceBackend<S>` 是主要生产边界，集中声明 statement context、basic/tree tracer、SQL 执行、错误码、affected rows、事件记录和关闭错误处理；所有方法都无默认实现。
- `TraceExec<B, S, R, E>` 保存后端、被包裹语句、格式和一次性状态。`resolveCtx`、`builder`、`optimizerTraceTarget` 与 Go 字段对齐，但本文件当前没有读取这三个字段。
- `drainRecordSet` 消费子语句的全部结果；`dfsTree` 原地排序并渲染树；`generateLogResult` 过滤并展开 span 日志。
- `OptimizerTraceStorage` 隔离目录、时钟、随机源与 archive 创建；`generateOptimizerTraceFile`、私有 `joinPath`、私有 `base64UrlEncode` 共同完成归档创建。

## 执行流程

`TraceExec::Next` 首先调用 `request.reset()`。若 `exhausted` 已为真则直接成功返回，使 TRACE 只产出一批结果。否则保存 `BaseExecutor` 的 statement context；之后所有正常或错误返回都经过统一的 `restore_statement_context`，模拟 Go `defer`。`optimizerTrace` 为真时直接返回后端提供的 deprecated 错误；其余情况先补充 trace exec details，再按 `format == "log"` 进入 log 路径，否则进入 row/JSON 路径。

log 路径依次执行 `begin_basic_trace`、`executeChild`、`finish_basic_trace`、`generateLogResult`。每个 span 先生成一行起始记录；随后仅把字段键等于 `TRACE_EVENT_KEY` 的 log field 展开成事件行。成功后置 `exhausted = true`。

row/JSON 路径依次执行 `begin_tree_trace`、`executeChild`、`finish_tree_trace`。row 分支只取 `traces.first_mut()`；存在根节点时由 `dfsTree` 先写根、按 `start_order` 稳定排序每层子节点，再递归写入，随后耗尽。JSON 分支调用 `marshal_trace_json`，按字节而不是字符每 4096 字节写一行；数据长度恰好是 4096 的倍数时还追加空行，以复现 Go 循环剩余片段的行为。

`executeChild` 保存原 `restricted_sql`，设为 `true`，并把 context 标记为内部 TRACE 来源。`execute_stmt` 的错误不会从该函数向外传播，而会转换 SQL 错误码并记入 trace event；若返回 RecordSet，则 `drainRecordSet` 循环读取至错误或空 Chunk，之后总是尝试 `close`。最后记录 affected rows 并恢复原 restricted SQL 值。

## 数据与状态

`TraceExec::exhausted` 是最重要的跨调用状态：只有各格式成功完成输出时才被置真；tracer 启动、结束或 JSON 序列化失败时保持原值，允许调用方观察错误而不是误判已完成。`CollectedSpans` 在当前 Rust 方法中没有被读写，实际 log span 直接取自 `finish_basic_trace` 返回值。

`dfsTree` 会原地重排 `TraceNode.children`，因此调用后输入树的兄弟顺序发生变化。缺失 `timespan` 的节点使用 `start_order = 0` 参与排序，并展示 `00:00:00.000000`、`0s`。树的输出列为 operation、formatted start、formatted duration；log 输出列为 timestamp、message、tags、operation。

`drainRecordSet` 的 `row_count` 累加每批 `num_rows()`；每个非空批次后必须 `reset` Chunk。`generateOptimizerTraceFile` 每次填充 16 字节随机数，以带 `=` padding 的 URL-safe Base64 编码，文件名为 `optimizer_trace_<key>_<unix-nanos>.zip`。

## 依赖与调用关系

RustCodeGraph 对 `TraceExec::Next` 的下游边确认其调用 `reset`、statement context 保存/恢复、trace details、deprecated error、`nextTraceLog` 和 `nextRowJSON`。`nextTraceLog` 继续调用 basic tracer 边界、`executeChild` 与 `generateLogResult`；`nextRowJSON` 调用 tree tracer 边界、`executeChild`、`dfsTree` 或 JSON marshal/`append_bytes`。`executeChild` 调用 restricted/source/execute/event/close 后端方法与 `drainRecordSet`。归档函数调用随机源、目录、`joinPath`、`base64UrlEncode` 和 `create_archive`。

该文件除 `std::fmt::Display` 外没有直接绑定外部 crate；真正的 TiDB 类型和第三方 tracer/存储依赖由 trait 实现方承担。`pkg/executor/Cargo.toml` 表明它位于大型 executor crate 中，且 `nextgen` 是唯一声明的 feature；trace 模块没有条件编译项，也不受该 feature 控制。

上游方面，RustCodeGraph 将文件标记为被多个文件引用，但精确 Rust 搜索显示生产代码没有 `TraceBackend` 实现或 `TraceExec` 构造点。确定的 Rust 调用者只有本文件内部调用和 `pkg/executor/trace_test.rs` 对公开 helper 的调用。完整应用中的对应生产入口目前仍在 Go：`pkg/executor/builder.go::buildTrace` 构造 `TraceExec`，log 格式还外包 `SortExec` 按时间列排序。

## 错误处理与边界

`Next`、tracer begin/finish 与 JSON marshal 使用 `Result` 和 `?` 传播后端错误，同时保证已保存的 statement context 在分派结果产生后恢复。需要注意：若后端方法自身 panic，Rust 没有 RAII guard，statement context 不会像 Go `defer` 那样恢复。

`executeChild` 有意把执行错误和结果集读取错误降级为 trace event，使用户仍能看到失败前后的诊断轨迹；RecordSet `close` 错误交给 `close_error`，同样不改变外层返回值。它在函数末尾显式恢复 restricted SQL，但发生 panic 时也没有 guard；后端实现需避免 panic，或未来改为作用域守卫。

JSON 按原始字节切分，允许边界落在 UTF-8 多字节字符中；这是接口专门提供 `append_bytes` 的原因。row 格式忽略第一棵以外的 trace 根；空 trace 列表仍会成功并耗尽。`generateLogResult` 假定后端已将事件值安全转为字符串，不复现 Go 的运行时类型断言 panic。

归档创建依次可能在随机生成和 `create_archive` 失败；任一步失败都返回错误且不返回文件名。与 Go 实现不同，获取全局 storage 和包装 `replayer.FileWriter` 的职责已经下沉到 `OptimizerTraceStorage::create_archive` 实现方，而当前仓库尚无该实现。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`TraceExec::Next` 需要 `&mut self`，同一实例的状态修改由 Rust 独占借用串行化；trait 没有声明 `Send`/`Sync`，不能据此推断跨线程安全。

basic/tree trace handle 的生命周期是 begin → 执行子语句 → finish；finish 失败时不会设置 `exhausted`。RecordSet 生命周期是可选获得 → 完全排空或遇错 → `close`，关闭失败仅记录。Archive writer 的所有权随 `generateOptimizerTraceFile` 成功返回给调用者，本文件不负责 flush、close 或 drop 时的错误处理。

statement context 与 restricted SQL 是临时会话状态。前者在 `Next` 的结果汇合点恢复，后者在 `executeChild` 末尾恢复；二者都依赖无 panic 的控制流。`dfsTree` 的递归深度等于 trace 树深度，代码没有深度限制；极端深树存在栈使用风险。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/trace.go`。Rust 的 `TraceExec::Next`、`nextTraceLog`、`nextRowJSON`、`executeChild`、`drainRecordSet`、`dfsTree`、`generateLogResult`、`generateOptimizerTraceFile` 分别对应 Go 同名符号，主分支、事件文本、树形前缀、兄弟排序、4096 字节 JSON 分片和归档命名均保留 Go 语义。

主要结构差异来自适配方式：Go 直接依赖 `exec.BaseExecutor`、session context、opentracing/basictracer/appdash、global external storage；Rust 通过 `TraceBackend`、`TraceRecordSet`、`TraceChunk`、`OptimizerTraceStorage` 注入。Rust 的时间和 duration 由后端预格式化，tags 也由后端提供 Go `%v` 表示。

Go `builder.go::buildTrace` 是生产构造入口，且为非 optimizer 的 log 格式添加按 timestamp 排序的 `SortExec`。Rust 当前无等价 builder 接线，因此不能仅凭本文件断言端到端 `TRACE` 已在 Rust server 可用。Go `trace_test.go::TestTraceExec` 覆盖 insert/select/delete/analyze、row 顺序和 log 查询；Rust `trace_test.rs` 目前只覆盖树排序/前缀与 event 字段过滤，未覆盖 `TraceExec::Next`、错误路径、JSON 分片、状态恢复或 archive 创建。

## 扩展指南

- 接入生产执行链时，应在 executor builder/adapter 层实现真实 `TraceBackend`、Chunk 和 RecordSet 适配，并构造 `TraceExec`；同时复现 Go `buildTrace` 对 log 输出的时间排序，而不是把排序隐含塞进 `generateLogResult`。
- 新增格式时修改 `TraceExec::Next` 的显式分派及相应 schema/构建逻辑。当前“不是 log 就走 row/JSON”意味着未知格式会被当作 row，解析/规划层必须保证格式合法，或在此增加验证。
- 修改 JSON 切分时必须保留字节边界、整倍数空末行等兼容细节，并在独立 `pkg/executor/trace_test.rs` 增加 4095、4096、4097 字节及多字节字符用例。
- 修改子语句执行时同步验证 statement context、restricted SQL、internal source、执行错误、读取错误、关闭错误和 affected rows；建议为状态恢复引入 guard 前先明确 panic 兼容语义。
- 修改树渲染时同步覆盖空 timespan、相同 `start_order`、多层前缀和极深树；修改 log 渲染时覆盖空 tags、多 field 和非 event 字段。
- 接入 optimizer archive 时实现 `OptimizerTraceStorage`，验证随机失败、创建失败、目录尾斜杠、URL-safe Base64 padding 及 writer 生命周期。测试逻辑应继续放在独立 `trace_test.rs`，不要内嵌进生产源文件。
- 性能风险主要是 trace 全量驻留内存、树递归与排序、JSON 一次性序列化；扩展时避免额外复制或在未验证 Go 兼容性的情况下改成流式输出。

## 验证依据

- Rust 源与符号：`pkg/executor/trace.rs`；RustCodeGraph `node --file` 核对 453 行全貌，并对 `Next`、`nextTraceLog`、`nextRowJSON`、`executeChild`、`drainRecordSet`、`dfsTree`、`generateLogResult`、`generateOptimizerTraceFile` 执行精确 `callees --file pkg/executor/trace.rs`。
- crate 与模块：`pkg/executor/Cargo.toml` 的 package/lib/features/dependencies；`pkg/executor/lib.rs` 的 `pub mod trace;` 及 `#[path = "trace_test.rs"] mod trace_test;`。
- Go 对照与生产入口：`pkg/executor/trace.go`、`pkg/executor/builder.go::buildTrace`；Go 回归测试为 `pkg/executor/trace_test.go::TestTraceExec`。
- Rust 独立测试：`pkg/executor/trace_test.rs::trace_tree_sorts_children_and_preserves_row_prefixes` 与 `trace_log_emits_only_event_fields_with_span_tags`。
- 接线限制：全仓精确搜索未发现 `TraceBackend` 或 `OptimizerTraceStorage` 的 Rust 实现，也未发现生产侧 Rust `TraceExec` 构造表达式；因此端到端 Rust TRACE 状态记为“未接线/未验证”，而非“已支持”。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前只执行任务指定的 11 章节结构校验，并人工复核上述路径、符号和限制描述。
