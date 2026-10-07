# `br/pkg/utils/key.rs`

## 文件定位

[`key.rs`](key.rs) 是 `astersql-br-pkg-utils` crate 中的键工具模块，由 [`lib.rs`](lib.rs) 通过 `#[path = "key.rs"] pub mod key` 挂载。它移植自同目录的 [`key.go`](key.go)，集中提供用户键解析、半开键区间比较与求交、时间显示、TiDB meta 键识别，以及备份元数据所需的十六进制编解码。

当前 Rust 接线需要与 Go 包级可见性区别看待：crate 根没有 `pub use key::*`，因此外部 crate 必须经 `astersql_br_pkg_utils::key::...` 访问。仓库内已确认的生产调用是 [`json.rs`](json.rs) 通过 `super::key::{hex_decode_string, hex_encode}` 读写备份 JSON 中的 `sha256`、`start_key` 和 `end_key`；其余多数公开函数目前主要由 [`key_test.rs`](key_test.rs) 验证。部分 BR 子 crate 仍在自己的 `stubs.rs` 中保留同名实现，不能据 Go 调用关系推断它们已经接到本文件。

## 核心职责

- `ParseKey` 将 CLI 风格的 `raw`、`escaped`、`hex` 文本统一转换为原始字节；未知格式保留 Go 的 `ErrInvalidArgument` 错误类别。
- `CompareEndKey`、`CompareBytesExt`、`clamp_in_one_range` 和 `IntersectAll` 实现备份半开区间 `[start, end)` 的比较与两路有序区间集合求交，其中空 `end` 表示正无穷。
- `FormatDate` 把 Unix 纳秒和显式 UTC 偏移转换为 Go `DateFormat` 对应的显示字符串，不读取系统时区数据库。
- `IsMetaDBKey`、`IsMetaDDLJobHistoryKey`、`IsDBOrDDLJobHistoryKey` 按字节前缀识别 TiDB meta 键。
- `EncodeTxnMetaKey` 按事务 meta 键格式组合 meta 编码、字节可比较编码和降序时间戳；`IsMetaAutoIDKey` 反向解码并识别 IID/TID/TARID/SID 字段。
- `hex_decode_string` 与 `hex_encode` 为备份 JSON 和 `ParseKey("hex", ...)` 提供严格解码与小写编码。

## 主要符号

- `pub const DateFormat: &str`：Go 时间布局的文档性对应常量。Rust 的 `FormatDate` 自行拼装结果，并不把该常量交给格式化库。
- `pub fn ParseKey(format: &str, key: &str) -> Result<Vec<u8>, SharedError>`：公开解析入口。`raw` 直接复制 UTF-8 字符串的底层字节，`escaped` 调用私有 `unescaped_key`，`hex` 调用公开的 `hex_decode_string`。
- `fn unescaped_key`、`fn scan_u8_prefix`、`IoEof`：实现 Go `fmt.Sscanf` 风格的转义扫描。支持 `\a\b\f\n\r\t\v\\\'\"`、`\x` 后最多两位以及最多三位八进制；孤立反斜杠以显示为 `EOF` 的本地错误返回。
- `pub fn CompareEndKey`：仅适用于两个排他上界；任一空上界都按正无穷处理。
- `pub fn CompareBytesExt`：分别由两个布尔参数决定对应空切片是否为正无穷；未启用时按普通字节字典序比较。
- `FailedToClampReason` 与 `fn clamp_in_one_range`：内部裁剪状态机。结果分为成功、原区间完全偏左、完全偏右和理论不可达四类。
- `pub fn IntersectAll(mut s1: Vec<KeyRange>, s2: Vec<KeyRange>) -> Vec<KeyRange>`：以双指针求两个已排序且各自无重叠的区间序列的交集。它拥有输入 `Vec`，并可能修改本地 `s1` 元素的 `StartKey` 以继续消费尚未结束的区间。
- `pub fn FormatDate(unix_nanos: i128, offset_seconds: i32) -> String` 与 `fn unix_secs_to_utc`：先应用固定偏移，再按预推格里高利历拆出日期和时间；负时间使用欧几里得除法，纳秒小数去掉尾零。
- 三个 `IsMeta*` 前缀函数：分别检查 `mDB`、`mDDLJobH`、较宽的 `mD`。
- `pub fn EncodeTxnMetaKey`：执行 `EncodeMetaKey(key, field)` → `EncodeBytes` → `EncodeUintDesc(ts)`，顺序构成兼容格式的一部分。
- `pub fn IsMetaAutoIDKey`：去掉末尾 8 字节时间戳，依次 `DecodeBytes`、`DecodeMetaKey`，最后调用 `astersql_meta` 的四个字段判定函数。
- `pub fn hex_decode_string`、`fn hex_nibble`、`pub fn hex_encode`、`fn hex_char`：偶数长度十六进制编解码；编码固定输出小写。

