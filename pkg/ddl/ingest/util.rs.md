# `pkg/ddl/ingest/util.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate，由 [`pkg/ddl/ingest/lib.rs`](lib.rs) 以公开模块 `pub mod util` 装配。它对应 Go 文件 [`pkg/ddl/ingest/util.go`](util.go)，目标是在 DDL add-index 的 ingest/快速回填路径遇到“底层发现重复键”错误时，把底层错误提升为携带冲突键值、索引名和表名的 `KeyExists` 错误。

这一转换位于失败语义的边界，不负责 DDL job、schema state、checkpoint 或本地引擎生命周期。完整应用中的 Go 路径会从 `pkg/ddl/backfilling_operators.go`、`pkg/ddl/backfilling_import_cloud.go`、`pkg/ddl/backfilling_merge_sort.go`、`pkg/ddl/index.go` 和 `pkg/ddl/ingest/backend.go` 调用 `ingest.TryConvertToKeyExistsErr`，使不可重试的重复键错误可继续进入 add-index 回滚处理。当前 RustCodeGraph 对 Rust 符号 `util.rs::try_convert_to_key_exists_err` 只找到 `pkg/ddl/ingest/util_test.rs` 中的调用，没有生产调用边，因此 Rust 文件目前是已导出的移植单元，而不是已验证接入 Rust DDL 主链的实现。

## 核心职责

- 用 `ERR_FOUND_DUPLICATE_KEYS_ID` 标识本文件认可的底层重复键错误类别。
- 用 `IngestError::Wrapped` 和私有 `IngestError::root_cause` 模拟 Go `errors.Cause` 的逐层解包语义。
- 严格检查底层错误形状：根因必须是指定 ID 的 `Terror`，参数必须恰好两个，且两项都必须是 `ErrorArgument::Bytes`。
- 对匹配错误构造 `IngestError::KeyExists`，并补充 `IndexInfo.name` 与 `TableInfo.name`；对所有不匹配情况原样返回输入错误。

本文件不解码索引键、不生成用户可见的重复值文本，也不处理 keyspace 前缀。仓库中更完整的 Rust 对照逻辑位于 `pkg/ddl/util/util.rs::GenKeyExistsErr`，但本函数当前没有调用它。

## 主要符号

- `pub const ERR_FOUND_DUPLICATE_KEYS_ID: u32 = 1001`：本地匹配用的错误 ID。它是硬编码常量；Go 对照通过 `common.ErrFoundDuplicateKeys.ID()` 取得 ID，而 `pkg/lightning/common/errors.rs::ErrFoundDuplicateKeys` 使用 RFC code 构造错误，当前 Rust 文件没有从该定义派生或校验常量。
- `pub enum ErrorArgument`：`Terror` 的参数载体。
  - `Bytes(Vec<u8>)` 是唯一可被转换函数接受的参数类型。
  - `Text(String)`、`Integer(i64)` 用于表达不匹配的参数形状并覆盖拒绝分支。
- `pub enum IngestError`：本文件自包含的错误模型。
  - `Terror { id, arguments }` 表示带 ID 和动态参数的底层错误。
  - `Wrapped { message, source: Arc<IngestError> }` 表示错误链；转换时外层 `message` 不参与判断。
  - `KeyExists { key, value, index_name, table_name }` 是成功转换后的结构化结果。
  - `Other(String)` 表示不应转换的普通错误。
- `pub struct IndexInfo { pub name: String }` 与 `pub struct TableInfo { pub schema: String, pub name: String }`：转换所需的最小元信息模型。`schema` 当前不会写入结果。
- `fn IngestError::root_cause(&self) -> &Self`：私有辅助函数，沿连续的 `Wrapped.source` 向内遍历，返回第一个非 `Wrapped` 根因。
- `pub fn try_convert_to_key_exists_err(origin_error, index_info, table_info) -> IngestError`：唯一公开行为入口，消费原错误并返回转换结果或原错误。

这些类型均派生 `Clone`、`Debug`、`Eq`、`PartialEq`，主要让错误值可拥有、复制、调试和在独立测试中作完整等值比较。

## 执行流程

1. `try_convert_to_key_exists_err` 借用 `origin_error` 调用 `root_cause`。该辅助函数从当前错误开始，只要遇到 `Wrapped` 就沿 `Arc` 指向的 `source` 继续向内。
2. 对根因模式匹配。只有 `IngestError::Terror` 且 `id == ERR_FOUND_DUPLICATE_KEYS_ID`、`arguments.len() == 2` 才继续；其余分支立即返回仍拥有所有权的 `origin_error`。
3. 分别检查两个参数。只有 `(ErrorArgument::Bytes(key), ErrorArgument::Bytes(value))` 被接受；文本、整数或混合形状均原样返回输入错误。
4. 对成功匹配的 `key` 和 `value` 执行深拷贝，从借用的根因中取得拥有所有权的字节向量。
5. 构造 `IngestError::KeyExists`：复制索引名和表名，且只使用 `table_info.name`，不拼接 `table_info.schema`。

