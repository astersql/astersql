# `pkg/ddl/ingest/message.rs`

## 文件定位

该文件属于 `astersql-ddl-ingest` crate，由 [`pkg/ddl/ingest/lib.rs`](lib.rs) 以公开模块 `message` 暴露。它位于 DDL 加索引的 ingest（本地排序、写引擎后批量导入）子系统，但自身不执行 DDL 状态迁移、回填、写盘或导入；它只集中定义日志/错误文案，以及把内存分配失败现场转换成结构化快照。

crate 边界由 [`pkg/ddl/ingest/Cargo.toml`](Cargo.toml) 确定，Go 包映射为 `pkg/ddl/ingest`。本文件的唯一直接 Rust 模块依赖是 [`crate::mem_root::MemRoot`](mem_root.rs)，不直接使用 Cargo 中列出的外部依赖。

## 核心职责

1. 提供 33 个公开的 `&'static str` 文案常量：19 个 `LIT_ERR_*`、2 个 `LIT_WARN_*` 和 12 个 `LIT_INFO_*`。源码注释要求它们与 [`message.go`](message.go) 保持同步。
2. 用 `IngestMemoryError` 保存内存拒绝发生时的固定消息、DDL job ID、相关索引 ID，以及 `MemRoot` 的当前用量和最大配额。
3. 分别为“批量创建索引引擎”和“创建单索引写入器”提供快照构造函数。

当前 Rust 生产代码中没有检出这些常量或两个构造函数的调用者；它们由 [`message_test.rs`](message_test.rs) 直接验证。因此，本文件目前提供的是公开的数据/文案接口和 Go 语义移植基础，不能据此断言 Rust ingest 生产链已经在内存不足时返回该结构。

## 主要符号

- `LIT_ERR_*`：错误场景文案，覆盖内存分配、排序目录、后端/引擎/写入器、存储配额、刷写/导入、远端重复键、并发上限和清理/重置等场景。
- `LIT_WARN_*`：环境初始化失败与后端配置构建失败的警告文案。
- `LIT_INFO_*`：环境、排序目录、后端、引擎、写入器、重复检查、导入及内存配置变化的过程文案。
- `pub struct IngestMemoryError`：可克隆、可调试、可相等比较的内存失败快照。`message` 是静态字符串；`job_id` 是 DDL 任务标识；`index_ids` 拥有索引 ID；`current_usage` 与 `max_quota` 是构造时读取的字节数值。
- `engine_alloc_memory_failed(mem_root, job_id, index_ids)`：复制完整索引切片，适合一次为多个索引创建引擎失败的上下文。
- `writer_alloc_memory_failed(mem_root, job_id, index_id)`：把单个索引 ID 包装为单元素向量，适合某索引的 writer 分配失败上下文。

所有符号均为公开项；文件没有 trait、impl、宏、泛型或条件编译项。

## 执行流程

两个构造函数采用相同的同步流程：

1. 调用者先在 ingest 的配额检查处判定无法继续分配；该判定本身不在本文件内。
2. 调用构造函数，并传入 `&dyn MemRoot`、DDL job ID 和一个或多个索引 ID。
3. 函数把 `message` 固定为 `LIT_ERR_ALLOC_MEM_FAIL`。
4. 引擎版本通过 `index_ids.to_vec()` 取得调用参数的所有权副本；writer 版本创建 `vec![index_id]`。
5. 依次调用 `MemRoot::current_usage()` 与 `MemRoot::max_memory_quota()`，把当时观测到的值写入结果。
6. 直接返回 `IngestMemoryError`，不写日志、不改变配额、不重试，也不把结构转换为通用错误类型。

Go 对照链更完整：[`engine_mgr.go`](engine_mgr.go) 的引擎配额检查失败时调用 `genEngineAllocMemFailedErr`；[`engine.go`](engine.go) 的 writer context 和本地 writer 缓存创建前检查失败时调用 `genWriterAllocMemFailedErr`。RustCodeGraph 与仓库文本搜索没有发现对应的 Rust 生产调用边。

