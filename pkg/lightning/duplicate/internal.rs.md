# `pkg/lightning/duplicate/internal.rs`

## 文件定位

本文件属于 `astersql-lightning-duplicate` crate，定义 Lightning 重复键检测流水线内部使用的可排序复合键及其编解码规则。crate 入口 `pkg/lightning/duplicate/lib.rs` 通过 `mod internal; pub use internal::*;` 将这些符号重新导出，仓库根 `pkg/lib.rs` 又通过 `facade_lightning_duplicate` 暴露该 crate；不过当前直接使用者集中在同 crate 的 `detector.rs` 与 `worker.rs`。

它位于“接收待检测键 -> 写入外部排序器 -> 排序 -> 并行扫描相邻项 -> 汇报重复组”链路的键表示层。`detector.rs::KeyAdder::add` 在写入排序器前调用 `encode_internal_key`，`detector.rs::Detector::get_range_bounds` 与 `worker.rs::Worker::scan_iterator` 从排序结果调用 `decode_internal_key`，后两处再用 `compare_internal_key` 判断扫描区间边界。

## 核心职责

1. 用 `InternalKey { key, key_id }` 同时保存业务键和该键的来源标识。`key` 决定哪些记录属于同一重复组，`key_id` 区分并稳定排序同一业务键的不同来源。
2. 定义与 Go `internalKey` 一致的二级字典序：先比较 `key`，相等时比较 `key_id`（`compare_internal_key`）。
3. 把业务键编码为 TiDB memcomparable 字节串，再原样追加 `key_id`（`encode_internal_key`）。因此外部排序器对编码字节的排序与内部键的逻辑排序一致。
4. 从已排序字节中恢复两部分：`DecodeBytes` 解出 `key`，未消费的尾部全部视为 `key_id`（`decode_internal_key`）。
5. 提供诊断展示格式：`Display` 将两段打印成大写十六进制，并仅在 `key_id` 非空时输出 `@` 分隔符。

本文件不负责判断“重复”：真正的分组条件是 `worker.rs::Worker::scan_iterator` 中相邻项的 `current_key.key == previous_key.key`；`key_id` 只参与全序和结果来源列表。

## 主要符号

- `pub struct InternalKey`：可克隆、可调试、可默认构造且支持相等比较的拥有型复合键。字段 `key`、`key_id` 是 `pub(crate)`，crate 外只能通过公开构造器和相关函数使用。
- `InternalKey::new(key: Vec<u8>, key_id: Vec<u8>) -> Self`：接管两段缓冲区，不复制其内容；常用于加入排序器以及生成扫描任务边界。
- `impl fmt::Display for InternalKey`：输出 `KEY` 或 `KEY@KEY_ID`，每个字节固定两位大写十六进制。空业务键、非空来源标识会显示为 `@KEY_ID`。
- `fn write_upper_hex(...)`：`Display` 的私有逐字节格式化辅助函数；每个 `write!` 错误原样返回。
- `pub fn compare_internal_key(...) -> i32`：返回严格规范化的 `-1`、`0` 或 `1`，不暴露 Rust `Ordering`，以对齐 Go `bytes.Compare` 的调用契约。
- `pub fn encode_internal_key(append_to: &mut Vec<u8>, ...)`：保留 `append_to` 中已有前缀，通过 `std::mem::take` 把原缓冲区交给 `EncodeBytes`，再追加原始 `key_id`。
- `pub fn decode_internal_key(data: &[u8], internal_key: &mut InternalKey) -> Result<(), Error>`：成功时复用并覆盖目标的两个 `Vec`；失败时返回外排模块统一的 `Error`，并模拟 Go 解码对原 slice 后备数组可能产生的部分写入。

文件没有常量、trait、条件编译项或异步函数。

## 执行流程

写入路径如下：

1. `detector.rs::KeyAdder::add` 清空自己的 `key_buf`，用输入的 `key` 和 `key_id` 构造 `InternalKey`。
2. `encode_internal_key` 把既有输出缓冲区移交给 `EncodeBytes`。编解码器按 8 字节分组、补零并添加 marker，生成保持字典序的业务键编码。
3. 函数把 `key_id` 原样接在编码业务键之后；`KeyAdder::add` 将整个结果作为 key、空切片作为 value 写入 `ExternalSorter`。
4. 外排完成后，编码字节可直接排序。`internal_test.rs::test_internal_key` 和 `internal_key_format_append_and_reuse` 对输入成对验证：编码字节比较结果等于 `compare_internal_key` 的结果。

读取与扫描路径如下：

