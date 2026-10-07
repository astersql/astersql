# `pkg/planner/cascades/util/string_writer.rs`

## 文件定位

本文件是 Cascades 规划器描述文本的最小写入适配层：它把 Rust 标准库的 `std::io::Write` 包装成只暴露“写字符串”和“刷新”的 `StrBufferWriter`。源码通过 `pkg/planner/cascades/util/lib.rs` 作为 `astersql-planner-cascades-util` crate 的公开 API 导出；同时，`pkg/planner/cascades/base/lib.rs` 在 `util` 子模块中用 `include!("../util/string_writer.rs")` 嵌入同一实现，使 `Task::Desc` 等基础接口可以使用 `cascades_base::util::StrBufferWriter`。

它位于优化器的诊断/描述输出链，而不是 SQL 解析、优化规则执行或结果持久化链：例如 `pkg/planner/cascades/base/task_stack_base.rs::Task::Desc` 接收该 trait，`pkg/planner/cascades/task/task.rs::Stack::Desc` 逐个调用任务的 `Desc` 并写入换行。文件本身只负责文本缓冲和底层 I/O 适配，不决定描述内容。

## 核心职责

- `StrBufferWriter` 把调用方约束到 `WriteString(&str)` 与 `Flush()` 两个操作，隐藏 `std::io::Write` 的字节数和 `io::Result`。
- `StrBuffer<W>` 持有 `BufWriter<W>`，将多次字符串追加合并为对底层 writer 的批量写入。
- `NewStrBuffer` 接受拥有所有权或借用的任意 `W: Write`，返回动态分发的 `Box<dyn StrBufferWriter + 'a>`，让描述接口不依赖具体 sink 类型。
- `WriteString` 和 `Flush` 将 I/O 失败转成断言 panic；这与无错误返回值的公开 trait 契约一致，但意味着本层不适合需要恢复 I/O 错误的生产输出路径。

## 主要符号

- `pub trait StrBufferWriter`：公开的字符串写入契约。`WriteString(&mut self, s: &str)` 追加 UTF-8 字符串；`Flush(&mut self)` 请求把缓冲内容提交到底层 writer。命名保留 Go 版本的导出方法风格。
- `pub struct StrBuffer<W: Write>`：具体包装器，唯一字段 `bio: BufWriter<W>` 为私有字段，调用方不能绕过 trait 直接操作缓冲器。
- `pub fn NewStrBuffer<'a, W>(w: W) -> Box<dyn StrBufferWriter + 'a> where W: Write + 'a`：构造 `StrBuffer { bio: BufWriter::new(w) }` 并擦除具体类型。显式生命周期 `'a` 允许传入 `&mut Vec<u8>` 等非 `'static` writer；返回对象不能活得比该借用更久。
- `impl<W: Write> StrBufferWriter for StrBuffer<W>`：`WriteString` 调用 `BufWriter::write_all(s.as_bytes())`，`Flush` 调用 `BufWriter::flush()`；两者均用 `assert!(result.is_ok(), ...)` 处理失败。

本文件没有模块级常量、条件编译项或内部辅助函数。

## 执行流程

1. 调用方准备一个实现 `std::io::Write` 的 sink，例如 `Vec<u8>`、`&mut Vec<u8>` 或自定义共享 writer，并调用 `NewStrBuffer`。
2. `NewStrBuffer` 将 sink 移入 `BufWriter`，再以 `Box<dyn StrBufferWriter>` 返回；之后调用方只通过 trait 操作。
3. 描述生产者调用 `WriteString`。输入 `&str` 通过 `as_bytes()` 转成 UTF-8 字节切片，`write_all` 保证成功返回时整段输入已被 `BufWriter` 接受；数据通常先进入内存缓冲，但缓冲容量不足时可在调用期间写到底层，因此接口只承诺“缓冲写入”，不承诺每次调用后底层仍为空。
4. 调用方完成一组描述后显式调用 `Flush`。`BufWriter::flush` 先提交其缓冲数据，再刷新底层 writer；成功后此前被接受的数据按调用顺序交给底层。
5. 在任务栈链路中，`Stack::Desc` 依次执行各任务的 `Task::Desc`，并追加 `"\n"`；具体任务（如 `task_opt_group.rs::Desc`、`task_apply_rule.rs::Desc`）负责拼接自己的字段，本文件不插入分隔符。

