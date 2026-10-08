# `pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs`

## 文件定位

[`tikv.rs`](./tikv.rs) 是 `astersql-store-mockstore-unistore-tikv-mvcc` crate 中面向 TiKV 存储格式的编解码层，与定义 MVCC 内存锁结构的 [`mvcc.rs`](./mvcc.rs) 分工：后者提供 `Lock`/`LockHdr`，本文件把这些状态转换为 TiKV Write CF 和 Lock CF 的字节布局。crate 入口 [`lib.rs`](./lib.rs) 以 `pub mod tikv` 挂载本文件，并通过 `pub use tikv::*` 再导出全部公开符号。

[`Cargo.toml`](./Cargo.toml) 将该 crate 定义为 workspace 内部库，直接使用 `kvproto` 的 `kvrpcpb::Op` 和 `thiserror`；本文件的整数/字节编码则由 crate 内 `codec_adapter.rs` 暴露的 `crate::codec` 提供。它不负责事务冲突检查、CF 写入、锁等待或网络 RPC，只生成或解析值部分的字节。

## 核心职责

1. 用 `WriteType{Put,Delete,Lock,Rollback}` 四个单字节常量定义 Write CF 记录类型，并用 `ParseWriteCFValue` 校验类型、解码 `StartTS` 和保留后续负载。
2. 用 `EncodeWriteCFValue` 生成 `write type + startTS(varint) + 可选短值段` 的 TiKV 兼容 Write CF 值。
3. 用 `EncodeLockCFValue` 把 `Lock` 的操作类型、primary key、时间戳、TTL、值以及悲观锁/异步提交字段编码为 Lock CF 值；以 `ShortValueMaxLen == 64` 为分界，将超长 value 单独返回给上层作为 default CF 载荷。
4. 用 `LockType{Put,Delete,Lock,Pessimistic}` 固定 `kvrpcpb::Op` 到 TiKV Lock CF 类型字节的映射。

这些职责都是纯计算：输入为借用的字节或 `Lock`，输出为拥有所有权的值，没有 I/O 或隐式全局状态。

## 主要符号

- `WriteType = u8` 及 `WriteTypeLock = b'L'`、`WriteTypeRollback = b'R'`、`WriteTypeDelete = b'D'`、`WriteTypePut = b'P'`：Write CF 类型的线上标记。类型别名本身不阻止传入其他 `u8`，解码器会校验，编码器不会。
- `WriteCFValue { Type, StartTS, ShortVal }`：`ParseWriteCFValue` 的拥有型结果。`ShortVal` 是 `DecodeUvarint` 解出 `StartTS` 后的全部剩余字节；如果输入由本文件编码，其内容为 `b'v' + 一字节长度 + value`，而不是纯 value。
- `WriteCFValueError::{Invalid, Codec(String)}`：分别表示空输入/未知写类型，以及 `StartTS` varint 解码失败。`errInvalidWriteCFValue` 是与 Go 错误文本对齐的公开字符串常量，函数实际返回类型化枚举。
- `ParseWriteCFValue(&[u8]) -> Result<WriteCFValue, WriteCFValueError>`：Write CF 值入口。它只验证类型和 varint，不进一步检查剩余短值字段的 `v/length/payload` 完整性。
- `shortValuePrefix = b'v'`、`forUpdatePrefix = b'f'`、`minCommitTsPrefix = b'm'`：Lock/Write CF 可选字段的单字节 tag。
- `ShortValueMaxLen = 64`：Lock CF 值内联阈值；恰好 64 字节仍内联，65 字节开始分离。
- `EncodeWriteCFValue(WriteType, u64, &[u8]) -> Vec<u8>`：追加写类型、可变长 `startTs`，非空短值再追加 tag、`u8` 长度和内容。
- `EncodeLockCFValue(&Lock) -> (Vec<u8>, Vec<u8>)`：第一个结果是 Lock CF 值，第二个结果是需分离写入 default CF 的长值；短值或空值时第二项为空。
- `LockType = u8` 及 `LockTypePut = b'P'`、`LockTypeDelete = b'D'`、`LockTypeLock = b'L'`、`LockTypePessimistic = b'S'`：Lock CF 首字节布局。

## 执行流程

`ParseWriteCFValue` 按以下顺序执行：

1. 空切片立即返回 `WriteCFValueError::Invalid`，避免访问 `data[0]` 越界。
2. 把首字节写入 `WriteCFValue.Type`，只允许 `P/D/L/R` 四种值；其他值返回 `Invalid`。
3. 对 `data[1..]` 调用 `codec::DecodeUvarint`。该调用同时返回未消费切片与 `startTS`：未消费部分拷贝到 `ShortVal`，时间戳写入 `StartTS`。
4. codec 错误转为 `WriteCFValueError::Codec(err.to_string())`，成功则返回完整结构。

`EncodeWriteCFValue` 先写入 `t`，再用 `codec::EncodeUvarint` 写入 `startTs`。`shortVal` 非空时追加 `b'v'`、将长度强制转为 `u8`，最后追加全部载荷；空值没有 tag 或长度字节。