该流程是纯同步、单次转换；没有重试、日志、I/O 或恢复动作。

## 数据与状态

函数没有全局可变状态。输入错误按值传入，因此失败路径可以不变地把原值返回；检查期间对其根因只做不可变借用。成功路径需要复制两份字节向量和两个名称字符串，原来的包装层与外层消息随被消费的 `origin_error` 一起丢弃。

`Wrapped.source` 使用 `Arc<IngestError>`，允许多个错误包装值共享同一根因；`root_cause` 不修改引用计数以外的业务状态，也不克隆错误链。`TableInfo.schema` 是模型中存在但此流程不读取的状态，测试明确固定结果中的 `table_name` 为 `users` 而不是 `accounts.users`。

关键不变量是：只有“正确 ID + 正好两个参数 + 两者都是字节”的组合会改变错误类型；任一条件不满足都保持整个 `origin_error` 的结构和值不变。

## 依赖与调用关系

直接源码依赖只有标准库 `std::sync::Arc`。`pkg/ddl/ingest/Cargo.toml` 将本目录定义为 `astersql-ddl-ingest` library crate（入口为 `lib.rs`），crate 还有 `fs2`、`fail`、`astersql-util-dbterror` 及 Windows 条件依赖，但本文件没有直接使用这些依赖，也没有 feature 或条件编译项。

RustCodeGraph 给出的主要边为：

- `try_convert_to_key_exists_err -> IngestError::root_cause`；
- `try_convert_to_key_exists_err -> ERR_FOUND_DUPLICATE_KEYS_ID`；
- `try_convert_to_key_exists_err -> IngestError::KeyExists`；
- `duplicate_key_conversion_matches_go_table_and_index_contract -> try_convert_to_key_exists_err`；
- `wrapped_duplicate_uses_root_cause_like_errors_cause -> try_convert_to_key_exists_err`。

图工具把通用的 `clone`/`len` 解析到了其他同名定义，不能据此认定本文件依赖 `pkg/ddl/backfilling_operators.rs` 或 `pkg/store/driver/txn/lib.rs`；源码事实只是调用标准 `Clone::clone` 和切片/向量长度检查。

Go 生产链的下游是 `pkg/ddl/util/util.go::GenKeyExistsErr`；Rust 仓库也有 `pkg/ddl/util/util.rs::GenKeyExistsErr`，但当前 Rust 转换函数直接构造自己的 `IngestError::KeyExists`，两者之间没有调用边。`pkg/ddl/ingest/lib.rs` 顶层允许 `dead_code`，也与当前仅测试使用的事实一致。

## 错误处理与边界

本函数采用保守转换：无法证明错误是预期重复键形状时，不产生新错误、不 panic，而是原样返回输入。已由 `non_matching_error_shapes_are_returned_unchanged` 覆盖的边界包括普通 `Other`、错误 ID、参数不足、首参数为文本和次参数为整数；源码同样会拒绝参数超过两个、任一参数为非字节的情况。

包装错误只以最深根因作判断。成功转换会丢弃所有外层 `Wrapped.message`；失败转换则保留完整包装链。函数不返回 `Result`，因为“不是可转换形状”不是转换器自身的失败，而是透传原错误。

与完整 Go 行为相比存在重要边界：Go `ddlutil.GenKeyExistsErr` 会处理 keyspace 前缀、依据表/索引元数据解码用户可见值，并在解码失败时生成回退错误；本文件只保存原始 `key`/`value` 与简单名称。因此不能把当前 `KeyExists` 结构视为完整 SQL 错误格式或生产等价实现。常量 `1001` 也没有在本文件中与 RFC 错误定义建立类型级关联，变更错误体系时存在漂移风险。

## 并发与资源生命周期

这里不启动线程、异步任务、通道、锁、事务或磁盘资源，也不拥有 ingest engine/checkpoint。每次调用的临时借用只持续到匹配结束，返回值完全拥有自己的字节与字符串。

`Arc` 仅用于共享包装错误的源错误，不代表本文件执行并发协调。遍历包装链的成本与包装深度线性相关；成功转换还会按键、值和名称长度分配并复制内存。典型错误链很短，因此当前实现没有缓存或深度上限。DDL job 的重试、取消和回滚生命周期由上层负责，本函数只改变上层看见的错误类别。

## 与 Go 版本的对应关系

