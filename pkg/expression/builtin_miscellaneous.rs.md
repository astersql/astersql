# `pkg/expression/builtin_miscellaneous.rs`

## 文件定位

源码链接：[pkg/expression/builtin_miscellaneous.rs](builtin_miscellaneous.rs)。本文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），是杂项 SQL 内建函数的**标量语义内核**。`pkg/expression/lib.rs` 通过 `#[path = "builtin_miscellaneous.rs"] mod builtin_miscellaneous_kernel;` 将它装入 crate；生产求值分派 `pkg/expression/builtin.rs` 直接调用其中部分函数。它不负责 SQL 函数注册、参数类型推导、表达式对象构造或向量化批处理；这些职责分别位于 `builtin.rs`、类型推导代码和 `builtin_miscellaneous_vec.rs`。

当前直接可见的生产接线包括：`CoreBuiltinKind::TiDBShard` 调用 `tidb_shard`，`CoreBuiltinKind::IsIPv4/IsIPv6` 调用对应谓词，`CoreBuiltinKind::Uuid` 调用 `uuid_v1`。`SLEEP` 的生产分派当前调用向量化内核 `vec_sleep`，而不是本文件的 `sleep_builtin`。其余公开函数在 RustCodeGraph 中主要由 `builtin_miscellaneous_test.rs` 与 `builtin_miscellaneous_18_aster_unit_test.rs` 调用；因此应把“本文件已经实现标量语义”和“该语义已经由生产分派接线”区分开，不能仅凭 `pub fn` 推断 SQL 路径已经完整迁移。

## 核心职责

- 网络地址：`inet_aton`/`inet_ntoa`、`inet6_aton`/`inet6_ntoa` 在文本、整数和网络字节序之间转换；`is_ipv4`、`is_ipv6`、`is_ipv4_compat`、`is_ipv4_mapped` 完成格式或二进制前缀判定。
- 透传函数：泛型 `any_value` 原样返回唯一参数，`name_const` 忽略名称参数并返回第二个参数。这里仅表达值语义；Go 版本中返回类型、混合类型和下推签名等构造逻辑不在本文件内。
- UUID：兼容 Go `google/uuid` 接受的规范式、无连字符、URN 和花括号形式；生成 v1/v4/v7；读取版本/时间戳；完成 UUID 文本与 16 字节值的可选时间字段交换。
- 分片：用零密钥 DES 复现 Vitess 的 64 位哈希，再由 `tidb_shard` 对 256 取模。
- 会话咨询锁：以 `AdvisoryLockContext` 隔离会话/存储实现，统一锁名校验、小写规范化、超时钳制及后端错误映射。
- 可中断休眠与未支持分支：`sleep_builtin` 以 10 ms 周期检查 kill 回调；`default_function`、`uuid_short`、`tidb_row_checksum` 返回与 Go 对照一致的固定错误。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `MiscError` | 公共错误枚举；覆盖错误字符串值、参数错误、锁名/死锁/后端锁错误、函数不存在和不支持。实现 `Display` 与 `Error`。 |
| `inet_aton` / `inet_ntoa` | 公共 IPv4 数值转换；前者接受 MySQL 历史短格式，后者仅接受可转为 `u32` 的非负值。 |
| `inet6_aton` / `inet6_ntoa` | 公共 IP 文本/字节转换；输入/输出分别限于 IPv4 的 4 字节或 IPv6 的 16 字节。 |
| `is_ipv4` / `is_ipv6` | 公共文本谓词；前者调用私有无分配扫描器 `is_ipv4_text`，后者要求解析结果确为 `IpAddr::V6`。 |
| `is_ipv4_compat` / `is_ipv4_mapped` | 公共二进制谓词；检查 16 字节值的 12 个零前缀或 `00..00 ff ff` 前缀。 |
| `any_value<T>` / `name_const<N,T>` | 公共泛型透传函数，保留 `Option<T>` 的 NULL 状态。 |
| `parse_google_uuid` | 私有兼容解析入口；特殊处理 38 字节花括号形式与 `urn:uuid:` 前缀。 |
| `uuid_v1` / `uuid_v4` / `uuid_v7` | 公共 UUID 生成器；v1 使用静态 `UUID_V1_CONTEXT` 和全零节点 ID。 |
| `uuid_version` / `uuid_timestamp` | 公共 UUID 元数据读取；后者只为 v1/v6/v7 返回 `UuidTimestamp`，并截断到微秒。 |
| `uuid_to_bin` / `bin_to_uuid` | 公共文本/16 字节互转；`swap_flag != 0` 时调用 `swap_binary_uuid` 或 `swap_string_uuid`。 |
| `vitess_hash_u64` / `vitess_hash` / `tidb_shard` | 公共哈希链；有符号输入按位转换为 `u64`，DES 后对 256 取模。 |
| `AdvisoryLockContext` | 公共抽象接口，提供获取、查询、单个释放与全部释放会话锁的能力。 |
| `AdvisoryLockError` | 后端锁错误分类：超时、死锁、其它文本错误。 |
| `LockWarning` / `LockOutcome` | 公共结果数据；保留调用者提交/实际采用的超时及 SQL 数值结果。 |
| `get_lock`、`release_lock`、`is_free_lock`、`is_used_lock`、`release_all_locks` | 公共锁语义适配函数；共享私有 `normalize_lock_name`。 |
| `sleep_builtin` | 公共同步休眠函数，以调用者提供的 `FnMut() -> bool` 轮询中断。 |
| `default_function` / `uuid_short` / `tidb_row_checksum` | 公共固定错误入口，表示当前表达式语境中的不可用行为。 |

