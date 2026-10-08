# `pkg/store/mockstore/unistore/tikv/mvcc/codec_adapter.rs`

## 文件定位

本文件是 `astersql-store-mockstore-unistore-tikv-mvcc` crate 的编解码适配层。crate 入口 `pkg/store/mockstore/unistore/tikv/mvcc/lib.rs` 通过 `#[path = "codec_adapter.rs"] pub mod codec;` 将它挂载为公开的 `codec` 模块，因此调用端看到的是 `crate::codec::*`，而不是文件名 `codec_adapter`。

它不独立实现一套 MVCC 编码算法，而是在编译期用 `include!` 引入 `pkg/util/codec/number.rs` 和 `pkg/util/codec/bytes.rs` 的实现，再只公开 MVCC 当前需要的六个入口。这样可以复用与 Go `pkg/util/codec` 对齐的字节格式，同时给 MVCC crate 提供本地的 `errors` 与 `binary::MaxVarintLen64` 名称环境。

crate 边界由 `pkg/store/mockstore/unistore/tikv/mvcc/Cargo.toml` 确定：库入口是 `lib.rs`，本适配文件没有独立 feature；编码错误最终使用该 crate 依赖的 `astersql-errors`，并经 `lib.rs::errors` 暴露的 `SharedError`、`New` 和 `Errorf` 构造。

## 核心职责

本文件有三项职责：

1. 定义 `binary::MaxVarintLen64 = 10`，满足被引入的 compact-bytes 实现对 Go `encoding/binary.MaxVarintLen64` 名称和容量上界的依赖。
2. 将共享 `number.rs` 与 `bytes.rs` 纳入当前 crate 的编译单元，使其解析到当前 crate 的 `errors` 模块，而不要求 MVCC crate直接依赖另一个 codec crate。
3. 以窄门面再导出 `EncodeCompactBytes`、`DecodeUintDesc`、`DecodeUvarint`、`EncodeUint`、`EncodeUintDesc`、`EncodeUvarint`，支持 MVCC 键时间戳、Write CF 和 Lock CF 的二进制布局。

它不是完整的 `pkg/util/codec` 公共接口。虽然被 `include!` 的私有子模块内部还含有其他函数，但外部稳定可见面由文件末尾两条 `pub use` 决定。

## 主要符号

- `binary`：公开子模块，仅包含 `MaxVarintLen64: usize = 10`。共享 `bytes_codec::EncodeCompactBytes` 用它预留“最长 varint 前缀 + payload”的容量。
- `number`：私有模块，编译期展开 `../../../../../util/codec/number.rs`。它提供本文件再导出的整数函数，也向 `bytes_codec` 提供 `EncodeVarint` 等内部依赖。
- `bytes_codec`：私有模块，先引入 `super::{binary, number::*}` 与 `crate::errors`，再展开 `../../../../../util/codec/bytes.rs`。这个显式作用域绑定是适配层成立的关键。
- `EncodeUint(Vec<u8>, u64) -> Vec<u8>`：追加 8 字节大端无符号整数，保持升序字典序。
- `EncodeUintDesc(Vec<u8>, u64) -> Vec<u8>` / `DecodeUintDesc(&[u8]) -> Result<(&[u8], u64), SharedError>`：对数值按位取反后使用 8 字节大端编码，并在解码时还原，用于让较新的时间戳排在字节序较前位置。
- `EncodeUvarint(Vec<u8>, u64) -> Vec<u8>` / `DecodeUvarint(&[u8]) -> Result<(&[u8], u64), SharedError>`：Go `encoding/binary` 兼容的无符号变长整数编码；解码保留未消费后缀。
- `EncodeCompactBytes(Vec<u8>, &[u8]) -> Vec<u8>`：先用有符号 varint 写入数据长度，再追加原字节。格式紧凑但不具备 memcomparable 性质。

## 执行流程

编译阶段先由 `lib.rs` 把本文件作为 `codec` 模块装入。随后 `number` 展开共享数字实现，`bytes_codec` 在能看到 `binary`、`number::*` 和 `crate::errors` 的作用域中展开共享字节实现，最后六个函数被再导出到 `codec` 模块表面。

运行时有三条直接主链：

1. MVCC 键时间戳链：`mvcc.rs::EncodeExtraTxnStatusKey` 调用 `codec::EncodeUintDesc` 将 `startTS` 追加到键尾；`mvcc.rs::DecodeKeyTS` 截取键尾 8 字节并调用 `codec::DecodeUintDesc`。额外事务状态键还会调整用户键首字节以隔离命名空间。
2. Write CF 链：`tikv.rs::EncodeWriteCFValue` 在类型字节后调用 `EncodeUvarint` 写 `startTs`；`tikv.rs::ParseWriteCFValue` 校验类型后调用 `DecodeUvarint`，把返回的未消费后缀作为 `ShortVal`，把解码值作为 `StartTS`。
3. Lock CF 链：`tikv.rs::EncodeLockCFValue` 依次写锁类型、`EncodeCompactBytes(primary)`、`EncodeUvarint(startTS)`、`EncodeUvarint(TTL)`；存在 `ForUpdateTS` 或 `MinCommitTS` 时，在字段前缀后用 `EncodeUint` 追加固定宽度值。