`EncodeLockCFValue` 的布局顺序为：

1. 把 `lock.LockHdr.Op` 中的 `Op::{Put,Del,Lock,PessimisticLock}` 映射为 `P/D/L/S`；其他值触发 `panic!("invalid lock op")`。
2. 先用 `EncodeCompactBytes` 编码 `lock.Primary`，再依次以 varint 编码 `StartTS` 和转为 `u64` 的 `TTL`。
3. `lock.Value.len() <= 64` 时，非空 value 以 `v + u8 length + payload` 内联；空 value 不写字段。长度大于 64 时，Lock CF 不包含 value，而是 clone 到第二返回值。
4. `ForUpdateTS > 0` 时追加 `f` 和 `codec::EncodeUint` 的 8 字节大端整数；`MinCommitTS > 0` 时以同样方式追加 `m` 字段。两者均存在时顺序固定为 `f` 后 `m`。

## 数据与状态

本文件没有可变全局状态。所有类型字节、字段 tag 和 64 字节阈值都是编译期常量，所有函数都使用局部 `Vec<u8>` 构建结果。

关键线上不变量是：

- Write CF 首字节表示写类型，紧随其后的 `StartTS` 使用 unsigned varint。
- Lock CF 的 primary 使用 compact-bytes 长度前缀；`StartTS` 与 `TTL` 使用 unsigned varint；`ForUpdateTS` 和 `MinCommitTS` 使用 `EncodeUint` 的固定宽度格式。
- `LockHdr` 中是否带旧版本、异步提交次键列表等字段不在本函数中编码；本文件只消费 `Op`、`StartTS`、`TTL`、`ForUpdateTS`、`MinCommitTS`、`Primary` 和 `Value`。
- 超长 value 的第二返回值是独立 clone，与输入 `Lock.Value` 不共享内存；本文件不为它选择 default CF key，也不执行写入。

## 依赖与调用关系

上游装配是 [`lib.rs`](./lib.rs)：它公开 `tikv` 模块并将符号再导出到 crate 根。workspace 根 `Cargo.toml` 以 `facade_store_mockstore_unistore_tikv_mvcc` 登记该 crate；`pkg/store/mockstore/unistore/{server,cophandler}/Cargo.toml` 和 `pkg/store/mockstore/unistore/tikv/{Cargo.toml,dbreader/Cargo.toml,kverrors/Cargo.toml}` 声明 crate 依赖。这些是 crate 级可达性证据，不等于对本文件函数的直接调用。

对全仓 Rust 源码的精确符号搜索只找到 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 直接调用 `ParseWriteCFValue`、`EncodeWriteCFValue` 和 `EncodeLockCFValue`，未找到生产 Rust 调用者。因此当前事实是“已编译进 crate 并公开导出，但尚未接入仓库内 Rust 生产调用链”；不应从 crate 的下游依赖推断这些函数已在运行时被使用。

下游直接依赖为：

- `crate::codec::{DecodeUvarint, EncodeUvarint, EncodeCompactBytes, EncodeUint}`，由 [`codec_adapter.rs`](./codec_adapter.rs) 提供 TiDB/TiKV 格式适配。
- `crate::mvcc::Lock` 及其 `LockHdr`，定义于 [`mvcc.rs`](./mvcc.rs)。
- `kvproto::kvrpcpb::Op`，用于解释 `LockHdr.Op` 的 RPC 操作编号。
- `thiserror::Error`，为 `WriteCFValueError` 生成错误展示实现。

## 错误处理与边界

- `ParseWriteCFValue` 对空输入和非 `P/D/L/R` 首字节返回 `Invalid`；对截断或非法 varint 返回带 codec 文本的 `Codec(String)`。迁移单测覆盖 `[]`、`X 01` 和截断的 `P 80` 三类错误。
- 解码成功不代表短值段完整：函数不检查 `ShortVal` 的 tag、长度或内容。需要消费纯短值的调用者必须另行解析。
- `EncodeWriteCFValue` 不检查 `t` 是否为已知类型，也没有显式限制 `shortVal.len() <= 255`。长度被 `as u8` 截断时仍会追加全部 payload，可产生不一致编码；安全调用应将该入参限定为协议允许的短值（本模块对 Lock 使用 64 字节阈值）。
- `EncodeLockCFValue` 对不支持的 `LockHdr.Op` 不返回 `Result`，而是直接 panic。调用前必须保证 op 属于四个已支持取值。
- `TTL` 从 `u32` 扩展为 `u64` 后编码，不会丢失信息；`ForUpdateTS`/`MinCommitTS` 为 0 时省略字段，因此空缺与 0 在线上语义中合并。
- `EncodeLockCFValue` 本身不验证 `LockHdr` 中的 `PrimaryLen` 是否与 `Primary.len()` 一致，因为 Lock CF 编码直接从 `Primary` 计算 compact length；这与 `Lock::MarshalBinary` 的固定头布局是不同路径。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、锁或数据库快照。入参只在函数调用期间借用，返回的 `WriteCFValue` 和 `Vec<u8>` 均拥有自己的数据，所以结果生命周期不依赖输入。这使得函数在多线程中没有模块级竞态；但它们也不提供对多 CF 写入的原子性。

