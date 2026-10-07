# `pkg/lightning/common/dupdetect.rs`

## 文件定位

本文件属于 `astersql-lightning-common` crate（`pkg/lightning/common/Cargo.toml`），实现导入链路共用的“有序 KV 流相邻重复检测”核心。`pkg/lightning/common/lib.rs` 以 `mod dupdetect` 装配模块并通过 `pub use dupdetect::*` 公开再导出其 API。它位于排序存储迭代器与重复记录库之间：上游提供当前位置已经有效、且按编码键排序的 `KVIter`，检测器借助 `KeyAdapter::Decode` 恢复业务键；下游通过 `WriteBatch` 持久化一个重复组中的全部原始 KV。

RustCodeGraph 已索引本文件的 30 个符号，并定位 `NewDupDetector`、`DupDetector` 与 `DupDetectLogger`；仓库搜索未发现独立 Rust 测试之外的 Rust 生产调用者。因此当前可确认的是：Rust API 已由 crate 根导出并有独立单元测试，但不能据此声称 Rust 生产导入主链已经接线。对应 Go 生产入口是 `pkg/ingestor/ingestctrl/iterator.go:newDupDetectIter`。

## 核心职责

- `DupDetector::Init` 从迭代器当前项建立比较基线，保存解码后的业务键、原始编码键和值。
- `DupDetector::Next` 向前扫描：跳过并处理与基线业务键相同的项，直到返回下一条不同的业务键，或到达流尾。
- 默认模式把重复组中的第一条和后续所有重复条目写入重复库；`DupDetectOpt::ReportErrOnDup` 模式则在首次重复处立即返回 `ErrFoundDuplicateKeys`，不记录该重复组。
- `record` 统一执行诊断回调、写批追加和约 4 MiB 阈值刷盘；`Close` 在关闭写批前提交尾批。
- `MemoryWriteBatch`、`NoopDupDetectLogger` 提供轻量默认实现/适配点，但核心逻辑并不创建真实磁盘数据库，也不负责构造或排序输入流。

## 主要符号

- `maxDuplicateBatchSize: usize = 4 << 20`：按原始键长度和值长度累计的刷批阈值；不是条数限制，也不计业务解码键和容器开销。
- `KVIter`：检测器所需的最小只读迭代器接口，包含 `Next`、`Key`、`Value`。调用 `Init` 时当前位置必须已经有效；本接口没有错误查询方法，底层迭代错误须由具体适配层另行暴露。
- `WriteBatch: Send`：重复库写批抽象。`Set` 追加 KV，`Commit(true)` 请求同步持久化，`Reset` 清空批状态，`Close` 释放资源。
- `MemoryWriteBatch`：以内存 `Pending`/`Committed` 向量模拟写批；同步提交会增加 `SyncCommits`，关闭只设置 `Closed`。它公开状态，适合检查行为，不等价于 Pebble 的事务/持久性实现。
- `DupDetectLogger: Send + Sync`：每次记录重复 KV 前接收解码键、值和原始键；`NoopDupDetectLogger` 明确丢弃诊断。
- `DupDetectOpt { ReportErrOnDup }`：控制“记录并继续”与“立即报错”两种冲突策略，默认值为 `false`。
- `DupDetector`：持有共享的 `Arc<dyn KeyAdapter>`、独占的 `Box<dyn WriteBatch>`、共享日志器、当前/下一键缓存及批大小。其字段私有，状态只能经构造函数和方法推进。
- `NewDupDetector`：注入适配器、写批、日志器和策略；不读取迭代器，也不隐式提交。
- `Init`、`Next`、`record`、`flush`、`Close`：分别对应基线初始化、扫描、单条记录、同步提交和终止清理。虽然 `record` 与 `flush` 当前为公开方法，它们依赖检测器内部计数不变量，外部直接调用需要承担维护该状态的责任。

## 执行流程

1. 调用方先把底层有序迭代器定位到第一条有效记录，再调用 `Init`。该方法用 `KeyAdapter::Decode(vec![], iter.Key())` 得到业务键，同时复制原始键和值，并把业务键和值的副本返回给调用方。
2. 此后调用方重复调用 `Next`。每轮先执行 `iter.Next()`，复制新项的编码键和值，并解码到 `nextKey`。
3. 若 `nextKey != curKey`，说明遇到下一组：交换当前键与下一键缓存，更新当前原始键和值，立即返回 `Some((key, value))`。因此每次调用最多向外暴露一条新的唯一业务键。
4. 若业务键相同且 `ReportErrOnDup` 为真，返回由原当前键和值生成的 `CommonError`；本次项不写批、日志器不被调用，扫描也立即停止。
5. 默认模式中，局部变量 `first` 保证一次 `Next` 扫描同一重复组时只补记一次组首旧项；随后当前重复项逐条经 `record` 写入。这样两条重复记录会落两条，多于两条时组首仍只落一次。
6. `record` 先调用 `DuplicateDetected`，再以原始编码键和值调用 `WriteBatch::Set`，累计 `raw.len() + value.len()`；达到阈值后调用 `flush`。
7. `flush` 执行 `Commit(true)`，成功后才 `Reset` 并把计数归零。流结束时 `Next` 返回 `Ok(None)`；调用方仍必须调用 `Close`，以提交未达阈值的尾批并关闭写批。

