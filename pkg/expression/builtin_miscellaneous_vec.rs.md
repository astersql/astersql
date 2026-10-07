# `pkg/expression/builtin_miscellaneous_vec.rs`

源文件：[`builtin_miscellaneous_vec.rs`](builtin_miscellaneous_vec.rs)。本文只描述当前仓库中的实现与接线状态；Go 文件用于核对移植语义，不代表所有 Rust 函数都已经接入生产向量执行框架。

## 文件定位

本文件属于 `astersql-expression` crate。`pkg/expression/Cargo.toml` 将 crate 根指定为 `lib.rs`；`lib.rs:243-244` 以 `builtin_miscellaneous_vec_kernel` 私有模块名装入本文件，并在 `cfg(test)` 下通过 `builtin_miscellaneous_vec` 重导出，供独立测试使用（`lib.rs:789-791`）。

它是 Go `pkg/expression/builtin_miscellaneous_vec.go` 的 Rust 列式算法层：输入通常是 `&[Option<T>]`，输出保持相同行序并以 `Option` 表示 SQL NULL。当前生产接线并不等于 Go 文件的完整 `vecEval*` 框架：RustCodeGraph 的调用证据表明，`builtin.rs::CoreBuiltin::evalInt` 的 `CoreBuiltinKind::Sleep` 分支会调用 `vec_sleep`；其余公开列函数在当前索引中主要由 `builtin_miscellaneous_vec_17_aster_unit_test.rs` 和 `builtin_miscellaneous_vec_test.rs` 调用。因此，本文件既包含已接线的 SLEEP kernel，也包含可供后续表达式签名委托的、经过独立测试的迁移算法。

## 核心职责

- 批量透传：`vec_any_value` 返回第一参数列的克隆，`vec_name_const` 返回值参数列的克隆；`vec_int_any_value_string` 在 hybrid 字段时保留原始字符串/二进制表示，否则使用普通整数转字符串后的回退列。
- IP 地址：`vec_inet_ntoa`、`vec_inet_aton`、`vec_inet6_aton`、`vec_inet6_ntoa` 完成文本、整数和 4/16 字节表示之间的转换；`vec_is_ipv4`、`vec_is_ipv6`、`vec_is_ipv4_compat`、`vec_is_ipv4_mapped` 执行格式或前缀判定。
- UUID：`vec_is_uuid`、`vec_uuid_v1/v4/v7`、`vec_uuid_version`、`vec_uuid_timestamp`、`vec_uuid_to_bin`、`vec_bin_to_uuid` 负责解析、生成、版本/时间戳提取和可选时间字段重排。
- Vitess Hash：`vec_vitess_hash` 使用全零 DES 密钥对大端 `u64` 块加密，复现 Go 的 Vitess 哈希算法。
- SLEEP：`vec_sleep` 按行处理休眠、严格错误/宽松 warning、kill 轮询及中断后剩余行填 `1`；`SleepSession` 保存本实现所需的最小会话状态。
- 清单：`VECTORIZED_SIGNATURES` 记录对应 Go 文件中 32 个 `vectorized() == true` 的签名名，是覆盖清单，不是 Rust 生产注册表。

## 主要符号

- `UUID_STR_LEN: usize = 36`：标准连字符 UUID 的字符串长度，用于说明生成结果契约。
- `VECTORIZED_SIGNATURES: [&str; 32]`：Go 向量化签名清单，测试会逐项比对内容与顺序。
- `EvalError`：统一承载三类本地错误：`IncorrectArguments`、带函数名和原值的 `WrongValueForType`、以及值列/flag 列长度不一致的 `MismatchedColumnLength`；实现了 `Display` 和 `std::error::Error`。
- `DecimalSeconds(i64)`：以整数微秒精确表示 `DECIMAL(20,6)` 秒，`Display` 固定输出六位小数；`from_micros`/`micros` 提供构造和读取。
- `InvalidArgumentMode::{Error, Warning}`：决定 SLEEP 的 NULL/负数参数是立即失败还是累计 warning。
- `SleepSession`：用 `AtomicBool` 保存 kill 状态，并保存是否涉及表、INSERT、UPDATE、DELETE 的四个布尔条件；`with_statement_side_effects`、`send_kill_signal`、`is_killed` 是外部接口，`reset_if_plain_select` 是内部复位规则。
- `SleepOutcome`：返回逐行 `values: Vec<i64>` 与 `warnings: usize`。
- 私有辅助函数：`is_ipv4` 执行严格四段十进制检查；`parse_google_uuid` 兼容 google/uuid 接受的裸串、URN 和 38 字节包装形式；`uuid_timestamp_micros` 解码 v1/v6/v7 时间字段；`validate_flags_len`、`swap_binary_uuid`、`swap_string_uuid` 支持 UUID 二进制转换；`vitess_hash` 和 `do_sleep` 分别实现单值哈希和单次可中断休眠。

