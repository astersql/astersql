# `br/pkg/pdutil/utils.rs`

## 文件定位

本文件属于 `astersql-br-pkg-pdutil` library crate；crate 边界由 [`br/pkg/pdutil/Cargo.toml`](./Cargo.toml) 声明，模块入口 [`br/pkg/pdutil/lib.rs`](./lib.rs) 以 `pub mod utils` 装载它，并通过 `pub use utils::*` 将其公开符号平铺到 crate 根。它对应同目录 Go 文件 [`br/pkg/pdutil/utils.go`](./utils.go)，位于 BR 与 PD 控制面交互的辅助层，而不是 SQL 执行或存储数据面。

文件同时承载两组能力：一组是 PD placement rule 的响应模型、拉取和按表匹配；另一组是调度器暂停操作使用的可撤销闭包契约。后者已由 [`br/pkg/pdutil/pd.rs`](./pd.rs) 的调度器恢复流程直接使用；仓库搜索未发现 `GetPlacementRules`、`SearchPlacementRule` 的 Rust 生产调用者，它们当前是已导出的迁移 API，并由独立测试验证。该目录没有 `doc.go`；包级 Rust 定位以 `lib.rs` 的模块说明为准。

## 核心职责

1. 用 `UndoFunc` 表达可跨调用边界持有、线程安全且可能失败的恢复操作，并以 `Nop`/`nop_undo` 提供无副作用实现。
2. 用 `PeerRoleType`、`LabelConstraintOp`、`LabelConstraint`、`Rule` 表达 PD `/pd/api/v1/config/rules` 的 JSON 数据，同时保留未知枚举字符串，避免 PD 新增取值时反序列化失败。
3. 通过 `PlacementHttpClient` 隔离 HTTP 传输，通过 `GetPlacementRules` 组装 URL、解释 HTTP 状态并反序列化规则列表。
4. 通过 `SearchPlacementRule` 将规则的十六进制 `start_key` 还原为 TiDB 原始键，再按 table ID 和 peer role 精确匹配。
5. 提供与 TiDB codec/tablecodec 对齐的最小键编解码辅助：`encode_bytes`、私有 `decode_bytes`、私有 `encode_int`/`decode_int`、`decode_table_id` 和 `table_start_key_hex`。

这些职责由 `utils.rs:35-46`、`utils.rs:49-206`、`utils.rs:213-265` 和 `utils.rs:267-373` 的真实符号支撑；文件不是桩，也不是仅转发到 Go 的兼容门面。

## 主要符号

- `pub type UndoFunc = Arc<dyn Fn(Context) -> Result<(), SharedError> + Send + Sync>`：拥有型回滚闭包。`Arc` 允许共享所有权，`Send + Sync` 允许跨线程持有/调用，参数 `Context` 和返回错误分别对齐 Go 的 `context.Context` 与 `error`。
- `Nop(Context)` / `nop_undo()`：前者总是 `Ok(())`，后者把它包装成 `Arc`。Rust 将 Go 的包级函数值 `Nop` 拆成函数与拥有型构造器。
- `PeerRoleType`：内建 `Voter`、`Leader`、`Follower`、`Learner`，未知值进入 `Unknown(String)`；`Serialize`/`Deserialize` 严格使用 PD 的小写字符串。`Voter` 等四个同名常量降低从 Go 符号迁移的成本。
- `LabelConstraintOp`：识别空值、`in`、`notIn`、`exists`、`notExists`，未知值同样原样保存。`LabelConstraint` 映射 `key`、`op`、`values`。
- `Rule`：覆盖 group/id/index/override、起止键、角色、计数、标签、隔离级别、版本和创建时间。`StartKey`、`EndKey` 标记为 `serde(skip)`，PD JSON 实际读写的是 `StartKeyHex`、`EndKeyHex`。
- `PlacementHttpClient::get_placement_rules`：同步注入点，输入借用的 `Context` 和完整 URL，输出 `(HTTP 状态码, 响应体字节)` 或 `SharedError`。
- `PLACEMENT_RULES_PATH`：固定为 `/pd/api/v1/config/rules`。
- `GetPlacementRules`：公开的规则拉取入口。
- `SearchPlacementRule`：公开的规则匹配入口，返回输入切片中规则的借用，不复制规则。
- `encode_bytes` / `decode_bytes`：8 字节分组的 memcomparable 正反变换；解码器保持私有。
- `decode_table_id`：识别普通 `t<table-id>` 键及带 4 字节 API V2 keyspace 前缀的键；不可识别时返回 `0`。
- `table_start_key_hex`：为给定 table ID 生成规则使用的十六进制 memcomparable 起始键，主要作为测试/规则构造辅助。

