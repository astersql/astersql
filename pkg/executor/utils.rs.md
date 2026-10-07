# `pkg/executor/utils.rs`

## 文件定位

`pkg/executor/utils.rs` 是 `astersql-executor` crate 的执行器通用工具模块，由
`pkg/executor/lib.rs` 中的 `pub mod utils` 公开。它不是某一种 SQL 算子的实现，而是把
Go 版 `pkg/executor/utils.go` 中若干相互独立、但由执行层共享的辅助能力迁移到 Rust：
集合字符串编解码、DML 子 `Chunk` 初始容量估算、已知总行数的分批遍历、用户认证密码
编码，以及轻量按需线程池。

当前生产接线并不均匀。RustCodeGraph 的符号边显示
`pkg/session/runtime/control.rs` 的 `ConcreteSession::execute_create_user` 会调用
`encodePasswordWithPlugin`；其余主要 API 当前没有查到 Rust 生产调用者，直接证据主要来自
`pkg/executor/utils_test.rs`。因此本文件既包含已进入建用户主链的认证逻辑，也保留了尚待其他
执行器迁入后复用的 Go 对齐工具，不能把后者描述为已经覆盖全部执行路径。

## 核心职责

1. `SetFromString`、`setToString`、`addToSet`、`deleteFromSet` 在保持元素顺序的前提下，
   处理用逗号持久化的“集合”字符串；唯一性由调用方与 `addToSet` 共同维护，而不是类型系统
   强制保证。
2. `newDMLChildChunk` 与 `estimateDMLChildChunkInitCap` 根据字段估算宽度，在执行器最大
   `Chunk` 行数、调用方初始容量上限及 256 KiB 目标字节数之间取界，避免宽行 DML 一开始
   就做过大的内存分配。
3. `batchRetrieverHelper::nextBatch` 把 `[0, total_rows)` 切成半开区间，供已知结果总数的
   内存表取回器逐批填充结果。
4. `encodePasswordWithPlugin`、`encodedPassword` 为 `CREATE USER` 等账户语义选择扩展认证
   插件或内置认证算法，并用返回值中的布尔量区分“可持久化”和“非法哈希格式”。
5. `workerPool` 按排队情况和可选扩容谓词创建 OS 线程，串行或并行消费闭包；空的
   `workerTask` 槽位通过全局池复用。
6. `growWorkerStack16K` 保留 Go 版扩栈兼容/测试钩子的意图；Rust 实现通过不可内联函数、
   栈上数组和 `black_box` 阻止优化掉该分配。

## 主要符号

- `AnalyzeProgressTest: ()`：与 Go 的测试注入变量同名的公开占位值。当前文件内没有行为，
  RustCodeGraph 也未显示生产调用边。
- `SetFromString(&str) -> Option<Vec<String>>`：空字符串返回 `None`，非空字符串按逗号
  原样切分；不去重、不去空格，`"a,,b"` 会保留空元素。
- `setToString(&[String]) -> String`：用逗号连接；空切片得到空字符串。
- `addToSet(Vec<String>, String) -> Vec<String>`：只在向量中不存在完全相等的字符串时
  追加，保留原顺序。
- `deleteFromSet(Vec<String>, &str) -> Vec<String>`：删除第一个相等元素；若输入本来含重复
  项，后续重复项仍存在。
- `dmlChildChunkTargetBytes: usize`：容量估算的 256 KiB 目标，不是硬内存上限。
- `DMLChildChunkExecutor`：把 `max_chunk_size` 和 `new_chunk_with_capacity` 两项执行器能力
  抽象出来，使工具不依赖具体执行器类型。
- `newDMLChildChunk(...) -> Chunk`：读取执行器最大行容量，调用估算函数，再由执行器构造
  `Chunk`。
- `estimateDMLChildChunkInitCap(...) -> usize`：容量计算核心，结果受两个行数上限约束；
  对非零行宽至少返回 1。
- `batchRetrieverHelper`：保存 `retrieved`、下一起点 `retrieved_idx`、`batch_size` 与
  `total_rows`；字段公开，但类型名仍遵循 Go 命名以方便移植对照。
