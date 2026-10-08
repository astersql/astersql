# `pkg/util/importer/data.rs`

## 文件定位

[`data.rs`](data.rs) 属于 `astersql-util-importer` crate；crate 入口 [`lib.rs`](lib.rs) 公开 `data` 模块并重导出其公开项。它不是数据库通用的 `Datum` 值类型，而是导入/压测数据生成器使用的“每列唯一值游标”。DDL 解析器在 [`parser.rs`](parser.rs) 的 `parse_table_sql` 中为每个 `Column` 创建一个 `Arc<Datum>`，行生成逻辑再由 [`db.rs`](db.rs) 的 `generate_column_data` 按列类型调用这里的生成方法。

该文件位于如下局部主链中：

`parse_table_sql` 创建列及 `Arc<Datum>` → `generate_row_data`/`generate_column_data` 判断该列是否属于唯一索引 → `Datum` 生成确定性递进值 → 上层把结果拼成 `INSERT` SQL。

[`Cargo.toml`](Cargo.toml) 将该目录定义为独立库 `astersql-util-importer`，库入口是 `lib.rs`；当前普通 `[dependencies]` 为空。本文件只依赖标准库，以及同 crate 的 `rand::{ALPHABET, civil_from_days, days_from_civil}`。

## 核心职责

- 维护整数游标、步长和可选上界，用于唯一整数及浮点值生成（`DatumState`、`set_init_int64_value`、`unique_i64`、`unique_f64`）。
- 把递增整数按 `ALPHABET` 编码成受最大长度约束的 base62 风格字符串（`unique_string`）。
- 维护一条共享时间序列，生成 `TIME`、`DATE`、`DATETIME/TIMESTAMP` 和 `YEAR` 的 SQL 文本形态（`advance_seconds` 及四个公开时间方法）。
- 通过单个 `Mutex<DatumState>` 串行化同一列的并发生成，保证一次调用中的读取与推进不可交错。

它不负责识别唯一列、解析列规则、随机生成非唯一值、SQL 转义或执行 SQL；这些职责分别位于 [`parser.rs`](parser.rs)、[`rand.rs`](rand.rs) 和 [`db.rs`](db.rs)。

## 主要符号

- `DatumState`：私有状态容器。
  - `int_value` 是整数和字符串生成共享的游标；默认值为 `-1`。
  - `step` 默认是 `1`，也用于时间生成。
  - `min_int_value` 保存初始化下界，但生成阶段不再读取；真正的当前值在 `int_value` 中。
  - `max_int_value` 与 `use_range` 控制整数上界停驻。
  - `initialized` 使初始化只生效一次。
  - `time_seconds` 保存时间序列的 Unix 秒；`None` 表示尚未取首值。
- `pub struct Datum`：仅包含 `Mutex<DatumState>` 的公开生成器。`Default` 委托给 `Datum::new`。
- `Datum::new() -> Self`：创建未初始化实例，整数游标为 `-1`、步长为 `1`、范围关闭、时间为空。
- `set_init_int64_value(&self, step, minimum, maximum)`：第一次调用写入步长；`minimum != -1` 时把整数游标设为该下界；仅当 `minimum < maximum` 时启用上界。后续调用直接返回。
- `unique_i64(&self) -> i64`：返回当前值，再以 wrapping 加法推进；启用范围且“下一值大于上界”时返回当前值并停驻。
- `unique_f64(&self) -> f64`：调用 `unique_i64` 后做 Rust `as f64` 转换，因此与整数序列共享进度，也可能因大整数超过精确表示范围而丢失低位精度。
- `unique_string(&self, maximum_length) -> String`：先把共享整数游标饱和加一，然后反复按 `ALPHABET.len()` 取余、反转，最多输出 `maximum_length` 个字符。
- `advance_seconds(&self, multiplier) -> i64`：私有时间推进器。第一次取当前 Unix 秒；之后增加 `step * multiplier`，乘法和加法都使用饱和语义。
- `unique_time`、`unique_date`、`unique_timestamp`、`unique_year`：分别输出 `HH:MM:SS`、`YYYY-MM-DD`、`YYYY-MM-DD HH:MM:SS`、`YYYY`。