文件没有条件编译项；测试模块由 `lib.rs` 上的 `#[cfg(test)]` 在独立文件中接入。

## 执行流程

`GetPlacementRules` 的流程如下：

1. 根据 `use_tls` 选择 `https://` 或 `http://`，拼接 `pdAddr` 与 `PLACEMENT_RULES_PATH`。
2. 调用注入的 `PlacementHttpClient::get_placement_rules`；传输错误用 `?` 原样传播。
3. 状态码为 412 时，把“PD 未启用 placement rules”解释为空规则列表。
4. 状态码不是 200 时，以 `ErrPDInvalidResponse` 为根因，并附加响应文本与状态码后返回。
5. 状态码为 200 时，以 `serde_json::from_slice` 解析 `Vec<Rule>`；JSON 无效时增加 `unmarshal placement rules` 上下文。

`SearchPlacementRule` 按输入顺序线性扫描：先 `hex::decode(StartKeyHex)`，再用 `decode_bytes` 去除 memcomparable 分组；任一步失败都跳过当前规则。成功解码后，只有 `rule.Role == role` 且 `decode_table_id(decoded) == tableID` 才返回该规则，因此首个满足“表 + 角色”的规则获胜；没有命中则返回 `None`。

键路径为 `table_start_key_hex(table_id)` → `encode_int` 生成翻转符号位的大端 8 字节整数 → 加 `t` 前缀 → `encode_bytes` 分组补零并附 marker → `hex::encode`。反向搜索依次执行 hex 解码、`decode_bytes`、可选 keyspace 前缀剥离和 `decode_int`。

## 数据与状态

本文件没有可变全局状态。`ENC_GROUP_SIZE = 8`、`ENC_MARKER = 0xFF`、`TABLE_PREFIX = b"t"` 和 placement API 路径均为只读常量。

`Rule` 是一次 HTTP 响应的拥有型快照；字符串和向量均归调用方所有。`SearchPlacementRule` 只借用规则切片并返回其中元素的引用，不修改规则，也不缓存结果。`StartKey`/`EndKey` 虽存在于模型中，但不会通过 serde 从 JSON 填充；本文件的搜索逻辑只读取 `StartKeyHex` 和 `Role`。

memcomparable 编码每 8 个数据字节追加一个 marker。最后一组不足 8 字节时以零补齐，marker 为 `0xFF - pad_count`；原始长度恰为 8 的倍数时仍会追加一个全零终止组。整数编码通过翻转 `i64` 的最高位再按大端存储，使字节顺序与有符号整数顺序一致。

## 依赖与调用关系

上游装配关系是 `Cargo.toml` → `lib.rs::utils` → 本文件，且 `lib.rs` 把公开符号再导出到 `astersql_br_pkg_pdutil` 根。`Cargo.toml` 为本文件提供 `astersql-br-pkg-errors`、`astersql-errors`、`hex`、`serde` 和 `serde_json`；`semver`、`uuid` 属于同 crate 的其他模块依赖，并非本文件直接使用。

已确认的生产调用边是 [`pd.rs`](./pd.rs) 导入 `UndoFunc`、`nop_undo`：`MakeUndoFunctionByConfig`、`MakeFineGrainedUndoFunction`、`GenRestoreSchedulerFunc` 构造恢复闭包，`RemoveSchedulers`、`RemoveSchedulersWithConfig`、`RemoveAllPDSchedulers` 等返回该闭包；失败路径可用 `nop_undo` 保持“始终可调用”的返回契约。