## 数据与状态

持久状态只有 `StrBuffer<W>::bio`。缓冲区的容量、已缓存字节和底层 writer 均由标准库 `BufWriter` 管理，本文件没有额外计数、编码状态或全局状态。字符串以 Rust `&str` 输入，因此入口处已保证 UTF-8 合法；写入后只按原始字节顺序保存，不进行转义、格式化或换行规范化。

`NewStrBuffer` 获取 `W` 的所有权；当 `W` 本身是可变借用时，所有权实际落在该借用值上，生命周期参数阻止包装器越过被借用对象。返回的 trait object 隐藏 `W`，也不提供取回底层 writer 或检查待刷新字节数的 API。

## 依赖与调用关系

下游依赖只有标准库 `std::io::{BufWriter, Write}`。`pkg/planner/cascades/util/Cargo.toml` 没有声明第三方依赖，crate 根 `lib.rs` 公开 `string_writer` 模块并 `pub use string_writer::*`。

上游存在两条导出路径：

- 独立 util crate：规则接口 `pkg/planner/cascades/rule/rule.rs::Rule::String` 等使用 `cascades_util::StrBufferWriter`；`pkg/planner/cascades/task/task_apply_rule.rs::RuleWriter` 在 util trait 与 base trait 之间转发 `WriteString`/`Flush`。
- base crate 内嵌路径：`pkg/planner/cascades/base/task_stack_base.rs::Task::Desc` 使用 `cascades_base::util::StrBufferWriter`，任务实现和 `pkg/planner/cascades/task/task.rs::Stack::Desc` 沿该接口生成调试描述。

RustCodeGraph 对目标文件给出的文件级引用包括 `pkg/planner/cascades/base/task_stack_base.rs` 以及若干对应测试。仓库文本引用还显示 `NewStrBuffer` 在 `pkg/planner/cascades/task/task_test.rs` 中把任务描述写入 `Vec<u8>`，而生产描述函数通常接收外部提供的 trait object；因此本文件是可复用的写入基础设施，不是自行启动的顶层入口。

## 错误处理与边界

- `WriteString` 与 `Flush` 都不返回 `Result`。底层 `write_all` 或 `flush` 失败时会触发 panic，消息分别为 `buffer-io WriteString should be no error in test` 和 `buffer-io Flush should be no error in test`。
- `write_all` 失败前底层可能已收到部分字节；断言只暴露失败，不能回滚 sink。因此不要把该接口用于要求原子写入或错误恢复的路径。
- 空字符串可交给 `write_all`，本文件没有特殊分支。大字符串可能绕过或冲刷部分缓冲，这是 `BufWriter` 的正常边界，调用方不应依赖“Flush 前底层必定为空”；现有测试仅对短字符串验证该现象。
- trait 没有提供关闭操作或自动错误报告。需要确认输出完成时必须显式 `Flush`；仅让对象离开作用域不能向调用方报告最终刷新错误。
- 本层不限制输出大小，也不做 SQL/标识符转义。任何格式与安全约束都属于上游描述实现。

## 并发与资源生命周期

文件没有锁、线程、异步任务、通道或事务。每个 `StrBuffer` 需要 `&mut self` 才能写入或刷新，因此同一实例的调用天然是串行可变访问。返回类型未声明 `Send` 或 `Sync`，调用方不能从本 API 推断 trait object 可跨线程共享；如需并发，应在上层选择合适的 sink、同步策略与 trait 约束，而不是共享可变 writer。

`BufWriter` 和底层 `W` 随 `StrBuffer` 一起释放。借用型 writer 的有效期由 `Box<dyn StrBufferWriter + 'a>` 约束；`pkg/planner/cascades/util/migration_aster_unit_test.rs::new_str_buffer_accepts_a_borrowed_writer_like_go_io_writer` 用内层作用域证明包装器释放后才重新读取被借用的 `Vec<u8>`。资源交接的完成点仍是显式 `Flush`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/util/string_writer.go`：

- Go `StrBufferWriter` 的 `WriteString(string)`/`Flush()` 对应 Rust 同名 trait 方法；两边都隐藏写入长度和错误返回。
- Go `StrBuffer` 持有 `*bufio.Writer`，Rust `StrBuffer<W>` 持有 `BufWriter<W>`；Rust 用泛型表达 Go `io.Writer` 的可替换性。
- Go `NewStrBuffer(w io.Writer) StrBufferWriter` 返回接口值；Rust 返回装箱 trait object，并用 `'a` 补充静态借用检查。
- Go `WriteString` 调用 `bufio.Writer.WriteString`，Rust 调用 `write_all(s.as_bytes())`；对合法 Rust 字符串，两者都按字符串字节顺序写入，并在成功路径接受完整输入。
- Go 用 `intest.Assert(err == nil, ...)` 检查错误，Rust 使用普通 `assert!(result.is_ok(), ...)`。文档只能确认两者的接口意图与当前源码分支一致；不同构建环境下 `intest.Assert` 与 Rust `assert!` 的启用策略是否完全相同，未在本文件范围内验证。