这些调用均在已有 `Vec<u8>` 后追加数据，函数返回可能复用或重新分配后的所有权容器；解码函数借用输入并返回剩余切片，不复制剩余数据。

## 数据与状态

本文件本身没有可变全局状态、缓存或实例字段。`MaxVarintLen64` 是编译期常量；`number` 与 `bytes_codec` 只是模块作用域。

编码状态完全存在于参数和返回值中：编码器取得 `Vec<u8>` 所有权并追加字节；`EncodeCompactBytes` 会按最坏 10 字节长度前缀加 payload 长度预留容量。解码器只借用 `&[u8]`，返回 `(剩余切片, 值)`，调用方必须决定剩余切片在上层格式中的含义。

格式不变量包括：`EncodeUint`/`EncodeUintDesc` 固定追加 8 字节；降序编码是 `!v` 的大端表示；uvarint 最长 10 字节且第十字节只能为 0 或 1；compact bytes 的长度前缀来自 `EncodeVarint(data.len() as i64)`，不是可比较编码。

## 依赖与调用关系

上游模块装配为 `mvcc/lib.rs -> codec_adapter.rs (公开名 codec)`。直接 Rust 调用者由精确引用搜索确认：

- `pkg/store/mockstore/unistore/tikv/mvcc/mvcc.rs`：`DecodeKeyTS`、`EncodeExtraTxnStatusKey` 使用降序 uint 编解码。
- `pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs`：`ParseWriteCFValue`、`EncodeWriteCFValue`、`EncodeLockCFValue` 使用 uvarint、compact bytes 和固定宽度 uint。
- `pkg/store/mockstore/unistore/tikv/mvcc/migration_aster_unit_test.rs`：直接用 `codec::EncodeUintDesc` 构造键，并通过上层函数验证结果。

下游实现来自 `pkg/util/codec/number.rs` 与 `pkg/util/codec/bytes.rs`。`number.rs` 的相关路径依赖大端字节转换及 `crate::errors`；`bytes.rs::EncodeCompactBytes` 依赖 `reallocBytes`、`EncodeVarint` 和 `binary::MaxVarintLen64`。本文件的相对 `include!` 路径与这些源码内部使用的未限定名称共同构成编译期耦合：移动目录、改名或改变共享源码所需作用域时，必须同步修改适配层。

RustCodeGraph 已索引目标文件和共享实现；目标文件被识别为 28 行、1 个本地符号，文件级结果显示没有普通 `use` 调用边。这是 `#[path]`、`include!` 和 `pub use` 组合未形成完整别名边的限制，因此直接调用关系另由上述精确源码引用核对，而不是把图中“0 files”误解为未使用。

## 错误处理与边界

`EncodeUint`、`EncodeUintDesc`、`EncodeUvarint` 和 `EncodeCompactBytes` 对所有 `u64` 或切片输入返回字节，不返回错误。潜在资源边界是输入长度和 `Vec` 分配；`EncodeCompactBytes` 将 `usize` 长度转换为 `i64`，实际使用必须受可分配内存规模约束。

`DecodeUintDesc` 在输入少于 8 字节时返回 `SharedError("insufficient bytes to decode value")`。`DecodeUvarint` 对未终止输入返回同一不足错误，对超过 64 位（包括非法第十字节）的输入返回 `SharedError("value larger than 64 bits")`，成功时保留后缀。

上层处理策略不同：`tikv.rs::ParseWriteCFValue` 把 uvarint 错误转为 `WriteCFValueError::Codec`；`mvcc.rs::DecodeKeyTS` 假设调用者提供至少 8 字节并对解码错误 `panic!`，而且其切片动作在过短输入时会先 panic。因此本适配层的 `Result` 不能自动保护所有上层入口，调用方仍须维持键长度不变量。

本门面只再导出 `EncodeCompactBytes`，不再导出对应的 `DecodeCompactBytes`；当前 Lock CF 路径在此 crate 内只需要编码。新增解码需求时不能假设该符号已公开。

## 并发与资源生命周期

所有公开函数都是无共享状态的同步纯计算；没有锁、原子变量、线程、异步任务、通道或事务生命周期。只要调用者分别管理自己的缓冲区，它们可被并发调用。

编码函数消费并返回 `Vec<u8>`，避免悬垂借用，并允许沿字段流水线连续追加。是否发生重新分配由容量决定；`EncodeCompactBytes` 会预留空间以降低增长次数。解码结果中的剩余切片借用原输入，生命周期不能超过输入缓冲；`DecodeUvarint` 返回的数值本身不借用输入。

此处的“MVCC”并不意味着适配层参与事务并发控制；事务/锁语义属于调用它的 `mvcc.rs`、`tikv.rs` 及更上层存储流程，本文件只保证二进制表示。

