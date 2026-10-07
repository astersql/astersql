# `pkg/expression/builtin_info.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的信息类标量函数内核。`pkg/expression/lib.rs:222-225` 通过 `#[path = "builtin_info.rs"] mod builtin_info_kernel;` 私有挂载它，并由相邻的 `builtin_info_vec.rs` 复用一部分函数构造批量结果。它覆盖会话信息、身份与角色、版本、DDL Owner、`BENCHMARK`、表达式字符集元数据、键编解码、MVCC、SQL digest、计划解码、序列和人类可读单位格式化等语义。

这里不是 Go `pkg/expression/builtin_info.go` 的完整结构复刻：Rust 文件不包含 function class、参数类型推导、SQL 函数注册或真实 session/infoschema/store/executor 的获取逻辑，而是把环境相关能力压缩成 `SessionInfo` 快照和若干 trait 注入点。当前 crate 内可确认的生产侧直接适配是 `pkg/expression/builtin_info_vec.rs`；SQL 名称/参数个数另见 `pkg/expression/builtin.rs:6805-6817`。因此应把本文件理解为可测试的语义内核，而非已经独立完成所有运行时接线的执行入口。

## 核心职责

- 从 `SessionInfo` 读取或更新会话状态：`database`、`found_rows`、`current_user`、`current_role`、`current_resource_group`、`user`、`connection_id`、`row_count`、两种 `last_insert_id` 以及序列最近值。
- 实现无外部状态的兼容逻辑：`benchmark`、字符集/排序规则元数据读取、版本字符串、DDL Owner 布尔转整数、SQL digest 哈希、序列名拆分、字节数/纳秒格式化。
- 通过 `KeyCodec`、`MvccProvider`、`SqlDigestRetriever`、`PlanDecoder`、`SequenceService` 隔离 schema、KV、内部 SQL 执行器、计划编解码器和序列目录等上层能力。
- 固化 Go 兼容的 NULL、权限、错误降级和警告契约。例如 digest JSON 解析失败返回 NULL 并追加警告，文本计划解码失败返回原文，二进制计划解码失败返回空串并追加警告。
- 为 `builtin_info_vec.rs` 提供可重复到每一行的标量结果；键编码与 MVCC 等函数是否可向量化由相邻文件的 `InfoBuiltinKind::vectorized` 决定，不在本文件中决定。

## 主要符号

### 会话与身份

- `UserIdentity` 保存登录身份和认证身份。`login_string` 供 `USER()` 使用；`authenticated_string` 供 `CURRENT_USER()` 使用，认证字段为空时逐字段回退到登录字段。
- `RoleIdentity::canonical_string` 生成反引号引用的 `` `user`@`host` ``，并把身份中的反引号加倍转义。
- `SessionInfo` 是本内核所需的最小会话快照。`active_roles: None` 表示属性缺失，`Some(vec![])` 则是合法的 `CURRENT_ROLE() = NONE`；`hinted_resource_group: Some("")` 也有意义，表示语句确实设置了空 hint，而不是退回会话默认值。
- `database` 以空库名表示 SQL NULL；`current_user`、`user` 和 `current_role` 在所需会话属性缺失时返回 `ExpressionError::MissingSession`；`current_role` 对规范角色串排序后拼接，保证稳定输出。
- `last_insert_id` 读取上一语句的 `previous_last_insert_id`；`last_insert_id_with_id` 仅在参数非 NULL 时将其按 Rust `as u64` 语义写入 `last_insert_id`，再原样返回参数。

### 外部能力边界

- `InfoValue` 是键编码参数的无标记联合，保留 NULL、整数、浮点、字符串、字节、布尔和 JSON 原生类别。
- `KeyCodec` 提供记录键、索引键编码与字符串键解码。`encode_record_key`/`encode_index_key` 把返回字节转小写十六进制；`map_table_access_error` 在已有用户和表名时把通用 `AccessDenied` 提升为 `TableAccessDenied`。
- `MvccProvider` 提供按编码键查询、索引键判别和临时索引键转换。`MvccResponse` 用 `#[serde(flatten)]` 展开 JSON 主体，而 `has_entries` 仅控制是否追加临时索引结果，不进入 JSON。
- `SqlDigestRetriever::retrieve_global` 批量返回 digest 到 SQL 文本的映射。`decode_sql_digests_with_warnings` 保持输入数组位置，非字符串、未知 digest 和空 SQL 都输出 JSON `null`。
- `PlanDecoder` 分离文本计划和二进制计划解码；私有 `UnavailablePlanDecoder` 明确表示默认包装函数尚未接入真实解码器。
- `SequenceService` 集中序列解析后的 id、取值、设值和权限检查；`SequencePrivilege` 区分 `Insert` 与 `Select`。