当 `EncodeLockCFValue` 返回非空 `longValue` 时，后续层必须把 Lock CF 编码和 default CF 载荷纳入同一个事务/写批次；本文件无法强制这一生命周期不变量。由于当前仓库内没有 Rust 生产调用者，该原子写入接线尚不能从现有 Rust 调用链验证。

## 与 Go 版本的对应关系

直接对照文件是 [`tikv.go`](./tikv.go)。Rust 保留了 Go 的公开符号命名、`P/D/L/R/S` 标记、字段顺序、64 字节短值阈值、未知 lock op 的 panic 语义，以及 `forUpdateTS` 后接 `minCommitTS` 的可选后缀顺序。`migration_aster_unit_test.rs` 对四种 WriteType 往返、非法 Write CF、四种 LockType，以及 64/65 字节分界的实际字节布局提供 Rust 侧回归证据。

可见差异主要是语言表达而非线上布局：

- Go `ParseWriteCFValue` 通过命名返回值和 `error` 传递失败；Rust 用 `Result<WriteCFValue, WriteCFValueError>` 并区分格式错误与 codec 错误。
- Go 的 `errInvalidWriteCFValue` 是错误对象；Rust 保留同名错误文本常量，真正的错误值是 `WriteCFValueError::Invalid`。
- Go 用 `y.SafeCopy(nil, lock.Value)` 复制长值；Rust 用 `lock.Value.clone()`，都产生独立缓冲。
- Go 的 `Lock` 字段直接平铺；Rust 时间戳、TTL 和 op 位于 `lock.LockHdr`，而 `Primary`/`Value` 仍位于 `Lock`。
- Go 生产文件在当前局部精确搜索中同样未出现这三个函数的外部直接调用；因此文档不声称 Rust 已替换某条已验证的 Go 运行时路径。

## 扩展指南

- 新增 WriteType 时，必须同时更新类型常量、`ParseWriteCFValue` 白名单、Go 对照语义和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的往返/非法输入用例。还要确认 TiKV 读写方是否识别该首字节。
- 新增 lock op 时，应在 `EncodeLockCFValue` 的 match 中明确映射，并增加独立 Rust 测试；不要用默认分支静默编码，因为 lock type 是持久化协议。
- 修改任何 tag、整数编码、字段顺序或 `ShortValueMaxLen` 都是兼容性变更；必须与 `tikv.go`、TiKV 存储格式和精确字节断言一起审查。
- 如果要让 `ParseWriteCFValue.ShortVal` 直接返回纯 payload，这不是内部重构，而是公开 API 语义改变；应优先新增显式的短值解析器，保留当前 Go 对齐行为。
- 如果将这些函数接入 Rust 生产写路径，要在调用层同时测试：长值的 Lock/default CF 原子写入、空值语义、悲观锁 `ForUpdateTS`、异步提交 `MinCommitTS`、以及非法 op 不能来自未信任输入。
- Rust 测试逻辑应继续保持在独立的 `migration_aster_unit_test.rs` 中，不应内嵌到 `tikv.rs`。对线上格式的新断言应使用确定性字节向量，不只做同一实现的 encode/decode 自往返。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 7,032 个 Rust 文件；`files --filter pkg/store/mockstore/unistore/tikv/mvcc` 确认目标文件、Go 对照和迁移测试都在索引中。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/tikv/mvcc/tikv.rs --offset 1 --limit 400` 读取了目标文件全部 166 行，确认类型、常量、错误和三个函数的实际实现。
- RustCodeGraph 对 `ParseWriteCFValue`、`EncodeWriteCFValue`、`EncodeLockCFValue` 的 `query --kind function --json` 均找到同路径 Go/Rust 定义。精确 `callers/callees` 查询未在限定时间内产生边，因此调用者结论另用全仓 Rust `rg` 直接引用搜索补证，结果仅有 `migration_aster_unit_test.rs`。
- 已读源码/配置：[`tikv.rs`](./tikv.rs)、[`lib.rs`](./lib.rs)、[`mvcc.rs`](./mvcc.rs)、[`codec_adapter.rs`](./codec_adapter.rs)、[`Cargo.toml`](./Cargo.toml)、Go 对照 [`tikv.go`](./tikv.go) 和独立 Rust 测试 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。Cargo 反向搜索还核对了 workspace facade 与 `server`/`cophandler`/`tikv`/`dbreader`/`kverrors` 的 crate 依赖。
- 独立测试的 `write_cf_round_trips_all_go_types_and_rejects_invalid_data` 证明四种 WriteType、`StartTS = 300`、保留的 `v + length + payload` 和三类非法输入；`lock_cf_short_and_long_values_match_tikv_encoding` 证明 `P/D/L/S` 映射、短值精确布局、`f/m` 后缀和 65 字节长值分离。
- 本任务是纯文档分析，按计划不运行 Cargo；完成判定依据是上述源码/调用证据、人工事实复核和任务规定的 11 章结构检查。
