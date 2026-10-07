# [`pkg/ddl/index_merge_tmp.rs`](./index_merge_tmp.rs)

## 文件定位

该文件属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 crate 入口，`pkg/ddl/lib.rs` 通过 `pub mod index_merge_tmp` 公开本模块，并仅在 `cfg(test)` 下装配独立测试 `index_merge_tmp_test.rs`。它对应在线 ADD INDEX 的 fast-reorg / txn-merge 阶段：历史数据回填期间，前台 DML 的增量先进入临时索引，回填完成后再把临时键空间中的有效操作重放到正式索引。

当前 Rust 文件只移植了合并所需的纯数据结构与纯内存算法，没有移植 Go `mergeIndexWorker` 的存储事务、重试、指标和 worker 接线。RustCodeGraph 对本文件公开函数的 callers 查询未发现生产调用者，仓库文本检索也只找到模块自身与 `pkg/ddl/index_merge_tmp_test.rs` 的引用。因此它目前是**已公开、已做局部单元验证，但尚未接入 Rust DDL 生产链路**的基础模块，不能据此认定 Rust 已完整执行临时索引合并。

## 核心职责

本文件提供五组能力：

1. `TemporaryIndexRecord` 把一条临时索引操作规范化为临时键、正式键、编码值、行句柄及 `distinct/delete/skip` 状态。
2. `check_temporary_index_key` 和 `batch_check_temporary_unique_key` 在唯一索引合并前判断重复键、删除匹配以及是否跳过写入。
3. `TemporaryIndexBuffers` 保存一批记录及与其位置一一对应的正式键、临时键，允许复用已分配容量。
4. `fetch_temporary_index_values` 在内存记录集合上模拟 `[start, end)` 有序范围扫描，产生下一批起点和扫描统计。
5. `find_index_info_by_decoding_key` 与 `decode_temporary_index_handle` 提供 tablecodec 风格键头中的索引 ID 和记录句柄解码。

它不负责创建 DDL job、推进 `BackfillState`、持久化 reorg checkpoint、开启事务、写删 KV、处理 owner 转移或清理临时索引。这些完整职责在 Go 版本的 `mergeIndexWorker.BackfillData`、`fetchTempIndexVals` 及其上游 add-index reorg 流程中实现。

## 主要符号

- `TemporaryIndexRecord`：一条待重放操作。`temporary_key` 是清理目标，`original_key` 是写入或删除目标；`value` 是编码索引值；`handle` 用于防止删除另一个仍存活行的唯一索引项；`distinct` 表示唯一且可判重的值，`delete` 选择删除分支，`skip` 由冲突检查写回。
- `OriginalIndexValue`：正式索引当前值的测试友好表示。`distinct` 区分唯一值和含 NULL 的非 distinct 值；`row_exists: Result<bool, MergeError>` 把 Go 版额外查行的结果或错误预先注入纯函数。构造器 `distinct`、`non_distinct`、`distinct_lookup_error` 分别覆盖三种输入。
- `MergeError`：局部错误枚举。`DuplicateKey`、`InvalidKey`、`Decode` 在当前实现中有返回路径；`NoProgress` 已定义但本文件内没有产生它的代码路径。
- `check_temporary_index_key(record, original)`：单记录冲突决策，返回的 `bool` 不是“成功”，而是“删除记录的 handle 与正式唯一索引值精确匹配，调用方应从批内快照删除该键”。
- `batch_check_temporary_unique_key(records, original, unique)`：唯一索引批量判重；非唯一索引直接成功。它克隆 `original` 为批内可变视图，用来检测同批新增项之间的冲突。
- `TemporaryIndexBuffers::{with_capacity, reset, add}`：维护 `records/original_keys/temporary_keys` 三个同序向量；`add` 是保持位置不变量的唯一追加入口。
- `TemporaryIndexResult`：返回 `next_key`、`scan_count`、`add_count` 和 `done`。当前扫描函数固定令 `add_count = 0`，因为真正写入统计应由后续事务合并阶段填写。
- `fetch_temporary_index_values`：验证区间和批大小，筛选并排序记录，最多装入 `batch_size` 条，然后用“最后键追加 `0x00`”构造下一起点。
- `find_index_info_by_decoding_key`：校验 `t{8-byte table id}_i{8-byte index id}` 头部，反转符号位并应用 `0x0000_ffff_ffff_ffff` 掩码，再确认 ID 位于候选列表。
- `decode_temporary_index_handle`：非删除记录必须从 `value_handle` 取得句柄；删除记录从键末尾八字节取得句柄。

