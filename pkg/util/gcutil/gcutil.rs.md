# `pkg/util/gcutil/gcutil.rs`

## 文件定位

`gcutil.rs` 是 Rust crate `astersql-util-gcutil` 的核心实现文件，处理 TiDB GC 全局开关、从 `mysql.tidb` 读取 GC safe point，以及基于 safe point 校验历史快照时间戳。crate 入口 [`lib.rs`](lib.rs) 以私有模块 `mod gcutil` 装载本文件，再用 `pub use gcutil::*` 暴露公开 API；根 crate 的 `pkg/lib.rs` 又通过 `facade_util_gcutil` 重导出它。

该文件属于 SQL 会话与 GC 元数据之间的工具边界，而不是 TiKV GC worker 本身：它通过抽象的会话能力读写 `tidb_gc_enable`，并执行受限 SQL 查询 `tikv_gc_safe_point`，不调度 GC、不推进 safe point，也不直接访问 TiKV/PD。对应的 Go 实现是 [`gcutil.go`](gcutil.go)。

RustCodeGraph 将本文件识别为含 27 个符号的 Rust 文件，并能验证文件内部调用边。仓库搜索只发现同目录 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 直接调用这里的 Rust API；若干 crate 在 Cargo manifest 中声明 `astersql-util-gcutil` 依赖，但这不足以证明这些 API 已接入其 Rust 运行时路径。

## 核心职责

1. 通过 `CheckGCEnable`、`DisableGC`、`EnableGC` 对 `vardef::TiDBGCEnable` 做窄范围读写，并沿用 `variable::TiDBOptOn`、`vardef::On`、`vardef::Off` 的统一变量语义。
2. 通过 `GetGCSafePoint` 以固定的 HIGH_PRIORITY 受限 SQL 查询 `mysql.tidb`，要求结果恰好一行且第 0 列存在，再把兼容格式的时间字符串转换为 TSO。
3. 通过 `ValidateSnapshot` 和 `ValidateSnapshotWithGCSafePoint` 保证 `snapshotTS >= safePointTS`；不满足时生成带稳定 MySQL 错误码 8055 的 `SnapshotTooOld`。
4. 用 `Context`、`GlobalVarAccessor`、`RestrictedSqlExecutor` 等窄 trait 隔离完整会话实现，避免为这一工具复制 Go `sessionctx.Context` 的庞大方法集。
5. 在 `GcUtilError` 中区分依赖错误、结果形状错误、时间解析错误和快照过旧，并为快照过旧错误保留可查询的稳定错误码。

本文件不缓存 safe point，不拥有后台任务，也不负责 GC 生命周期编排。每次 `ValidateSnapshot` 都重新读取 safe point；已经持有 safe point 的调用者可使用纯比较入口 `ValidateSnapshotWithGCSafePoint`。

## 主要符号

- `selectVariableValueSQL: &str`：公开常量，值为带 `HIGH_PRIORITY` 的参数化查询；参数占位符 `%?` 与 Go 受限 SQL 调用约定一致。
- `GC_SAFE_POINT_VARIABLE`：查询参数 `tikv_gc_safe_point`。
- `GC_TIME_FORMAT`：只用于构造解析失败消息的 Go 风格格式描述，不是 chrono 实际格式串。
- `INTERNAL_TXN_GC`：受限 SQL 上下文中的内部来源标记 `"gc"`。
- `TSO_LOGICAL_BITS`：TSO 低 18 位的逻辑部分宽度；时间转 TSO 时物理毫秒左移 18 位，反向展示时右移 18 位。
- `GcUtilError`：公开错误枚举。
  - `Dependency(sessionctx::GoError)` 通过 `#[from]` 透明包装全局变量或受限 SQL 依赖错误。
  - `MissingSafePoint` 表示查询结果不是恰好一行，或唯一行缺少第 0 列。
  - `InvalidSafePointTime { value }` 保留无法解析的原字符串。
  - `SnapshotTooOld { code, message }` 保存稳定错误码和已格式化消息；`code()` 只对该变体返回 `Some(code)`。
