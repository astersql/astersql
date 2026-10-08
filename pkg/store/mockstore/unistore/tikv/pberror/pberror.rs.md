# `pkg/store/mockstore/unistore/tikv/pberror/pberror.rs`

## 文件定位

本文件属于独立 crate `astersql-store-mockstore-unistore-tikv-pberror`，crate 入口是同目录的 `lib.rs`。它位于内嵌 mock TiKV（Unistore）的错误适配层：把 `kvproto::errorpb::Error` 包装成 Rust 错误类型，并生成与 Go protobuf compact text 风格一致的字符串。`lib.rs` 公开再导出 `kvproto::errorpb` 和本文件的全部公开项；根 `pkg/lib.rs` 又在 `store::mockstore::unistore::tikv::pberror` 路径下再导出该 crate。

`Cargo.toml` 只依赖固定 tag `v0.0.2-aster.20260929` 的 `kvproto`（启用 `protobuf-codec`）和精确版本 `protobuf = 2.8.0`。父 crate `pkg/store/mockstore/unistore/tikv/Cargo.toml` 仅在 Windows 目标依赖此 crate；仓库级 facade 则无条件声明它。仓库搜索未发现测试之外直接构造或调用 `PBError` 的 Rust 生产代码，因此当前事实是“公共适配类型和 facade 已存在，行为由独立测试覆盖”，不能据此断言它已进入 mock TiKV 的生产请求错误主链。

## 核心职责

1. `PBError` 保存一个可空的 `errorpb::Error`，提供 Go 风格的 `Error()`，并实现 Rust 的 `Display` 与 `std::error::Error`。
2. `compact_text_string` 按 protobuf 字段顺序展开 `errorpb::Error` 的 Region 级错误分支，包括 leader/epoch、Store 不匹配、繁忙、磁盘满、flashback、witness、bucket version 等信息。
3. 一组私有字段写入器统一实现 Go compact text 的省略、转义、重复字段和嵌套消息规则，避免依赖 Rust protobuf 的另一套显示格式。

该文件只负责错误对象的文本适配，不负责创建 Region 错误、判断重试策略、发送 RPC 或记录日志。

## 主要符号

- `pub struct PBError { pub RequestErr: Option<errorpb::Error> }`：唯一公开数据类型。`Option` 对应 Go 指针可能为 `nil`；字段保持 Go 名 `RequestErr`，文件级 lint 允许非 snake case。
- `PBError::Error(&self) -> String`：唯一公开固有方法。`Some` 调用 `compact_text_string`，`None` 返回字面量 `"<nil>"`。
- `impl Display for PBError`：将格式化操作委托给 `Error()`；`to_string()` 因而与 Go `error.Error()` 一致。
- `impl std::error::Error for PBError`：无额外 `source`，仅提供标准错误 trait 身份。
- `compact_text_string(&errorpb::Error) -> String`：私有总调度器，按 `errorpb.Error` 的字段顺序写入顶层 `message` 和各可选嵌套错误。
- `optional_message` / `message`：前者仅为 `Some` 写入，后者生成 `name:<...> ` 结构；闭包负责具体字段。
- `string_field` / `bytes_field`：空值省略，非空值使用 `protobuf::text_format::quote_escape_bytes` 引号与转义。
- `uint64_field` / `uint32_field` / `bool_field`：分别省略默认值 `0`、`0`、`false`；`uint64_repeated_field` 用于 repeated 数值，连零值也写出。
- `peer_fields`：写 `metapb::Peer` 的 `id`、`store_id`、非默认 `role` 和 `is_witness`。
- `region_fields`：写 `metapb::Region` 的标识、键范围、epoch、重复 peers、加密元数据和 flashback 状态。

文件没有模块级常量、trait 定义、异步函数或条件编译项；条件编译仅存在于 `lib.rs` 的测试模块声明中。

## 执行流程

典型路径为 `PBError` 格式化 → `Display::fmt` → `PBError::Error` → `compact_text_string`：

1. `Error()` 先区分 `RequestErr`。`None` 立即返回 `"<nil>"`；`Some(errorpb::Error)` 进入序列化。
2. `compact_text_string` 创建局部 `String`，先处理顶层非空 `message`，再按代码中固定顺序检查每个 `Option` 错误分支。
3. `optional_message` 跳过不存在的分支；存在时由 `message` 写前缀和尖括号，再调用对应闭包写字段，最后追加 `"> "`。
4. `epoch_not_match.current_regions`、`disk_full.store_id`、`bucket_version_not_match.keys`、`Region.peers` 等 repeated 字段按输入顺序逐项追加，不排序也不去重。
5. 标量字段遵守 protobuf 默认值省略；字符串和字节先转义。空消息分支（例如 `stale_command`）仍输出空的消息块。
6. 完成所有分支后返回拥有所有权的 `String`；`Display::fmt` 再把它写入调用方 formatter。