1. `Detector::get_range_bounds` 解码排序器的第一项和最后一项，用它们建立初始扫描范围；最后一项的业务键追加 `0`，形成排他的上界。
2. `Worker::scan_iterator` 编码任务起点并 `seek`，随后逐项调用 `decode_internal_key`。成功解码时，`DecodeBytes` 的剩余切片成为 `key_id`。
3. worker 用 `compare_internal_key(current, end)` 判断是否到达任务上界，但用业务键本身的相等性识别重复组；同组内的 `key_id` 已按字典序排列并交给 handler。
4. 扫描每 1000 项可能生成新的 `InternalKey` 分割点，把剩余区间交给其他 worker。这里依赖本文件定义的编码序、逻辑序和边界比较三者一致。

## 数据与状态

`InternalKey` 完全拥有两段 `Vec<u8>`：

- `key` 是用户/业务可见键，也是重复分组身份。
- `key_id` 是来源标识；本文件不解释其业务结构，只按不透明字节处理。

编码没有长度字段分隔两段。边界由 memcomparable 编码自身的终止分组确定：`DecodeBytes` 消费完整业务键编码，返回的 `leftover` 即全部来源标识。因此扩展编码格式时不能在两段之间随意插入字节，也不能让 `key_id` 参与业务键解码。

成功解码会先取得临时结果，然后分别 `clear` 并 `extend_from_slice` 到目标缓冲区，允许反复复用同一个 `InternalKey`，同时避免让目标处于一半更新的成功态。编码同样支持在调用方已有前缀之后追加；测试以 `[42, 43]` 前缀确认前缀不会被覆盖。

`Default` 产生两个空向量。空键是合法输入；由于 memcomparable 编码仍含终止组，其编码不是空字节。`Display` 的空/非空组合也有独立断言。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 索引核对）：

- `detector.rs::KeyAdder::add -> encode_internal_key`：生成外排 key。
- `detector.rs::Detector::get_range_bounds -> decode_internal_key`：恢复全局首末边界。
- `detector.rs::Detector::detect_inner -> compare_internal_key`：空范围或退化范围直接结束。
- `worker.rs::Worker::scan_iterator -> encode_internal_key/decode_internal_key/compare_internal_key`：定位起点、逐项恢复键、限制扫描范围。
- `internal_test.rs` 与 `migration_aster_unit_test.rs` 直接调用三个函数验证契约。

下游依赖：

- `crate::util::codec::{EncodeBytes, DecodeBytes}` 由 `lib.rs` 从 `astersql-util-codec` 重导出，提供 TiDB memcomparable 编解码。
- `crate::util::extsort::external_sorter::Error` 由 `astersql-util-extsort` 重导出，是解码失败对外使用的统一错误类型。
- Rust 标准库的 `Ordering`、`fmt` 和 `Vec` 提供比较、展示与缓冲区所有权。

`pkg/lightning/duplicate/Cargo.toml` 声明 crate 名为 `astersql-lightning-duplicate`，并以路径依赖连接 codec、external sorter 和 Lightning log；本文件本身不直接使用 `goish`、日志或 `tokio-util`。清单没有 feature 条件，因此这些符号在该 crate 的普通构建中始终存在。

## 错误处理与边界

`encode_internal_key` 和 `compare_internal_key` 没有可恢复错误返回；内存分配失败仍遵循 Rust 运行时行为。`Display` 会传播 formatter 的 `fmt::Error`。

`decode_internal_key` 的主要错误来自 `DecodeBytes`：输入不足 9 字节分组、marker 推导出的 padding 大于 8、padding 字节非法等都会失败，并经 `error.into()` 转成 external sorter 的 `Error`。成功之前不会清空 `key_id`；失败时 `key_id` 保持原值。

失败路径第 90--110 行是刻意的 Go 兼容逻辑，而不是第二套宽松解码器。Go 的 `codec.DecodeBytes(data, ikey.key[:0])` 可能在最终报错前写入原 slice 的后备数组；Rust 的拥有型 `DecodeBytes(data, None)` 会丢弃临时缓冲区。为保持可观察状态一致，Rust 在失败后按完整 9 字节组重放“能写回原容量且落在原长度内”的前缀：非法 marker、终止组或容量不足都会停止。函数依然返回错误，绝不会把畸形输入当作成功。`internal_test.rs::decode_errors_preserve_go_slice_state` 覆盖空/短输入、非法 marker、多个分组、padding 错误以及超过容量后 Go 临时 slice 脱离原后备数组的情形。

一个重要协议边界是 `key_id` 没有独立校验或长度限制：任意尾部（包括空尾部）都合法。调用者必须确保传入的 `data` 确实以一段合法的 memcomparable 业务键开头。

## 并发与资源生命周期

本文件没有线程、锁、channel、异步任务、文件句柄或显式关闭流程。所有函数只使用参数和局部状态；`InternalKey` 由普通 Rust 所有权管理，因此自身没有共享可变状态。

它仍处于并发扫描的关键路径：`Detector::detect_inner` 为多个 scoped worker 创建独立的 `Worker`，每个 `scan_iterator` 都持有自己的 `previous_key`、`current_key` 和编码缓冲区。本文件函数不依赖全局状态，因而可由不同线程并行调用。`InternalKey` 的字段是拥有型 `Vec`，任务队列通过 clone 转移边界时不会共享底层可变引用。