- `GlobalVarAccessor: Send + Sync`：读取、写入全局系统变量的窄接口。setter 使用 `&self`，把同步责任留给具体会话适配器的内部可变性机制。
- `RestrictedSqlContext`：受限 SQL 请求元数据，仅保存可选的静态内部来源；构造辅助 `with_internal_source_type` 为私有，`internal_source_type` 公开供执行器读取。
- `RestrictedRow`：字符串列容器；`new` 构造行，`get_string` 做有界下标访问并返回借用。
- `RestrictedSqlExecutor: Send + Sync`：接收上下文、SQL 和字符串参数切片并返回行集合的窄接口。
- `Context`：聚合 `GlobalVarAccessor` 与 `RestrictedSqlExecutor` 两种能力的公开 trait。
- `CheckGCEnable(ctx)`：读取 `tidb_gc_enable` 并用 `TiDBOptOn` 判断 ON/1 等真值。
- `DisableGC(ctx)` / `EnableGC(ctx)`：分别写入标准值 `OFF` / `ON`。
- `ValidateSnapshot(ctx, snapshotTS)`：先动态读取 safe point，再委托给比较函数。
- `ValidateSnapshotWithGCSafePoint(snapshotTS, safePointTS)`：不访问外部状态的边界比较函数。
- `GetGCSafePoint(ctx)`：受限 SQL、结果验证、兼容时间解析和 TSO 转换的主入口。
- `compatible_parse_gc_time`、`parse_gc_time_prefix`：私有时间解析链。
- `ts_convert_to_time`、`format_go_time`：私有 TSO 展示链，用于构造快照过旧错误消息。

文件没有条件编译项；测试条件编译位于相邻 `lib.rs`，它把独立测试文件挂为 `#[cfg(test)]` 模块。

## 执行流程

GC 开关读取流程如下：

1. `CheckGCEnable` 从 `Context::global_vars_accessor` 取得访问器。
2. 用 `vardef::TiDBGCEnable` 调用 `get_global_sys_var`。
3. 依赖错误经 `?` 转为 `GcUtilError::Dependency`；成功值交给 `variable::TiDBOptOn`，最终返回布尔值。

开关写入流程更短：`DisableGC` 或 `EnableGC` 取得同一访问器，分别以 `vardef::Off` 或 `vardef::On` 调用 `set_global_sys_var`，成功后返回 `Ok(())`。函数本身不维护旧值，也不提供自动恢复守卫。

safe point 读取与转换流程如下：

1. `GetGCSafePoint` 从默认 `RestrictedSqlContext` 构造带 `internal_source_type = "gc"` 的上下文。
2. 通过 `Context::restricted_sql_executor` 执行 `selectVariableValueSQL`，唯一参数是 `tikv_gc_safe_point`。
3. 结果行数必须等于 1；零行或多行均返回 `MissingSafePoint`。唯一行还必须有第 0 列，否则同样返回该错误。
4. `compatible_parse_gc_time` 先尝试直接解析整个值；若失败，使用 `rsplit_once(' ')` 仅去掉最后一个空格分段再试，以兼容旧 client-go 值末尾的时区缩写。
5. `parse_gc_time_prefix` 接受 `%Y%m%d-%H:%M:%S%.f %z`，并以不含小数秒的 `%Y%m%d-%H:%M:%S %z` 作为后备。
6. 转换逻辑先以 wrapping 的 `i64` 秒、纳秒运算构造 Unix 纳秒，再除以一百万得到向零截断的物理毫秒；随后仍以 `i64` 左移 18 位，最后转为 `u64`。这保留了 Go `oracle.GoTimeToTS` 对 epoch 前时间的行为。

快照校验流程如下：

