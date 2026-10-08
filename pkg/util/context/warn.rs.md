# `pkg/util/context/warn.rs`

## 文件定位

本文件属于 `astersql-util-context` crate；crate 入口 `pkg/util/context/lib.rs` 以 `pub mod warn` 声明模块，并以 `pub use warn::*` 重导出其公开项。`pkg/util/context/Cargo.toml` 把该 crate 映射到 Go 包 `pkg/util/context`，本文件的直接 Go 对照是 `pkg/util/context/warn.go`。

它位于 SQL 执行链的通用上下文层，不负责生成具体业务错误，而是把表达式求值、类型转换、错误降级、计划缓存和执行器产生的错误按 `Error`、`Warning`、`Note` 三种 `SHOW WARNINGS` 级别收集起来。上层典型持有者是 `pkg/sessionctx/stmtctx/stmtctx.rs` 的语句上下文；更窄的消费方通过 `WarnAppender` trait 接收追加能力，例如 `pkg/types/context.rs`、`pkg/errctx/context.rs`、`pkg/expression/exprstatic/evalctx.rs` 和 `pkg/util/context/plancache.rs`。

这是实际实现而非门面或桩：它定义数据格式、trait 契约、线程安全容器、忽略型实现和测试回调实现。文件本身没有条件编译项；独立测试由 `pkg/util/context/lib.rs` 的 `#[cfg(test)]` 模块声明接入。

## 核心职责

1. 用 `WarnLevelError`、`WarnLevelWarning`、`WarnLevelNote` 固定对外可见的严重级别字符串，供 `SHOW WARNINGS` 及上层展示逻辑使用。
2. 用 `SQLWarn` 保存一条 warning 的级别和共享错误，并通过 `MarshalJSON` / `UnmarshalJSON` 保持 Go 版本的两路 JSON 语义：结构化 `errors::Error` 写入 `err`，其它错误只写入 `msg`。
3. 通过 `WarnAppender`、`WarnHandler`、`WarnHandlerExt` 分层暴露追加、计数、复制、截断、批量与全量替换能力，使只需要追加的热路径无需依赖完整容器接口。
4. 以 `StaticWarnHandler` 提供受 `Mutex` 保护的默认内存实现，并用 `u16::MAX` 限制单条追加入口的累计数量，以对应 Go 的 `math.MaxUint16` 判断。
5. 以 `IgnoreWarn` 提供零记录单例，以 `NewFuncWarnAppenderForTest` 提供可观察追加动作的测试适配器。

## 主要符号

- `WarnLevelError` / `WarnLevelWarning` / `WarnLevelNote`：公开字符串常量，值分别为 `"Error"`、`"Warning"`、`"Note"`；级别比较是区分大小写的字符串比较。
- `SQLWarn { Level, Err }`：公开 warning 记录。`Err` 是 `Option<errors::SharedError>`，但 `MarshalJSON` 要求它为 `Some`；`Clone` 复制共享错误句柄而非重新构造错误。
- `jsonSQLWarn { Level, SQLErr, Msg }`：公开但主要供序列化内部使用的中间结构。Serde 字段名为 `level`、`err`、`msg`，空的可选字段按属性省略。
- `SQLWarn::MarshalJSON(&self) -> Result<Vec<u8>, errors::SharedError>`：先经 `errors::Cause` 去掉外层包装；最内层若可向下转换为 `errors::Error`，保存完整结构到 `err`，否则保存错误文本到 `msg`。
- `SQLWarn::UnmarshalJSON(&mut self, data)`：解析中间结构并覆盖接收者；有 `err` 时恢复结构化错误，否则始终由 `msg` 新建普通错误。
- `WarnAppender`：最小追加接口，包含 `AppendWarning` 和 `AppendNote`。
- `WarnHandler: WarnAppender`：增加 `WarningCount`、已弃用的 `TruncateWarnings` 和 `CopyWarnings`。
- `WarnHandlerExt: WarnHandler`：增加 `AppendWarnings`、`AppendError`、`GetWarnings`、`SetWarnings`、`NumErrorWarnings`；其中再次声明 `AppendNote`，调用时可能需要像测试那样以 `WarnHandlerExt::AppendNote` 消除 trait 方法歧义。
- `StaticWarnHandler { warnings: Mutex<Vec<SQLWarn>> }`：默认容器。字段公开，因此同 crate 外部也能直接加锁观察或修改，但正常调用应优先经过 trait/API。
- `NewStaticWarnHandler(sliceCap)`：正容量时预分配，零或负值时构造空 `Vec`；返回值本身不是 `Arc`，共享所有权由调用方按需包装。
- `NewStaticWarnHandlerWithHandler(h)`：从 `Option<&dyn WarnHandler>` 复制快照；`None` 构造空容器，非空输入先读计数再调用 `CopyWarnings`。
- `StaticWarnHandler::Reset`：清空元素并保留已分配容量。
- `StaticWarnHandler::appendWarningWithLevel`：单条追加的内部汇合点，负责加锁、上限检查和构造 `SQLWarn`。
- `ignoreWarn` / `IgnoreWarn`：所有操作均丢弃、计数恒为零的实现与全局实例；`ignoreWarn` 类型本身也是公开的。
- `funcWarnAppender` / `NewFuncWarnAppenderForTest`：把 Warning/Note 级别和错误转交给 `Send + Sync + 'static` 回调，返回 `Box<dyn WarnAppender>`。

