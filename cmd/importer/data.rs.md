# [`cmd/importer/data.rs`](./data.rs)

## 文件定位

`cmd/importer/data.rs` 属于 Cargo 包 `astersql-cmd-importer`，由 `cmd/importer/lib.rs` 以 `pub mod data` 装入 importer 的库目标和二进制目标。它位于“列定义已经解析、INSERT 字面量尚未生成”之间：`cmd/importer/parser.rs::parseTable` 为每一列创建一个 `Arc<datum>`，`cmd/importer/parser.rs::column::parseRule` 再把列注释中的 `step`、`repeats` 和 `probability` 写入该对象；`cmd/importer/db.rs::genColumnData` 根据列类型和本轮是否走 incremental 分支，调用这里的顺序值或随机值原语。

本文件不是进程入口，也不解析 DDL、不拼接 SQL、不执行导入任务。进程装配发生在 `cmd/importer/main.rs::run_with_args`，列到 SQL 字面量的分派发生在 `cmd/importer/db.rs::genColumnData`。Cargo 清单把该包标为 `kind = "binary"`，但具体代码通过 `lib.rs` 共享给 `bin_main.rs` 和独立测试；直接外部依赖只有 `toml`、`serde_json`，本文件自身只使用标准库 `Mutex` 和本 crate 的 `stubs`。

## 核心职责

本文件有两组职责。

第一组是 `datum` 状态机：保存一列的整数/时间当前值、整数边界、重复次数、步长和概率设置；在取值时串行化访问、钳制整数边界、惰性初始化时间值，并消费 `remains`。它为整数、字符串、DATE、DATETIME/TIMESTAMP、DURATION 和 YEAR 的 incremental 路径提供共同状态，但不负责选择列类型或决定是否进入 incremental 路径。

第二组是无状态随机辅助：`randInt`、`randInt64` 提供闭区间整数采样，`randString` 用 63 位随机缓存生成固定长度的字母数字串。`cmd/importer/rand.rs`、`stats.rs` 和 `db.rs` 都复用这些函数；字符表和位缓存常量也由 `rand.rs` 再导出。

一个重要的当前实现事实是：`step` 可由规则配置且会被读取，但本文件及 `cmd/importer` 其他 Rust/Go 生产代码都没有推进 `intValue` 的赋值。`nextInt64` 只钳制并返回当前值。本文因此把它描述为“当前值/重复状态机”，不把尚不存在的数值递增行为写成已支持功能。

## 主要符号

- `alphabet: &str`：62 个字符的稳定字符表，顺序为数字、大写字母、小写字母。`nextString` 把整数按它的长度转换，`randString` 用它过滤 6 位索引；顺序必须与 Go 保持一致。
- `letterIdxBits`、`letterIdxMask`、`letterIdxMax`：随机字符串位缓存参数，分别表示每个候选索引占 6 位、6 位掩码以及一个 63 位随机数可供尝试的索引数。
- `datum`：公开但字段私有的共享列状态，内部只有 `Mutex<DatumInner>`。调用者可以跨线程共享 `Arc<datum>`，但不能绕过方法直接读写状态。
- `DatumInner`：私有状态载体。`intValue/minIntValue/maxIntValue/useRange/init` 管整数值与一次性范围初始化；`timeValue` 缓存第一次取到的时间；`remains/repeats/probability` 管重复消费；`step` 保存配置。
- `newDatum() -> datum`：创建默认状态，`intValue=0`、`step=1`、`repeats=remains=1`、`probability=100`，范围未初始化，时间为 `CivilTime::default()`。
- `step`/`set_step`、`remains`/`set_remains`、`repeats`/`set_repeats`、`probability`/`set_probability`：互斥保护下的访问器。`set_repeats` 同时重置 `remains`，其余 setter 原样接受值；有效概率范围是在 `parser.rs::column::parseRule` 中校验为 `(0, 100]`，不是在这里校验。
- `dec_remains`：仅在 `remains > 0` 时减一；`db.rs::genColumnData` 在配置为 incremental、但本轮概率未命中时调用。
- `setInitInt64Value(minv, maxv)`：只允许首次范围初始化。负步长时用 `minv.wrapping_add(maxv) / 2` 设定当前值，然后置 `useRange` 与 `init`。
- `updateRemains`：私有的随机消费函数。按 `100 - probability` 的阈值选择“随机跳过 1..=remains”或“减一”，并使用 wrapping 减法保留无符号环绕语义。
- `nextInt64`/`nextString`：前者把当前整数钳制到闭区间后消费 `remains`；后者先调用前者，再按 62 进制从低位取字符、反转成正常顺序。
- `nextTime`/`nextDate`/`nextTimestamp`/`nextYear`：首次调用时通过 `CivilTime::now()` 缓存时间，以固定格式返回其中一种视图，并各消费一次 `remains`。
- `randInt`/`randInt64`：通过 `maxv - minv + 1` 构造闭区间宽度，再委托 `stubs::rand_intn`/`rand_int63n`。
- `randString`：从末尾向前填充结果；每次从缓存最低 6 位取索引，拒绝 62、63，缓存耗尽后重新取随机数，从而避免简单取模造成的偏差。