模块常量包括 `TIDB_SHARD_BUCKET_COUNT = 256`、锁名 Unicode 标量上限 `ADVISORY_LOCK_NAME_LIMIT = 64` 和睡眠轮询间隔 `SLEEP_POLL_INTERVAL = 10ms`；静态量 `UUID_V1_CONTEXT` 为 v1 时间戳提供序列上下文。文件没有条件编译项。

## 执行流程

1. 表达式层先完成函数注册、参数求值与 SQL 类型转换；已接线的 `CoreBuiltinKind` 分支再把普通 Rust 值交给本文件。例如 `TiDBShard` 先处理 SQL NULL，再调用 `tidb_shard`。
2. IP 转换路径中，`inet_aton` 逐字节累计每段十进制值，拒绝非数字、段值大于 255、超过三个点以及结尾点，并按点数左移补齐 MySQL 短格式；`inet6_aton` 则委托标准库 `IpAddr` 解析。反向转换只接受合法宽度/范围。
3. UUID 路径统一经 `parse_google_uuid` 解析。`uuid_to_bin` 额外拒绝首尾空白，再输出 16 字节；非零交换标志把 time-high、time-mid、time-low 调整为 MySQL 兼容顺序。`bin_to_uuid` 要求恰好 16 字节，按需在规范文本上做逆向字段重排。
4. 哈希路径把 `i64` 按补码位型解释为 `u64`，转为大端 8 字节，以全零 8 字节 DES 密钥加密，再把密文解释为大端 `u64`；`tidb_shard` 只取其模 256 的低桶号。
5. `get_lock` 先用 `normalize_lock_name` 检查 NULL、空串和超过 64 个 Unicode 标量的名称，并转小写；NULL 超时视为 0，负值或超过上限的值钳制为调用者给定上限并返回 `LockWarning`。随后调用 trait：成功为 1，超时为 0，死锁和其它后端错误转成 `MiscError`。
6. `sleep_builtin` 验证非 NULL、非负、有限且纳秒换算不溢出；零秒立即返回 0。其余按“不超过 10 ms”的片段休眠，每段之后检查 kill：被中断返回 1，到期返回 0。

## 数据与状态

大部分函数是无状态纯函数：输入用 `Option` 表达 SQL NULL，用 `Result` 区分正常 NULL 与求值错误。IP 二进制值用 `Vec<u8>` 或借用切片承载，UUID 二进制值固定为 `[u8; 16]`，避免有效结果出现长度歧义。

`UuidTimestamp { unix_seconds, microseconds }` 将 UUID 时间拆成秒和微秒；`decimal_string` 固定输出六位小数。纳秒到微秒使用整数除以 1,000，行为是截断而非四舍五入。

唯一模块级可变语义来自 `UUID_V1_CONTEXT: ContextV1`，由 `uuid` crate 内部维护序列计数；节点 ID固定为六个零字节。咨询锁状态不存放在本文件，而由调用者实现的 `AdvisoryLockContext` 持有。`sleep_builtin` 只保留局部 `Instant` 和持续时间，不创建后台任务。

## 依赖与调用关系

