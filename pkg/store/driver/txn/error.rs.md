# `pkg/store/driver/txn/error.rs`

## 文件定位

本文件是 `astersql-store-driver-txn` crate 的事务错误边界与键格式化模块。crate 入口 `pkg/store/driver/txn/lib.rs` 以私有 `mod error` 装配它，再通过 `pub use error::*` 将公开项重导出给事务驱动及上层存储适配器。`pkg/store/driver/txn/Cargo.toml` 表明它直接依赖 canonical `astersql-kv` 错误定义、`astersql-tablecodec` 编解码能力、`astersql-errors` 错误模板，以及带固定 tag 的 `tikv-client`。

它位于 TiKV/client-rust 后端错误与 SQL/KV 层可见错误之间：一方面定义本 crate 内部统一使用的 `DriverError`，另一方面把重复键、写冲突、共享锁丢失、锁升级冲突和 `TxnLockNotFound` 中的原始键转换为稳定、可读且尽量与 Go `pkg/store/driver/txn/error.go` 一致的形式。真正发起事务操作、提取表元数据和接收 client-rust 错误的主流程在相邻的 `txn_driver.rs`，本文件不执行网络请求或事务提交。

## 核心职责

- 用 `DriverError` 统一表示缺失、重复键、写冲突、锁升级、可重试、非法选项、未实现和后端透传等错误；`Display` 决定未被 canonical 错误模板接管时的用户可见文本。
- 用 `ExtractKeyExistsErrFromHandle` 和 `ExtractKeyExistsErrFromIndex` 从 TiDB record/index key、行值以及 `TableInfo` 中恢复重复键的约束名和值，生成 SQL 风格的 `Duplicate entry ... for key ...`。
- 用 `extractKeyErr` 对少数特殊事务错误做语义归一：共享锁丢失映射到 `kv::ErrSharedLockLost`，第二个共享锁升级者映射为不可重试死锁，写冲突映射到 canonical `kv::ErrWriteConflict`，可重试错误附加锁键详情。
- 用 `newWriteConflictError`、`prettyWriteKey` 和 `prettyLockNotFoundKey` 将 table/index/record/meta key 解码成人可读片段，同时保留原始十六进制键，供诊断、日志脱敏和上层错误分类使用。
- 用 `decode_table_key_head` 为 `txn_driver.rs` 的重复键分派提供 `(table_id, index_id, is_record)`，决定调用 handle 还是 index 解码路径。

## 主要符号

- `pub enum DriverError`：crate 的统一错误枚举，派生 `Clone/Debug/Eq/PartialEq` 并实现 `std::error::Error`。`NotFound` 与 `ClientNotExist` 是两类缺失哨兵；`KeyExists` 已含 SQL 可读的值和约束名；`BackendKeyExists` 保留后端原始 key/value 以便稍后结合表元数据解码；`WriteConflict(WriteConflict)` 保留结构化冲突；`SharedLockLost`、`LockUpgradeConflict`、`Deadlock` 保存锁语义；其余变体覆盖可重试消息、非法选项、未实现、提交器工作中与字符串后端错误。
- `DriverError::is_not_found(&self) -> bool`：只把 `NotFound` 和 `ClientNotExist` 归为缺失。`batch_getter.rs` 依靠这一分类兼容本地缓冲与 client-go 风格的不存在语义。
- `impl Display for DriverError`：为每个变体给出稳定文本。需要错误码、重试标记或脱敏的特殊错误通常先经 `extractKeyErr`/`newWriteConflictError` 转成 canonical 模板；直接显示 `WriteConflict` 只使用结构体中预存的美化字段。
- `pub struct WriteConflict`：保存本事务、冲突事务及冲突提交时间戳，冲突 key/primary、reason，以及四个预格式化片段。`txn_driver.rs::map_client_error` 从 client-rust protobuf 冲突填充原始字段，`generate_write_conflict_for_locked_with_conflict` 还会填充展示字段。
- `ExtractKeyExistsErrFromHandle(key, value, table) -> DriverError`：处理主键/record key。整数 handle 直接输出（无符号主键按 `u64` 重解释）；复合 handle 则结合 primary index、列类型、行值和 handle datum 解码各列，应用前缀长度及 binary/bit 转义。
- `ExtractKeyExistsErrFromIndex(key, value, table, index_id) -> DriverError`：查找目标索引，建立 rowcodec 列信息，解码 index KV 和 datum，再用 `-` 拼接索引列值。找不到索引或任何解码失败时返回保守的 `UNKNOWN`/原始 key 表示。
- `extractKeyErr(Option<DriverError>) -> Result<(), DriverError>`：`None` 成功；特殊变体转换后返回 `Err`；其他错误原样传播。名称沿用 Go 版本，受 crate 级 `non_snake_case` 允许项保护。
- `newWriteConflictError(Option<WriteConflict>) -> errors::SharedError`：无详情时克隆 canonical `kv::ErrWriteConflict`；有详情时重新解码 key/primary、追加原始十六进制，并通过 `FastGenByArgs` 生成带 `[kv:9007]`、重试标记与脱敏行为的共享错误。
- `prettyWriteKey(&[u8]) -> (String, String)`：按 index key、record key、meta key 的顺序尝试解码；均失败时返回原始字节的 Debug 表示。返回值被拆成 table-id 前缀和剩余片段，是为了匹配 canonical 错误模板的参数布局。
- `prettyLockNotFoundKey(&str) -> String`：仅处理包含 `TxnLockNotFound` 的字符串，提取第一对 `[...]` 内的十进制 `u8` 列表并调用 `prettyWriteKey`；格式不完整、含越界或非数字项时返回空字符串。
- `pub(crate) decode_table_key_head(&[u8])`：`tablecodec::DecodeKeyHead` 的薄适配，只在 crate 内公开，将解码失败压成 `None`。