## 执行流程

1. 一般列函数遍历输入切片，为每行保留 `None`，只对 `Some` 执行转换或判定，最终收集为等长结果列。`vec_any_value` 和 `vec_name_const` 则直接克隆已求值列。
2. IPv4 文本判定由 `is_ipv4` 手工扫描 ASCII 数字和点，要求恰好三个点、段非空且每段不超过 255。`vec_inet_aton` 使用另一套兼容 MySQL 的缩写解析：每遇到点便左移 8 位，结束时依据点数补齐缺省字节，所以 `127`、`127.255` 等写法可被接受。
3. `vec_inet6_aton` 借助 `std::net::IpAddr` 解析，IPv4 输出 4 字节、IPv6 输出 16 字节；反向的 `vec_inet6_ntoa` 按长度分派，并对 `00..00 ff ff` 前缀单独输出 `::ffff:a.b.c.d`。
4. UUID 解析先经过 `parse_google_uuid`。版本函数读取 UUID version nibble；时间戳函数对 v1/v6 解码 100ns 时间并减去 Gregorian/Unix 纪元差，对 v7 读取前 48 位毫秒，最后转换成微秒；无时间语义的版本返回 NULL，而语法错误返回错误。
5. `vec_uuid_to_bin`/`vec_bin_to_uuid` 先验证可选 flag 列长度。NULL 值直接传播；flag 为 NULL 或 0 时保持标准布局，非零时交换 UUID 的 time-low、time-mid、time-high 字段。文本转二进制还显式拒绝首尾空白。
6. `vec_vitess_hash` 将每个非 NULL `u64` 写为大端 8 字节块，交给 `NULL_KEY_DES` 加密，再读回大端整数。`LazyLock<Des>` 使密钥调度只初始化一次。
7. `vec_sleep` 预置全零结果并顺序处理每一行。NULL 或负数按 `InvalidArgumentMode` 报错/计 warning；超出显式浮点上限时报错；零、NaN 以及转换到 Go `time.Duration` 会溢出的有限值立即完成。实际休眠由 `do_sleep` 每最多 10ms 检查 kill；一旦中断，当前行及后续行全填 `1` 并提前返回。

## 数据与状态

绝大多数函数是无共享可变状态的纯列转换：结果新分配，输入只读，NULL 和行序保持。UUID 生成函数按 `rows` 次数产生新字符串；v1 使用固定节点 ID `02:00:00:00:00:01`，时间部分仍取当前时刻，v4 使用随机源，v7使用当前时间和库提供的随机字段，因此生成结果本身不可复现。

`DecimalSeconds` 避免用浮点表达 UUID 时间戳，保证六位小数精度。UUID swap flag 与值列必须等长；每行 `None` flag 按“不交换”处理。`SleepOutcome.values` 长度始终等于输入长度，warning 仅计 NULL/负数的宽松处理次数。

共享状态只有 `NULL_KEY_DES` 与 `SleepSession.killed`。DES 实例只用于不可变的块加密调用；kill 标志通过 Release 写、Acquire 读跨线程可见。`SleepSession` 中的语句属性在构造后不可变，用于判断 kill 是否允许复位。

## 依赖与调用关系

- crate 装配：`pkg/expression/lib.rs:243-244` → `builtin_miscellaneous_vec.rs`；测试别名位于 `lib.rs:789-791`。`Cargo.toml` 直接声明本文件使用的 `des = "0.8"`、`cipher = "0.4"` 和带 `v1/v4/v7` feature 的 `uuid = "1"`。
- 已确认生产边：RustCodeGraph `node vec_sleep` 给出 `builtin.rs::evalInt` → `vec_sleep`；源码 `builtin.rs:4190-4201` 表明 `CoreBuiltinKind::Sleep` 先标量求值参数，再以单行切片、默认 `SleepSession` 和 `Error` 模式调用本文件。
- UUID 内部边：`vec_uuid_timestamp` → `parse_google_uuid`、`uuid_timestamp_micros`、`wrong_value`；`vec_uuid_to_bin` → `validate_flags_len`、`parse_google_uuid`、`swap_binary_uuid`；`vec_bin_to_uuid` → `validate_flags_len`、`swap_string_uuid`。
- IP 与哈希内部边：公开列函数调用 `std::net` 解析/格式化；`vec_vitess_hash` → `vitess_hash` → `NULL_KEY_DES.encrypt_block`。
- SLEEP 内部边：`vec_sleep` → `do_sleep` → `SleepSession::{is_killed,reset_if_plain_select}`，底层使用 `Instant`、`Duration` 和 `thread::sleep`。
- 当前未确认有生产调用的公开列函数不应被视作完整 SQL 接线。RustCodeGraph 对 `vec_uuid_timestamp`、`vec_bin_to_uuid`、`vec_vitess_hash`、`vec_inet6_aton` 只报告测试导入/调用；新增接线应从表达式签名或 `CoreBuiltinKind` 分派处显式委托，而不能只把名字加入 `VECTORIZED_SIGNATURES`。