### 纯计算帮助函数

- `benchmark` 对非负次数精确调用闭包：负数返回 NULL，零次返回非 NULL 的 0，闭包错误立即传播。
- `ExpressionMetadata` 与 `charset`、`collation`、`coercibility` 暴露表达式元数据，不自行推导类型。
- `encode_sql_digest` 调用 `parser::DigestHash(sql).String()`；输入 NULL 仍为 NULL。
- `get_schema_and_sequence` 按所有点拆分但只取前两段，与 Go `strings.Split` 后取 `res[0]`、`res[1]` 一致；裸名称返回空 schema。
- `format_bytes` 和 `format_nano_time` 按绝对值选单位，保留原符号；`format_scaled` 对单位为基础单位的值取零位小数，其他值取两位小数，缩放绝对值达到 `100_000` 后改用带显式符号、至少两位指数的科学计数法。

## 执行流程

1. 上层构造 `SessionInfo`，并按函数需要注入 codec/provider/retriever/decoder/service。对简单会话函数，内核直接读取快照；`builtin_info_vec.rs` 会把同一标量结果复制到指定行数。
2. 身份和角色函数先区分“属性缺失”与“合法空值”。角色存在时逐项规范引用、排序并逗号连接；资源组先检查语句 hint 的存在性，再回退会话默认值。
3. 键编码函数先确认 codec 已注入，再把 `InfoValue` 参数交给实现；错误经表权限映射后传播，成功的字节键转十六进制。`decode_key` 对 NULL 短路，在无 codec 时兼容性地返回原字符串。
4. `tidb_mvcc_info` 在读取参数前先校验 SUPER 权限，然后十六进制解码并查询普通键。若它是非临时索引键，则原地转换为临时索引键再次查询，只在 `has_entries` 为真时追加第二项，最后序列化 JSON 数组。
5. `decode_sql_digests_with_warnings` 先校验 PROCESS 权限，再解析 JSON 数组并抽取字符串 digest 批量检索。取消错误直接返回；其他检索失败或 JSON 解析失败写 warning 并返回 NULL。成功时按原数组逐位置回填 SQL，可按字节截断并追加 `...`，最后序列化结果数组。
6. 计划解码先传播输入 NULL。文本解码失败吞掉错误并回退原串；二进制解码失败把错误文本写入 warning 向量并返回空的非 NULL 字符串。
7. `next_val`/`last_val`/`set_val` 先用 `resolve_sequence_name` 补齐当前库，再按操作校验权限。`next_val` 获取 id 和新值后更新会话 `sequence_state`；`last_val` 只查该 map；`set_val` 直接委托服务，且任一输入 NULL 都短路为 NULL。
8. 两个格式化函数以输入绝对值跨越单位阈值，但以原值做缩放和输出，从而对正负数采用相同单位。

## 数据与状态

`SessionInfo` 是本文件唯一直接可变的业务状态载体。带参 `LAST_INSERT_ID` 写 `last_insert_id`，但无参读取的是 `previous_last_insert_id`，两者不能合并；这反映 Go 中当前语句副作用与上一语句可见值的区分。序列状态是 `HashMap<i64, i64>`，键为服务返回的稳定 sequence id，值仅由成功的 `next_val` 更新；`set_val` 不更新该 map，因此 `LASTVAL` 仍表示本会话最近一次 `NEXTVAL`。

警告由调用者提供的 `&mut Vec<String>` 承载，适用于 digest 与二进制计划的可降级错误。它不是全局日志，也不持久化。`MvccInfoResult` 是私有序列化形状；`InfoValue` 和 `MvccResponse` 使用 `serde`，外部 JSON 数据及序列化失败会被映射到 `ExpressionError`。

常量 `KIB` 到 `EIB`、`MICRO` 到 `DAY` 只描述二进制字节和纳秒单位阈值，无运行时可变状态。`UnavailablePlanDecoder` 也是零状态占位实现。

## 依赖与调用关系