本文件没有 trait、枚举、模块级常量或条件编译项；公开 API 全部集中在 `Datum` 及其方法。

## 执行流程

1. [`parser.rs`](parser.rs) 解析 CREATE TABLE 时创建 `Column { data: Arc::new(Datum::new()), step, ... }`；同一列的生成任务共享这一个实例。
2. [`db.rs`](db.rs) 的 `generate_column_data` 通过 `table.unique_indices.contains(&column.name)` 决定走唯一序列还是随机序列。
3. 唯一整数先经 `unique_integer` 解析列的取值范围，再调用 `set_init_int64_value(column.step, minimum, maximum)`。由于初始化幂等，多行生成不会重置已经推进的游标；随后 `unique_i64` 返回当前值并尝试推进。
4. 唯一字符串直接调用 `unique_string(field_type.length)`；首次调用把 `-1` 加到 `0`，所以默认序列从 `"0"` 开始。编码从低位取余，最后反转为高位在前；长度为零时返回空串。
5. 唯一时间值第一次调用时以当前系统时间为种子。之后：`TIME`/`TIMESTAMP` 每次增加 `step` 秒，`DATE` 每次增加 `step * 86_400` 秒，`YEAR` 则解出民用日期、给年份增加 `step`，再合成为 Unix 秒。
6. `generate_column_data` 给文本/时间结果加单引号，`generate_row_data` 再把所有列拼为完整 `INSERT`；本文件自身只返回原始数字或格式化字符串。

整数范围有一个需要保留的细节：判定使用 `int_value.wrapping_add(step) > max_int_value`，但真正推进也使用相同的 wrapping 加法。因此在 `i64` 正上界附近会像 Go 的 `int64` 一样溢出到负值，而不是报错；[`data_test.rs`](data_test.rs) 的 `unique_i64_wraps_like_go_at_the_i64_boundary` 固定了这一行为。

## 数据与状态

整数/字符串状态与时间状态放在同一个 `DatumState` 中，并由同一把锁保护。正常调用链中一个 `Datum` 属于一列，而列的字段类型固定，因此通常只使用其中一类状态；API 本身没有禁止混合调用，若交替调用 `unique_i64` 与 `unique_string`，两者会共同改变 `int_value`。

`set_init_int64_value` 的 `minimum == -1` 是哨兵：它只设置步长，不覆盖默认或已有的 `int_value`。`minimum < maximum` 才启用范围，等值或反向范围均视为无范围。范围逻辑只有上界，没有对负步长的下界约束；调用者若引入负步长，不能把当前逻辑理解为双向区间检查。

达到上界前若下一步将越过上界，`unique_i64` 会持续返回当前值。这保证不超过上界，但意味着范围耗尽后不再具有“每次调用都唯一”的性质；文件没有耗尽错误或回卷策略。

时间序列在 `time_seconds` 中以 Unix 秒保存。`unique_time` 对一天秒数取欧几里得余数；`unique_date`/`unique_timestamp` 通过 `civil_from_days` 转成年月日；`unique_year` 使用 `days_from_civil` 写回调整年份后的日期并保留日内时钟。

## 依赖与调用关系

上游直接证据：

- [`parser.rs`](parser.rs)：`Column.data` 的类型是 `Arc<Datum>`；`parse_table_sql` 为每列构造实例，并从注释规则填充 `Column.step`。
- [`db.rs`](db.rs)：`unique_integer` 调用 `set_init_int64_value` 和 `unique_i64`；`generate_column_data` 根据 `FieldKind` 调用 `unique_string`、`unique_date`、`unique_timestamp`、`unique_time`、`unique_year`。
- [`lib.rs`](lib.rs)：公开 `data` 模块，并通过 `pub use data::*` 暴露 `Datum`。

下游依赖：