- `batchRetrieverHelper::nextBatch`：计算下一半开区间、执行调用方闭包并推进状态。
- `encodePasswordWithPlugin(...) -> (String, bool)`：扩展插件优先的总入口。
- `encodedPassword(...) -> (String, bool)`：内置插件的明文编码和已有哈希格式校验。
- `WorkerFn`、`workerTask`、`WorkerPoolState`：分别表示可在线程间移动的一次性闭包、可复用
  任务槽，以及受互斥锁保护的 FIFO 队列/worker 计数。
- `global_task_pool()`：通过 `OnceLock<Mutex<Vec<workerTask>>>` 延迟创建进程级空闲槽池。
- `workerPool::new`、`workerPool::submit`、`run_worker`：创建池、提交任务和执行 worker 循环。
- `growWorkerStack16K()`：不可内联的栈增长辅助函数。

## 执行流程

集合路径是纯值变换：字符串经 `SetFromString` 形成有序向量，调用方用 `addToSet` 或
`deleteFromSet` 修改，最后由 `setToString` 恢复逗号格式。测试
`executor_set_and_batch_helpers_preserve_order_and_stop_after_error` 验证了去重追加、首项删除、
顺序保持及空字符串行为。

DML `Chunk` 路径由 `newDMLChildChunk` 发起。它先读取 `executor.max_chunk_size()`，再让
`estimateDMLChildChunkInitCap` 累加每个 `FieldType` 的 `chunk::EstimateTypeWidth`。任一行数
上限为零时返回 `chunk::ZeroCapacity`；估算行宽为零时只取两个行数上限的较小值；否则计算
`256 KiB / row_width`，与两个上限再取最小值，并把最终值下限钳为 1。随后调用
`executor.new_chunk_with_capacity(fields, initial_capacity, maximum_chunk_size)` 完成分配。

分批路径每次先检查 `retrieved_idx >= total_rows` 并结束；否则计算
`start = retrieved_idx` 与 `end = min(start + batch_size, total_rows)`。回调成功后将下一起点推进
到 `end`，恰好达到总行数时置 `retrieved = true`；回调失败则不推进索引，但立即置完成标志并
原样返回错误，后续调用不会再次执行回调。

认证路径先看 `UserSpec.AuthOpt`。没有认证选项视为合法空密码。传入扩展 `AuthPlugin` 时，
明文调用 `GenerateAuthString`，已有哈希调用 `ValidateAuthString`；没有扩展插件时转入
`encodedPassword`。后者优先采用用户显式插件，否则采用默认插件：明文的 caching SHA-2
和 TiDB SM3 走 `NewHashPassword`，socket 插件不保存密码，其他插件走原生 MySQL
`EncodePassword`；已有哈希则按插件决定直接透传、长度/前缀校验或拒绝。生产调用者
`execute_create_user` 在布尔值为 `false` 时生成 `invalid password hash` 会话错误，合法密码才
进入用户记录持久化流程。

线程池路径中，`submit` 先从 `global_task_pool` 取空槽或新建槽，装入 `FnOnce` 后在池锁内
入 FIFO 队列。若当前没有 worker、没有扩容谓词，或谓词认可当前 `(workers, queued_tasks)`，
则先增加计数，再创建线程运行 `run_worker`。worker 每轮只在锁内弹出任务，锁外执行闭包，
执行后清空闭包并把槽位归还全局池；发现队列为空时在锁内减少 worker 数并退出。

## 数据与状态

集合和密码函数没有共享可变状态，输入字符串/向量的所有权或借用关系决定其生命周期。
密码 API 的 `(String, bool)` 不使用 `Result`：字符串是拟持久化值，布尔量才是格式是否合法的
判据；合法空密码表现为 `(String::new(), true)`，必须与非法输入返回的空字符串配合
`false` 区分。

`batchRetrieverHelper` 是调用方持有的显式游标状态机。`retrieved_idx` 表示已经成功处理到的
排他下标，只有回调成功才推进；`retrieved` 是终止闩。构造时必须保证状态字段彼此一致，类型
本身不提供构造器或校验。

`workerPool` 的每个实例拥有一个 `Arc<Mutex<WorkerPoolState>>`，所以提交者和所有工作线程
共享同一队列与 worker 计数。`workers` 统计已决定启动且尚未因空队列退出的线程；队列长度
临时充当 `tasks` 参数。所有池实例还共享一个进程级 `Vec<workerTask>` 空闲槽池，但闭包在
归还前已由 `Option::take` 清除，不会把捕获对象跨任务保留。

## 依赖与调用关系