## 执行流程

1. `parser.rs::parseTable` 解析 CREATE TABLE 时，为每个 `column` 调用 `newDatum` 并放入 `Arc`。
2. `parser.rs::column::parseRule` 解析列注释。`step` 调 `set_step`；`repeats` 在唯一列约束检查后调 `set_repeats`；`probability` 在 `(0,100]` 检查后调 `set_probability`。
3. 导入生成一行时，`db.rs::genRowData` 逐列进入 `genColumnData`。若列配置为 incremental，它先用 `datum.probability()` 随机决定本轮路径；未命中时可能用 `dec_remains()` 消费预算；唯一索引列则强制走 incremental。
4. 整数/浮点/DECIMAL 的 incremental 分支经 `db.rs::nextInt64Value` 解析列边界，首次调用 `setInitInt64Value`，再调用 `nextInt64`。`nextInt64` 在锁内钳制 `intValue`、更新 `remains` 并返回当前值。
5. 字符串 incremental 分支调用 `nextString(flen)`，它复用一次 `nextInt64` 的取值和消费，然后把整数转换为最多 `flen` 个字符。非 incremental 字符串最终可由 `db.rs::randStringValue` 调用本文件的 `randString`。
6. 日期时间类 incremental 分支分别调用四个 `next*` 方法。某个 `datum` 第一次进入其中任一方法时捕获一次 `CivilTime::now()`；以后所有时间视图复用同一个值。非 incremental 路径由 `rand.rs` 处理，并复用本文件的随机整数函数。
7. `stats.rs` 在直方图采样的桶位置、整数范围和字符串补齐路径中直接调用 `randInt`、`randInt64`、`randString`，这条调用链不经过 `datum`。

## 数据与状态

`datum` 的全部可变字段处于同一个 `Mutex` 临界区，因此单次 getter、setter 或取值方法看到一致快照。`set_repeats(v)` 保持 `repeats == remains == v` 的重置不变量；随后的 `updateRemains` 或 `dec_remains` 只改变 `remains`。`repeats` 在本文件内不会自动重新装载到 `remains`，是否以及何时开始下一轮必须由上层另行定义。

整数范围只初始化一次：第一次 `setInitInt64Value` 决定后续上下界，后续即使调用者传入另一组边界也会被忽略。这与一个 `datum` 固定绑定一列相匹配；若将同一对象跨不同 SQL 类型或边界复用，会保留首次范围。`nextInt64` 采用先取上界最小值、再取下界最大值的钳制顺序；对于正常的 `min <= max` 得到闭区间值，对反向边界没有单独报错。

时间状态同样是一次性惰性初始化。`nextTime`、`nextDate`、`nextTimestamp` 和 `nextYear` 共用 `timeValue`，所以同一列在不同格式间不会分别读取时钟。`stubs.rs::CivilTime::now` 当前按 Unix 秒构造近似 UTC 的 civil time，注释明确它不承担完整本地时区语义；这是 Rust stub 与 Go `time.Now()` 可能产生环境时区差异的边界。

随机数状态不在 `datum` 中。所有随机函数委托 `stubs.rs` 的线程局部 xorshift 状态；测试可用 `stubs::seed_rng` 固定种子。`randString` 自己只维护一次调用内的字节缓冲、63 位 cache 和剩余候选数。

## 依赖与调用关系

上游直接关系如下：

- `cmd/importer/lib.rs` 声明 `pub mod data`，并通过同一 crate 根装入 `data_test.rs` 和 `parity_test.rs`。
- `cmd/importer/parser.rs` 导入 `datum/newDatum`，`column.data: Arc<datum>` 是本状态机的所有者；`parseTable` 创建实例，`parseRule` 配置状态。
- `cmd/importer/db.rs` 导入三个随机函数，并从 `column.data` 调用 `probability`、`remains`、`dec_remains`、`setInitInt64Value`、各类 `next*` 方法。这是数据状态进入 INSERT SQL 的主要生产调用链。
- `cmd/importer/rand.rs` 和 `cmd/importer/stats.rs` 导入随机函数；`rand.rs` 还再导出字符表与位缓存常量。
- `cmd/importer/rand_test.rs` 创建 `datum` 作为列状态；`data_test.rs` 和 `parity_test.rs` 直接验证本文件契约。