`pkg/planner/cascades/task/task_test.go::TestTaskFunctionality` 与 Rust 的 `pkg/planner/cascades/task/task_test.rs::TestTaskFunctionality` 都通过 `NewStrBuffer`、任务 `Desc` 和 `Flush` 检查 LIFO 弹栈后的文本值，提供跨语言调用语义证据。

## 扩展指南

- 新增描述内容时，应在具体 `Desc`/`String` 实现中调用现有 `WriteString`，不要把业务格式塞进 `StrBuffer`；同步扩展对应目录的独立 `*_test.rs`，不得把测试嵌入本生产文件。
- 若需要传播 I/O 错误，应设计新的返回 `io::Result` 的接口并逐层修改调用者，不能只把内部 `assert!` 改为忽略错误，否则会改变现有失败可见性。还需与 Go 接口及其测试共同评估兼容性。
- 若需要取回 sink、配置缓冲容量或保证跨线程传递，最可能修改 `StrBuffer`/`NewStrBuffer` 的类型与返回契约；这会影响动态分发、借用生命周期以及 base/util 两条导出路径，必须同时检查 `pkg/planner/cascades/base/lib.rs` 的 `include!` 接线。
- 若只需要测试某个上游格式，可像 `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs::StringWriter` 那样实现轻量 trait writer；若要验证真实缓冲行为，则扩展 `pkg/planner/cascades/util/migration_aster_unit_test.rs`。
- 性能修改应保持写入顺序和显式刷新语义，并覆盖小片段连续追加、大字符串触发底层写入、底层 write/flush 失败等边界。当前独立 Rust 回归测试尚未构造失败 sink。

## 验证依据

- RustCodeGraph `status`：本仓库索引可用，覆盖 7,032 个 Rust 文件；`files --filter pkg/planner/cascades/util` 列出目标 Rust/Go 文件、crate 入口和独立回归测试。
- RustCodeGraph `node --file pkg/planner/cascades/util/string_writer.rs --offset 1 --limit 240`：读取目标文件全部 67 行，并报告其被 `task_stack_base.rs` 及相关测试引用。
- RustCodeGraph `query`：唯一定位 Rust trait `pkg/planner/cascades/util/string_writer.rs::StrBufferWriter`、Rust 函数 `...::NewStrBuffer` 和 Rust struct `...::StrBuffer`；限定符 `callers`/`callees` 查询未在等待窗口内返回，已停止，调用关系改由文件级索引结果和精确源码引用交叉核对。
- 源码与配置：`pkg/planner/cascades/util/string_writer.rs`、`pkg/planner/cascades/util/lib.rs`、`pkg/planner/cascades/util/Cargo.toml`、`pkg/planner/cascades/base/lib.rs`、`pkg/planner/cascades/base/task_stack_base.rs`、`pkg/planner/cascades/task/task.rs`、`pkg/planner/cascades/task/Cargo.toml`。
- Go 对照：`pkg/planner/cascades/util/string_writer.go`、`pkg/planner/cascades/task/task_test.go`。
- Rust 独立测试：`pkg/planner/cascades/util/migration_aster_unit_test.rs` 验证短文本在显式 Flush 前保持缓冲、连续写入顺序以及借用 writer；`pkg/planner/cascades/task/task_test.rs::TestTaskFunctionality` 验证真实任务描述经 Flush 后的文本；`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_apply_test.rs::StringWriter` 证明规则侧可注入自定义 trait 实现。
- 本任务为纯文档分析，按计划未运行 Cargo；交付前另运行任务指定的 11 章节结构检查。