1. `ValidateSnapshot` 调用 `GetGCSafePoint`，读取失败立即返回对应错误。
2. `ValidateSnapshotWithGCSafePoint` 判断 `safePointTS > snapshotTS`。相等是合法边界，只有 snapshot 严格更早才失败。
3. 失败时取 `variable::error::ErrSnapshotTooOld` 描述符，将 safe point 的 TSO 右移 18 位还原物理毫秒并转换为 UTC。
4. `format_go_time` 输出 Go `time.Time.String()` 风格的 `YYYY-MM-DD HH:MM:SS[.fraction] +0000 UTC`；小数部分只在非零时出现，并去掉尾随零。
5. 描述符生成最终消息，返回 `GcUtilError::SnapshotTooOld { code, message }`。

## 数据与状态

本文件自身没有全局可变状态。常量定义协议值，调用期间的状态均为局部值或由调用方提供：

- GC 开关的权威状态位于全局系统变量存储，由 `GlobalVarAccessor` 访问。
- GC safe point 的权威文本值位于 `mysql.tidb.variable_value`，查询键为 `tikv_gc_safe_point`。
- `RestrictedSqlContext` 和 `RestrictedRow` 是拥有其字段的轻量值对象；前者可克隆，后者持有 `Vec<String>`。
- TSO 使用 `u64` 表示，约定高位是物理毫秒、低 18 位是逻辑计数。safe point 从时间字符串生成时逻辑部分为零。
- `GetGCSafePoint` 不缓存查询结果，因此连续调用可以看到不同 safe point，也会重复产生受限 SQL 开销。

关键不变量是：查询成功需要唯一一行且有第 0 列；快照合法条件是 `snapshotTS >= safePointTS`；时间转 TSO 必须保持 Go 的有符号、向零截断和回绕顺序，而不能简单改为无符号算术。

## 依赖与调用关系

直接依赖由 [`Cargo.toml`](Cargo.toml) 给出：

- `chrono`：解析带固定偏移的 GC 时间，并在错误消息中格式化 UTC 时间。
- `sessionctx`：提供可透明包装的 `GoError` 类型；本文件没有直接要求完整的 `sessionctx::Context`。
- `thiserror`：派生 `GcUtilError` 的展示和错误来源转换。
- `vardef`：提供 `TiDBGCEnable`、`On`、`Off` 协议常量。
- `variable`：提供 `TiDBOptOn` 和 `ErrSnapshotTooOld` 错误描述符。

RustCodeGraph 验证的关键内部调用边包括：

- `ValidateSnapshot -> GetGCSafePoint`；
- `ValidateSnapshot -> ValidateSnapshotWithGCSafePoint`；
- `ValidateSnapshotWithGCSafePoint -> ts_convert_to_time -> format_go_time`（后两者都由校验函数用于错误展示；图中记录为该函数的直接 callee）；
- `GetGCSafePoint -> Context::restricted_sql_executor -> RestrictedSqlExecutor::exec_restricted_sql`；
- `GetGCSafePoint -> RestrictedRow::get_string -> compatible_parse_gc_time`；
- `compatible_parse_gc_time -> parse_gc_time_prefix`；
- 开关函数通过 `Context::global_vars_accessor` 分别调用 getter 或 setter。

`lib.rs` 将全部公开符号重导出。工作区根 `Cargo.toml` 以 `facade_util_gcutil` 引入该 crate，`pkg/lib.rs` 再在 `pkg::util::gcutil` 门面中重导出。`pkg/ddl`、`pkg/executor`、`pkg/server/handler/tikvhandler` 及部分测试 crate 的 manifest 声明了此依赖；当前精确 Rust 搜索未发现这些公开函数在同目录测试之外的直接调用，因此不能据此断言已经存在完整 Rust 生产调用链。Go 侧则有 DDL、executor/recover 等测试调用开关函数，这是 Go 实现的使用证据，不等同于 Rust 接线证据。

## 错误处理与边界