Rust `try_convert_to_key_exists_err` 对应 Go `TryConvertToKeyExistsErr`，主要分支保持一致：先取根因，再检查重复键错误身份，再要求两个参数且均为字节，任何不匹配均返回原错误。`root_cause` 对应 `errors.Cause`；使用未限定 schema 的 `table_info.name` 与 Go `GenKeyExistsErr` 形成 `table.index` 时采用 `tblInfo.Name` 的契约一致。

但当前迁移不是完整类型/行为复刻：

- Go 接受真实的 `error`、`*terror.Error`、`model.IndexInfo` 和 `model.TableInfo`；Rust 使用本文件自定义的最小枚举与元信息结构。
- Go 通过 `common.ErrFoundDuplicateKeys.ID()` 判断身份；Rust 使用硬编码 `1001`，而 Rust Lightning 公共错误当前以 RFC code 表达。
- Go 调用 `ddlutil.GenKeyExistsErr` 解码冲突值并生成标准 KV 错误；Rust 直接保存原始字节，没有调用仓库已有的 Rust `GenKeyExistsErr`。
- Go 符号有多处 DDL/ingest 生产调用；Rust 符号在当前索引中只有独立单元测试调用。

因此本文件应描述为“保留 Go 转换判定语义的局部模型，生产接线和标准错误生成尚未由当前证据验证”，而不是完整接管 Go ingest 错误路径。

## 扩展指南

- 若要接入 Rust DDL 生产链，优先在真实 ingest/backfill 返回 `ErrFoundDuplicateKeys` 的边界调用本函数，并补充独立集成测试证明错误会驱动既有 add-index 回滚语义；不要在转换函数内实现 job 状态迁移。
- 若要达到 Go 等价，应评估以真实公共错误类型和 `pkg/ddl/util/util.rs::GenKeyExistsErr` 取代本地最小模型，并让重复键身份来自同一 RFC/错误定义，避免维护硬编码 ID。修改时要同步验证 keyspace 前缀、索引值解码失败回退及用户可见错误格式。
- 新增可转换参数形状时，应保持“无法确认则原样返回”的兼容原则，并在 `pkg/ddl/ingest/util_test.rs` 增加成功与拒绝两类用例。Rust 单元测试必须继续放在该独立测试文件中，不要内嵌进 `util.rs`。
- 修改包装语义时，应增加多层包装、非匹配根因及外层信息保留测试；若成功结果需要保留错误链，应明确改变当前“成功即丢弃包装层”的契约。
- 修改表/索引命名时，应与 Go `pkg/ddl/util/util.go::GenKeyExistsErr` 同步，特别避免误把 schema 加入目前只要求 `table.index` 的名称。
- 性能方面，接线到高频冲突路径前应关注成功分支对 key/value 的深拷贝；正确性风险高于常规吞吐风险，因为错误类型会影响 DDL 是否回滚以及最终返回给用户的信息。

## 验证依据

- RustCodeGraph 索引状态：项目共索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/ddl/ingest` 能找到目标、模块入口与独立测试。
- RustCodeGraph 源码/符号查询：`node --file pkg/ddl/ingest/util.rs --offset 1 --limit 260`、`query try_convert_to_key_exists_err --kind function --json`、`node try_convert_to_key_exists_err`、`node root_cause`。
- RustCodeGraph 调用查询：`callers 'util.rs::try_convert_to_key_exists_err' --json` 只返回 `pkg/ddl/ingest/util_test.rs` 的两个直接调用；`callees` 确认 `root_cause` 及构造/引用边，并人工排除了通用同名 `clone`、`len` 的误解析。
- 已读 Rust 文件：`pkg/ddl/ingest/util.rs`、`pkg/ddl/ingest/lib.rs`、`pkg/ddl/ingest/util_test.rs`、`pkg/ddl/util/util.rs`、`pkg/lightning/common/errors.rs`。
- 已读配置：`pkg/ddl/ingest/Cargo.toml`；确认 crate 名、library 入口、依赖和条件依赖，本文件自身无条件编译项。
- 已读 Go 对照：`pkg/ddl/ingest/util.go`、`pkg/ddl/util/util.go`、`pkg/lightning/common/errors.go`；并用 `rg` 核对 Go 生产调用点及 Rust 当前没有生产调用点。
- 已读 DDL 上下文：`docs/agents/ddl/README.md`、`docs/agents/ddl/03-reorg-backfill.md`、`docs/agents/ddl/06-add-index.md`；应用位置结论仍以代码和调用搜索为准。
- 测试证据来自独立文件 `pkg/ddl/ingest/util_test.rs`：覆盖标准转换、包装根因、错误 ID/参数数量/参数类型的透传，以及表名不含 schema。按任务范围未运行 Cargo，本文档没有把测试文件当生产实现。