该算法依赖相同业务键在解码后相邻。若输入未按与 `KeyAdapter` 一致的编码顺序组织，同键项被其他键隔开时不会被识别为同一重复组。

## 数据与状态

`curKey` 是当前组的解码业务键，`curRawKey`/`curVal` 保存该组当前代表项的原始编码键和值；`nextKey` 是扫描时的解码暂存。遇到新组时只交换两个解码键向量，而原始键和值被新项覆盖。所有从迭代器获得的切片都会复制进自有 `Vec<u8>`，所以不会跨 `Next` 持有底层迭代器的借用。

`curBatchSize` 只统计成功执行 `Set` 后的原始键和值字节数。阈值允许单条记录使累计值越界，然后立即整体同步提交；没有把一条 KV 拆分到多个批次。`MemoryWriteBatch::Commit` 将全部 `Pending` 移入 `Committed`，`Reset` 清空尚存待提交项，便于测试观察与 `WriteBatch` 协议一致的状态转换。

关键不变量是：调用 `Next` 前必须成功 `Init`；输入按解码业务键分组；检测器独占写批并在生命周期末显式 `Close`。源码没有保存“已初始化/已关闭”标志，错误顺序不会被主动拒绝。

## 依赖与调用关系

- crate 内下游：`KeyAdapter::Decode`（`pkg/lightning/common/key_adapter.rs`）决定哪些编码键属于同一业务键；`CommonError` 和 `ErrFoundDuplicateKeys`（`pkg/lightning/common/errors.rs`）承载解码、写批、提交、关闭和重复错误。
- 标准库依赖：`Arc` 让不可变键适配器与日志器可共享；`Box` 让检测器独占可变写批。该文件不直接依赖 `pkg/lightning/common/Cargo.toml` 中的外部 crate，Cargo 清单只声明本 common crate 的边界及 `astersql-lightning-log`、`libc` 等 crate 级依赖。
- Rust 上游：`pkg/lightning/common/lib.rs` 再导出全部公开符号；仓库范围 Rust 搜索仅发现 `pkg/lightning/common/dupdetect_test.rs` 直接构造检测器，未发现生产 Rust 调用边。因此外部 Cargo 依赖该 common crate，并不等于调用了此模块。
- Go 上游主链：`pkg/ingestor/ingestctrl/iterator.go:newDupDetectIter` 创建 Pebble 迭代器和批，调用 `common.NewDupDetector`；其 `dupDetectIter` 在 `First`/`Next` 中驱动检测器，在 `Close` 中先关闭检测器再关闭底层迭代器。`pkg/executor/importer/import.go` 默认记录重复，`pkg/ddl/ingest/config.go` 配置遇重复报错。
- Go 测试证据：`pkg/ingestor/ingestctrl/iterator_test.go` 使用 `DupDetectKeyAdapter`、随机写入且由 Pebble 排序的输入，验证唯一键迭代结果和重复库包含完整重复组，并提供重复检测 benchmark。

## 错误处理与边界

`Init` 和 `Next` 原样传播 `KeyAdapter::Decode` 的 `CommonError`；`record` 传播 `WriteBatch::Set` 或阈值触发的提交错误；`flush` 在 `Commit(true)` 失败时不会执行 `Reset` 或清零计数。`Close` 若刷盘失败会提前返回，不再调用写批的 `Close`；若刷盘成功，则返回底层关闭错误。这与 Go 文件的控制顺序一致。

`ReportErrOnDup` 分支使用当前组首的解码键和值生成 `ErrFoundDuplicateKeys`，并在日志/写批之前短路。独立测试只断言错误文本含键，并验证批与日志均为空；错误 ID 和值内容可由 `pkg/lightning/common/errors.rs:ErrFoundDuplicateKeys` 进一步核验。

需要调用方处理的边界包括：空迭代器不能安全调用 `Init`；单条或无重复输入不会在 `Next` 内写批；即使批为空，`Close` 仍会调用一次同步 `Commit(true)`；日志回调发生在 `Set` 之前，所以若写入失败，诊断可能已产生而数据未落库。当前接口没有自动 `Drop` 清理，也没有把底层迭代器错误并入返回值。

## 并发与资源生命周期

`DupDetector` 的扫描与批状态都需要 `&mut self`，设计上由单个任务顺序驱动；源码没有锁，也没有声明 `DupDetector` 可在多个线程并发使用。`KeyAdapter` 要求 `Send + Sync`，日志器要求 `Send + Sync`，允许安全共享这些只读/回调依赖；写批只要求 `Send`，被检测器独占并串行修改。