下游只有标准库 `std::sync::Mutex` 与 `crate::stubs`。`CivilTime` 提供时钟、零值判断和格式化；`rand_intn`、`rand_int31n`、`rand_int63`、`rand_int63n` 提供随机原语。Cargo 清单没有为本文件引入单独第三方库。

RustCodeGraph 的文件节点报告 `cmd/importer/data.rs` 被 20 个索引文件使用，并能解析 `newDatum` 对 `datum`/`DatumInner` 的实例化及 `randString` 对三个位缓存常量的引用。不过精确 `callers` 查询没有恢复 camelCase 方法的 Rust 调用边；上述上游关系因此由索引文件依赖和 `rg` 定位到的源码调用点交叉确认，而非把空 callers 输出解释为无调用者。

## 错误处理与边界

本文件不返回 `Result`，错误主要表现为 panic、互斥锁中毒后的 panic，或显式环绕行为。

- 所有锁访问均使用 `lock().unwrap()`；任何持锁 panic 导致 mutex poisoned 后，后续访问会 panic，没有恢复分支。
- `randInt`/`randInt64` 要求合法且可表示的闭区间宽度。`stubs::rand_intn`/`rand_int63n` 断言宽度大于零；反向范围、空范围，或 `max - min + 1` 的整数溢出都不在本文件校验。
- `randString` 把负长度钳制为零并返回空串；Go `make([]byte, n)` 对负长度会失败，这是一个明确的防御性语义差异。正常生产调用应由字段长度提供非负值。
- `nextString` 的 `n == 0` 返回空串；它没有为负 `n` 建立契约。负的 `intValue` 会产生负余数并在转换为 `usize` 后越界，Go 对负索引同样会 panic。
- `updateRemains` 用 `wrapping_sub` 保留 `uint64` 减法环绕。如果 `remains == 0`，随机跳减分支在 Rust 中也直接环绕到 `u64::MAX`；对应 Go 代码会先调用 `rand.Int63n(0)` 而 panic。因此调用方应维持只在有剩余量时消费的前置条件，不能把零值行为当作稳定跨语言契约。
- `100 - probability` 假定概率不超过 100。生产解析器已校验该约束，但公开 setter 本身不校验；直接传入大于 100 的值可能在 debug 构建触发下溢。
- 负步长中点使用 `wrapping_add` 后除二，刻意避免 Rust 加法 panic，并模拟 Go 有符号溢出。`data_test.rs::negative_step_range_initialization_matches_go_overflow` 覆盖 `i64::MAX + i64::MAX` 的边界。

## 并发与资源生命周期

`datum` 通过内部 `Mutex` 可安全放入 `Arc`，`parser.rs::column` 正是这样持有它。每个公开状态操作的锁粒度是单次方法：`nextInt64` 的钳制、remains 消费和返回值选择在同一临界区；四个时间方法的惰性初始化、消费和格式化也在同一临界区，因此两个线程不会为同一 `datum` 初始化两个时间值。

`nextString` 先通过 `nextInt64` 完成锁内状态操作，随后释放锁再做局部编码；编码不依赖后续共享状态。getter 后再由调用者采取动作不是复合原子操作，例如 `db.rs` 的 `remains() > 0` 与随后 `dec_remains()` 之间可以被其他线程穿插。不过 `db.rs` 注释说明 incremental 模式只使用一个 worker；若未来放宽该约束，应把这类检查与修改合并为单个 `datum` 方法。

本文件不创建线程、通道、任务、文件、网络连接或数据库事务，也没有显式析构逻辑。`datum` 在最后一个 `Arc` 被释放时随列对象自然销毁；`Mutex` 只保护内存状态。随机源是 `stubs.rs` 中线程局部状态，不受 `datum` 的 mutex 保护，也不与它共享生命周期。

## 与 Go 版本的对应关系

`datum`、`newDatum`、`setInitInt64Value`、`updateRemains`、五个 `next*` 方法逐项对应 `cmd/importer/data.go`。Rust 的 `DatumInner + Mutex` 对应 Go 结构体内嵌的 `sync.Mutex`；默认 `step/repeats/remains/probability` 相同；首次范围初始化、负步长中点、闭区间钳制、时间值缓存、格式字符串和 remains 消费顺序均按 Go 结构保留。

`alphabet`、三个 `letterIdx*` 常量、`randInt`、`randInt64`、`randString` 实际对应 `cmd/importer/rand.go`，而不是 `data.go`。迁移时把通用随机原语收到了 Rust `data.rs`，再由 Rust `rand.rs` 导入或再导出；职责位置不同但生产调用意图一致。

已核对的差异和迁移限制包括：

