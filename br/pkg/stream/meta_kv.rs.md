# `br/pkg/stream/meta_kv.rs`

## 文件定位

`meta_kv.rs` 属于 Cargo 包 `astersql-br-pkg-stream`（`br/pkg/stream/Cargo.toml`），由 `br/pkg/stream/lib.rs` 通过 `#[path = "meta_kv.rs"] pub mod meta_kv` 纳入 crate，并由 `pub use meta_kv::*` 扁平再导出。它位于 BR 流式备份/恢复的元数据处理层：一端理解 TiDB 事务 meta key，另一端理解 TiKV write-CF value，使上层能够在不破坏事务编码的前提下重写数据库、表、时间戳和内联元数据。

本文件是 `br/pkg/stream/meta_kv.go` 的 Rust 对照实现，不是独立存储引擎，也不负责扫描备份文件、选择映射规则或反序列化 DB/Table JSON。生产调用者主要是 `br/pkg/stream/rewrite_meta_rawkv.rs`、`br/pkg/stream/table_mapping.rs` 和 `br/pkg/stream/search.rs`；底层字节、meta key 与错误类型当前由同 crate 的 `stubs.rs` 提供。

## 核心职责

文件包含两组相互独立但共同服务于流恢复的编解码能力。

1. `RawMetaKey`、`ParseTxnMetaKeyFrom`、`ParseDBIDFromTableKey` 和 `RawMetaKey::EncodeMetaKey` 在事务 key 与 `{Key, Field, Ts}` 三元组之间往返。上层据此识别 DB/表级 meta key，替换 ID 或提交时间戳，再生成合法的事务 key。
2. `WriteType`、`WriteTypeFrom` 与 `RawWriteCFValue` 解析、检查并重新编码 TiKV write-CF value。上层据此区分 Put/Delete/Rollback/Lock，读取或替换内联 short value，并把 Lightning 物理导入来源位写入 `txnSource`。

文件只解释编码容器，不决定某条记录是否应该恢复。过滤、ID 映射、JSON/protobuf 内容改写及 DefaultCF/WriteCF 配对逻辑均由调用方完成。

## 主要符号

- `pub struct RawMetaKey { Key, Field, Ts }`：事务 meta key 的可修改视图。三个字段均公开，另提供 `UpdateKey`、`UpdateField`、`UpdateTS` 以保持 Go API 形态。
- `pub fn ParseTxnMetaKeyFrom(txnKey: &[u8]) -> Result<RawMetaKey, Error>`：先用 `codec::DecodeBytes` 分离编码后的 raw meta key 与尾部时间戳，再用 `tablecodec::DecodeMetaKey` 拆出业务 `Key`/`Field`，最后用 `codec::DecodeUintDesc` 读取提交时间戳。
- `pub fn ParseDBIDFromTableKey(key: Vec<u8>) -> Result<i64, Error>`：复用完整事务 key 解析，再对 `RawMetaKey.Key` 调用 `meta::ParseDBKey`。它适用于 DB ID 位于 `Key` 的表级 key；DB 列表项的 DB ID 位于 `Field`，不能混用。
- `RawMetaKey::EncodeMetaKey(&self) -> kv::Key`：按 `tablecodec::EncodeMetaKey`、`codec::EncodeBytes`、`codec::EncodeUintDesc` 的顺序重建字节布局，与解析顺序互逆。
- `pub type WriteType = u8` 及 `WriteTypeLock/WriteTypeRollback/WriteTypeDelete/WriteTypePut`：分别对应字节 `L/R/D/P`。
- `pub fn WriteTypeFrom(t: u8) -> Result<WriteType, Error>`：限制 write type 为上述四种；其他值返回带 `berrors::ErrInvalidArgument` 注解的错误。
- `pub struct RawWriteCFValue`：保存 write type、`startTs`、short value、overlapped rollback、GC fence、last-change 信息和事务来源。字段私有，外部通过方法读取或定向修改。
- `RawWriteCFValue::ParseFrom(&mut self, data: &[u8])`：解析固定头和按 flag 串联的可选后缀。flag 为 `v`（short value）、`R`（overlapped rollback）、`F`（GC fence）、`l`（last change）和 `S`（txn source）。
- `IsRollback`、`IsDelete`、`IsPut`、`HasShortValue`、`GetShortValue`、`GetStartTs`、`GetWriteType`：供恢复分支判断和取值。
- `UpdateShortValue`：替换 write-CF 内联值；`MarkPhysicalImportTxnSource`：将 `LightningPhysicalImportTxnSource` 位 OR 入现有 `txnSource`，不会覆盖其他来源位。
- `EncodeTo(&self) -> Vec<u8>`：按 type、startTs、short value、overlapped rollback、GC fence、last change、txn source 的固定顺序重新编码。