## 数据与状态

`IngestMemoryError` 是拥有数据的不可变快照，而不是对 `MemRoot` 的持续观察：索引 ID 被复制进新 `Vec<i64>`，用量与配额也按值保存。构造后 `MemRoot` 的变化不会反映到已有快照中。

`message` 的类型为 `&'static str`，当前两个构造函数都只赋值 `LIT_ERR_ALLOC_MEM_FAIL`。结构没有时间戳、worker ID、底层错误、上下文对象或错误码，也没有实现 `Display`、`std::error::Error` 或序列化 trait。

一次构造要分别读取当前用量和最大配额。`MemRoot` 接口没有提供同时读取两者的原子快照操作，因此在并发修改配额/用量时，两字段可能代表相邻时刻的观测；该结构只保证记录函数实际读到的值，不保证跨字段一致性。

## 依赖与调用关系

- 模块入口：[`lib.rs`](lib.rs) 的 `pub mod message` 暴露本文件；`#[cfg(test)] mod message_test` 把测试保持在独立文件中。
- 下游依赖：两个构造函数仅调用 [`mem_root.rs`](mem_root.rs) 的 `MemRoot::current_usage` 和 `MemRoot::max_memory_quota`。默认 `MemRootImpl` 用不同的 `Mutex` 分别保护用量和最大配额。
- 已验证 Rust 上游：[`message_test.rs`](message_test.rs) 调用两个构造函数，并通配导入全部消息常量。
- Rust 生产上游：RustCodeGraph `callers` 查询和 `rg` 均未发现；当前没有证据表明生产路径消费 `IngestMemoryError`。
- Go 上游：[`engine_mgr.go`](engine_mgr.go) 在多个引擎所需内存超额时走 engine 版本；[`engine.go`](engine.go) 在 writer context 固定开销或本地 writer 缓存开销超额时走 writer 版本。
- Cargo：[`Cargo.toml`](Cargo.toml) 将 crate 名定为 `astersql-ddl-ingest`，并记录 Go 包映射；本文件不直接调用其中的 `astersql-util-dbterror`，这正是它与 Go 返回错误路径尚未等价的一项证据。

## 错误处理与边界

本文件描述错误，但不执行错误处理。两个函数均无 `Result` 返回值、无显式失败分支，也不会检查 `current_usage > max_quota`；是否拒绝分配由调用者负责。它们接受任意 `i64` job/index ID 和 `MemRoot` 返回值，因而不会拒绝负 ID、负用量或负配额。

潜在 panic 来自具体 `MemRoot` 实现而非本文件签名。例如 [`MemRootImpl`](mem_root.rs) 使用 `Mutex::lock().unwrap()`，锁中毒时读取方法会 panic。引擎版本还会为 `index_ids.to_vec()` 分配内存；在极端分配失败时不能保证仍能成功建立错误快照。

与 Go 版本相比，Rust 构造函数不接收 `context.Context` 等价物，不记录 warning，也不返回 `dbterror.ErrIngestFailed.FastGenByArgs("memory used up")` 对应的用户可见错误。扩展生产调用前必须决定：该结构仅供诊断，还是需要实现标准错误并进入统一 DDL 错误传播链。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁、文件、引擎或事务，也没有需要显式释放的资源。`&dyn MemRoot` 只在构造调用期间借用；返回对象不持有该引用，因此其生命周期独立于配额根。

`MemRoot: Send + Sync` 允许调用者跨线程共享具体实现。构造函数本身只读，但默认实现的两次读取各自获取互斥锁；并发安全由 `MemRoot` 实现保证，跨字段原子一致性则不在接口契约内。`Vec<i64>` 的所有权复制使调用者可以在返回后修改或销毁原索引切片而不影响快照。

## 与 Go 版本的对应关系

