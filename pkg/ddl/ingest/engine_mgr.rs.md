# `pkg/ddl/ingest/engine_mgr.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate；crate 入口 `pkg/ddl/ingest/lib.rs` 以公开模块 `engine_mgr` 导出它。它没有定义名为“manager”的结构体，而是在 `BackendContext` 之上提供两个薄的公开生命周期入口：`register_engines` 和 `finish_and_unregister_engines`，并定义注销选项的位布局。底层引擎集合、配额与重复值收集实际由 `pkg/ddl/ingest/backend.rs` 的 `BackendContext` 管理，引擎关闭及数据清理由 `pkg/ddl/ingest/engine.rs` 的 `EngineInfo` 完成。

在完整 DDL 架构中，这组概念对应本地磁盘 ingest 加索引回填的“创建索引写入引擎—写入—结束并注销”边界。Go 生产实现通过 `BackendCtx.Register` / `BackendCtx.FinishAndUnregisterEngines` 接入 `pkg/ddl/backfilling.go` 和 `pkg/ddl/backfilling_read_index.go`。但当前 Rust 调用图和全仓搜索都表明，本文件的两个函数只被 `pkg/ddl/ingest/engine_mgr_test.rs` 调用，尚未接入 Rust 生产回填主链；不能把 Go 的生产接线视为 Rust 已支持的行为。

## 核心职责

- `register_engines` 保留 engine-manager 层的公开入口，将索引 ID、唯一性标记和每个 writer 的内存额度原样转交给 `BackendContext::register`。
- `finish_and_unregister_engines` 统一执行结束顺序：空集合快速成功；关闭全部引擎；按位标志选择是否清理本地缓冲以及是否检查重复值；成功后清空注册表。
- `UnregisterOpt`、`OPT_CLOSE_ENGINES`、`OPT_CLEAN_DATA`、`OPT_CHECK_DUP` 保持与 Go `UnregisterOpt` 及 `Opt*` 常量相同的 `1/2/4` 位布局。
- 本文件只负责编排，不创建真实 Lightning engine、不刷新或导入数据、不推进 checkpoint，也不持有自己的锁或状态。相关能力位于 `BackendContext`、`EngineInfo` 或当前尚未迁入的 Go 生产实现中。

## 主要符号

- `pub type UnregisterOpt = u8`：位标志容器。调用者可用按位或组合选项；未知高位当前会被忽略。
- `OPT_CLOSE_ENGINES = 1 << 0`：与 Go 的关闭位保持 ABI/语义布局一致。需要特别注意，Rust 函数目前无条件调用 `Engine::close`，并不根据这一位决定是否关闭；该常量主要表达调用意图并保持 Go 对齐。
- `OPT_CLEAN_DATA = 1 << 1`：控制传给 `Engine::close(cleanup)` 的 `cleanup` 参数。置位时 `EngineInfo::close` 清空缓冲行；未置位时行数据仍可从已有 `Arc<EngineInfo>` 观察到，但引擎已关闭。
- `OPT_CHECK_DUP = 1 << 2`：关闭后触发远端重复值收集的开关。`BackendContext::collect_remote_duplicate_rows` 对非唯一索引直接返回空，对唯一索引按 value 计数并返回出现次数大于一的值。
- `register_engines(&mut BackendContext, &[i64], &[bool], i64) -> Result<Vec<Arc<EngineInfo>>, String>`：公开注册门面，完整结果和错误来自 `BackendContext::register`。
- `finish_and_unregister_engines(&mut BackendContext, UnregisterOpt) -> Result<(), String>`：公开注销门面。它直接访问 `backend.engines`，因此与 `BackendContext::finish_and_unregister` 是两条不同的结束路径，不能假定两者顺序相同。

## 执行流程

`register_engines` 的流程只有一步：调用 `backend.register(index_ids, unique, writer_memory)` 并原样返回。下游 `BackendContext::register` 会先拒绝已关闭的上下文和长度不一致的两个切片；若请求中的全部索引已存在，则按请求顺序返回已有引擎；若只部分存在则报错；完全未注册时按两个切片的 zip 顺序创建 `EngineInfo`、写入 `BTreeMap` 并返回 `Arc` 列表。`writer_memory` 最终由 `EngineInfo::new` 截断为不小于零的值。

`finish_and_unregister_engines` 按以下顺序运行：

1. 若 `backend.engines` 为空，直接返回 `Ok(())`，从而支持成功后的重复调用。
2. 计算 `cleanup = options & OPT_CLEAN_DATA != 0`，遍历所有已注册引擎并调用 `engine.close(cleanup)`。关闭动作无条件发生，`OPT_CLOSE_ENGINES` 是否置位不改变这一点。
3. 若设置 `OPT_CHECK_DUP`，先复制当前 `BTreeMap` 的所有 index ID，再逐个调用 `collect_remote_duplicate_rows`。发现第一个非空结果即返回 `Err("duplicate rows for index ...")`。
4. 只有所有检查都成功后才执行 `backend.engines.clear()` 并返回成功。