`GetPlacementRules` 的下游是注入的 HTTP trait、`ErrPDInvalidResponse`/`Annotate` 和 `serde_json`。`SearchPlacementRule` 的下游是 `hex`、私有 memcomparable 解码和 table ID 解码。RustCodeGraph 将本文件列为被 `br/pkg/pdutil/utils_test.rs`、`br/pkg/pdutil/parity_test.rs` 等文件使用；逐符号仓库搜索表明 placement 两个公开入口的直接 Rust 使用目前集中在这两个测试文件，未发现生产调用者。

## 错误处理与边界

- HTTP transport 错误：`GetPlacementRules` 不吞掉错误，由 trait 返回的 `SharedError` 直接上抛。
- HTTP 412：不是错误，明确返回 `Ok(Vec::new())`，对齐 Go 将 placement rule 未启用视为空集的行为。
- 其他非 200：返回可通过 `astersql_errors::Is` 识别为 `ErrPDInvalidResponse` 的错误，并附加 lossy UTF-8 响应文本和状态码。
- HTTP 200 但 JSON 非法：返回带 `unmarshal placement rules` 上下文的新错误。
- 未知 role/op：不报错，分别进入 `Unknown(String)`，保证新 PD 枚举值可往返序列化；默认 role 是空字符串的 `Unknown`，默认 op 是 `Empty`。
- 搜索中的坏十六进制、坏 marker、非零 padding 或不足一个完整分组：跳过该规则，不中止整批搜索。
- `decode_table_id` 对非表键、非法/过短整数返回 `0`。因此 table ID 为 `0` 时，不可识别键也可能得到相同数值；调用方若要严格区分，不能只依赖这个返回值。
- API V2 兼容仅接受首字节为 `r` 或 `x` 且总长至少 4 的前缀，剥离 4 字节后仍必须看到 `t`。

## 并发与资源生命周期

`UndoFunc` 本身要求 `Send + Sync`，并以 `Arc` 管理共享生命周期；`pd.rs` 生成闭包时会克隆 HTTP 客户端、版本和配置等拥有型状态，使恢复操作可在创建它的方法返回后继续存在。`Nop` 不捕获状态，调用后立即完成。

`PlacementHttpClient` 同样要求 `Send + Sync`，但 `GetPlacementRules` 只同步借用客户端和 `Context`，不创建任务、线程、锁或通道。本文件没有连接池和响应流对象，HTTP 请求取消、socket/response body 关闭与重试策略均属于 trait 实现的责任；当前接口只接收已经聚合的响应体字节。

规则匹配和编解码全部使用函数局部所有权或不可变借用，不共享可变状态。复杂度方面，拉取后的 JSON 解析与响应大小线性相关；搜索最多扫描全部规则，并为每个候选分配 hex 解码和 memcomparable 解码缓冲区。

## 与 Go 版本的对应关系

[`utils.go`](./utils.go) 提供 `UndoFunc`、包级 `Nop`、`GetPlacementRules` 和 `SearchPlacementRule`。Rust 对应关系如下：

- Go `UndoFunc func(context.Context) error` 对应线程安全、共享所有权的 Rust `Arc<dyn Fn(Context) -> Result<...>>`；Go 变量 `Nop` 对应 Rust 函数 `Nop` 加构造器 `nop_undo`。
- Go 在 `GetPlacementRules` 内创建 `httputil` 客户端并负责 request/response body；Rust 把网络边界抽成 `PlacementHttpClient`，以 `use_tls: bool` 替代 `*tls.Config` 是否为空。URL scheme、路径、412、非 200 和 JSON 分支保持一致。
- Go 直接使用 `pdtypes.Rule`、`pdtypes.PeerRoleType`；Rust 在本文件中定义兼容模型。Rust 的未知枚举分支显式保留新字符串，并完整覆盖测试所需的 Rule 响应字段。
- Go 调用公共 `codec.DecodeBytes` 与 `tablecodec.DecodeTableID`；Rust 在本文件内实现所需子集，并额外暴露 `encode_bytes`、`decode_table_id`、`table_start_key_hex` 作为迁移和测试辅助。
- Go 搜索函数返回循环变量副本的指针；Rust 返回输入切片元素的借用。两者都按顺序返回首个表 ID 与角色同时匹配的规则，并跳过坏键。