已显式覆盖的顶层分支为：`not_leader`、`region_not_found`、`key_not_in_region`、`epoch_not_match`、`server_is_busy`、`stale_command`、`store_not_match`、`raft_entry_too_large`、`max_timestamp_not_synced`、`read_index_not_ready`、`proposal_in_merging_mode`、`data_is_not_ready`、`region_not_initialized`、`disk_full`、`recovery_in_progress`、`flashback_in_progress`、`flashback_not_prepared`、`is_witness`、`mismatch_peer_id`、`bucket_version_not_match` 和 `undetermined_result`。

## 数据与状态

`PBError` 唯一持久状态是公开字段 `RequestErr`，类型为拥有所有权的 `Option<errorpb::Error>`。派生的 `Clone`、`Debug`、`Default` 意味着它可复制底层 protobuf 值、用于调试，并且默认状态为 `RequestErr: None`。本文件不缓存生成结果；每次调用 `Error()` 都重新分配并构造字符串。

序列化过程只有函数栈上的 `String` 可变，没有全局状态。输出顺序由 `compact_text_string` 和各字段函数的调用顺序决定；输入 repeated 集合的顺序被保留。`Some(errorpb::Error::new())` 与 `None` 有意不同：前者没有已设置字段，输出空字符串；后者模拟 nil 指针文本，输出 `"<nil>"`。

## 依赖与调用关系

上游装配关系为：

- `pberror/lib.rs` 声明 `pub mod pberror`，并以 `pub use pberror::*` 导出 `PBError`。
- 仓库根 `Cargo.toml` 以 `facade_store_mockstore_unistore_tikv_pberror` 指向该 crate；`pkg/lib.rs` 将其再导出到对应模块路径。
- 父 `tikv/Cargo.toml` 在 `cfg(target_os = "windows")` 依赖该 crate，但父 `tikv/lib.rs` 没有声明本目录为内嵌模块。
- 全仓 Rust 搜索只在本文件、`pberror_test.rs`、`migration_aster_unit_test.rs` 和 facade 再导出位置命中 `PBError`；没有证据表明其他生产函数目前构造它。

内部调用边经 RustCodeGraph 核对：`PBError::Error -> compact_text_string`；`Display::fmt -> PBError::Error`；`compact_text_string` 调用 `optional_message`、`message`、各标量写入器，并引用 `peer_fields`、`region_fields`；`region_fields` 又调用 `peer_fields` 及相同写入器。图中把若干同名 `Error` 的 import 误关联为调用方，因此生产调用结论以精确符号节点和全仓 `rg` 交叉验证为准。

下游库依赖为 `kvproto::errorpb`、`kvproto::metapb::{Peer, Region, PeerRole}`、`protobuf::ProtobufEnum`（枚举描述名）和 `protobuf::text_format::quote_escape_bytes`（文本转义），以及标准库 `fmt::Write`。

## 错误处理与边界

- 写入目标始终是内存 `String`。所有 `write!` 结果使用 `expect("writing to String cannot fail")`；对 `String` 的 `fmt::Write` 按标准库契约不会因 I/O 失败，因此这里不是可恢复外部错误路径。
- `RequestErr: None` 返回 `"<nil>"`，而空 protobuf 返回 `""`；调用方若依赖文本必须保留两者差异。
- 默认标量被省略，但 repeated `u64` 中的零仍输出；修改通用写入器会同时改变多个错误分支的兼容文本。
- 字符串和原始字节共用 protobuf quote escaping，二进制 key 不做 UTF-8 解码。
- `PeerRole::Voter` 作为默认值省略；其他角色通过 protobuf descriptor 名称输出，而不是 Rust `Debug` 名称。
- 本实现手工枚举 `errorpb::Error` 字段。若上游 `kvproto` 新增字段而此函数未同步，编译仍可能成功，但新字段不会出现在错误文本中，这是最重要的演进边界。
- `PBError` 没有自定义 `source()`，不能从标准错误链继续取得内部 protobuf；protobuf 值本身也不是 Rust error。

## 并发与资源生命周期