- `std::sync::Mutex` 提供内部可变性和线程间串行化。
- `std::time::{SystemTime, UNIX_EPOCH}` 提供时间序列首值；系统时间早于 Unix epoch 时由 `unwrap_or_default` 回退为零时长。
- [`rand.rs`](rand.rs) 的 `ALPHABET` 决定字符串编码表，`civil_from_days`/`days_from_civil` 完成 Unix 日数和公历日期互转。这里虽从 `rand` 模块导入，但日期换算是确定性的，不产生随机数。

RustCodeGraph 的文件节点确认 `data.rs` 被 `data_test.rs`、`db.rs`、`parser.rs`、`parser_test.rs`、`rand_test.rs` 等文件引用；精确方法名查询未建立可独立寻址的方法节点，因此具体调用边又用上述相邻源码和精确调用点搜索核对。

## 错误处理与边界

所有公开生成方法直接返回值，没有 `Result`。需要调用方特别理解的边界如下：

- `Mutex::lock().unwrap()` 在锁被其他线程 panic 毒化后会继续 panic；本文件没有恢复 poisoned state 的路径。
- `SystemTime::duration_since(UNIX_EPOCH).unwrap_or_default()` 不传播时钟错误；若系统时间早于 epoch，首值静默退回 epoch。
- 整数推进明确使用 `wrapping_add`，保留 Go `int64` 溢出语义；时间推进使用 `saturating_mul`/`saturating_add`，在极值处停驻而不回卷。
- 字符串游标使用 `saturating_add(1)`；到 `i64::MAX` 后会停驻并重复输出。`maximum_length == 0` 返回空串。长度过短会截掉更高位，所以跨越容量后可能产生重复字符串。
- `String::from_utf8(output).unwrap()` 依赖 `ALPHABET` 是合法 UTF-8 字节；当前字母表为 ASCII，因而该 unwrap 对当前实现安全。若未来修改字母表，必须同步审查此不变量。
- 范围只检查“下一值是否大于最大值”；零步长会永远重复，负步长不会检查最小值，溢出后的比较也可能绕过预期上界。
- `unique_f64` 是数值强制转换，不保证所有 `i64` 都可被 `f64` 精确表示。

本文件不验证生成字符串是否适合直接作为 SQL 字面量；当前编码表不含引号等特殊字符。若扩展字符集，SQL 转义责任需要在 [`db.rs`](db.rs) 一并处理。

## 并发与资源生命周期

`Datum` 不显式实现线程 trait，但其唯一字段是 `Mutex<DatumState>`，所以可以放入 `Arc` 跨线程共享；[`data_test.rs`](data_test.rs) 的 `integer_generation_serializes_shared_state` 用 16 个线程验证同一实例产生 `0..15` 且无重复。

每次方法调用只持锁到状态读取/更新完成。`unique_string` 在取得并推进整数游标后显式 `drop(state)`，编码和分配字符串时不占锁；各时间方法的格式化发生在 `advance_seconds` 释放锁之后。`unique_year` 必须在一次临界区中同时读取日期、推进年份并写回合成秒数，所以直到写回后才释放锁。

实例没有后台任务、通道、文件句柄、网络连接或显式关闭流程；其生命周期由拥有它的 `Arc<Datum>` 决定，最后一个引用释放时状态和互斥锁一并销毁。锁只保护单个列实例，不协调不同列之间的生成顺序。

## 与 Go 版本的对应关系

直接对照文件是同目录 [`data.go`](data.go)。Rust 的 `Datum`/`DatumState` 对应 Go 的 `datum`，`Mutex<DatumState>` 对应结构体内嵌的 `sync.Mutex`，公开 snake_case 方法逐一对应 Go 的 `newDatum`、`setInitInt64Value`、`uniqInt64`、`uniqFloat64`、`uniqString`、`uniqTime`、`uniqDate`、`uniqTimestamp`、`uniqYear`。

已保持的关键语义包括：默认整数值 `-1`、默认步长 `1`、初始化只执行一次、`-1` 下界哨兵、仅 `min < max` 时启用范围、返回当前整数后再推进、字符串先递增再编码，以及整数在 `i64` 边界按补码回卷。Rust 独立测试明确覆盖这些行为中的整数、字符串和并发部分。