## 执行流程

1. 用户键解析从 `ParseKey` 进入。`raw` 不解释内容；`hex` 要求偶数长度且每个字符合法；`escaped` 逐字节扫描，普通字符直接输出，反斜杠序列按 Go 规则转换。对于 `\x1z` 这类输入，扫描器消费两个字符但接受合法数字前缀，因此输出 `0x01`；`\x` 没有可解析数字时沿用先前读到的反斜杠值，这是为匹配 Go 实现而保留的非直观行为。
2. 单区间裁剪先把待裁区间的起点抬到 `clamp_in.StartKey`，再把终点压到 `clamp_in.EndKey`。起点空值按普通最小字节串处理，终点空值按正无穷处理。裁剪后若 `start >= end`，返回默认空 `KeyRange` 及能够指导指针推进的失败原因。
3. `IntersectAll` 同时维护 `s1` 与 `s2` 下标。成功时输出交集：若当前 `s1` 已结束则推进 `s1`，否则把其起点移到当前裁剪区间的终点继续处理；完全偏左时推进 `s1`，完全偏右时推进 `s2`。`BuggyUnknown` 只记录两侧区间和当前位置，不产生结果。
4. `FormatDate` 将时区偏移换算为纳秒并加到绝对时刻，拆成秒与纳秒后调用 `unix_secs_to_utc`。整秒不输出小数点；非整秒输出九位纳秒再删除尾部零；偏移单独生成 `+/-HHMM`。
5. 事务 meta 键由 `EncodeTxnMetaKey` 生成。识别时，`IsMetaAutoIDKey` 先确认至少存在 8 字节时间戳，再解开可比较字节层和 meta 层，最后仅接受 auto-increment、auto-table-id、auto-random 或 sequence 字段。
6. JSON 路径中，`json.rs::insert_hex` 调用 `hex_encode` 写出字段，`from_json_file` 和 `from_json_raw_range` 调用 `hex_decode_string` 恢复二进制字段；解码错误经 `trace_err` 上抛。

## 数据与状态

本文件没有全局可变状态、缓存、锁或环境依赖。所有输出由参数决定，公开函数可重入。

区间数据使用 `crate::stubs::KeyRange`，其 `StartKey`/`EndKey` 是键字节包装类型。关键不变量是半开区间必须满足 `start < end`；空起点仍是普通最小字节串，而空终点在明确启用 `*_empty_as_inf` 时代表全键空间的正无穷上界。`IntersectAll` 的正确性还依赖两侧输入都按起点排序、各自内部无重叠；函数不会自行排序或折叠输入。