- 全局变量 getter/setter 和受限 SQL 执行器的 `sessionctx::GoError` 通过 `GcUtilError::Dependency` 原样展示；工具层不重试、不改写消息。
- 受限 SQL 返回零行、多行或唯一行缺少第 0 列，都归并为 `MissingSafePoint`，消息固定为 `can not get 'tikv_gc_safe_point'`。
- 时间解析仅接受紧凑日期时间、可选小数秒和数字时区偏移；兼容路径最多移除一个尾部空格字段。因此 `+0800 CST` 可接受，而多余的两个尾部字段仍失败。
- `ValidateSnapshotWithGCSafePoint` 的边界是严格大于：safe point 与 snapshot 相等时允许读取。
- 只有 `SnapshotTooOld` 暴露稳定代码，`GcUtilError::code()` 对其他错误返回 `None`。
- `ts_convert_to_time` 对无法表示为 chrono UTC 时间的物理毫秒使用 `expect`，会 panic；正常 TiDB TSO 应满足其可表示范围。若外部不可信值可能进入 `ValidateSnapshotWithGCSafePoint`，扩展时需决定是否把该 panic 改为显式错误，并同步 Go 兼容性判断。
- 时间转 TSO 有意使用 `wrapping_mul`、`wrapping_add`、`wrapping_shl` 和最终 `as u64`。这不是普通溢出保护，而是对 Go 有符号中间运算的兼容；修改前必须覆盖 epoch 前与极值输入。

## 并发与资源生命周期

`GlobalVarAccessor` 与 `RestrictedSqlExecutor` 要求 `Send + Sync`，允许共享会话适配器被并发调用。它们的方法只接收 `&self`，具体实现若要记录状态、复用连接或维护事务，必须自行提供内部同步；本文件不加锁，也不规定锁粒度。

一次 `GetGCSafePoint` 调用创建并按值传递 `RestrictedSqlContext`，借用 SQL 常量和参数字符串，拥有执行器返回的 `Vec<RestrictedRow>`，函数返回后这些临时资源自然释放。代码没有启动线程、异步任务、通道或显式事务，也没有保存行或上下文的跨调用引用。

`DisableGC`/`EnableGC` 是独立写操作，不组成 RAII 生命周期：调用者若临时关闭 GC，必须自己确保后续恢复，并处理并发调用者可能改变同一全局变量的竞态。本工具也不保证读取开关与随后操作之间的原子性。

## 与 Go 版本的对应关系

Rust 的六个公开操作与 [`gcutil.go`](gcutil.go) 一一对应：`CheckGCEnable`、`DisableGC`、`EnableGC`、`ValidateSnapshot`、`ValidateSnapshotWithGCSafePoint`、`GetGCSafePoint`。固定 SQL、变量名、ON/OFF 值、safe point 比较方向和 SnapshotTooOld 描述符保持一致。

主要适配差异如下：

- Go 直接接收完整 `sessionctx.Context`；Rust 定义 `Context` 窄 trait，并把全局变量和受限 SQL 能力进一步拆成两个 `Send + Sync` trait。
- Go 用 `context.Background()` 和 `kv.WithInternalSourceType(..., kv.InternalTxnGC)`；Rust 用值对象 `RestrictedSqlContext` 保存等价的 `"gc"` 标记。
- Go 的受限 SQL 返回 chunk row；Rust 用仅含字符串列的 `RestrictedRow` 表示本文件所需最小数据面。
- Go 用 `util.CompatibleParseGCTime` 与 `oracle.GoTimeToTS`；Rust 在本文件内重建解析兼容和转换顺序。独立测试覆盖旧时区缩写、小数秒、非法尾部字段以及 epoch 前毫秒截断。
- Go 以 `errors.Trace` 保留错误栈；Rust 通过透明 `Dependency` 保留依赖错误值和展示文本，但没有模拟 Go 栈语义。
- Go 的快照过旧错误是 terror/error descriptor 生成的错误；Rust 把稳定 code 与最终 message 存入 `GcUtilError::SnapshotTooOld`，通过 `code()` 暴露 code。