## 执行流程

重复主键流程从 `txn_driver.rs::extract_key_error` 开始：后端 `BackendKeyExists` 被交给 `extract_key_exists_error`；后者先通过 `decode_table_key_head` 找 table/index 类型，再从事务保存的表信息查出 `TableInfo` 和冲突值。record key 进入 `ExtractKeyExistsErrFromHandle`：先 `DecodeRecordKey`，整数 handle 直接格式化；复合 handle 查找 primary index，构造列 ID 到字段类型的映射，依次执行 `DecodeRowToDatumMap`、`DecodeHandleToDatumMap` 和 datum `ToString`，最终生成 `表名.PRIMARY` 错误。任一前置条件或解码步骤失败，都降级为仍可返回给调用者的重复键错误，不把诊断失败覆盖成新的内部错误。

唯一索引流程同样由 `extract_key_exists_error` 分派，但调用 `ExtractKeyExistsErrFromIndex`：按 `index_id` 找 `IndexInfo`，由 `rowcodec_columns` 校验每个索引列 offset 并建立 `ColInfo`，再由 `DecodeIndexKV` 拆出编码列、`DecodeColumnValue` 恢复 datum。每列完成字符串化及 binary/bit 转义后用 `-` 连接，错误名为 `表名.索引名`。

client-rust 写冲突首先在 `txn_driver.rs::map_client_error` 中从单个、聚合或悲观锁嵌套错误里递归找到 protobuf `WriteConflict`，转换成本文件的 `WriteConflict`，再调用 `newWriteConflictError`。该函数分别美化 conflict key 与 primary，追加 `originalKey`/`originalPrimaryKey`，最后把 8 个参数交给 `kv::ErrWriteConflict.FastGenByArgs`；因此错误码、重试标记与脱敏由 canonical 错误模板统一实施。事务内部生成的 `DriverError::WriteConflict` 也会在 `txn_driver.rs::extract_key_error` 走同一路径。

`extractKeyErr` 的分支顺序体现错误分类优先级：无错误立即成功；`SharedLockLost` 用大写十六进制 key 生成专用 `[tikv:9015]` 错误；`LockUpgradeConflict` 变成 `retryable: false` 的 `Deadlock`，使上层中止第二个升级者并释放共享锁；`WriteConflict` 进入 canonical 写冲突模板；`Retryable` 尝试从文本恢复锁键并保留 Go 行为中的分隔空格；其他变体不改变。

## 数据与状态

本文件没有全局可变状态。输入数据以借用的 key/value、不可变 `TableInfo` 或按值传入的 `DriverError`/`WriteConflict` 表示；输出是新分配的字符串、向量或错误值。`Key` 是 crate 在 `lib.rs` 中定义的 `Vec<u8>` 别名。

`WriteConflict` 同时保存原始 key 与展示字段。`newWriteConflictError` 以原始 `key`/`primary` 重新计算展示内容，不信任结构体中已有的 `key_table_id` 等字段；而 `DriverError` 自身的 `Display` 会直接读取这四个字段。因此构造该结构体时必须知道后续走哪条显示路径，避免展示字段为空导致文本不完整。