[`message.go`](message.go) 是直接语义对照文件：33 个 Rust 常量与 Go 常量的文本逐项一致，且 [`message_test.rs`](message_test.rs) 对每一项固定文本做了断言。

`engine_alloc_memory_failed` 对应 Go `genEngineAllocMemFailedErr` 的诊断字段：job ID、多个 index ID、当前内存用量和最大配额。`writer_alloc_memory_failed` 对应 `genWriterAllocMemFailedErr`，区别是单个 index ID 被 Rust 统一存入 `Vec<i64>`。

关键差异是 Go 函数有两项副作用/结果：通过上下文 logger 写 warning，并返回统一的 `ErrIngestFailed("memory used up")`；Rust 函数只返回结构化数据，不写日志、不返回标准错误。Go 调用点已接入引擎和 writer 的真实配额拒绝分支，而 Rust 侧目前只在测试中使用。因此迁移状态应表述为“文案和失败现场数据已移植，生产错误传播与调用接线未验证/未检出”，不能视为完整等价。

## 扩展指南

- 新增或修改文案时，同时更新 [`message.go`](message.go) 与 [`message_test.rs`](message_test.rs) 的逐项断言；文案可能被日志检索、运维告警或兼容性检查依赖，应避免无意改词。
- 给 `IngestMemoryError` 增加字段时，应在两个构造函数中同时初始化，并在独立测试文件补充 engine 多索引和 writer 单索引断言。不要把测试嵌入生产源文件。
- 若接入 Rust 生产配额拒绝路径，应优先复用现有 `MemRoot::check_consume` 周边逻辑，并明确日志、错误码和调用方期望；需对照 Go 的 [`engine_mgr.go`](engine_mgr.go) 和 [`engine.go`](engine.go)，但仅移植当前调用语义，不扩大为重建整个 ingest 子系统。
- 若实现 `Display`/`Error` 或转换到数据库错误，需保留 job/index/配额诊断信息，同时验证外部可见错误是否必须与 Go 的 `ErrIngestFailed("memory used up")` 兼容。
- 若要求严格一致的用量/配额快照，应扩展 `MemRoot` 提供单次一致读取，而不是假设现有两个 getter 原子组合；这会影响 trait、实现及 [`mem_root_test.rs`](mem_root_test.rs)，需要单独评估并发和兼容风险。
- 性能上，engine 构造函数的主要额外成本是复制索引 ID；通常索引数较小，但高频错误路径或超大切片场景仍应避免不必要重复构造。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ddl/ingest` 覆盖 `message.rs`、`message_test.rs`、`mem_root.rs` 及 Go 对照；`explore "pkg/ddl/ingest/message.rs ..."` 返回目标文件全貌；`query IngestMemoryError` 定位结构与两个构造函数；两个函数的 `callers`/`callees` 查询未给出生产调用边。
- 源码：[`message.rs`](message.rs) 核对 33 个常量、结构字段和两个函数；[`mem_root.rs`](mem_root.rs) 核对 trait 读取方法、`Send + Sync` 约束及默认互斥锁实现；[`lib.rs`](lib.rs) 核对公开模块和独立测试模块。
- crate 配置：[`Cargo.toml`](Cargo.toml) 核对 crate 名、入口、Go 包映射和依赖边界。
- Go 对照：[`message.go`](message.go) 核对文案、日志字段和数据库错误；[`engine_mgr.go`](engine_mgr.go) 与 [`engine.go`](engine.go) 核对 Go 生产调用点。
- 测试：[`message_test.rs`](message_test.rs) 验证全部常量，以及配额 1,024、当前用量 256 时 engine/writer 快照的消息、job/index ID 和用量字段。
- 仓库搜索：`rg` 仅发现 Rust 构造函数在 `message_test.rs` 被调用，未发现其他 Rust 常量消费者；这支持“Rust 生产接线未检出”的限定结论。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前仅执行固定十一章节的结构验证，并人工复查上述结论均指向源码、索引、配置或测试证据。