Go 同路径没有专属 `gcutil_test.go`；Rust 的行为证据集中在独立文件 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。仓库中的 Go DDL/executor 测试能证明 Go 开关 API 的真实使用，但不能替代 Rust 接线测试。

## 扩展指南

- 新增依赖会话的能力时，优先扩展现有窄 trait 或新增同样窄的 trait；不要仅为便利把整个会话对象耦合进本 crate。适配器需要继续满足 `Send + Sync`。
- 修改 safe point SQL、变量名或 internal source 时，应同时更新 `GetGCSafePoint`、相关常量与 `get_gc_safe_point_uses_the_exact_sql_internal_source_and_go_time_conversion`，并核对 Go `gcutil.go`。
- 扩展时间格式时，应在 `compatible_parse_gc_time`/`parse_gc_time_prefix` 做最小变更，并在独立测试文件的有效值和无效值表中加入用例；避免过度宽松地接受 Go 不接受的字符串。
- 修改 TSO 转换时，必须保留或有意说明 `i64` 中间值、纳秒到毫秒向零截断、左移 18 位和最终转 `u64` 的顺序，并同步 epoch 前测试。
- 修改快照边界或错误消息时，应在 `ValidateSnapshotWithGCSafePoint` 附近实施，更新相等/早一 tick/晚一 tick 用例，并继续断言错误码 8055 与完整消息。
- 若接入真实 Rust 会话实现，应为 `Context` 两个 accessor 提供生产适配器，并增加独立集成测试，证明受限 SQL 的内部来源、错误传播和变量读写；不要把测试模块内嵌回 `gcutil.rs`。
- 临时关闭 GC 的新工作流需要在更上层设计恢复策略或守卫，本文件目前只有无状态的单次 setter，不应被误认为自动恢复机制。
- 任何兼容性变更都应同时检查 [`gcutil.go`](gcutil.go) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)；性能风险主要来自重复受限 SQL 和不必要的分配，正确性风险集中在时间解析、TSO 算术和全局开关恢复。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件，目标目录包含 `gcutil.rs`、`lib.rs`、`migration_aster_unit_test.rs` 和 Go 对照 `gcutil.go`。
- RustCodeGraph `node --file pkg/util/gcutil/gcutil.rs --offset 1 --limit 400`：读取本文件完整 259 行和 27 个符号，并确认内部实现。
- RustCodeGraph 对 `GetGCSafePoint`、`ValidateSnapshot`、`ValidateSnapshotWithGCSafePoint`、`CheckGCEnable`、`EnableGC`、`DisableGC` 的 `query`/`callees`：确认上述内部调用边。`callers` 对同名 Go/Rust 符号未返回可用的目标限定结果，因此上游使用情况改由精确仓库搜索核验，没有把图缺失解释为不存在调用。
- [`Cargo.toml`](Cargo.toml)：确认 crate 名称、`lib.rs` 入口、五项直接依赖和 `go-package = "pkg/util/gcutil"` 移植元数据。
- [`lib.rs`](lib.rs) 与 `pkg/lib.rs`：确认模块装载、公开重导出、测试模块挂载和工作区 facade。
- [`gcutil.go`](gcutil.go)：核对六个对应操作、固定 SQL、变量访问、内部事务来源、时间解析/TSO 转换及快照边界。
- [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)：核对 ON/1 读取，OFF/ON 写入，依赖错误透传，SQL/参数/`gc` 来源，零行/多行，小数秒与旧时区缩写，非法时间，epoch 前转换，以及相等边界和错误码 8055。
- `rg` 对 Rust 函数名、crate 名和重导出名的搜索：确认同目录迁移测试是当前可见的直接 Rust API 调用者；Cargo 声明和 facade 重导出作为可达性证据单独记录，不冒充运行时调用证据。

本任务是纯文档分析，依计划未运行 Cargo。交付前另以任务规定的命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核没有把未验证的 Rust 生产接线写成既成事实。