因此，重复错误发生在“引擎已关闭但注册表尚未清空”的中间状态；这是 `engine_mgr_test.rs::duplicate_error_keeps_go_closed_engines_registered` 明确锁定的契约。

## 数据与状态

本文件本身没有结构体或静态可变状态。所有状态都通过可变借用的 `BackendContext` 进入：核心是 `BTreeMap<i64, Arc<EngineInfo>>` 类型的 `engines`。映射键是索引 ID，值可被注册表和调用者共同持有，所以清空映射并不保证 `EngineInfo` 立即销毁；已有 `Arc` 仍可读取保留的数据，但关闭后不能再创建 writer 或写入。

注册结果保持 `index_ids` 的输入顺序，而后续注销和查重遍历采用 `BTreeMap` 键序。唯一性、writer 内存额度和行缓冲属于 `EngineInfo`。`cleanup=false` 时 `close` 仅设置 closed 并释放标签内存，保留 rows；`cleanup=true` 还会清空 rows。重复检测统计的是引擎中相同 value 的出现次数，不是相同 key；相同 key 在 `BTreeMap` 写入时会覆盖，因此当前内存版模型的“重复行”语义由 `BackendContext::collect_remote_duplicate_rows` 决定。

## 依赖与调用关系

直接 Rust 依赖只有 crate 内的 `backend::BackendContext`、`engine::{Engine, EngineInfo}` 以及标准库 `Arc`。导入 `Engine` trait 是为了让 `Arc<EngineInfo>` 可调用 trait 方法 `close`。`pkg/ddl/ingest/Cargo.toml` 将该目录定义为包 `astersql-ddl-ingest`，库入口为 `lib.rs`；本文件没有直接使用 Cargo 中的 `fs2`、`fail` 或 `astersql-util-dbterror`，也没有条件编译项。

已验证的 Rust 调用边为：

- `register_engines -> BackendContext::register -> EngineInfo::new`；
- `finish_and_unregister_engines -> EngineInfo::close`；
- 可选的 `finish_and_unregister_engines -> BackendContext::collect_remote_duplicate_rows -> EngineInfo::rows/unique`；
- 上游仅有 `pkg/ddl/ingest/engine_mgr_test.rs` 中三个行为测试；当前没有 Rust 生产调用者。

Go 对照的生产链是 `pkg/ddl/backfilling.go` 或 `pkg/ddl/backfilling_read_index.go` 创建 `BackendCtx`，调用 `Register` 后把 engines 交给 add-index ingest pipeline；失败路径使用关闭或清理选项，成功路径使用 `OptCleanData | OptCheckDup`。这条链只证明 Go 设计位置，不能证明 Rust 门面已接线。

## 错误处理与边界

`register_engines` 不包装错误，可能返回 `"backend closed"`、`"index/unique length mismatch"` 或部分重叠引擎组的数量不匹配错误。创建当前内存版 `EngineInfo` 本身不返回错误，因此这里没有 Go `OpenEngine` 失败后的部分创建回滚路径。

注销空集合成功。`Engine::close` 没有返回值，互斥锁中毒会因其内部 `unwrap` panic，而不是转换为 `Result`。查重阶段若 index 不存在会传播 `"engine not found"`；发现重复值则在本文件构造含 index ID 的字符串错误。任何查重错误都会提前返回，保留 `backend.engines` 映射，但其中已遍历的全部引擎在查重前就已关闭。再次调用仍会再次关闭并再次查重；它只在一次成功清空后具备无副作用的幂等快速路径。

当前实现不会读取 `OPT_CLOSE_ENGINES`，也不会拒绝未知位组合。`OPT_CHECK_DUP` 会遍历所有引擎，但非唯一引擎由下游函数过滤。查重发生在 close 之后且没有先 flush；这与 `BackendContext::finish_and_unregister` 的“先 flush、再查重、最后 close”顺序不同，扩展时不得混用两者的错误状态假设。

## 并发与资源生命周期

两个公开函数都要求 `&mut BackendContext`，Rust 借用规则会阻止同一上下文通过安全引用并发执行注册或注销；本文件没有额外的 `Mutex`。这不同于 Go `litBackendCtx` 的 `unregisterMu`，后者专门串行化并发的 `FinishAndUnregisterEngines` 调用。

`Arc<EngineInfo>` 允许 worker 或调用者在注册表之外共享引擎。`EngineInfo` 用 `Mutex<EngineState>` 串行化 writer 创建、写入与 close：close 与 writer 创建在同一锁下检查状态，关闭后新 writer 会收到 `"engine closed"`；已经存在的 writer 后续写入也会因 closed 状态失败。`close` 释放引擎标签对应的内存额度，writer 自身的额度在 `WriterContext::drop` 时释放。`cleanup` 决定是否同时丢弃 rows。