- Rust 用 getter/setter 代替 Go 包内对字段的直接访问，并让字段始终受 mutex 保护。
- Rust 负步长中点显式 `wrapping_add`，对应 Go `int64` 的二进制环绕；独立 Rust 测试覆盖了极值。
- Rust `dec_remains` 在零时不减，而 Go 上层也是先检查 `remains > 0` 再直接减；Rust 把保护下沉了一层。
- `CivilTime::now` 是本地 stub 的 UTC 近似，而 Go 使用 `time.Now()`；输出形状相同，时区语义未完全等价。
- `randString(n < 0)` 和 `updateRemains(remains == 0)` 的异常边界与 Go 不完全相同，不能据此宣称逐个非法输入均 parity。
- 两个版本当前都只在本模块初始化或钳制 `intValue`，没有按 `step` 推进它；这属于共同的当前实现事实，而非 Rust 独有简化。

Go 目录没有独立的 `data_test.go`。直接 Go 测试证据主要来自同目录生产调用与 `db_test.go` 的相邻生成逻辑；Rust 则有 `data_test.rs` 的溢出回归，以及 `parity_test.rs::contract_boundary` 对默认值、随机字符串长度和闭区间随机值的契约检查。

## 扩展指南

若要真正增加整数序列推进，最可能修改 `datum::nextInt64` 或新增一个锁内“取值并推进”方法。必须先以 `cmd/importer/data.go` 的目标变更为依据，明确推进发生在返回前还是返回后、到边界时钳制/回绕/停止、`step == 0`、负步长和溢出语义；不要只在 `db.rs` 外部拼接 getter/setter，否则会破坏原子性。对应回归测试应放在独立的 `cmd/importer/data_test.rs`，并补 `db_test.rs` 或 parity 测试证明 SQL 可观察结果。

若扩展 repeats/probability，优先把“检查并消费”封装为一个持锁方法，并明确 `remains == 0` 如何重装 `repeats`。需要同步检查 `parser.rs::column::parseRule` 的配置约束和 `db.rs::genColumnData` 的路径选择；尤其不能让唯一索引列因重复策略产生重复值。

若增加字符集或随机算法，必须同步 `alphabet` 和 `letterIdx*` 的数学关系，并核对 `rand.rs` 的再导出、`stats.rs` 的补齐路径与 `db.rs::randStringValue`。改变字符表顺序会让同一随机索引生成不同数据，影响可复现性；扩大到超过 64 个字符则不能继续使用当前 6 位索引方案。

若改变时间行为，需要同时核对四个 `next*` 方法和 `stubs.rs::CivilTime`，决定仍共享一次采样还是每次读取时钟，并为时区、闰日与格式补独立测试。若只是新增 SQL 类型分支，类型选择应接在 `db.rs::genColumnData`，不要把 SQL 引号或类型判断塞入本文件。

任何修改都应继续把 Rust 生产逻辑和测试逻辑分文件保存，保留现有 PingCAP Apache License；修复后的 Rust 源文件应保留顶部 `// Copyright 2026 AsterSQL.`。本次任务只创建说明文档，不修改上述代码。

## 验证依据

事实核验读取了以下文件：

- 目标实现：`cmd/importer/data.rs`。
- crate 与模块边界：`cmd/importer/Cargo.toml`、`cmd/importer/lib.rs`、`cmd/importer/main.rs`。
- 直接 Rust 上下游：`cmd/importer/parser.rs`、`cmd/importer/db.rs`、`cmd/importer/rand.rs`、`cmd/importer/stats.rs`、`cmd/importer/stubs.rs`。
- Rust 测试：`cmd/importer/data_test.rs`、`cmd/importer/rand_test.rs`、`cmd/importer/parity_test.rs`。
- Go 对照：`cmd/importer/data.go`、`cmd/importer/rand.go`、`cmd/importer/parser.go`、`cmd/importer/db.go`、`cmd/importer/stats.go`；同目录未发现 `data_test.go`。

RustCodeGraph 在索引状态中报告 7,032 个 Rust 文件、索引包含 `cmd/importer/data.rs` 的 30 个符号；执行过文件列表、目标文件 node、`newDatum`/`nextInt64`/`nextTime`/`randInt`/`randInt64`/`randString` 的 callers/callees 查询。有效图证据包括目标文件被 20 个索引文件使用、`newDatum` 实例化 `datum` 和 `DatumInner`、`randString` 引用三个位缓存常量；camelCase Rust 方法的 callers 未被图完整恢复，调用点另用 `rg` 在上述直接上下游文件中核实。

结构验收使用任务指定命令，要求文件存在且恰有 11 个固定二级标题。本任务是纯文档分析，按计划未运行 Cargo、Rust 单元测试或 Go 测试；因此这里验证的是文档结构和源码可追溯性，不是重新执行运行时行为。
