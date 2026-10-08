# `pkg/statistics/cmsketch_util.rs`

## 文件定位

本文件属于 `astersql-statistics` crate（见 `pkg/statistics/Cargo.toml`），由 `pkg/statistics/lib.rs` 以 `mod cmsketch_util` 纳入并通过 `pub use cmsketch_util::*` 对外重导出。它位于统计信息的 TopN 编码值与类型化 `Datum` 之间：TopN 在 `TopNMeta::Encoded` 中保存扁平编码字节，而直方图合并、运行时统计构造等消费者需要按“索引值”或“列值”的语义恢复数据。文件本身不构建 CMSketch/TopN，也不维护统计计数。

## 核心职责

- `topNMetaToDatum` 将一条 `TopNMeta` 转成可比较、可写入类型化直方图边界的 `types::Datum`。索引 TopN 的键本来就是组合索引字节，因此直接返回字节；列 TopN 则解出单值并恢复列类型。
- `DecodeColumnTopNValue` 专用于列 TopN。它同样先解出一个值，但对字符串字段保留解码所得的原始比较字节，避免 `Unflatten` 改变用于排序/比较的字节表示；非字符串类型则恢复为字段自身的 Datum kind。
- 两个函数共同解决扁平存储表示与类型化统计表示的差异，例如 Duration、Timestamp、ENUM/SET/BIT 和浮点值在编码中可能不是最终列 Datum kind。依据为本文件注释、Go 对照 `pkg/statistics/cmsketch_util.go` 以及相关测试。

## 主要符号

- `pub fn topNMetaToDatum(value: &TopNMeta, field_type: &types::FieldType, is_index: bool, location: chrono_tz::Tz) -> Result<types::Datum, astersql_errors::SharedError>`：crate 对外重导出的通用 TopN 转换入口。`value.Count` 不参与解码；只读取并在必要时克隆 `value.Encoded`。
- `pub fn DecodeColumnTopNValue(encoded: &[u8], field_type: &types::FieldType, location: chrono_tz::Tz) -> Result<types::Datum, astersql_errors::SharedError>`：列 TopN 解码入口；以 `types_field::IsString(field_type.GetType())` 决定是否跳过反扁平化。
- `TopNMeta` 定义在 `pkg/statistics/cmsketch.rs`，包含 `Encoded: Vec<u8>` 与 `Count: u64`。本文件只消费编码值，不改变频次。
- 本文件没有常量、trait、impl、条件编译项或私有辅助函数；两个 API 均由 `pkg/statistics/lib.rs` 暴露。

## 执行流程

`topNMetaToDatum` 的流程如下：

1. 检查 `is_index`。为 `true` 时克隆 `TopNMeta::Encoded`，用 `types::NewBytesDatum` 包装并立即成功返回；此分支不调用解码器，任意索引字节均按原样保留。
2. 列值分支调用 `codec::DecodeOne(&value.Encoded)`，忽略返回的剩余切片并取得第一个 `Datum`；解码错误通过 `?` 立即返回。
3. 调用 `tablecodec::Unflatten(decoded, Box::new(field_type.clone()), Some(location))`，依据字段类型和时区恢复列语义，并直接返回其结果。

`DecodeColumnTopNValue` 的流程如下：

1. 调用 `codec::DecodeOne(encoded)`；失败时立即返回错误，因此字符串特判也不会掩盖非法编码。
2. 若 `field_type.GetType()` 属于字符串类型，直接返回解码出的 Datum，以保留比较字节。
3. 其他类型调用 `tablecodec::Unflatten`，传入克隆的字段类型与 `Some(location)`，恢复 Duration、Timestamp 等类型化值。

## 数据与状态

输入数据均以借用形式进入；函数不修改 `TopNMeta`、`FieldType` 或输入切片。索引分支会复制编码 `Vec<u8>`，列分支由 `codec::DecodeOne` 产生 Datum，并为 `Unflatten` 克隆一份 `FieldType` 放入 `Box`。`location` 按值传递，影响 Timestamp 等与时区有关的恢复结果；`pkg/statistics/go_merge_47_test.rs::go_merge_47_top_n_timestamp_respects_location` 验证 UTC 与 Asia/Tokyo 会得到不同显示值。