- crate 边界：`pkg/expression/Cargo.toml` 定义 `astersql-expression`，本文件直接依赖该 manifest 中的 `serde`、`serde_json`、`hex`，并通过 crate 再导出使用 `astersql-parser`、`astersql-parser-mysql` 和 `astersql-util-printer`。
- 模块入口：`pkg/expression/lib.rs:222-225` 挂载标量内核和向量内核；测试仅在 `cfg(test)` 下由 `lib.rs:498-502` 挂载，符合生产逻辑与测试文件分离要求。
- 上游：`pkg/expression/builtin_info_vec.rs` 调用 `database`、`connection_id`、`tidb_version`、`row_count`、身份/角色/资源组函数、DDL Owner、`found_rows`、`last_insert_id`、`version` 和 `decode_key`。仓库搜索还显示该内核的其余公开符号主要由独立 Rust 测试直接调用；未从现有证据确认它们已由完整 Rust SQL expression dispatch 接线。
- 下游：RustCodeGraph 的文件级/调用边确认 `encode_record_key`→`KeyCodec::encode_record_key`→`map_table_access_error`，`tidb_mvcc_info`→四个 `MvccProvider` 方法，`decode_sql_digests`→`decode_sql_digests_with_warnings`→`SqlDigestRetriever::retrieve_global`，`next_val`→名称解析/权限错误构造及 `SequenceService::{verify,sequence_id,next_value}`，格式化入口→`format_scaled`。
- SQL 注册相关证据：`pkg/expression/builtin.rs:6805-6817` 列出 `tidb_encode_record_key`、`tidb_mvcc_info`、`tidb_decode_sql_digests` 等名称和参数范围；该注册层没有在本文件内实现。

## 错误处理与边界

- 所有用 `Option` 表达的 SQL NULL 都显式短路；不要把 `active_roles: None`、空角色列表、空资源组 hint 或空 SQL 文本混为同一种状态。
- `found_rows`、`connection_id`、`last_insert_id` 使用无检查整数转换以保持 Go 位模式兼容；测试确认 `u64::MAX as i64 == -1` 的表现。扩展时不应擅自改成溢出错误。
- 键编码缺少 codec 是硬错误；键解码缺少 codec 却返回原文，这是有意的不对称契约。只有通用 `AccessDenied` 且用户、表名齐全时才转换为表级拒绝错误。
- MVCC 在权限检查后才处理 NULL；无 SUPER 权限即使输入为 NULL 也返回拒绝。非法 hex、provider 错误和 JSON 序列化错误均传播。
- digest 解码在权限检查后处理 NULL。JSON 解析和普通检索失败降级为 warning + NULL；`ExpressionError::Cancelled` 保持硬错误。截断长度 `<= 0` 表示不截断；正数按 UTF-8 字节切片，切断码点时通过 `from_utf8_lossy` 产生替换字符以模拟 Go `encoding/json` 的结果。
- 默认 `decode_plan`/`decode_binary_plan` 使用不可用解码器，因此前者总会对非 NULL 输入返回原串，后者总会记录 warning 并返回空串。需要真实解码时必须调用 `*_with` 并注入实现；文档不能把默认包装描述成已接线。
- `get_schema_and_sequence("db.seq.extra")` 忽略第三段及以后内容，这是 Go 行为，不是严格限定名解析器。
- 格式化使用 `f64`；极大整数可能已经失去精确度，且本文件没有为 NaN/无穷值单独定义错误路径。

## 并发与资源生命周期

本文件不创建线程、异步任务、事务、锁、通道或长期资源。所有外部操作均在调用栈内同步完成，借用的 provider/service/decoder/retriever 生命周期由调用者管理。

`KeyCodec`、`MvccProvider` 明确要求 `Send + Sync`，可安全地作为共享只读能力跨执行上下文引用；`SqlDigestRetriever` 和 `PlanDecoder` 同样要求 `Send + Sync`。`SequenceService` 则接收 `&mut dyn SequenceService` 且没有 `Send + Sync` 约束，体现每次序列操作可能更新服务状态并要求独占借用。`SessionInfo` 在 `last_insert_id_with_id` 和 `next_val` 中以 `&mut` 独占修改，其余入口只读。

Go digest 实现会创建带超时的 context 并执行全局检索；Rust 内核只把一次同步批量调用交给 `SqlDigestRetriever`，没有自行实现超时或取消资源。取消语义只能由注入实现以 `ExpressionError::Cancelled` 上报。

## 与 Go 版本的对应关系

`pkg/expression/builtin_info.go` 是主要语义来源。Rust 的 `database`、身份/角色、资源组、行数、连接 id、两种 LAST_INSERT_ID、版本、DDL Owner、BENCHMARK、元数据、键处理、MVCC、digest、plan、sequence 及格式化函数，分别对应 Go 文件中的同名 function class / `builtin*Sig.eval*` 路径。`pkg/expression/builtin_info_test.go` 覆盖基础信息函数、`BENCHMARK`、LAST_INSERT_ID 和格式化；Rust 的 `pkg/expression/builtin_info_test.rs` 复核了相同边界。