## 执行流程

在当前 Rust 纯函数层，典型批次流程如下：

1. 调用者准备待合并的 `TemporaryIndexRecord` 集合、扫描区间和可复用 `TemporaryIndexBuffers`。
2. `fetch_temporary_index_values` 拒绝 `start >= end` 或 `batch_size == 0`，清空缓冲区，筛出临时键位于 `[start, end)` 的记录，按临时键排序后取最多一批。
3. 扫描结果非空时，`next_key` 为本批最后一个临时键追加零字节；空批时为 `end` 追加零字节，且只有空批才设置 `done = true`。因此不足一整批并不直接表示完成，调用方仍需再做一次空扫描。
4. 若目标索引唯一，调用 `batch_check_temporary_unique_key`。它先克隆正式索引快照，然后按输入顺序处理记录；遇到已有正式键时调用 `check_temporary_index_key`，否则把 distinct 插入值加入批内视图，以便后续同键记录参与判重。
5. `check_temporary_index_key` 对插入记录：distinct 且编码值不同则返回 `DuplicateKey`；否则将 `skip` 设为 true，因为正式键已经存在。对删除记录：正式值非 distinct 时允许继续重放；handle 不同则按 `row_exists` 决定是否跳过删除；handle 相同则返回 true，让批处理移除该键，避免后续记录受到已被本批删除项的假冲突。
6. 本文件到此结束。真正依据 `skip/delete` 对正式键执行 Set/Delete、删除临时键、提交事务并更新计数的阶段尚未在 Rust 中接线；Go `mergeIndexWorker.BackfillData` 是该后半段的直接对照。

解码辅助函数是上述流程的前置步骤：`find_index_info_by_decoding_key` 从任务起始键确定待处理索引，`decode_temporary_index_handle` 为冲突判定补齐 handle。当前 Rust 扫描函数接受已经解码好的记录，没有像 Go `fetchTempIndexVals` 那样从原始临时索引值完成过滤、覆盖折叠和 handle 解码。

## 数据与状态

核心状态是不持久化的批内状态：

- `TemporaryIndexRecord.skip` 初始应为 false，由唯一性检查原地更新。复用记录前必须明确重置，否则旧批次决定会泄漏到新流程。
- `TemporaryIndexBuffers` 的三个向量依赖相同下标对应同一条操作；直接分别修改公开字段可能破坏这一不变量，正常追加应经过 `add`。
- `batch_values` 是 `BTreeMap<Key, OriginalIndexValue>` 的克隆，只在单次调用内模拟正式索引快照及本批已观察到的插入/删除，不会写回调用者的 `original`。
- `next_key` 使用字典序后继的简化表示；`scan_count` 是装入缓冲区的记录数，区别于 Go 版本解码临时值后、过滤覆盖版本前累计的元素数；`add_count` 在本实现中始终为零。
- 文件没有 DDL job、schema version、`BackfillState` 或 reorg checkpoint 字段。它也没有为 `MergeError::NoProgress` 建立状态转换。

## 依赖与调用关系

直接 Rust 依赖非常小：`crate::backfilling::Key` 提供键类型（字节向量别名），标准库 `BTreeMap` 提供有序批内快照；本文件没有直接使用 `pkg/ddl/Cargo.toml` 中的外部 crate。crate 边界由 `pkg/ddl/lib.rs` 的公开模块声明建立。

RustCodeGraph 的符号查询确认六个主要公开函数位于本文件；对 `check_temporary_index_key`、`batch_check_temporary_unique_key`、`fetch_temporary_index_values`、`find_index_info_by_decoding_key`、`decode_temporary_index_handle` 的 callers 查询没有给出生产调用边。文本检索进一步确认：除内部互调外，调用仅存在于 `pkg/ddl/index_merge_tmp_test.rs`，而 `decode_temporary_index_handle` 当前连独立测试引用也没有。

完整应用中的概念上游是 add-index 的 fast-reorg / txn-merge 状态机：索引进入 merge 阶段后，由 worker 按任务键区间扫描临时索引。完整 Go 调用链的局部事实是 `mergeIndexWorker.BackfillData` 先调用 `findIndexInfoByDecodingKey`，在新事务内调用 `fetchTempIndexVals` 和 `batchCheckTemporaryUniqueKey`，然后按 `skip/delete` 写删正式索引并删除临时键。Rust 尚未形成这条生产调用边，所以这里只能作为未来接线位置，而不是现有 Rust 运行链。