本模块没有全局变量、缓存或内部可变状态。`TopNMeta::Count` 是上层频次数据，在这里保持不变且不参与转换。`DecodeOne` 返回的未消费尾部目前被两个函数有意忽略，因此接口只解释首个编码 Datum，不负责验证输入是否恰好包含一个值。

## 依赖与调用关系

直接依赖如下（crate 声明见 `pkg/statistics/Cargo.toml`）：

- crate 内部 `TopNMeta`（`pkg/statistics/cmsketch.rs`）提供编码载体。
- `codec::DecodeOne` 负责从字节流解出首个 Datum；Cargo 包名为 `astersql-util-codec`。
- `tablecodec::Unflatten` 负责由扁平 Datum、字段类型和时区恢复列 Datum。
- `types::{Datum, FieldType}` 与构造器承载结果；Cargo 包名为 `astersql-types-datum`。
- `types_field::IsString` 判断 MySQL 字段类型是否为字符串；Cargo 包名为 `astersql-types-field`。
- `chrono_tz::Tz` 提供时区，`astersql_errors::SharedError` 统一承载下游错误。

RustCodeGraph 的 `explore/query` 结果显示，Rust 上游中 `topNMetaToDatum` 被 `pkg/statistics/histogram.rs::MergePartitionHist2GlobalHistWithLocation` 的 TopN 回填流程及 `pkg/statistics/merge_global.rs::decode` 调用；后者统一处理全局合并时的索引/列解码。`DecodeColumnTopNValue` 被 `pkg/statistics/handle/runtime_stats.rs::canonical_histogram` 使用，用于把运行时统计桶边界恢复成直方图 Datum。两者也分别被 `pkg/statistics/go_merge_47_test.rs` 覆盖。通过 `lib.rs` 的公开重导出，子 crate `handle/runtime_stats` 可使用 `astersql_statistics::DecodeColumnTopNValue`。

## 错误处理与边界

- 列分支的无效或截断编码由 `codec::DecodeOne` 返回 `SharedError`；`go_merge_47_decode_duration_and_preserve_string_comparison_bytes` 用 `[0xff]` 验证错误不会被字符串快路径吞掉。
- `tablecodec::Unflatten` 的字段类型不匹配、类型恢复或时区相关错误直接向上传播，本文件不包装错误，也不回退到扁平值。
- 索引分支不解析字节，因此不会因编码内容返回错误；专项测试以 `[0xff, 0x00]` 证明原样返回。这是索引键的契约，不代表字节已被验证为合法列编码。
- 字符串列在成功 `DecodeOne` 后跳过 `Unflatten`，保证包括 `0x00`、`0xff` 在内的比较字节不变；Go 测试 `TestDecodeColumnTopNValuePreservesStringComparisonBytes` 与 Rust 对应测试均覆盖该边界。
- 当前实现忽略 `DecodeOne` 的剩余字节。如果未来需要“恰好一个 Datum”的严格输入契约，应在两个入口同步决定是否拒绝尾部，并添加独立测试，不能静默改变现有兼容行为。

## 并发与资源生命周期

两个函数都是同步、无状态的纯转换过程：不创建线程/任务，不使用锁、通道、事务、文件或网络资源，也不持有传入借用。返回后，临时解码值、克隆的字段类型和中间 `Box` 按 Rust 所有权规则释放；成功结果拥有自己的数据。因没有共享可变状态，同一函数可被并发调用，其线程安全性由参数及所依赖类型的 `Send`/`Sync` 使用边界决定，而不是由本文件额外协调。