## 执行流程

单条 warning 的主流程如下：上层持有 `WarnAppender` trait object 或具体 `StaticWarnHandler`，调用 `AppendWarning` / `AppendNote`；实现选择对应级别常量后进入 `appendWarningWithLevel`；该方法获得 `warnings` 互斥锁，仅在当前长度小于 `u16::MAX` 时追加 `SQLWarn { Level, Err: Some(...) }`，随后随锁守卫析构释放锁。`AppendError` 走同一内部方法，只把级别改为 `Error`。

读取与变更流程均以一次锁持有为原子区间：`WarningCount` 读取长度；`GetWarnings` 克隆整个向量；`SetWarnings` 整体替换向量；`CopyWarnings` 根据目标容量决定复用或重新分配，再复制全部元素；`Reset` 清空但保留容量。`TruncateWarnings(start)` 在锁内复制 `[start..]` 后缀并把内部向量截到 `start`，因此调用者拿到的结果不与内部向量共享底层存储。

`AppendWarnings` 是批量入口：它只在追加前检查当前长度是否小于 `u16::MAX`，满足时一次 `extend` 整批写入；它不会裁剪输入批次。因此从接近上限的位置追加大批数据后，实际长度可以超过 65535，这是与 Go 源码一致的当前行为，而不是“总量永不超过上限”的保证。

JSON 流程先处理错误因果链，再交给 Serde。普通错误和经 `Trace` 包装的普通错误最终只留下最内层消息；结构化 `errors::Error` 保存在 `err`，外层包装不进入 JSON。反序列化优先使用 `err`；若没有 `err`，即使 `msg` 缺失也会创建消息为空字符串的普通错误。

## 数据与状态

持久状态只有 `StaticWarnHandler.warnings` 中按追加顺序排列的 `Vec<SQLWarn>`。级别没有 Rust enum 约束，`SetWarnings`、`AppendWarnings` 以及公开字段允许放入任意字符串或 `Err: None`；后续 `NumErrorWarnings` 只统计 `Level == WarnLevelError` 的元素，其余值仍计入总数。

`NewStaticWarnHandler(sliceCap)` 的容量只影响初始分配，不限制后续增长。`Reset` 和有效的 `TruncateWarnings` 改变长度但通常保留容量；`SetWarnings` 接管调用方传入向量的容量；`CopyWarnings` 在容量足够时复用目标向量分配，否则创建恰能容纳当前元素的新向量。