本文件没有 trait、异步函数、条件编译项或模块级可变状态。

## 执行流程

事务 meta key 的典型恢复流程如下。

1. `rewrite_meta_rawkv.rs` 或 `table_mapping.rs` 调用 `ParseTxnMetaKeyFrom`。
2. `DecodeBytes` 解出 raw meta key；`DecodeMetaKey` 将其拆为 `Key` 和 `Field`；尾部 8 字节降序时间戳由 `DecodeUintDesc` 还原。
3. 调用方根据 `meta::IsDBkey`、表字段类型及映射表决定是否保留，并通过公开字段或 update 方法替换 DB ID、table ID、field 或 `Ts`。
4. `EncodeMetaKey` 把修改后的三元组恢复为可供后续 RawKV 恢复使用的 key。`rewrite_meta_rawkv.rs` 中 DefaultCF 通常保留原 `Ts`，WriteCF 路径会换成 `SchemasReplace::RewriteTS`。

write-CF value 的典型流程如下。

1. `RawWriteCFValue::default()` 创建空视图，调用 `ParseFrom`。输入首先必须至少 9 字节，首字节经 `WriteTypeFrom` 校验，其余从 `data[1..]` 解出 `startTs`。
2. 循环按 flag 消费后缀：`v` 读取一字节长度和 short value；`R` 只置位；`F` 读取 8 字节无符号整数；`l` 依次读取 8 字节 `lastChangeTs` 与 uvarint `versionsToLastChange`；`S` 读取 uvarint `txnSource`。
3. `rewrite_meta_rawkv.rs::rewriteValue` 对 Rollback 原样分类退出；对其他 WriteCF 先调用 `MarkPhysicalImportTxnSource`。Delete 直接回编，Put 若有 short value 则改写内容，否则只保留元信息并回编。
4. `EncodeTo` 以规范顺序输出已设置字段，使已知布局的解析—修改—编码稳定往返。

`table_mapping.rs` 的 DB/表映射收集路径也解析 write-CF：Delete/Rollback 会清理按 `startTs` 暂存的 DefaultCF 值，Put 则借助 `GetStartTs` 将 write 记录与先前值配对。`search.rs` 在合并 DefaultCF 与 WriteCF 搜索结果时使用同一解析器识别 write 记录。

## 数据与状态

`RawMetaKey` 自有三个 `Vec<u8>/u64` 值，解析时复制 key 和 field；`EncodeMetaKey` 只读取自身，不改变状态。事务 key 的逻辑布局是 `EncodeBytes(EncodeMetaKey(Key, Field)) + EncodeUintDesc(Ts)`，其中降序时间戳保证相同业务 key 的较新版本在字节排序中靠前。

`RawWriteCFValue` 是有状态的可变解析结果：

- 定长/必需部分为 `t` 与 `startTs`；可选部分由 flag 是否出现决定。
- `shortValue` 非空才会由 `HasShortValue` 视为内联值，也只有非空值会被 `EncodeTo` 输出。因此“存在但长度为 0”的 short-value flag 无法与“没有 short value”区分。
- `hasOverlappedRollback` 与 `hasGCFence` 显式记录布尔存在性，允许 GC fence 数值为 0 时仍被重新编码。
- last-change 没有单独的存在位；仅当 `lastChangeTs > 0 || versionsToLastChange > 0` 时编码。
- `txnSource` 是位图；`MarkPhysicalImportTxnSource` 使用 OR，当前 `stubs.rs` 中物理导入标记为 `1 << 16`。

`ParseFrom` 不会在解析开始时重置 `self`。同一实例若被多次复用，后一次输入缺少的可选字段可能保留前一次状态；当前调用点通常以 `RawWriteCFValue::default()` 新建实例后解析。扩展或复用时必须先重置为默认值，或明确实现覆盖语义。

## 依赖与调用关系

crate 边界由 `br/pkg/stream/Cargo.toml` 确认：这是 `astersql-br-pkg-stream` library，`lib.rs` 公开 `meta_kv` 并扁平再导出。`meta_kv.rs` 的直接 Rust 依赖均来自 `crate::stubs`：