## 错误处理与边界

- 一般无效 IP 输入返回 NULL/0，不返回 `EvalError`。`vec_inet_ntoa` 只接受 `0..=u32::MAX`；`vec_inet6_ntoa` 只接受 4 或 16 字节；IPv4 compat/mapped 必须正好 16 字节且匹配各自 12 字节前缀。
- `parse_google_uuid` 对非 ASCII 直接交给 `Uuid::parse_str`；ASCII 输入支持大小写不敏感的 `urn:uuid:` 前缀，并刻意保持 google/uuid 对任意 38 字节首尾包装字符的历史行为。`vec_is_uuid` 和 `vec_uuid_to_bin` 额外要求没有首尾空白。
- UUID 语法错误由版本、时间戳和二进制转换函数映射为 `WrongValueForType`；无时间戳的合法 UUID 在 `vec_uuid_timestamp` 中返回 NULL。二进制 UUID 长度不是 16 时同样报错。
- flag 列与值列长度不同会在逐行索引前返回 `MismatchedColumnLength`，避免越界或静默截断；NULL flag 等价于 0。
- SLEEP 的 NULL/负数由调用方选择错误或 warning；显式极大浮点值返回 `IncorrectArguments`。`do_sleep` 将 NaN 和非正数视为无需休眠；超过 `i64` 纳秒范围但未触及显式 `f64::MAX / 1e9` 上限的值按 Go 转换语义立即完成。
- `swap_string_uuid` 依赖调用方传入已规范化的 36 字节 ASCII UUID，使用 `debug_assert` 表达该内部不变量；它不是面向任意字符串的公开解析函数。

## 并发与资源生命周期

本文件不创建长期任务、通道、锁或事务。每次列求值拥有自己的输入借用和结果 `Vec`，没有跨调用缓存；`NULL_KEY_DES` 是进程期惰性初始化的只读对象。

`SleepSession` 可安全地由多个线程共享 kill 标志：测试通过 `Arc<SleepSession>` 在辅助线程发送 kill，求值线程每 10ms 左右轮询。普通 SELECT（无表 ID，且不是 INSERT/UPDATE/DELETE）被中断后清除 kill；有表或写语句副作用时保留 kill，避免吞掉上层应观察到的中断。`do_sleep` 使用同步 `thread::sleep` 阻塞当前线程，不是异步定时器；中断检测粒度和最坏额外延迟约为 10ms。

UUID v1/v4/v7 的时间/随机源以及标准库 IP 解析均由依赖库管理，无本文件级资源需要释放。所有临时 `Vec`、字符串和 DES 块在调用结束时按 Rust 所有权自动释放。

## 与 Go 版本的对应关系

Go 对照主文件为 `pkg/expression/builtin_miscellaneous_vec.go`，标量/函数类与更多边界测试位于 `builtin_miscellaneous.go`、`builtin_miscellaneous_test.go`，向量测试位于 `builtin_miscellaneous_vec_test.go`。

- Rust 的 `VECTORIZED_SIGNATURES` 对应 Go 文件内 32 个返回 `true` 的 `vectorized()` 方法；泛型 `vec_any_value`/`vec_name_const` 合并了 Go 按 JSON、Real、String、Decimal、Duration、Int、Time 分开的 `vecEval*` 方法。
- IP 算法保留 Go 的 NULL、非法值、缩写 IPv4、mapped/compat 前缀与文本格式语义；Rust 使用 `std::net` 代替 Go `net.ParseIP` 的缓冲区写入过程。
- UUID 生成对应 Go `uuid.NewUUID`、`NewRandom`、`NewV7`；解析兼容 google/uuid 的已测试形式。时间戳仍只对 v1/v6/v7产生值，Rust 以 `DecimalSeconds` 的整数微秒替代 Go `MyDecimal`。
- UUID_TO_BIN/BIN_TO_UUID 保持 Go 的首尾空白严格检查、NULL flag 等同不交换、非零 flag 交换及错误类型意图。Rust 额外显式检查两列长度，因为接口接收独立切片；Go chunk 保证相同行数。
- SLEEP 对齐 Go `vecEvalInt`/`doSleep` 的严格与 warning 分支、10ms kill 轮询、剩余行填 1 和普通 SELECT 复位规则；Rust 的 `SleepSession` 是最小状态模型。当前生产 `CoreBuiltinKind::Sleep` 使用默认会话且强制 Error 模式，因此尚未把真实会话 killer、语句错误级别完整传入，此限制不能等同于 Go 的完整 session 接线。
- Rust 独立测试 `builtin_miscellaneous_vec_17_aster_unit_test.rs` 覆盖从 Go 用例迁移的主要边界；`builtin_miscellaneous_vec_test.rs` 提供较小的接线冒烟集和 duration 溢出回归。Go 的向量框架测试还覆盖真实 `chunk.Column`、allocator、`EvalContext` 与 benchmark，这些框架层能力不在本文件的切片 API 中。