主要资源成本是索引分支克隆整段编码字节、反扁平化分支克隆 `FieldType`，以及 codec/tablecodec 的解码分配。扩展热路径时应避免额外重复复制，但不能用借用结果破坏现有返回值的独立所有权。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/statistics/cmsketch_util.go`，Rust 保留了两个同名入口和相同分支顺序：

- Go `topNMetaToDatum` 在 `isIndex` 时用 `SetBytes` 返回索引字节；Rust 用 `NewBytesDatum(value.Encoded.clone())` 达到同样的拥有式结果。否则两边都先 `DecodeOne`，再按字段类型与时区 `Unflatten`。
- Go `DecodeColumnTopNValue` 在解码错误或字符串类型时返回当前 Datum/错误；Rust 用 `?` 先传播错误，再对字符串成功结果直接返回，外部可观察语义一致。
- Go 使用 `*time.Location`，Rust 使用 `chrono_tz::Tz` 并包装为 `Some(location)`；Timestamp 时区差异由 Rust 的 `go_merge_47_top_n_timestamp_respects_location` 验证。
- Go 注释明确列出 ENUM、SET、BIT、时间和 Float 的扁平表示原因；Rust 当前用更简短的模块注释表达同一契约。Rust 的 `cmsketch_util_test.rs::top_n_meta_decodes_double_through_unflatten` 直接验证 Float64 恢复，`go_merge_47_test.rs` 补充 Duration、字符串、索引和 Timestamp 行为。

未发现本文件相对 Go 的有意功能删减；命名保留 Go 风格是 crate 全局 `#![allow(non_snake_case)]` 下的移植兼容选择。

## 扩展指南

- 若新增扁平类型恢复规则，优先修改下游 `tablecodec::Unflatten`；本文件应继续只负责索引/列分流及字符串比较字节例外。只有 TopN 特有契约才应在这两个入口增加分支。
- 改动索引语义时同步检查 `topNMetaToDatum`、`pkg/statistics/merge_global.rs::decode` 和 `pkg/statistics/histogram.rs::MergePartitionHist2GlobalHistWithLocation`，防止组合索引键被误当成单列编码。
- 改动列语义时同步检查 `DecodeColumnTopNValue` 与 `pkg/statistics/handle/runtime_stats.rs::canonical_histogram`；字符串类型判断变化必须验证二进制字节、排序/比较语义及各种 MySQL 字符串类型。
- 回归测试继续放在独立文件，不能内嵌到生产源文件。小范围函数测试放入 `pkg/statistics/cmsketch_util_test.rs`；Go 合并语义兼容可扩展 `pkg/statistics/go_merge_47_test.rs`；必要时与 `pkg/statistics/cmsketch_test.go` 的 Duration/字符串用例保持一一对应。
- 至少覆盖：非法/空编码、索引原样返回、字符串原始字节、数值与 Duration 反扁平化、Timestamp 多时区、以及若改变尾部策略则覆盖多值/尾随字节。兼容风险集中在 Datum kind 与排序语义，性能风险集中在热路径复制和反扁平化开销。

## 验证依据

- 源码与模块边界：`pkg/statistics/cmsketch_util.rs`、`pkg/statistics/cmsketch.rs::TopNMeta`、`pkg/statistics/lib.rs`、`pkg/statistics/Cargo.toml`。
- Rust 上游：`pkg/statistics/histogram.rs::MergePartitionHist2GlobalHistWithLocation`、`pkg/statistics/merge_global.rs::decode`、`pkg/statistics/handle/runtime_stats.rs::canonical_histogram`。
- Go 对照：`pkg/statistics/cmsketch_util.go`；Go 边界测试：`pkg/statistics/cmsketch_test.go::TestDecodeColumnTopNValueDuration` 与 `TestDecodeColumnTopNValuePreservesStringComparisonBytes`。
- Rust 独立测试：`pkg/statistics/cmsketch_util_test.rs::top_n_meta_decodes_double_through_unflatten`，以及 `pkg/statistics/go_merge_47_test.rs::go_merge_47_decode_duration_and_preserve_string_comparison_bytes`、`go_merge_47_top_n_timestamp_respects_location`。
- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点和 1,848,419 条边；`query` 同时定位 Rust/Go 两个实现；`explore` 给出上述 Rust 调用者。精确 `callers/callees/node` 命令本次未产生额外文本，故调用关系又以直接源码引用复核。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test`/`rg` 命令验证文档存在且固定二级标题恰好为 11 个，并人工复核仅新增本说明、未修改 Rust/Go/Cargo/`plan.md`。