`NewStaticWarnHandlerWithHandler` 获取的是两次调用之间的快照：它先调用 `WarningCount`，再调用 `CopyWarnings`。对并发变化的任意 `WarnHandler`，最终复制数量以 `CopyWarnings` 执行时为准，先读的计数只用于初始容量。测试证明复制结果与源 handler 不共享 `Vec` 底层数组；其中的 `SharedError` 仍按其共享语义克隆。

## 依赖与调用关系

直接依赖来自 `pkg/util/context/Cargo.toml`：`astersql-errors` 提供 `SharedError`、`Error`、`Cause`、`New` 等错误能力，`serde` / `serde_json` 提供 JSON 派生和编解码；`std::sync::Mutex` 保护集合。parser terror 由 crate 入口重导出，测试用它构造结构化错误，本文件通过统一的 `errors::Error` 类型识别该类错误。

RustCodeGraph 将本文件标记为被 45 个文件使用。关键上游调用边包括：

- `pkg/sessionctx/stmtctx/stmtctx.rs` 创建两个 `NewStaticWarnHandler`，把它们桥接为类型转换、错误上下文和计划缓存所需的 `WarnAppender`，并转发 `GetWarnings`、`SetWarnings`、`AppendWarning`、`AppendNote`。
- `pkg/expression/exprstatic/evalctx.rs` 创建、克隆并适配 handler，说明它是表达式静态求值上下文的 warning 存储。
- `pkg/errctx/context.rs` 以 `Arc<dyn WarnAppender + Send + Sync>` 接收该接口，在错误被降级时追加 warning。
- `pkg/types/context.rs` 和 `pkg/planner/planctx/context.rs` 仅依赖最小追加 trait，避免暴露完整 warning 集合控制面。
- `pkg/util/context/plancache.rs` 通过 `WarnAppender` 报告计划缓存跳过或强制使用风险；其迁移测试用 `NewFuncWarnAppenderForTest` 验证只告警一次的 range fallback。
- RustCodeGraph 的调用者结果还显示 `AppendWarning` 进入 `pkg/distsql/context/context.rs`、`pkg/executor/compact_table.rs`、`pkg/executor/foreign_key.rs`、`pkg/planner/core/plan_cache_utils.rs` 和 `pkg/session/tidb.rs`；`GetWarnings` / `SetWarnings` 则进入会话、规划、DDL 与 `SHOW WARNINGS` 相邻流程。

下游调用集中且无 I/O：构造函数调用 `Vec` 与 `Mutex`；追加/读取调用锁和向量操作；JSON 方法调用 `errors::Cause`、类型向下转换及 `serde_json::{to_vec, from_slice}`。本文件不启动线程、任务或通道，也不直接访问 SQL、网络或存储。

## 错误处理与边界

- `MarshalJSON` 在 `Err == None` 时通过 `expect("SQLWarn.MarshalJSON requires Err")` panic；`pkg/util/context/warn_test.rs::TestSQLWarnMarshalNilErrorPanics` 把这记录为与 Go 对非法 nil 错误输入相同的失败类别。
- Serde 编解码失败以 `errors::SharedError` 返回；反序列化在解析成功后才覆盖接收者，因此语法错误不会部分修改 `SQLWarn`。
- 所有 `StaticWarnHandler` 锁操作都以 `expect("StaticWarnHandler mutex poisoned")` 处理锁中毒；持锁代码 panic 后，后续访问会 panic，而不是返回可恢复错误。
- `TruncateWarnings(start)` 对 `start >= len` 返回空且不修改状态；合法范围复制后缀再截断。负数会转换为巨大 `usize` 并在切片时 panic，保留 Go 对负索引非法调用的失败语义。
- 单条追加达到 65535 条后静默丢弃，不返回错误或丢弃计数；批量追加只检查批次前长度，可能越过该值。
- `NumErrorWarnings` 的错误计数为 `u16` 并用 `wrapping_add`；如果集合因批量或 `SetWarnings` 超过 65535 个 Error，计数按 16 位回绕，总数仍以 `usize` 返回。
- `IgnoreWarn` 明确丢弃输入；它适用于调用方有追加契约但不需要观察 warning 的路径，不适合需要审计或展示 warning 的路径。
- 回调型 appender 不捕获 panic；回调 panic 会直接传播给调用者。