本文件没有锁、原子量、channel、线程、异步任务、事务或外部资源。`Error()` 只借用 `&self`，读取 protobuf 并创建独立 `String`，不会修改 `PBError`；并发可用性取决于生成的 `errorpb::Error` 是否满足调用场景所需的 `Sync`/`Send`，本文件没有额外的同步保证或限制。

临时字符串在调用中创建，并随返回值移交给调用方；闭包只在嵌套消息写入期间同步执行，不逃逸。没有需要显式关闭或回收的资源，主要成本是一次输出字符串分配、字段转义和嵌套/重复字段的线性遍历。

## 与 Go 版本的对应关系

同目录 `pberror.go` 定义 `type PBError struct { RequestErr *errorpb.Error }`，其 `Error()` 直接返回 `re.RequestErr.String()`。Rust 版本保留类型名、字段名和方法名，并以 `Option<errorpb::Error>` 表达 Go 指针的可空性；同时额外实现 `Display` 和 `std::error::Error`，便于进入 Rust 错误生态。

关键差异是 Go 依赖生成 protobuf 类型自身的 `String()`，Rust 则在 `compact_text_string` 中手工复刻 compact text。这个差异解释了大量私有写入器，也意味着升级 `kvproto` 时必须人工检查字段覆盖与字段名大小写。当前测试特别确认 Go 字段名中的例外：`RecoveryInProgress`、`FlashbackInProgress`、`FlashbackNotPrepared` 保持大写开头，而其他字段如 `server_is_busy` 使用 snake case。

Go 中对 nil `RequestErr` 调用生成消息的 `String()` 会得到 `"<nil>"`；Rust 通过显式 `None` 分支复现。`migration_aster_unit_test.rs` 同时固定普通 message、嵌套 `ServerIsBusy`、空消息、nil 和标准 Error trait；`pberror_test.rs` 固定三个 flashback 相关字段名。

## 扩展指南

新增或更新 `errorpb::Error` 字段时，应在 `compact_text_string` 中按 Go 生成代码的字段顺序和文本字段名增加分支，并复用合适的标量/嵌套写入器。若新消息嵌套 `Peer` 或 `Region`，优先扩展 `peer_fields` / `region_fields`，但先确认变更不会改变所有既有调用点的文本。新增 protobuf 标量种类时再增加专用私有写入器，并分别明确默认值省略与 repeated 零值行为。

测试逻辑必须保持在独立文件中，不应嵌回 `pberror.rs`。一般语义对齐扩展 `migration_aster_unit_test.rs`；字段名或特定 Region 错误回归可扩展 `pberror_test.rs`。至少覆盖：默认值与非默认值、字符串/字节转义、`None` 与空消息、repeated 顺序及零值、嵌套 Region/Peer，以及 Go 实际输出的精确字符串。

兼容风险主要是错误文本被日志、断言或上层分类逻辑依赖；不要随意改变字段顺序、空格、尖括号、大小写或默认值省略规则。性能上避免为单字段引入多次中间字符串；当前实现向同一缓冲区线性追加。若要把 `PBError` 接入新的生产路径，应先确认 crate 的目标条件与 facade 导入方式，并新增独立调用路径测试，而不能把“可再导出”视为“已经接线”。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标目录的 `pberror.rs`、`lib.rs`、两份 Rust 测试和 Go 对照均在索引中。
- RustCodeGraph 精确节点：`pberror.rs::PBError`、`pberror.rs::Error`、`pberror.rs::compact_text_string`、`pberror.rs::peer_fields`、`pberror.rs::region_fields`。关键边为 `Error -> compact_text_string`、`compact_text_string ->` 字段写入器/嵌套序列化器、`Display::fmt -> Error`。
- 源码：`pkg/store/mockstore/unistore/tikv/pberror/pberror.rs`（完整 355 行）与 crate 入口 `pkg/store/mockstore/unistore/tikv/pberror/lib.rs`。
- crate/装配：`pkg/store/mockstore/unistore/tikv/pberror/Cargo.toml`、`pkg/store/mockstore/unistore/tikv/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/store/mockstore/unistore/tikv/pberror/pberror.go`。
- 独立 Rust 测试：`pkg/store/mockstore/unistore/tikv/pberror/migration_aster_unit_test.rs`、`pkg/store/mockstore/unistore/tikv/pberror/pberror_test.rs`；同目录不存在 `pberror_test.go`。
- 全仓精确搜索确认 `PBError` 的非测试生产定义与 facade 再导出，但未发现其他生产 Rust 构造点；因此本文未宣称存在尚未验证的运行时调用链。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅检查文件存在及固定十一个二级标题，并人工复核上述源码事实。