成功注销会清空 `BackendContext.engines` 对 `Arc` 的所有权，但外部克隆可延长对象寿命。重复错误则有意保留映射，以复现 Go 的“先关闭、出错时不重建 map”状态。当前函数不管理 backend、checkpoint、磁盘目录或异步任务的生命周期。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/ingest/engine_mgr.go`。位布局完全对应：Rust 的 `u8` 常量值为 `1/2/4`，Go 的 `int` + `iota` 产生相同值；`engine_mgr_test.rs::unregister_options_match_go_bit_layout` 对此做了断言。两版都支持整组已有引擎的重复注册、拒绝部分重叠，都在注销时先关闭全部引擎，并且只在结束成功后清空注册表；因查重失败返回时，引擎保持关闭且仍被注册表引用。

已确认的差异与迁移限制如下：

- Go `Register` 接收 `table.Table`，创建真实 Lightning engine、检查 `MemRoot`、配置 import TS、记录 table，并在部分打开失败时清理已打开引擎；Rust 门面改为 `writer_memory`，下游只创建内存版 `EngineInfo`，没有真实 backend 打开、table 或日志行为。
- Go 注销由 `unregisterMu` 保证并发安全；Rust 依靠 `&mut BackendContext`，没有对应运行时锁。
- Go 只对 `ei.unique` 调用带 table 的远端重复检查，并保留具体存储/冲突错误和 failpoint；Rust 对所有 ID 调用简化的内存行值统计，由下游对非唯一索引返回空，错误统一为 `String`，没有该 failpoint。
- Go map 的遍历次序不保证稳定；Rust `BTreeMap` 使关闭和首个重复错误按 index ID 排序。
- `OPT_CLOSE_ENGINES` / `OptCloseEngines` 在两版结束函数中都不控制是否关闭：关闭始终执行。它表示调用场景，与 clean/check-dup 位组合兼容。
- 当前 Go 接口已接入本地 ingest 回填生产链；当前 Rust 本文件只有测试调用，尚无等价生产入口证据。

## 扩展指南

若新增注销行为，应分配不冲突的位、在 `finish_and_unregister_engines` 中明确执行顺序，并同步 `engine_mgr_test.rs` 的位布局和组合行为测试；同时核对 Go `UnregisterOpt`，避免两端协议漂移。不要仅根据常量名让 `OPT_CLOSE_ENGINES` 开始控制 close，除非同时审计所有现有调用者和 Go 兼容契约，因为当前任何选项组合都会关闭。

若调整注册语义，优先修改真正拥有规则的 `BackendContext::register`，并同步独立测试 `pkg/ddl/ingest/backend_test.rs` 与门面测试；需保持输入顺序、整组幂等、部分重叠报错、unique 切片长度校验及 writer 配额语义。Rust 测试必须继续放在独立的 `*_test.rs` 文件，不能内嵌到生产源文件。

若要把该门面接入 Rust DDL 主链，应先定位 Rust add-index pipeline 的实际入口，建立与 Go `BackendCtx` 调用点等价的显式调用边，并补充失败、成功清理、重复冲突及并发生命周期测试。真实 Lightning backend、table 元数据、checkpoint/failpoint 和错误类型的缺口不应通过在本文件内堆叠简化桩来掩盖。

修改结束顺序时需要重点评估：查重失败后映射是否保留、引擎何时关闭、数据是否仍可诊断、writer 是否可能存活、是否需要 flush，以及 `BackendContext::finish_and_unregister` 与本门面是否应合并。二者当前流程不同，合并必须以 Go 行为和现有回归测试为依据。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/ingest` 确认目标、Go 对照和测试均已索引。
- RustCodeGraph `node --file pkg/ddl/ingest/engine_mgr.rs --offset 1 --limit 260`：核对目标文件 74 行源码、公开符号、分支和唯一直接使用文件。
- RustCodeGraph 精确查询及调用图探索：确认 `register_engines`、`finish_and_unregister_engines` 的上游仅为 `pkg/ddl/ingest/engine_mgr_test.rs`，并核对到 `BackendContext::register`、`EngineInfo::close`、`BackendContext::collect_remote_duplicate_rows` 的下游关系。自然语言 explore 对常见符号产生了大量歧义结果，因此结论只采用其中目标路径的精确条目及后续文件级 node 证据。
- RustCodeGraph 文件节点：`pkg/ddl/ingest/backend.rs` 与 `pkg/ddl/ingest/engine.rs`，用于核对注册规则、重复值算法、`BTreeMap`/`Arc`/`Mutex` 状态、close 与 writer 生命周期。
- 直接读取：`pkg/ddl/ingest/Cargo.toml`、`pkg/ddl/ingest/lib.rs`、`pkg/ddl/ingest/engine_mgr.go`、`pkg/ddl/ingest/backend.go`、`pkg/ddl/ingest/engine_mgr_test.rs`，以及 Go 生产调用片段 `pkg/ddl/backfilling.go`、`pkg/ddl/backfilling_read_index.go`。
- 全仓 `rg`：确认 Rust 非测试生产文件没有调用两个门面函数；确认 Go 调用点的选项组合。`pkg/ddl/ingest` 没有直接覆盖 `Register` / `FinishAndUnregisterEngines` 的 Go `*_test.go`，当前细粒度回归证据来自独立 Rust 测试文件。
- 按任务要求仅做文档事实与结构验证，不运行 Cargo；最终结构命令及退出状态在任务交付时报告。