## 并发与资源生命周期

`StaticWarnHandler` 的每个公开操作独立获得 `std::sync::Mutex`，所以单次追加、复制、截断、重置或替换不会观察到半写状态。它没有内部 `Arc`；跨线程共享时，上层通常像 `stmtctx.rs` 和表达式上下文那样包装为 `Arc<...>`。`Mutex<Vec<SQLWarn>>` 以及 `SharedError` 的线程属性决定具体 handler 能否满足 `Arc<dyn WarnAppender + Send + Sync>` 的调用位置。

复合操作不具备跨方法事务性。例如“读 `WarningCount`，执行工作，再 `TruncateWarnings`”之间可插入其它线程追加；trait 文档因此把 `TruncateWarnings` 标为弃用，并建议使用临时 handler 或专门的计数 handler。`NewStaticWarnHandlerWithHandler` 同样不是针对任意并发源的单锁快照。

锁只覆盖内存操作，JSON 编解码和测试回调都不在 `StaticWarnHandler` 锁内执行。`GetWarnings` 在 Rust 中克隆后释放锁，调用者可自由持有或修改返回值而不影响 handler；`CopyWarnings` 与 `TruncateWarnings` 也返回独立向量。回调 appender 要求闭包为 `Send + Sync + 'static`，但闭包内部状态的同步仍由闭包实现负责。

资源均由 RAII 管理：锁守卫在方法返回时释放，向量和错误引用随所有者释放，没有显式关闭流程。`Reset` 保留容量，适合语句上下文重复使用；若需要立即释放大容量，应以 `SetWarnings(Vec::new())` 替换，而不是依赖 `Reset`。

## 与 Go 版本的对应关系

`pkg/util/context/warn.go` 是逐项对照基准。三个级别常量、`SQLWarn` / `jsonSQLWarn` 字段、三层接口、`StaticWarnHandler`、`ignoreWarn` 和 `funcWarnAppender` 均有一一对应符号。Rust 的 `Option<&dyn WarnHandler>` 对应可为 nil 的 Go 接口；`Mutex<Vec<_>>` 对应嵌入的 `sync.Mutex` 加 warning slice；`u16::MAX` 对应 `math.MaxUint16`。

重要语义保持包括：只序列化最内层结构化错误；普通错误退化为消息；单条追加在上限后静默丢弃；批量追加可能越过上限；复制结果与内部切片分离；截断返回被移除后缀；忽略 handler 永远为空；测试回调仅建议用于测试。

存在需要扩展者注意的 Rust 表达差异：

- Go `GetWarnings` 返回内部 slice 别名且注释要求不得修改；Rust 无法安全地跨锁返回该引用，所以返回完整克隆。调用者修改返回值不会改变 handler，并多出 O(n) 克隆成本。
- Go 的 handler 构造函数返回指针；Rust 返回值类型，调用方需要共享时自行包装 `Arc`。
- Rust 的 `Reset` 也加锁，而当前 Go `Reset` 未加锁；Rust 在并发下更一致，但两端仍不应把多方法序列当作事务。
- Go mutex 没有 poisoning；Rust 锁中毒后通过 `expect` panic。
- Rust `WarnHandlerExt` 与 `WarnAppender` 都声明 `AppendNote`，静态分派处可能需要显式指定 trait；Go 的嵌入接口没有这种调用语法问题。
- Go JSON 方法由 `encoding/json` 自动发现；Rust 当前由调用者显式调用 `MarshalJSON` / `UnmarshalJSON`，`SQLWarn` 本身没有派生或实现 Serde trait。

`pkg/util/context/warn_test.go` 与 `pkg/util/context/warn_test.rs` 的同名测试确认核心移植语义；Rust 额外的 `migration_aster_unit_test.rs` 覆盖 Warning/Note/Error 全路径、错误计数、Reset、结构化 JSON 和计划缓存集成。