存在必须知晓的实现差异：

- Go `time.Now()`/格式化使用 `time.Time` 的本地 location；Rust 从 Unix 秒直接换算 civil date/time，等价于不带时区偏移的 UTC 计算。在非 UTC 环境中，首个日期和时钟文本可能与 Go 不同。
- Go 日期/年份使用 `time.AddDate` 的日历规则；Rust 日期用固定 `86_400` 秒步进，年份用自有 civil 换算。DST 边界以及闰日跨年需要单独兼容测试，当前 [`data_test.rs`](data_test.rs) 只验证形状和相邻步长，没有证明这些边界完全等价。
- Go 整数算术自然回卷；Rust 只在整数序列中显式 wrapping，而字符串和时间为避免 debug/release 差异采用 saturating。极值之后的重复行为因此不是 Go 源码的逐运算复刻。

同目录未发现专门的 `data_test.go`；Go 对齐依据来自 [`data.go`](data.go) 的实现及 Rust 的 [`data_test.rs`](data_test.rs)，不能据此宣称所有时区/日历边界都已验证。

## 扩展指南

- 新增一种唯一列类型时，优先在 `Datum` 增加只负责状态推进/格式化的方法，再在 [`db.rs`](db.rs) 的 `generate_column_data` 接入相应 `FieldKind`；不要把 SQL 引号或字段类型判断下沉到本文件。
- 修改整数范围语义时，应同时审查 `set_init_int64_value`、`unique_i64`、`db.rs::unique_integer` 和 `parser.rs::Column.step`，并明确范围耗尽、负步长、零步长及溢出的契约。
- 修改字符串字母表或编码算法时，必须同步检查 [`rand.rs`](rand.rs) 的 `ALPHABET`、UTF-8 安全、最大长度容量和 SQL 转义，并扩展 [`data_test.rs`](data_test.rs)；测试逻辑继续放在独立文件，不能内嵌回 `data.rs`。
- 修改时间逻辑前先决定目标是严格追随 Go 本地时区/`AddDate`，还是维持当前 UTC/固定秒模型；随后为非 UTC 时区、DST、闰年 2 月 29 日、负 Unix 秒和算术极值增加独立回归测试。
- 若要报告范围耗尽或锁毒化错误，返回类型将从纯值变为 `Result`，会影响 `db.rs::generate_column_data` 的错误传播以及全部直接调用者，不能只改本文件。
- 性能调整应保留“每次状态推进原子化”的不变量。可以把格式化留在临界区外，但不能把读值和写回拆成两次加锁，否则并发调用会重复。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳为 `1791342965170`；`files --filter pkg/util/importer` 确认目标、模块入口、Go 对照和独立测试均在索引中。
- RustCodeGraph：`node --file pkg/util/importer/data.rs --offset 1 --limit 500` 读取完整 188 行源码并报告 17 个符号、6 个引用文件；同样读取了 `data_test.rs`、`data.go`、`lib.rs`、`db.rs` 和 `parser.rs` 的相关内容。
- 调用点核对：[`parser.rs`](parser.rs) 的 `Column.data`/`parse_table_sql`，以及 [`db.rs`](db.rs) 的 `unique_integer`/`generate_column_data`，构成从列解析到各 `Datum` 方法的直接调用证据。
- crate 边界核对：[`Cargo.toml`](Cargo.toml) 的包名、`[lib] path = "lib.rs"`、移植元数据和依赖节；[`lib.rs`](lib.rs) 的模块声明、重导出及 `#[cfg(test)] mod data_test`。
- 行为测试核对：[`data_test.rs`](data_test.rs) 覆盖 `i64` 溢出、初始化幂等和范围停驻、浮点转换、base62 序列及长度上限、16 线程共享状态、时间/日期步长与时间戳格式。
- Go 语义核对：[`data.go`](data.go) 的同名状态字段和九个对应构造/生成方法；仓库搜索未发现同目录 `data_test.go`，因此时区和日历差异保留为未验证兼容风险。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文恰有 11 个固定二级章节。