crate 边界由 `pkg/executor/Cargo.toml` 确认，包名为 `astersql-executor`、入口为 `lib.rs`，且
直接声明了本文件使用的 `astersql-extension`、`astersql-parser-ast`、
`astersql-parser-auth`、`astersql-parser-mysql`、`astersql-types` 和
`astersql-util-chunk` 路径依赖。本文件没有受 `nextgen` feature 控制的条件编译项。

主要下游关系如下：

- `newDMLChildChunk → estimateDMLChildChunkInitCap → chunk::EstimateTypeWidth`，随后回调
  `DMLChildChunkExecutor::new_chunk_with_capacity`。
- `encodePasswordWithPlugin → AuthPlugin::{GenerateAuthString, ValidateAuthString}`，或
  `encodePasswordWithPlugin → encodedPassword → {NewHashPassword, EncodePassword}`。
- `workerPool::submit → global_task_pool`，并在线程入口调用 `run_worker → global_task_pool`。

RustCodeGraph 的上游边确认
`ConcreteSession::execute_create_user → astersql_executor::utils::encodePasswordWithPlugin`；
`pkg/executor/utils_test.rs` 则直接导入并覆盖集合、容量估算、认证和 worker 池符号。搜索当前
Rust 源码未发现 `newDMLChildChunk`、`batchRetrieverHelper`、`workerPool` 或
`growWorkerStack16K` 的其他生产调用者，因此它们目前应视为已实现、已做局部测试但尚未普遍
接线的移植能力。

## 错误处理与边界

- 集合格式没有转义规则，元素自身若含逗号就无法往返；函数也不验证重复项或空元素。
- `estimateDMLChildChunkInitCap` 使用 `usize` 累加字段宽度，依赖
  `EstimateTypeWidth` 给出合理值；256 KiB 仅控制初始行数。超宽行令整数除法得到零时，下限
  仍保证一行。
- `batchRetrieverHelper` 要求 `batch_size > 0`（除非 `total_rows == 0`）。当总行数为正而批
  大小为零时，回调会反复收到空区间且索引不前进；当前类型没有防御性检查。`retrieved_idx`
  大于等于总行数会直接结束，不回调。
- 扩展插件对象被视为已正确注册；需要的 `GenerateAuthString` 或 `ValidateAuthString` 为
  `None` 时会 `expect` panic，而不是返回非法标志。LDAP 哈希（实际为 DN）按 Go 行为直接
  透传，代码没有验证 DN 格式。未知插件的非空已有哈希被拒绝，但未知插件的明文仍退回
  `EncodePassword`，这是现有分支事实。
- worker 池使用 `expect` 获取互斥锁，锁中毒会 panic。任务闭包若 panic，当前 worker 会展开
  栈，来不及归还任务槽，也不会减少 `workers`；池没有捕获 panic、关闭、等待完成、队列上限
  或任务错误通道。扩展或复用时不能把它当作具备完整生命周期管理的通用线程池。

## 并发与资源生命周期

提交路径持锁范围只覆盖入队、计算扩容决策和递增 worker 数，线程创建发生在解锁后；执行路径
也在弹出任务后立即解锁，所以任务可以递归调用同一池的 `submit`，测试
`worker_pool_honors_single_worker_spawn_policy` 正是这样验证单 worker 的 FIFO 顺序。双 worker
测试用通道握手验证：谓词允许 `workers < 2` 时，第二个排队任务能在第一个任务仍阻塞时启动。

扩容谓词接收当前 worker 数和入队后的队列长度。谓词为 `None` 时，每次提交都会决定启动一个
worker；这不等于永久线程，而是按需线程，队列清空后立即退出。由于没有 `JoinHandle`，调用方
只能用任务自身的通道、锁或其他同步原语观察完成；丢弃 `workerPool` 不会主动取消已经持有
`Arc` 的工作线程。

全局任务槽池的 `Mutex<Vec<_>>` 只复用装闭包的 `Box` 外壳；闭包捕获资源在调用完成并被
`take` 后释放。`growWorkerStack16K` 的数组只存活于函数栈帧，`black_box` 仅确保分配不会被
优化消除，不产生跨线程或堆资源。

## 与 Go 版本的对应关系