- crate 装配：`pkg/expression/lib.rs` 将文件命名为私有模块 `builtin_miscellaneous_kernel`；测试配置下通过 `expression_builtin_miscellaneous` 重导出全部符号给独立测试。
- 生产上游：RustCodeGraph 和 `pkg/expression/builtin.rs` 显示 `tidb_shard`、`is_ipv4`、`is_ipv6`、`uuid_v1` 被核心求值分派直接调用。图中 `inet_aton` 等大量符号的 Rust 调用者是类型推导/独立测试；咨询锁和本文件的睡眠入口未发现生产分派调用。
- 内部调用边：`wrong_value` 被 `inet_aton`、`inet6_aton`、`uuid_version`、`uuid_timestamp`、`uuid_to_bin`、`bin_to_uuid` 使用；`parse_google_uuid` 被四个 UUID 解析入口使用；`vitess_hash -> vitess_hash_u64`，`tidb_shard -> vitess_hash`；五个单锁操作共享 `normalize_lock_name`。
- 标准库：`std::net` 提供 IP 解析/格式化，`std::time` 与 `std::thread` 提供同步休眠。
- 外部 crate：`des` 与 `cipher` 实现 Vitess DES；`uuid` 提供解析、版本、时间戳及 v1/v4/v7 生成。`pkg/expression/Cargo.toml` 明确启用 `uuid` 的 `v1`、`v4`、`v7` features。
- Go 对照：`pkg/expression/builtin_miscellaneous.go` 是 SQL function class、签名、类型属性、可选会话属性与具体标量行为的完整基准；Rust 本文件只抽取其中可独立表达的值/错误语义。

## 错误处理与边界

- NULL 并非统一等于错误：网络/UUID 转换和谓词大多传播为 `Ok(None)`/`None`；`sleep_builtin(None, ...)` 与 NULL 锁名则为错误。调用者必须保留每个 SQL 函数各自契约。
- `inet_aton` 的错误使用函数名 `inet_aton`；`inet6_aton` 当前构造的 `WrongValue.function` 是 `inet_aton6`，这是源码现状，文档不将其修正为预期名称。
- `inet_ntoa` 对负数或大于 `u32::MAX` 返回 NULL；`inet6_ntoa` 对非 4/16 字节返回 NULL，而不是错误。
- `is_ipv4` 只接受恰好四段的十进制文本，不接受 IPv4-mapped IPv6；`is_ipv6` 接受可解析的 IPv4-mapped IPv6，但拒绝纯 IPv4。
- UUID 校验拒绝首尾空白。为保持 `google/uuid` 历史兼容，38 字节且以 `{` 开头的输入只取内部 36 字节，测试甚至保留了末字节不是 `}` 仍可解析的行为；改动此处有兼容风险。
- `uuid_timestamp` 对解析失败报错，对无时间戳的合法版本返回 NULL；`bin_to_uuid` 非 16 字节报错。交换辅助函数假定规范 36 字节文本，`swap_string_uuid` 只用 `debug_assert` 保护该内部前置条件。
- 锁名限制按 Unicode 标量数而非字节数计算；名称大小写不敏感。超时是结果 0，死锁是专用错误，其它错误保留后端文本。
- `sleep_builtin` 拒绝负数、NaN、无穷和无法换算到纳秒范围的值。kill 仅在每次分段休眠后检查，因此中断延迟上界约为 10 ms 加调度延迟。

## 并发与资源生命周期

本文件不显式加锁、不启动线程，也不持有数据库事务。`thread::sleep` 阻塞当前执行线程；调用方必须评估其所在线程池是否允许阻塞。kill 回调由调用者拥有并在当前线程同步调用。

`UUID_V1_CONTEXT` 是进程期静态对象，依赖 `uuid::ContextV1` 的内部并发安全序列机制；v1 节点 ID不代表实际网卡。v4 随机源和 v7 时钟也由 `uuid` crate 管理。

咨询锁资源的真实生命周期完全属于 `AdvisoryLockContext` 实现：本文件只转发获取/释放并规范化返回值，不保证锁的持久化、重入、跨会话死锁检测或会话结束清理。安全扩展时应让会话层实现负责清理，并保持 `release_all_locks` 的计数语义。测试中的 `MockLocks` 仅用 `HashMap` 模拟接口，不是生产并发模型。

## 与 Go 版本的对应关系