- `codec`：memcomparable bytes、定长 uint、降序 uint 与 uvarint 编解码。
- `tablecodec`：TiDB meta hash key 的 `Key/Field` 编解码。
- `meta`：DB key 识别与 ID 解析。
- `kv::Key`：编码 key 的返回类型。
- `Error`/`berrors`：错误包装和 `ErrInvalidArgument` 分类。
- `LightningPhysicalImportTxnSource`：事务来源位。

RustCodeGraph 对 `ParseTxnMetaKeyFrom` 给出的 Rust 调用者包括 `ParseDBIDFromTableKey`、`ParseMetaKvAndUpdateIdMapping`、`RewriteMetaKvEntry`、`rewriteEntryForDB`、`rewriteEntryForTable`、`rewriteKeyForDB`、`rewriteKeyForTable` 及相关独立测试。图中 `EncodeTo` 的直接调用者包括 `rewrite_meta_rawkv_test.rs` 与 `parity_test.rs`；生产调用在 `rewrite_meta_rawkv.rs::rewriteValue` 中通过解析、标记、可选 short-value 改写后回编。`RawWriteCFValue::ParseFrom` 还由 `table_mapping.rs` 和 `search.rs` 使用。

下游调用链分别是：`ParseTxnMetaKeyFrom -> codec::DecodeBytes -> tablecodec::DecodeMetaKey -> codec::DecodeUintDesc`，以及 `RawWriteCFValue::ParseFrom -> WriteTypeFrom/codec::DecodeUvarint/codec::DecodeUint`。编码链使用对应的 `Encode*` 函数。

## 错误处理与边界

- meta key 的三个解码步骤以及 `ParseDBIDFromTableKey` 的 DB key 解析均以 `?` 返回 `Error`，不做容错重写；输入必须同时满足外层 bytes 编码、内层 meta hash key 格式和尾部降序时间戳格式。
- write-CF 输入长度小于 9 立即报 `ErrInvalidArgument`。首字节不是 `L/R/D/P` 时，`WriteTypeFrom` 返回同类参数错误。
- short value 在读取长度前要求至少两个字节，并再次检查 `2 + vlen` 的完整载荷；独立 Rust/Go 测试覆盖缺长度、载荷不足、声明 255 但不足，以及合法 255 字节值。
- `F` 与 `l` 中的定长 uint 必须有 8 字节；`l` 的 versions 和 `S` 的 txn source 必须是完整 uvarint。实现把这些底层错误统一转换成带字段语义的 `ErrInvalidArgument`。
- 遇到未知 flag 时 `ParseFrom` 返回成功并停止读取。未消费的未知后缀没有保存在结构体中，随后 `EncodeTo` 会丢弃它；这提供的是“停止误读”的有限前向兼容，并非未知字段的无损往返。
- `EncodeTo` 把 `shortValue.len()` 转成 `u8`，没有显式拒绝超过 255 字节的值。调用 `UpdateShortValue` 时应维持 TiKV short-value 最大 255 字节的不变量，否则长度会截断且生成不可可靠解析的编码。
- 解析器接受 flag 的任意出现顺序和重复出现；重复的带值字段以后一次为准，布尔字段保持 true，而 `EncodeTo` 总是规范化为固定顺序。
- `ParseFrom` 出错前可能已经修改部分字段，因此失败后的实例不能当作完整有效值继续使用。

## 并发与资源生命周期

该文件不创建线程、异步任务、锁、通道、事务或外部 I/O。所有解析和编码都在调用线程同步完成；返回的 `Vec<u8>` 拥有其数据，不借用输入。`RawWriteCFValue::GetShortValue` 返回克隆，调用方改动返回值不会影响原对象，必须通过 `UpdateShortValue` 写回。

类型本身没有内部同步，也没有全局共享状态。只读借用方法可按 Rust 类型规则并发读取；可变解析或更新需要调用方持有独占 `&mut self`。生命周期边界是一次 key/value 的解析—改写—编码，推荐每条 write-CF 记录使用新的 `RawWriteCFValue::default()`，避免跨记录残留状态。内存和临时缓冲均由 Rust 所有权在作用域结束时释放。

## 与 Go 版本的对应关系

`br/pkg/stream/meta_kv.rs` 与 `br/pkg/stream/meta_kv.go` 的公开符号、字段含义、flag 字节、解析顺序和编码顺序逐项对应。Rust 的 `Result<T, Error>` 对应 Go 的 `(T, error)`；Rust `Vec<u8>` 替代 `[]byte`；Go 返回 `*RawMetaKey`，Rust 返回拥有所有权的 `RawMetaKey`。Rust 方法保留 Go 风格命名，以便迁移期对照。