资源所有权从 `NewDupDetector` 转移到检测器：`Box<dyn WriteBatch>` 在生命周期内唯一持有。正常终止路径必须显式调用 `Close`，其顺序是同步提交、重置、关闭。若调用方忘记关闭，未达阈值的待提交重复项没有由本文件保证持久化；若中途方法返回错误，本文件也不自动回滚或关闭，清理由上层决定。

## 与 Go 版本的对应关系

Rust 的 `DupDetector`、`NewDupDetector`、`KVIter`、`Init`、`Next`、`record`、`flush`、`Close` 和 `DupDetectOpt` 逐一对应 `pkg/lightning/common/dupdetect.go`。两者共享 4 MiB 阈值、相邻解码键比较、重复组首只记录一次、同步提交后重置，以及“报错模式不记录”的语义。

实现差异主要来自抽象与所有权：Go 直接依赖 `*pebble.Batch` 和 `log.Logger`，Rust 以 `WriteBatch`、`DupDetectLogger` trait 注入；Go 复用切片容量，Rust 当前多次构造/克隆 `Vec<u8>`；Go 返回 `(key, value, ok, err)`，Rust 返回 `Result<Option<(Vec<u8>, Vec<u8>)>, CommonError>`。Go 的重复错误显式复制键值以隔离后续缓冲复用；Rust 的 `ErrFoundDuplicateKeys` 接收切片并立即构造自有错误消息，达到相同生命周期隔离目的。

Go 生产代码已经用 Pebble 完成接线；Rust 当前文件只提供抽象实现、内存批和测试替身，仓库搜索没有找到真实磁盘 `WriteBatch` 适配器或生产构造调用。因而它是行为移植完成但生产接线尚未由当前证据确认的模块，不能把 `MemoryWriteBatch` 描述为生产重复库。

## 扩展指南

- 接入真实 Rust 导入链时，应在调用侧实现独立的磁盘 `WriteBatch` 适配器和 `KVIter`，保持 `Commit(true) -> Reset -> Close` 协议，并在调用侧合并底层迭代错误；不要把存储实现塞入本文件。
- 新增冲突策略时，修改 `DupDetectOpt` 和 `DupDetector::Next` 的重复分支，并同步 `pkg/lightning/common/dupdetect_test.rs`；还应与 `pkg/executor/importer/import.go`、`pkg/ddl/ingest/config.go` 的 Go 策略语义对齐。
- 调整批量策略时，集中修改 `maxDuplicateBatchSize`/`record`，补充独立测试覆盖“恰小于、恰等于、单条超阈值、提交失败不重置”。性能风险主要是同步提交频率和 Rust 当前的键值复制次数。
- 改变键相等定义时，应优先修改/扩展 `KeyAdapter` 并同步 `pkg/lightning/common/key_adapter_test.rs`，同时验证排序顺序仍保证相同解码键连续；否则本文件的相邻扫描算法不成立。
- 增加自动清理或错误恢复时，要明确提交失败后是否允许重试、关闭失败优先级及日志副作用，避免用 `Drop` 隐式执行可能失败的同步 I/O。
- 测试逻辑继续放在独立的 `pkg/lightning/common/dupdetect_test.rs`，不要内嵌进生产源文件；真实接线的集成行为应在拥有迭代器/存储生命周期的 ingestor 测试中覆盖。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/lightning/common` 确认目标源、独立测试、Go 对照和 crate 入口均已索引；`node --file pkg/lightning/common/dupdetect.rs --offset 1 --limit 500` 读取完整 168 行和 30 个符号；`query NewDupDetector --kind function --json` 同时定位 Rust/Go 构造函数；`query DupDetectLogger --kind trait --json` 定位 Rust trait。精确 `callers/callees` 查询未返回可用边，故按技能规则用仓库搜索核对未覆盖调用关系。
- Rust 源与 crate：`pkg/lightning/common/dupdetect.rs`、`pkg/lightning/common/key_adapter.rs`、`pkg/lightning/common/errors.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/Cargo.toml`。
- Rust 独立测试：`pkg/lightning/common/dupdetect_test.rs` 验证两条重复记录均被记录、日志调用两次、关闭时同步提交并关闭，以及报错模式不写批也不记录日志。
- Go 对照与生产入口：`pkg/lightning/common/dupdetect.go`、`pkg/ingestor/ingestctrl/iterator.go`、`pkg/executor/importer/import.go`、`pkg/ddl/ingest/config.go`。
- Go 测试：`pkg/ingestor/ingestctrl/iterator_test.go` 验证排序输入的唯一键输出、完整重复组落库、解码适配和 benchmark。`pkg/lightning/common` 下没有直接对应的 `dupdetect_test.go`。
- 人工复核结论：文件存在是为了把排序迭代、业务键解码、冲突策略与重复库批写解耦；安全扩展必须维持输入排序、初始化先行、完整重复组记录、提交成功后才重置以及显式关闭这些契约。按任务要求未运行 Cargo。