`pkg/expression/builtin_miscellaneous.go` 同时包含 function class、返回/参数类型、protobuf 下推码、会话可选属性读取器和 `eval*`；Rust 本文件主要对应其中 `eval*` 的核心算法。关键对齐点是：INET 短格式与边界、严格四段 IPv4、IPv4-mapped IPv6、ANY_VALUE/NAME_CONST 透传、UUID 多格式/严格空白/版本/时间戳/交换、Vitess 哈希、锁名小写和 64 字符限制、锁超时钳制、SLEEP 的 kill 返回值，以及三个固定不可用错误。

存在明确的结构差异：Go 通过 `BuildContext`/`EvalContext` 产生 TiDB 错误与 warning，并从 session optional properties 取得锁和 kill 状态；Rust 用 `MiscError`、`LockOutcome.warning`、trait 与回调显式传递这些信息。Rust 的 `any_value`/`name_const` 不承担 Go 代码里的类型元数据复制和不同 eval type 签名选择。Rust `uuid_v1` 使用全零节点 ID并复用静态上下文，语义与当前文件注释及测试一致。

接线状态也不完全对等：Go 文件中的函数类是完整 SQL 路径；Rust `builtin.rs` 当前只直接调用本文件少数内核，且 `SLEEP` 改走向量化实现。扩展或判定迁移完成度时必须同时检查注册表、factory、标量分派、向量化分派与类型推导，不能只比较本文件函数数量。

## 扩展指南

新增或修改杂项函数时，先确认职责应落在标量内核、`builtin_miscellaneous_vec.rs`，还是 `builtin.rs` 的注册/类型/求值层。若加入本文件，应保持 SQL NULL 与错误的明确区分，并在独立测试文件中覆盖正常值、NULL、边界值和错误；不要把测试模块内嵌回生产源文件。

- IP 逻辑：修改 `inet_*` 或谓词时，同步 `pkg/expression/builtin_miscellaneous_test.rs` 与更完整的 `builtin_miscellaneous_18_aster_unit_test.rs`，并对照 Go 表；注意短格式、映射地址和 4/16 字节边界。
- UUID：统一复用 `parse_google_uuid`，谨慎保留花括号历史行为与首尾空白差异；新增版本需更新 Cargo feature、版本/时间戳规则和二进制往返测试。交换布局变化会破坏已存储键的排序/兼容性，属于高风险修改。
- 锁：通过扩展 `AdvisoryLockContext` 接入会话状态，不在此处引入全局锁表；新增错误必须同时定义后端分类、`MiscError` 映射和 warning 传播。名称规则或超时规则变化会影响 MySQL 兼容性。
- 哈希/分片：DES 密钥、大小端或桶数任一变化都会改变数据分布；必须保留 Go/Vitess 固定向量，并评估既有生成列或分片键兼容性与重分布成本。
- 接线：实现函数后还需检查 `pkg/expression/lib.rs`、`builtin.rs` 的 kind/factory/arity/求值分支、类型推导及向量化实现。仅新增 `pub fn` 不代表 SQL 可调用。

## 验证依据

- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；用 `node --file pkg/expression/builtin_miscellaneous.rs` 阅读 566 行全文件，并用 `explore`、`callers`、`callees` 核对公开入口和内部调用。图证据显示 `wrong_value` 的六个直接调用者，以及 `builtin.rs`/独立测试对 IP、UUID、分片函数的调用。
- 生产与装配：阅读 `pkg/expression/builtin_miscellaneous.rs`、`pkg/expression/lib.rs`、`pkg/expression/builtin.rs`；确认生产分派直接使用 `tidb_shard`、`is_ipv4`、`is_ipv6`、`uuid_v1`，而 `SLEEP` 使用 `builtin_miscellaneous_vec_kernel::vec_sleep`。
- crate 边界：阅读 `pkg/expression/Cargo.toml`；确认 crate 名、lib 入口以及 `cipher = 0.4`、`des = 0.8`、`uuid = 1`（features `v1`/`v4`/`v7`）。
- Go 对照：阅读 `pkg/expression/builtin_miscellaneous.go`；核对 function class、`eval*`、会话锁、SLEEP、UUID、INET、Vitess/TIDB_SHARD 和不可用函数分支。
- Rust 独立测试：阅读 `pkg/expression/builtin_miscellaneous_test.rs` 与 `pkg/expression/builtin_miscellaneous_18_aster_unit_test.rs`；后者额外验证 UUID 时间戳、固定哈希向量、锁名/告警/错误映射、kill 和固定错误。测试文件保持独立，符合仓库约束。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅执行任务指定的 11 章节结构验证，并人工复核本文没有把未发现生产调用的 API 描述为已完成 SQL 接线。