Go 同目录没有专门的 `utils_test.go`；本仓库的 Rust 行为证据来自独立的 `utils_test.rs` 与 `parity_test.rs`。这意味着对齐结论基于 Go 实现本身和 Rust 测试，而非同名 Go 单测。

## 扩展指南

- 扩充 PD `Rule` 字段时，在 `Rule`/相关枚举上使用准确的 serde 名称，并同步验证未知值和大整数不会丢失；优先扩展 [`utils_test.rs`](./utils_test.rs) 的响应契约测试。
- 改变 HTTP 行为时，从 `PlacementHttpClient` 和 `GetPlacementRules` 接入；必须保留 Context 取消语义在具体客户端中的传递，并为 scheme、412、非 200、无效 JSON 和错误根因增加独立测试。不要在此层隐式加入重试，除非同时定义幂等性和取消边界。
- 增加规则匹配条件时修改 `SearchPlacementRule`，并明确“首个命中”是否仍是不变量；同步覆盖坏键、角色不匹配、普通表键与 API V2 keyspace 键。
- 修改键编解码前应与 TiDB `pkg/util/codec`、`pkg/tablecodec` 的 Go 行为逐字节核对，重点测试空输入、8 字节整倍数、非法 marker/padding、负 table ID 和 keyspace 前缀。相关测试必须继续放在独立 `utils_test.rs` 或 `parity_test.rs`，不要内嵌到源文件。
- 调整 `UndoFunc` 签名会影响 `pd.rs` 的所有恢复闭包和调用者；需检查捕获对象是否仍满足 `Send + Sync`，以及失败时是否仍能提供安全的 no-op 回滚。
- 当前 placement API 未发现 Rust 生产调用者；接线新调用点时应从 crate 根导入公开 API，并用真实客户端实现 trait，而不是绕过状态码与错误映射逻辑。

## 验证依据

- RustCodeGraph `status`：索引可用；`node --file br/pkg/pdutil/utils.rs --offset 1 --limit 400` 返回完整 373 行、全部类型/函数及文件级使用者。
- RustCodeGraph `query GetPlacementRules --kind function` 与 `query SearchPlacementRule --kind function`：均确认 Go/Rust 两个同名定义；`query table_start_key_hex` 确认唯一 Rust 定义。通用 `callers/callees` 因 Go/Rust 同名解析未在合理时间内返回，故调用关系改由图的文件使用者结果和精确仓库引用核验。
- 已读生产与声明文件：`br/pkg/pdutil/utils.rs`、`br/pkg/pdutil/Cargo.toml`、`br/pkg/pdutil/lib.rs`、`br/pkg/pdutil/pd.rs`。
- 已读 Go 对照：`br/pkg/pdutil/utils.go`、`br/pkg/pdutil/pd.go`。
- 已读独立 Rust 测试：`br/pkg/pdutil/utils_test.rs`、`br/pkg/pdutil/parity_test.rs`。前者覆盖未知 role/完整 Rule 字段和 API V2 前缀；后者覆盖坏键跳过、角色过滤、memcomparable 常量、HTTP/HTTPS URL、412、非 200 错误分类和 200 JSON 成功路径。
- 全仓精确引用搜索确认：`UndoFunc`/`nop_undo` 的生产使用在 `pd.rs`；`GetPlacementRules`/`SearchPlacementRule` 的 Rust 直接调用位于上述两个测试文件。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另执行任务指定的 11 章节结构验证、Markdown diff 检查和仅目标文件的 Git 暂存检查。