## 错误处理与边界

- `check_temporary_index_key` 对唯一插入的同键异值返回 `MergeError::DuplicateKey`；同值或非 distinct 插入标记 `skip`。删除冲突查行的预注入错误通过 `?` 原样传播。
- `batch_check_temporary_unique_key` 对 `unique = false` 完全不修改记录；唯一批中任一错误会立即终止，之前记录的 `skip` 修改不会回滚，因此调用者若重试应重建或重置输入。
- `fetch_temporary_index_values` 以 `InvalidKey` 拒绝空/逆序区间和零批大小。输入无需预排序；`sort_by` 会保留相同临时键的原输入顺序，但本结构没有版本号或覆盖折叠规则，调用者仍不应把等键顺序当成完整的临时索引版本语义。
- `find_index_info_by_decoding_key` 对错误首字节、缺失 `_i`、键长不足以及索引 ID 不在白名单统一返回 `Decode`。它只解析键头，不验证 table ID，也不解码后缀。
- `decode_temporary_index_handle` 对非删除记录缺少 `value_handle` 返回 `Decode`；删除键不足八字节也返回 `Decode`。它按末八字节截取，不解释 handle 编码类型。
- `done` 仅在扫描结果为空时为 true，这与独立测试和 Go 的“再空扫一次确认结束”行为一致；调用者不能用 `scan_count < batch_size` 提前结束。
- 当前实现不会返回 `NoProgress`，也没有 Go 版的 retryable transaction error、动态缩小 batch、cancel/runnable 检查和具体错误包装。

## 并发与资源生命周期

本文件自身没有线程、异步任务、锁、channel、事务或外部资源。所有结构均由调用者拥有，函数同步执行；`TemporaryIndexBuffers` 通过 `reset` 清空长度但保留容量，适合由单个 worker 在连续批次间复用。由于方法接收 `&mut self`，同一缓冲区不能在安全 Rust 中被并发无同步写入。

这不代表完整临时索引合并没有并发和资源约束。Go `mergeIndexWorker.BackfillData` 把每批扫描、冲突检查、正式键写删和临时键清理放入一个新事务；遇到可重试冲突会检查 reorg 是否仍可运行、减半批大小、退避重试，并在退出时恢复原批大小。Rust 若接入生产链，必须在上层恢复这些事务原子性、取消/owner 切换、重试与 checkpoint 语义，不能把本文件的内存步骤拆成彼此独立提交。

## 与 Go 版本的对应关系

Rust 文件来自同路径 `pkg/ddl/index_merge_tmp.go` 的局部移植：

- `TemporaryIndexRecord` 对应 Go `temporaryIndexRecord`，但 Rust 额外直接保存 `temporary_key/original_key`；Go 的键分别位于 `tempIdxBuffers.tmpIdxKeys/originIdxKeys`。
- `OriginalIndexValue` 是 Rust 为隔离存储访问引入的适配结构。Go `checkTempIndexKey` 会用 tablecodec 解码正式值，并在 handle 不同时经事务读取行；Rust 由调用者提前提供 `distinct` 和 `row_exists`。
- `check_temporary_index_key` 对应 `checkTempIndexKey`，其插入冲突、skip 规则、删除 handle 匹配和存活行保护语义一致。
- `batch_check_temporary_unique_key` 对应 `batchCheckTemporaryUniqueKey`，但 Go 会对正式键执行事务 `BatchGet` 并把底层重复键转成带索引上下文的错误；Rust 接受现成 `BTreeMap`，只返回粗粒度 `MergeError`。
- `TemporaryIndexBuffers` / `TemporaryIndexResult` 对应 `tempIdxBuffers` / `tempIdxResult`。Rust 保留并行数组和容量复用思想，但扫描计数语义更简单。
- `fetch_temporary_index_values` 对应 `fetchTempIndexVals` 的范围和分页骨架。Go 从 snapshot 读取、解码多版本临时值、`FilterOverwritten`、过滤 merge/delete key version 并生成正式键；Rust 只对已构造记录做内存筛选排序，不能替代真实存储扫描。
- `find_index_info_by_decoding_key` 对应 `findIndexInfoByDecodingKey`，返回 ID 而不是 `IndexInfo`；两者都屏蔽临时索引高位并要求候选中存在该 ID。
- `decode_temporary_index_handle` 只覆盖 Go `decodeTempIndexHandleFromIndexKV` 的简化来源选择。Go 使用 `tablecodec.DecodeIndexHandle` 兼容索引列数和具体编码，Rust 删除分支固定截取末八字节，非删除分支直接使用传入值。