Rust 文件逐段对应 `pkg/executor/utils.go`，名称刻意保留 Go 风格。集合、256 KiB 容量目标、
半开批区间、认证插件分支及 worker 扩容谓词的语义基本一致；
`pkg/executor/utils_test.go` 的 `TestBatchRetrieverHelper`、`TestEncodePasswordWithPlugin`、
`TestWorkerPool` 和 `TestEncodedPassword` 是原始行为依据，Rust 的独立
`pkg/executor/utils_test.rs` 对这些场景做了直接移植。

实现层面的主要差异是：Go 空集合返回 `nil []string`，Rust 用 `Option<Vec<String>>` 表达；
Go 的 DML helper 直接接受 `exec.Executor`，Rust 以 `DMLChildChunkExecutor` trait 隔离所需能力；
Go 的链表队列和 `sync.Pool` 在 Rust 中分别变为 `VecDeque` 与
`OnceLock<Mutex<Vec<workerTask>>>`；goroutine 变成未保留句柄的 `std::thread`。Rust worker
计数不再单独存储 tasks 计数，而是读取队列长度。

存在测试覆盖差异：Go `TestWorkerPool` 还包含“允许一个任务积压后再扩容”的
`TolerateOnePendingTask` 场景，当前 Rust 测试覆盖单 worker、立即第二 worker 和启动超时路径，
未逐字复制该第三个谓词场景。Go 容量估算通过实际执行器接口使用，当前 Rust 测试只直接验证
估算函数，尚未为 `newDMLChildChunk` 提供 mock trait 实现。以上属于迁移覆盖现状，不应据此
宣称生产接线已完全对齐。

## 扩展指南

- 新增集合格式能力时，应先决定是否仍允许逗号出现在值中；若改变编码协议，应同时检查所有
  持久化字段的兼容性，并在独立的 `pkg/executor/utils_test.rs` 增加往返、重复项和空元素用例。
- 接入新的 DML 执行器时，实现 `DMLChildChunkExecutor`，复用 `newDMLChildChunk`，不要在调用
  点复制容量公式。应为 trait 构造路径增加独立测试，并关注字段宽度求和溢出、超宽行最低一行
  以及初始分配对内存峰值的影响。
- 使用 `batchRetrieverHelper` 时必须保证正的 `batch_size`，并让回调只在成功后产生可提交的
  外部副作用；若要支持重试，现有“首次错误即永久完成”的状态语义需要有意识地修改并同步
  Go 行为与测试。
- 新增认证插件规则时，修改入口应是 `encodePasswordWithPlugin`/`encodedPassword`，并同步
  parser/mysql 的长度常量或哈希实现、`pkg/session/runtime/control.rs` 的用户创建错误路径，以及
  Rust/Go 两侧认证测试。要明确区分合法空密码与非法格式，避免只检查返回字符串。
- 扩展 `workerPool` 前先确定是否需要 panic 隔离、关闭/等待、背压或最大线程数；这些都不是
  现有契约。测试必须留在 `pkg/executor/utils_test.rs`，使用通道等确定性同步，避免依赖 sleep
  推断调度顺序。
- 本文件已经带有 `// Copyright 2026 AsterSQL.`，任何后续源码修改都应保留该标记及原有
  PingCAP Apache License 注释。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点与 1,848,419 条边；通过
  `node --file pkg/executor/utils.rs` 阅读了 323 行完整源文件。
- RustCodeGraph 符号节点：`SetFromString`、`newDMLChildChunk`、
  `estimateDMLChildChunkInitCap`、`encodePasswordWithPlugin`、`encodedPassword`、
  `run_worker`、`growWorkerStack16K`。关键边包括
  `newDMLChildChunk → estimateDMLChildChunkInitCap`、`submit → run_worker`、
  `run_worker → global_task_pool`，以及
  `execute_create_user → encodePasswordWithPlugin → encodedPassword`。
- 读取的 crate/装配文件：`pkg/executor/Cargo.toml`、`pkg/executor/lib.rs`；前者确认直接依赖，
  后者确认公开模块和 `#[cfg(test)] #[path = "utils_test.rs"]` 的独立测试挂载。
- 读取的 Rust 生产/测试证据：`pkg/executor/utils.rs`、
  `pkg/session/runtime/control.rs` 中的 `execute_create_user`、`pkg/executor/utils_test.rs`。
- 读取的 Go 对照证据：`pkg/executor/utils.go` 与 `pkg/executor/utils_test.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务规定的 `rg -c` 命令验证固定的十一个
  二级章节，并人工复核未把无生产调用边的符号写成已接线能力。