## 与 Go 版本的对应关系

Rust 没有同路径同名的 Go 适配文件；对应语义来自 `pkg/util/codec/number.go`、`pkg/util/codec/bytes.go`，实际使用方式由同目录 `mvcc.go`、`tikv.go` 给出。

- `EncodeUint`、`EncodeUintDesc`、`DecodeUintDesc` 与 Go 版本一样使用 8 字节大端，降序变体写入/读取按位取反值。
- `EncodeUvarint`、`DecodeUvarint` 对齐 Go `binary.PutUvarint`/`binary.Uvarint` 的字节向量、10 字节上限、后缀保留和不足/溢出错误分类。
- `EncodeCompactBytes` 与 Go 一样用 `EncodeVarint` 写长度，再追加 payload，并只承诺紧凑而不承诺字典序可比较。
- `mvcc.go::DecodeKeyTS`、`EncodeExtraTxnStatusKey` 与 Rust 同名函数采用相同 codec 操作；`tikv.go` 的 Write CF/Lock CF 字段顺序也与 Rust `tikv.rs` 相同。

Rust 的结构性差异是：Go 直接导入全局 `pkg/util/codec` 包；Rust 因 crate 边界用 `include!` 建立局部实现副本和错误命名环境，再以窄 API 再导出。Rust 错误类型是 `SharedError`/枚举映射，Go 返回 `error`；字节切片追加在 Rust 中表现为消费并返回 `Vec<u8>`。

## 扩展指南

新增 MVCC 编码功能时，应先判断它是否已在 `pkg/util/codec` 的 Go/Rust 对照实现中存在。若存在，优先继续复用共享实现，并只在本文件增加必要的 `pub use`；若共享实现需要新的常量、错误或辅助名称，应在 `bytes_codec`/`number` 的 include 作用域显式提供，而不是在 MVCC 中复制算法。

修改固定宽度、取反规则、varint 格式或 compact 长度前缀会改变持久化键/值和 TiKV 兼容格式，必须同步核对 `pkg/util/codec/number.go`、`bytes.go`、同目录 `mvcc.go`、`tikv.go`。尤其不能把 compact bytes 当作 memcomparable 编码，也不能将时间戳降序编码替换为普通大端编码。

最可能的接入点是文件末尾的 `pub use`、`binary` 兼容常量、`bytes_codec` 的作用域绑定，以及直接消费者 `mvcc.rs`/`tikv.rs`。测试应保持独立文件：MVCC 布局或字段组合扩展 `migration_aster_unit_test.rs`；通用整数/字节算法扩展 `pkg/util/codec/number_test.rs`、`float_2_aster_unit_test.rs`、`bytes_1_aster_unit_test.rs` 或 `codec_test.rs`，并与 Go `codec_test.go` 向量对齐。

兼容风险高于代码体量所暗示的程度：这些字节进入键排序和 CF 值布局，格式变化可能破坏旧数据读取或跨实现互操作。性能风险主要是额外分配、重复复制和误用非 memcomparable 格式；正确性风险主要是端序、取反、字段顺序、后缀消费和错误边界漂移。

## 验证依据

本说明读取并核对了以下直接证据：

- 目标与装配：`pkg/store/mockstore/unistore/tikv/mvcc/codec_adapter.rs`、`lib.rs`、`Cargo.toml`。
- Rust 实现与调用：`pkg/util/codec/number.rs`、`pkg/util/codec/bytes.rs`、`pkg/store/mockstore/unistore/tikv/mvcc/mvcc.rs`、`tikv.rs`。
- Go 对照：`pkg/util/codec/number.go`、`bytes.go`、`pkg/store/mockstore/unistore/tikv/mvcc/mvcc.go`、`tikv.go`。
- 独立测试：`pkg/store/mockstore/unistore/tikv/mvcc/migration_aster_unit_test.rs` 验证时间戳键、Write CF、Lock CF；`pkg/util/codec/float_2_aster_unit_test.rs` 验证 uvarint 向量、10 字节最大值和不足/溢出错误；`bytes_1_aster_unit_test.rs` 验证 compact bytes 前缀/payload/错误；`codec_test.rs` 验证固定宽度、降序、uvarint 与 compact bytes 往返。
- RustCodeGraph：`status` 确认本地索引包含 11,467 个文件；`files --filter .../codec_adapter.rs` 确认目标已索引；`node --file` 读取目标、共享实现、调用者和测试；`query` 定位六个公开函数的共享实现节点；`callers` 对这些再导出名称未返回可归属边，随后以精确引用搜索补齐调用证据。

人工复核结论：本文件存在的理由是跨 crate 复用 Go 兼容 codec 实现并控制 MVCC 暴露面；运行路径由键时间戳、Write CF 和 Lock CF 三条调用链构成；安全扩展必须同时维护 include 作用域、公开门面、Go/Rust 字节兼容和独立测试覆盖。