主要结构差异如下：

- Go 在 function class 构建阶段验证参数、设置返回 FieldType/字符集/长度并声明 OptionalEvalProps；Rust 本文件只接受已求值参数与最小状态，不负责这些构建职责。
- Go 直接从 `EvalContext`、session vars、infoschema、privilege checker、KV store 和 SQL executor 取得能力；Rust 通过快照和 trait 注入，权限也被压缩成布尔值或 `SequenceService::verify`。
- Go digest 检索创建最长 20 秒的 context；Rust 不管理超时，只保留“取消传播、普通失败警告化”的结果契约。
- Go 计划函数调用真实 `plancodec`；Rust 默认解码器明确不可用，只在调用者提供 `PlanDecoder` 时才有真实解码行为。
- Go 的标量函数还有完整类型分派和注册；Rust 当前明确可见的生产消费主要是 `builtin_info_vec.rs`。这属于迁移/接线状态限制，不应从单元测试通过推断所有 SQL 路径已经可用。

Rust 独立测试 `pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs` 进一步覆盖 key/MVCC/digest/plan/sequence 与向量能力，虽然其文件名来自组合迁移测试；它验证了这里描述的注入边界和错误契约。`pkg/expression/builtin_info_vec_test.rs` 则验证标量值批量复制、带参 LAST_INSERT_ID 取最后一个非 NULL、以及不可向量化矩阵。

## 扩展指南

- 新增简单会话信息函数时，先在 `SessionInfo` 增加最小必要字段，再实现纯标量函数；若可向量化，在 `builtin_info_vec.rs` 增加 kind 和包装，并同步 `builtin_info_test.rs`、`builtin_info_vec_test.rs`。必须同时核对 Go 的 NULL、无符号转换、FieldType 和 OptionalEvalProps 语义。
- 新增依赖 schema、存储或执行器的函数时，优先扩展窄 trait 或新增专用 trait，不把具体上层类型耦合进 expression crate。明确 trait 是否必须 `Send + Sync`，以及错误应传播、警告化还是回退。
- 扩展键值类别需修改 `InfoValue`，同时检查 serde 表示、codec 实现和表权限错误映射；相关回归应放在独立测试文件，不能内嵌进生产源文件。
- 修改 digest 时要保留输入数组位置、非字符串元素、空语句、字节截断、PROCESS 权限和取消错误边界。尤其不要用 Unicode 字符计数替代 Go 的字节截断。
- 接入真实计划解码器应在上层提供 `PlanDecoder`，并分别保留文本失败回原文与二进制失败 warning + 空串的契约；若要替换默认包装，需先证明所有调用点都能取得实现。
- 修改序列逻辑要同步检查 `resolve_sequence_name`、INSERT/SELECT 权限、sequence id 以及会话 `sequence_state`。`NEXTVAL` 更新 LASTVAL 状态，而 `SETVAL` 不更新，是当前 Go 对齐行为。
- 修改格式化阈值或显示格式时同步 `builtin_info_test.rs` 和 Go `builtin_info_test.go` 的单位边界、负值和科学计数法案例，并考虑 `f64` 极值风险。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件查询确认 `pkg/expression/builtin_info.rs` 共 745 行并列出模块符号。
- RustCodeGraph 精确查询：`decode_sql_digests_with_warnings` 定位于 441 行，`next_val` 定位于 620 行，`tidb_mvcc_info` 定位于 378 行。callees 输出确认本文件内部及 trait 边，包括 digest wrapper/retriever、MVCC provider、sequence service 和格式化 helper；其中若干常见方法名的跨文件候选存在歧义，因此文档仅采用可由目标源码直接复核的边。
- 已读生产路径：`pkg/expression/builtin_info.rs`、`pkg/expression/builtin_info_vec.rs`、`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/builtin.rs`。
- 已读 Rust 测试：`pkg/expression/builtin_info_test.rs`、`pkg/expression/builtin_info_vec_test.rs` 的相关片段、`pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs` 的 key/MVCC/digest/plan/sequence/format 片段。
- 已读 Go 对照：`pkg/expression/builtin_info.go` 的相应 `builtin*Sig.eval*`、注入变量、digest、plan、sequence、format 路径，以及 `pkg/expression/builtin_info_test.go` 的测试目录与相关用例。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构验证要求本文恰好包含约定的十一个二级标题。