`EncodeTxnMetaKey` 的结果由 meta key/field、memcomparable 字节编码和 8 字节降序时间戳组成。`IsMetaAutoIDKey` 只检查键的字段类型，不返回其中的数据库、表或时间戳。十六进制编码每个输入字节生成两个 ASCII 字符，因此输出长度恒为输入的两倍。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-br-pkg-utils`，库入口是 `lib.rs`。本文件的直接依赖为：

- `crate::stubs::{KeyRange, TableKey, EncodeMetaKey, DecodeMetaKey}`：迁移期的范围类型与 meta 编解码边界。
- `astersql-util-codec::{EncodeBytes, DecodeBytes, EncodeUintDesc}`：可比较字节编码与降序时间戳编码。
- `astersql-meta` 的 `is_auto_increment_id_key`、`is_auto_table_id_key`、`is_auto_random_table_id_key`、`is_sequence_key`：AutoID/Sequence 字段分类。
- `astersql-errors::{SharedError, Annotate, Trace}` 与 `astersql-br-pkg-errors::ErrInvalidArgument`：统一错误载体和错误语境。
- `astersql-br-pkg-logutil::{log, StringifyKeys, StringifyRange}`：仅用于 `IntersectAll` 理论不可达分支的诊断日志。

RustCodeGraph 将本文件标为被 `br/pkg/utils/json.rs`、`br/pkg/utils/key_test.rs` 等文件使用；结合源码导入可确认的生产边是 `json.rs` → `hex_encode`/`hex_decode_string`。图中对 `hex_encode`、`CompareBytesExt` 等常见名称还返回了没有本模块导入路径的跨 crate 同名调用，属于名称消歧不充分，未作为真实接线依据。`EncodeTxnMetaKey` 的图调用者主要是其他子 crate 的测试辅助，但这些子 crate 常经自己的 `stubs::utils` 导入，不能一概认定调用本实现。

## 错误处理与边界

- `ParseKey` 的未知格式通过 `Annotate(ErrInvalidArgument, "unknown format")` 返回；`raw` 永不因内容失败。
- `unescaped_key` 的普通读取错误包装为 `SharedError`；末尾孤立反斜杠返回 `IoEof`。八进制序列完全无合法前缀时返回 `InvalidInput("invalid syntax")`。十六进制转义刻意保留 Go 忽略扫描错误的行为，不能“顺手严格化”。
- `hex_decode_string` 拒绝奇数长度和非十六进制字符，分别报告 `invalid hex length` 与 `invalid hex digit`；接受大小写输入。
- `IsMetaAutoIDKey` 将过短键、任一解码失败或非目标字段统一视为 `false`，适合作为过滤谓词，但调用方无法从返回值区分损坏键和正常非 AutoID 键。
- `CompareEndKey` 只适合比较排他终点；若将起点传入，空键会被错误地视为最大值。更一般的场景应使用 `CompareBytesExt` 并明确两个空值标志。
- `IntersectAll` 不校验排序、无重叠和区间合法性。违反前置条件时结果没有集合语义保证；空输入自然得到空结果。
- `FormatDate` 接受任意 `i128` 纳秒和 `i32` 秒偏移，不检查现实世界时区范围，也不包含夏令时规则。它表达固定数值偏移，不等价于命名时区转换。

## 并发与资源生命周期

模块没有异步任务、线程、通道、锁、文件句柄或网络资源；函数仅分配并返回拥有所有权的 `Vec`/`String`。`ParseKey` 的切片读取器只借用输入调用期间；`IntersectAll` 获取两个区间向量的所有权，内部克隆当前区间与日志快照，返回后不保留引用。

主要资源风险是分配与复制：`ParseKey`、十六进制编解码和 `FormatDate` 分配新缓冲区；`IntersectAll` 预留 `s1.len()` 容量，并在循环中克隆 `KeyRange`。不可达分支还会克隆两组完整区间用于日志格式化。扩展热路径时应保持无共享状态，同时评估长键和大区间列表的复制成本。

## 与 Go 版本的对应关系

[`key.go`](key.go) 是逐项语义基准：Rust 保留了 `ParseKey` 三格式、转义扫描、空 end 为正无穷、`IntersectAll` 双指针裁剪、三个 meta 前缀、事务 meta 键编码次序和 AutoID/Sequence 判定。[`key_test.go`](key_test.go) 的 ParseKey、CompareEndKey、区间求交、日期和前缀用例在 [`key_test.rs`](key_test.rs) 中有对应覆盖。

已确认的语言适配差异如下：

- Go `FormatDate(time.Time)` 依赖传入值已有 location；Rust 接受绝对 Unix 纳秒与固定 `offset_seconds`，自行计算墙钟。固定偏移输出与 Go 对齐，但 Rust 不支持命名时区或夏令时跃迁。
- Go `IntersectAll` 原地改变传入的 `s1` slice 元素；Rust 获取 `Vec` 所有权后只改变本地副本，调用方原数据若在传参前克隆则保持不变，键字节也不会通过共享切片被修改。
- Go `hex.DecodeString` 的具体错误文本来自标准库；Rust 以 `SharedError` 返回本地 `InvalidInput` 文本。成功字节语义一致，但错误字符串不应视为完全相同。
- Go 的 `log.L().DPanic` 在开发配置下可能 panic；Rust `log::Warn` 只记录告警。因此 `BuggyUnknown` 的诊断严重度不同，但正常可达算法分支一致。
- Rust 新增 `test_parse_key_matches_go_scan_prefix_semantics` 和 Unix epoch 前日期用例，用来固定 Go 标准库行为；当前 Rust 测试明确没有直接覆盖 `EncodeTxnMetaKey`/`IsMetaAutoIDKey`。

## 扩展指南

- 新增键文本格式时修改 `ParseKey` 的分派，并在独立的 [`key_test.rs`](key_test.rs) 增加成功、空输入、非法输入和错误类别用例；若 Go 同路径仍是兼容基准，应同步 `key.go`/`key_test.go` 或明确记录有意差异。
- 修改转义规则时同时覆盖不完整输入、合法数字前缀、超范围值和孤立反斜杠。不要把 Go `Sscanf` 的宽松前缀行为无意改成全字段严格解析。
- 修改区间算法时优先扩展 `test_clamp_key_ranges`，至少覆盖空 start、空 end、相邻区间、跨多个区间、完全不相交和正反参数。若需要接受未排序或重叠输入，应在进入 `IntersectAll` 前显式排序/折叠，而不是静默改变当前线性算法的前置条件。
- 修改 meta 编码时必须保持 `EncodeMetaKey` → `EncodeBytes` → `EncodeUintDesc` 的顺序，并新增 `EncodeTxnMetaKey` 与 `IsMetaAutoIDKey` 的直接往返、短键、损坏编码以及 IID/TID/TARID/SID/非 AutoID 字段测试。还要核对使用局部 stub 的 `br/pkg/stream`、`br/pkg/restore/log_client` 是否需要同步，避免同名实现漂移。
- 修改日期行为时补充负时间、闰日、世纪年、纳秒去尾零、负偏移和非整小时偏移；若需求涉及命名时区，应引入明确的时区语义，而不是继续扩张固定偏移函数。
- 调整公开路径前先决定是继续使用 `astersql_br_pkg_utils::key::X`，还是在 `lib.rs` 增加再导出；后者会扩大 crate 根 API，并可能与现有同名再导出冲突。
- 性能优化应以长键、大 JSON 元数据和大区间集为基准，重点检查 `KeyRange` 克隆和不可达日志的全量格式化，不得牺牲空终点的正无穷语义。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标 `br/pkg/utils/key.rs` 已索引为 35 个符号。通过 `node --file` 阅读全文件，并对 `ParseKey`、`CompareEndKey`、`CompareBytesExt`、`IntersectAll`、`FormatDate`、三个前缀函数、`EncodeTxnMetaKey`、`IsMetaAutoIDKey`、`hex_decode_string`、`hex_encode` 执行了 `callers`/`callees` 查询。
- 实现与 crate 证据：[`key.rs`](key.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`json.rs`](json.rs)。
- Go 对照证据：[`key.go`](key.go)、[`key_test.go`](key_test.go)。
- Rust 测试证据：[`key_test.rs`](key_test.rs)；它覆盖三类解析及错误、Go 扫描前缀、空 end 比较、双向区间求交、固定 `+0800` 日期、epoch 前日期和 meta 前缀，同时明确未直接覆盖事务 meta 键的编码/识别。
- 接线核对：全仓库检索了 `astersql-br-pkg-utils` 依赖、`key::` 导入及所有主要符号；确认 `json.rs` 的直接生产边，并确认高层 `backup_raw.rs`、`stream` 等仍存在局部 stub 同名实现，未把这些调用错误归到本文件。
- 本任务是纯文档分析，未运行 Cargo。结构验收以任务规定的 11 个固定二级标题检查为准。