重复键解码临时建立 `HashMap<column_id, FieldType>`、handle 列 ID 列表和渲染结果向量。前缀索引截断以字节长度为上限，但 Rust 会向前退到 UTF-8 字符边界，避免产生非法字符串；binary/bit 值把 ASCII 可打印范围外的字节写成大写 `\xNN`。所有 datum 时间解释当前固定使用 `tablecodec::time::UTC`。

## 依赖与调用关系

上游主要在相邻 `pkg/store/driver/txn/txn_driver.rs`：`map_client_error` 调用 `newWriteConflictError`；`tikvTxn::extract_key_error` 处理 `BackendKeyExists`/`WriteConflict`；`extract_key_exists_error` 调用 `decode_table_key_head` 以及两个重复键入口；`generate_write_conflict_for_locked_with_conflict` 调用 `prettyWriteKey`。`pkg/store/driver/txn/lib.rs` 还让 `DriverError` 成为 `KvIterator`、`Getter`、`BatchGetter` 等公共 trait 的错误类型，`batch_getter.rs`、`snapshot.rs`、`scanner.rs`、`union_iter.rs` 与 `unionstore_driver.rs` 都会构造或传播其变体。

下游依赖分三组：`astersql-tablecodec` 提供 key head、record/index/meta key、row/index KV 和 datum 解码以及 canonical model 类型；`astersql-kv` 提供 `ErrWriteConflict`、`ErrSharedLockLost` 等稳定错误模板；`astersql-errors::ErrorArg` 负责向这些模板传递类型化参数。标准库的 `Display`/`Error` 建立普通 Rust 错误接口，`HashMap` 只服务复合主键解码。

RustCodeGraph 对目标文件报告 38 个符号，并显示文件级使用者包括 `pkg/store/driver/tikv_driver.rs` 与 `pkg/store/helper/*`；精确业务入口则由目标文件及 `txn_driver.rs` 的符号读取确认。Cargo 中没有本文件专属 feature gate，错误模块始终随该 crate 编译；测试由 `lib.rs` 通过独立的 `error_test.rs` 和 `driver_test.rs` 模块接入。

## 错误处理与边界

重复键提取坚持“业务错误优先”：元数据缺失、offset 越界、空 value、key/row/datum 解码失败都不会把原本的重复键替换为内部解码错误，而是通过 `gen_key_exists_error` 返回较少结构化信息。handle 路径通常保留 `表名.PRIMARY`，index 不存在时名称为 `UNKNOWN`。这种降级提升可用性，但 Rust 当前不像 Go 的 `genKeyExistsError` 那样记录具体降级原因，因此排障信息只剩原始 key/value 文本。

`rowcodec_columns` 和所有 offset 转换都返回 `Option`，避免负数或越界索引 panic；UTF-8 前缀截断会回退到字符边界。`prettyLockNotFoundKey` 对文本格式极为保守，只接受十进制字节列表，失败返回空串；`extractKeyErr` 仍会在 retryable 原文后追加一个空格，以保持 Go 输出契约。`prettyWriteKey` 依次尝试三种 key 编码，最终总能返回可显示的 fallback，不抛解码错误。

共享锁升级冲突被明确改写为不可重试死锁，而不是写冲突或事务可重试错误；这是打破多个升级者都持有共享锁的等待环所需的控制流语义。共享锁丢失使用专用 canonical 错误模板。普通 `Backend(String)` 则原样显示，调用者若需要错误码或分类，必须在构造它之前选用对应 canonical 模板。

## 并发与资源生命周期

本文件自身不持锁、不启动任务、不持有通道、网络连接或事务句柄，所有函数均为同步的纯转换或局部分配。`DriverError`、`WriteConflict` 的拥有型字段允许错误跨调用层传递而不借用事务缓冲；是否可跨线程由字段类型自然决定，本文件没有额外 `unsafe` 或手写并发实现。

生命周期上的关键点发生在调用侧：锁升级冲突转换成不可重试死锁后，上层必须终止该事务，事务释放共享锁才会让已等待的升级者继续；本文件只编码该决定，不负责释放。写冲突和重复键转换只读取传入的快照数据，不修改 memBuffer 或表元数据。`driver_test.rs` 会临时修改 `astersql-errors` 的全局脱敏模式并在断言后恢复，这是测试隔离要求，不是本模块运行时状态。

## 与 Go 版本的对应关系

Rust 的同名主要入口逐一对应 `pkg/store/driver/txn/error.go`：两个 `ExtractKeyExistsErr*`、`extractKeyErr`、`newWriteConflictError`、`prettyWriteKey` 和 `prettyLockNotFoundKey` 保留相同职责及大体相同分支。整数/无符号主键、复合主键、二进制值、索引值连接、index/record/meta key 展示、原始十六进制键、共享锁丢失、第二升级者不可重试死锁和 `TxnLockNotFound` 文本解析均有明确 Rust 对照。`pkg/store/driver/txn/driver_test.rs` 还移植了 Go `driver_test.go` 的锁键与写冲突格式断言。