## 扩展指南

新增级别时，至少同步级别常量、所有构造 warning 的入口、`NumErrorWarnings` 或新的分类统计、Go 对照文件以及独立测试；还要确认 `SHOW WARNINGS` 的上层展示是否接受新字符串。若只是新增 warning 生产点，应依赖 `WarnAppender`，不要无必要地要求 `WarnHandlerExt`，以保持调用面最小。

修改容量策略时应同时检查 `appendWarningWithLevel` 和 `AppendWarnings`。若目标是严格总量上限，不能只修改单条入口；还需定义批次是截断、全丢弃还是返回错误，并评估与 Go 当前行为和已有调用方的兼容性。修改 `NumErrorWarnings` 的计数类型会影响公开 trait，属于跨 crate API 变化。

替换或删除 `TruncateWarnings` 前，应先迁移依赖“读取起点后取新增 warning”的调用者，优先为局部操作建立临时 handler。不要让调用者直接操作公开的 `warnings` 锁；若要收紧字段可见性，需要先搜索测试和外部 crate 的直接访问。

修改 JSON 时必须分别覆盖普通错误、Trace 包装的普通错误、结构化 `errors::Error`、缺失字段、非法 JSON 和 `Err: None`。任何字段名或省略规则变化都可能破坏 Go/Rust 互操作或持久化数据兼容。若希望 `SQLWarn` 直接支持 Serde，应验证不能绕过当前 `errors::Cause` 和结构化错误分支。

并发扩展应避免在持有 `warnings` 锁时调用外部回调、格式化复杂错误或执行 I/O。若要提供一致快照加后续变更的复合操作，应新增单方法 API，在一次锁持有内完成，而不是组合 `WarningCount`、`GetWarnings`、`TruncateWarnings`。

测试必须保持在独立文件中：核心对照用 `pkg/util/context/warn_test.rs`，AsterSQL 特有或跨模块迁移场景用 `pkg/util/context/migration_aster_unit_test.rs`；同时核对 `pkg/util/context/warn_test.go` 的测试意图，不把测试内嵌回生产文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/context` 确认 `warn.rs`、`warn.go`、两端测试及模块文件均被索引。
- RustCodeGraph `node --file pkg/util/context/warn.rs --offset 1 --limit 500`：读取目标文件全部 433 行，确认常量、结构体、trait、实现、可见性和无条件编译分支。
- RustCodeGraph `query` / `explore` / `callers` / `callees`：确认 `NewStaticWarnHandler`、`NewStaticWarnHandlerWithHandler`、`NewFuncWarnAppenderForTest` 的双语言定义，并取得 `AppendWarning`、`GetWarnings`、`SetWarnings` 的上游调用结果；精确图命令对部分 trait/方法没有单独输出，因此再以索引的 `explore` 调用者结果及窄范围 `rg` 核对具体 Rust 使用点。
- 已读生产与装配文件：`pkg/util/context/warn.rs`、`pkg/util/context/warn.go`、`pkg/util/context/lib.rs`、`pkg/util/context/Cargo.toml`；调用关系抽查了 `pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/errctx/context.rs`、`pkg/types/context.rs`、`pkg/planner/planctx/context.rs`、`pkg/util/context/plancache.rs` 的符号引用。
- 已读独立测试：`pkg/util/context/warn_test.rs`、`pkg/util/context/warn_test.go`、`pkg/util/context/migration_aster_unit_test.rs`。覆盖证据包括 JSON 往返与 nil panic、IgnoreWarn 空操作、CopyWarnings 容量复用/重新分配、TruncateWarnings 边界、handler 独立复制、三种级别和错误计数、Reset，以及测试回调在计划缓存 range fallback 中的使用。
- 本任务是纯文档分析，按计划不运行 Cargo；最终仅用任务指定命令验证本文档存在且恰有 11 个固定二级标题，并人工复核没有把未验证设计写成当前事实。