## 扩展指南

- 新增或迁移杂项向量函数时，先在本文件增加保持等长、行序和 NULL 语义的列 kernel；若需要错误，复用或扩展 `EvalError`，不要以 NULL 掩盖 Go 原本的错误路径。
- 接入 SQL 运行时必须同步修改真实表达式分派/签名层，并用 RustCodeGraph 确认生产 caller；仅更新 `VECTORIZED_SIGNATURES` 或增加测试导入不构成生产接线。
- 增加 Go 已向量化签名时同步维护 `VECTORIZED_SIGNATURES` 及 `builtin_miscellaneous_vec_17_aster_unit_test.rs` 的精确清单断言。新测试应继续放在独立 `*_test.rs` 文件，不嵌入生产源文件。
- 修改 IP 解析时重点回归省略段、空段、溢出段、IPv4-mapped IPv6、4/16 字节长度和 NULL。标准库解析器与 Go `net` 的接受集合可能不同，任何替换都应以 Go 测试样例核验。
- 修改 UUID 时同时回归裸 32 位十六进制、连字符、URN、38 字节包装、首尾空白、v1/v6/v7 时间戳、16 字节约束和双向 swap；不要破坏 `DecimalSeconds` 的微秒精度。
- 修改 SLEEP 时必须保留严格/宽松模式、浮点溢出、kill 轮询、普通 SELECT 复位和有副作用语句保留 kill 的不变量；将其接入真实会话时应替换/桥接 `SleepSession`，而不是另造一套未同步的状态。
- 性能方面，当前通用接口会为结果列和多数字符串逐行分配；若接入真正的批量执行器，应复用 chunk/column 缓冲区，但必须保持本文件已验证的行为。SLEEP 是有意的同步阻塞路径，不宜在普通计算线程池中无界并发。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；精确 `query` 定位了 `vec_sleep`、`vec_uuid_to_bin`、`SleepSession`、`vec_inet_aton`。
- RustCodeGraph `node vec_sleep`：确认其调用 `do_sleep`、构造 `SleepOutcome`，生产调用者为 `builtin.rs::evalInt`，测试调用者为两个独立 Rust 测试文件。
- RustCodeGraph `node vec_uuid_to_bin`：确认其调用 `validate_flags_len`、`wrong_value`、`swap_binary_uuid`；调用者仅见独立测试。`node vec_uuid_timestamp`、`vec_bin_to_uuid`、`vec_vitess_hash`、`vec_inet6_aton` 进一步核对了内部边和当前测试侧调用状态。
- 完整阅读：`pkg/expression/builtin_miscellaneous_vec.rs`、`pkg/expression/Cargo.toml`、`pkg/expression/builtin_miscellaneous_vec_test.rs`；读取模块接线 `pkg/expression/lib.rs` 与生产委托 `pkg/expression/builtin.rs` 的相邻代码。
- Go 对照：`pkg/expression/builtin_miscellaneous_vec.go`、`pkg/expression/builtin_miscellaneous_vec_test.go`，并检索 `pkg/expression/builtin_miscellaneous.go`、`pkg/expression/builtin_miscellaneous_test.go` 的对应函数与边界测试。
- 扩展测试证据：`pkg/expression/builtin_miscellaneous_vec_17_aster_unit_test.rs` 覆盖 IP、UUID、透传、SLEEP、Vitess Hash 和签名清单；`builtin_miscellaneous_vec_test.rs` 覆盖冒烟路径与 Go `time.Duration` 溢出行为。
- 结构验收使用任务指定命令，要求本文存在且固定二级标题恰好 11 个。本任务是纯文档分析，按计划不运行 Cargo 或代码测试。