行为一致点包括：四种 write type、9 字节最小检查、short-value 两阶段边界检查、未知 flag 停止循环、last-change 的输出条件、txn source 位 OR，以及 `EncodeMetaKey` 的三段编码。`br/pkg/stream/meta_kv_test.rs` 与 `meta_kv_test.go` 具有相同的九组核心用例：DB/table key 往返、write type、无/有 short value、short-value 溢出保护、rollback、delete 和 GC fence。

需要注意的 Rust 表达差异：Go 测试可直接检查同包私有字段，Rust 测试通过公开 getter 和 `EncodeTo` 往返间接确认部分状态；Rust `GetShortValue` 克隆数据，而 Go 返回底层 slice；Rust 底层依赖目前指向 `stubs.rs` 中的本地移植实现，而非 Cargo.toml 中的独立 TiDB/TiKV codec crate。因此修改 stub 编码语义也会直接改变本文件行为，必须与 Go codec 布局共同核对。

## 扩展指南

- 新增事务 meta key 变换时，优先在调用方解析一次，修改 `RawMetaKey` 后调用 `EncodeMetaKey`；不要手工拼接 encoded bytes。务必区分 DB 列表项 ID 在 `Field` 与表级 key 的 DB ID 在 `Key`。
- 新增 write-CF flag 时，需要同步修改 `RawWriteCFValue` 状态、`ParseFrom` 分支和 `EncodeTo` 的规范顺序，并核对 TiKV `txn_types::Write` 与 Go `meta_kv.go`。同时扩展独立的 `br/pkg/stream/meta_kv_test.rs`，不得把测试写入生产文件。
- 若要求真正无损地转发未来未知 flag，现有 `break` 设计不足；需要显式保存未解析尾部并定义它与已知字段重新编码的顺序，且先确认 Go/TiKV 兼容契约。
- 若允许外部传入 short value，应在 `UpdateShortValue` 或更上层入口增加不超过 255 字节的约束，并为 256 字节边界补回归测试；不能依赖 `as u8` 截断。
- 若要复用解析对象，应在 `ParseFrom` 开始处清空全部字段或新增返回新值的构造式 API，并为“先解析全字段、再解析简短记录”添加状态残留测试。
- 改动 key 编码需同步检查 `rewrite_meta_rawkv.rs`、`table_mapping.rs`；改动 write-CF 语义还需检查 `search.rs`。兼容风险主要是字节级不可逆、错误分类变化和未知后缀丢失，性能风险主要来自额外克隆或重复编解码。

## 验证依据

- 目标源码：`br/pkg/stream/meta_kv.rs`，覆盖全部 287 行以及其中的公开类型、函数、方法、常量和私有 flag。
- crate 与模块入口：`br/pkg/stream/Cargo.toml`、`br/pkg/stream/lib.rs`；确认 library 名称、模块挂载、扁平再导出和独立测试挂载方式。
- Go 对照：`br/pkg/stream/meta_kv.go`；逐项核对 `RawMetaKey`、`RawWriteCFValue`、write type、flag、错误分支与编码顺序。
- 独立测试：`br/pkg/stream/meta_kv_test.rs`、`br/pkg/stream/meta_kv_test.go`、`br/pkg/stream/parity_test.rs`、`br/pkg/stream/rewrite_meta_rawkv_test.rs`；确认往返、边界错误、类型谓词、物理导入来源位及 key 重写语义。
- 直接调用证据：`br/pkg/stream/rewrite_meta_rawkv.rs`、`br/pkg/stream/table_mapping.rs`、`br/pkg/stream/search.rs`。
- 底层布局证据：`br/pkg/stream/stubs.rs` 中 `LightningPhysicalImportTxnSource`、`codec::{Encode/DecodeBytes, Encode/DecodeUint, Encode/DecodeUintDesc, Encode/DecodeUvarint}`、`tablecodec::{EncodeMetaKey, DecodeMetaKey}` 和 `meta::ParseDBKey`。
- RustCodeGraph：运行 `status`、`files --filter br/pkg/stream`、目标文件 `node --file ... --offset 1 --limit 500`、`query ParseTxnMetaKeyFrom`，并以 `explore` 核对 `ParseTxnMetaKeyFrom`、`RawWriteCFValue::ParseFrom`、`EncodeTo` 的调用关系。索引显示目标文件被 11 个文件使用，并列出上述生产与测试调用者。
- 本任务仅新增说明文档；按计划不运行 Cargo。交付前另执行任务指定的 11 章节结构检查、路径存在性检查和 Git diff 检查。