当前可见差异需要在兼容修改时谨慎处理：Go 的重复键 datum 解码使用 `time.Local`，Rust 使用 `tablecodec::time::UTC`；Go 降级会记录原始解码错误，Rust 静默生成保守错误；Go 的 `extractKeyErr` 最终经 `derr.ToTiDBErr` 转换一般后端错误，Rust 对未匹配的 `DriverError` 原样传播；Rust 用 UTF-8 字符边界保护前缀截断，而 Go 直接按字节切字符串。Rust 的 `prettyLockNotFoundKey` 手工解析 `u8`，Go 用 JSON 反序列化，但对当前 `[n, ...]` 输入契约结果一致。

Rust 还显式拥有 `DriverError` 与 `WriteConflict` 适配类型，因为 client-rust/canonical Rust crate 的错误表示不同于 Go interface/error chain；这不是 Go 源文件中的一一类型翻译。`Cargo.toml` 的 porting metadata 指向同一个 Go package，说明该文件应持续以 Go 行为为兼容基准，但当前源码和测试才是已实现事实。

## 扩展指南

新增后端错误类型时，先判断它应保持结构化 `DriverError`、映射到已有 canonical 错误模板，还是仅可安全透传字符串；随后同步更新枚举、`Display`、归一化入口及独立测试。若错误会影响重试/中止决策，必须像 `LockUpgradeConflict -> Deadlock { retryable: false }` 一样保留明确语义，不能只修改文案。

扩展重复键解码时，优先修改 `ExtractKeyExistsErrFromHandle`、`ExtractKeyExistsErrFromIndex` 或 `rowcodec_columns`，同时核对 `txn_driver.rs::extract_key_exists_error` 的 table/index 分派和值来源。要覆盖新的字段类型、prefix 或 handle 形态，应把 Rust 测试放在独立的 `pkg/store/driver/txn/driver_test.rs` 或 `error_test.rs`，并与 Go `error.go`/`driver_test.go` 的预期比较；不要把测试内嵌进生产源文件。

扩展键展示格式时需保持 `prettyWriteKey` 的二元返回契约以及 `kv::ErrWriteConflict.FastGenByArgs` 的参数顺序，否则上层对 `tableID` 的解析、错误码、重试标记或脱敏边界可能改变。任何包含原始 key 的新增输出都必须覆盖关闭、全脱敏和标记脱敏模式。性能方面应避免在热错误路径重复解码或复制大型 row/key；正确性方面应保留所有失败降级，兼容性方面应重点验证稳定错误字符串和 canonical `Equal` 分类。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/store/driver/txn` 确认目标、模块入口、Go 对照及独立测试；`explore "pkg/store/driver/txn/error.rs ..."` 和 `node --file pkg/store/driver/txn/error.rs` 核对 38 个符号、完整 474 行源码及文件级使用者。对关键符号执行了带 `--file pkg/store/driver/txn/error.rs` 的 callers/callees 查询；该 CLI 未返回细粒度边，因此调用关系进一步由索引中的 `txn_driver.rs` 精确源码节点验证，而未据同名全仓结果推断。
- 源码与装配：`pkg/store/driver/txn/error.rs`、`pkg/store/driver/txn/lib.rs`、`pkg/store/driver/txn/txn_driver.rs`；确认重导出、公共 trait 错误类型、client-rust 映射、重复键分派和写冲突生成调用点。
- crate 边界：`pkg/store/driver/txn/Cargo.toml`；确认 crate 名、`lib.rs` 入口、三项本地 canonical 依赖及固定 tag 的 `tikv-client`。
- Rust 独立测试：`pkg/store/driver/txn/error_test.rs` 覆盖 retryable 分隔空格、共享锁丢失和第二升级者死锁；`pkg/store/driver/txn/driver_test.rs` 覆盖锁键格式、index/meta 写冲突与脱敏、无符号主键、canonical index datum 和 client-rust 写冲突错误码/原因。
- Go 对照：`pkg/store/driver/txn/error.go` 与 `pkg/store/driver/txn/driver_test.go`；确认同名流程、fallback、锁错误分类、错误模板和格式断言，并据源码记录 UTC/Local、日志及通用错误转换差异。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 shell 命令验证目标文件存在且恰有 11 个固定二级标题，并人工复查没有修改 Rust、Go、Cargo 或只读 `plan.md`。