性能生命周期以缓冲区复用为核心：`KeyAdder` 重复清空 `key_buf`，worker 重复解码进两个 `InternalKey` 并交换它们。修改实现时应保留这种容量复用，避免为每个排序项引入额外长期分配；同时不能为了复用而破坏失败时的 Go slice 状态兼容。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/lightning/duplicate/internal.go`：

- Rust `InternalKey` 对应 Go `internalKey`，`key_id` 对应 `keyID`。
- Rust `Display` 对应 Go `String()`，格式均为大写十六进制 `KEY` 或 `KEY@KEYID`。
- `compare_internal_key` 对应 `compareInternalKey`，均先比较 key 再比较 key ID。Go `bytes.Compare` 只承诺负/零/正；Rust 将结果规范化为 `-1/0/1`，与现有测试向编码字节比较转换后的值一致。
- `encode_internal_key` 对应 `encodeInternalKey`：都在调用方已有缓冲区末尾追加 memcomparable key，再追加原始 key ID。
- `decode_internal_key` 对应 `decodeInternalKey`：都把 codec 剩余字节复制/复用为 key ID。

两种语言的所有权模型不同。Go 直接把 `ikey.key[:0]` 交给 decoder，错误前的 append 可能改变原后备数组；Rust 先用临时缓冲区解码，并仅在错误分支重放 Go 可观察到的原数组写入。这一差异已有专门 Rust 回归测试，不能简化为“失败时目标完全不变”。

`internal_test.go::TestInternalKey` 的 11 组输入在 `internal_test.rs::test_internal_key` 和 `migration_aster_unit_test.rs::internal_key_round_trip_preserves_go_ordering` 中均有对应覆盖。Rust 额外测试了格式化、已有输出前缀、更多 8 字节分组边界、缓冲区复用和错误状态兼容。

## 扩展指南

- 若要改变排序或编码规则，必须同步审查 `compare_internal_key`、`encode_internal_key`、`decode_internal_key` 三者，保持“逻辑比较序 = 编码字节序”。否则外排相邻扫描、范围上界和任务拆分都会出现不一致。
- 若给 `InternalKey` 增加字段，应先确定它是否参与全序、是否需要持久化进外排 key，以及旧编码如何解码；当前格式没有版本号或额外长度边界，直接插入字段会产生兼容风险。
- 若修改错误处理，应保留 external sorter `Error` 类型以及 Go slice 后备数组的状态语义，并扩展 `internal_test.rs::decode_errors_preserve_go_slice_state`。不要把测试嵌入生产源文件。
- 若优化分配，应优先维持 `append_to`、`internal_key.key` 和 `internal_key.key_id` 的现有复用接口，并在 `internal_test.rs::internal_key_format_append_and_reuse` 增加跨 8 字节分组、空 ID 和非空前缀用例。
- 若改变展示格式，只影响诊断文本而不应影响编码；相应修改 `Display`/`write_upper_hex` 并更新独立的 `internal_test.rs`。
- 行为变更还应与 `internal.go`、`internal_test.go` 核对，必要时同步 Go/Rust 两端。重点风险是磁盘排序兼容性、重复组漏报/错报和热路径额外分配；当前文件不涉及锁竞争或网络兼容。

## 验证依据

本说明基于以下直接证据：

- 源码：`pkg/lightning/duplicate/internal.rs`（完整 119 行）；crate 入口 `pkg/lightning/duplicate/lib.rs`；直接调用者 `detector.rs::{detect_inner,get_range_bounds,KeyAdder::add}` 与 `worker.rs::Worker::scan_iterator`。
- RustCodeGraph：`status` 显示索引含目标文件；`files --filter pkg/lightning/duplicate` 确认同目录生产/测试文件；`node internal.rs::{InternalKey,compare_internal_key,encode_internal_key,decode_internal_key}` 核对定义与调用边；`node detector.rs::{detect_inner,add,get_range_bounds}`、`node worker.rs::scan_iterator` 核对应用链；`node bytes.rs::{EncodeBytes,DecodeBytes}` 核对编解码分组和错误来源。
- crate 声明：`pkg/lightning/duplicate/Cargo.toml`；workspace 成员与 facade 依赖：根 `Cargo.toml`；根再导出：`pkg/lib.rs`。
- Go 对照：`pkg/lightning/duplicate/internal.go`、`pkg/lightning/duplicate/internal_test.go`。
- Rust 测试：`pkg/lightning/duplicate/internal_test.rs`、`pkg/lightning/duplicate/migration_aster_unit_test.rs::internal_key_round_trip_preserves_go_ordering`。

本任务只新增说明文档，按计划不运行 Cargo。结构验收要求本文恰含上述 11 个固定二级标题；行为描述通过源码、调用图、Cargo、Go 对照和独立测试交叉复核。