缺失于 Rust 的重要 Go 行为包括 `mergeIndexWorker` 构造与 worker trait、事务写删、临时键清理、可重试冲突退避、批大小自适应、metrics/failpoint/slow log，以及真实 tablecodec 临时值的版本过滤。扩展时应逐项移植并保持语义，不应把当前简化函数宣称为完整等价实现。

## 扩展指南

若只扩展局部判定规则，应优先修改 `check_temporary_index_key`，并在独立文件 `pkg/ddl/index_merge_tmp_test.rs` 增加插入/删除、distinct/NULL、handle 相同/不同、行存在/不存在及错误传播的矩阵；不要把测试内嵌回生产源文件。

若扩展批内唯一性规则，应维护“删除精确匹配后从 `batch_values` 移除”和“首次 distinct 插入写入批内视图”两个不变量，并验证输入中多个相同 `original_key` 的顺序组合。若封装 `TemporaryIndexBuffers`，建议逐步收紧三个向量的公开可变性，避免长度错位。

若接入真实 DDL worker，需要在本文件之外补齐：tablecodec 完整解码、snapshot 范围扫描、覆盖版本过滤、同一事务内的正式键写删和临时键删除、retry/cancel/owner-transfer 处理、持久化 reorg 进度、指标与 failpoint。接线后再为本模块建立真实生产 callers，并以 Go `mergeIndexWorker.BackfillData` 为行为基准；不能仅复用当前内存扫描来替代存储引擎顺序和 MVCC 快照。

兼容性风险集中在临时键编码及 index ID 掩码；正确性风险集中在唯一键冲突、错误删除仍存活行、批内多次修改顺序和事务原子性；性能风险集中在当前 `fetch_temporary_index_values` 每批全量过滤排序及 `batch_check_temporary_unique_key` 克隆整个正式值映射。生产接线前必须以范围迭代和批量读取替代这些测试友好实现。

## 验证依据

- 源码：`pkg/ddl/index_merge_tmp.rs`，逐项核对全部结构、构造器、方法、函数、错误分支与常量；文件无 trait、异步函数或条件编译项。
- crate 与模块：`pkg/ddl/Cargo.toml`（package `astersql-ddl`、`[lib] path = "lib.rs"`、Go package 元数据）和 `pkg/ddl/lib.rs`（`pub mod index_merge_tmp`、独立 `cfg(test)` 测试模块）。
- 包契约：`pkg/ddl/doc.go` 说明 DDL 的在线 schema 版本不变量；`docs/agents/ddl/README.md` 与 `docs/agents/ddl/06-add-index.md` 仅用作定位线索，并由源码核对了本文件涉及的临时索引 merge 阶段。
- RustCodeGraph：运行 `status` 确认索引包含 7032 个 Rust 文件；用 `node --file pkg/ddl/index_merge_tmp.rs` 读取完整 297 行；用 `query` 定位五个关键公开函数，并对其执行 callers/callees 查询。`check_temporary_index_key` 的内部调用边来自 `batch_check_temporary_unique_key`，未发现生产调用者。
- 调用检索：对 `pkg/ddl/**/*.rs` 检索模块名及关键函数，确认模块由 `lib.rs` 公开，外部使用仅在 `pkg/ddl/index_merge_tmp_test.rs`；`decode_temporary_index_handle` 当前无测试调用。
- Go 对照：`pkg/ddl/index_merge_tmp.go`，核对 `batchCheckTemporaryUniqueKey`、`checkTempIndexKey`、`mergeIndexWorker.BackfillData`、`findIndexInfoByDecodingKey`、`fetchTempIndexVals`、`decodeTempIndexHandleFromIndexKV` 及事务重试、写删和指标逻辑。
- 独立测试：`pkg/ddl/index_merge_tmp_test.rs` 覆盖插入命中/重复、删除 handle 命中/冲突、行存活/已删/查找错误、非 distinct 删除、批内重复、扫描排序与空扫完成、临时 index ID 掩码；尚未覆盖 `decode_temporary_index_handle`、非法扫描参数、非法键头和缓冲区位置不变量。仓库中的 Go `_test.go` 未检索到这些同名内部函数的直接测试引用。
- 本任务为纯文档分析，按任务约束未运行 Cargo；结构检查用于确认目标文件存在且恰有规定的十一个二级标题。
