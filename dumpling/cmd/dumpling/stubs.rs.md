# `dumpling/cmd/dumpling/stubs.rs`

## 文件定位

源码入口：[`dumpling/cmd/dumpling/stubs.rs`](./stubs.rs)。

本文件属于 Cargo crate `astersql-dumpling-cmd-dumpling`。`dumpling/cmd/dumpling/Cargo.toml` 将该 crate 声明为带库入口 `lib.rs` 和二进制入口 `bin_main.rs` 的 dumpling 命令行程序，并只依赖 `dumpling/cli`、`dumpling/export`、`dumpling/log` 三个本地 crate。Cargo 注释明确说明此处以本地桩替代 `pflag` 和 Prometheus collector，从而保持 arm64 Darwin 构建路径不引入 kv、domain、kvproto 或 grpcio。

`dumpling/cmd/dumpling/lib.rs` 通过 `pub mod stubs` 暴露本模块；同 crate 的 `config_flags.rs` 使用它定义和读取命令行参数，`main.rs` 使用它完成参数解析、运行时 collector 注册和默认 gatherer 安装。文件没有条件编译项；测试模块由 `lib.rs` 对独立的 `parity_test.rs` 和 `config_flags_test.rs` 使用 `#[cfg(test)]` 接入，测试逻辑没有内嵌在本文件。

它是有意缩小能力面的兼容层，而不是通用 `pflag` 或 Prometheus 实现。当前真实业务配置仍由 `astersql_dumpling_export::Config` 承担，真实导出流程仍位于 `dumpling/export`；本文件只负责 CLI 边界需要的值解析和名称级指标注册副作用。

## 核心职责

1. 以 `FlagSet`、`FlagValue` 和内部 `Flag` 保存 dumpling 已注册的命令行参数、短名索引、显式修改状态、隐藏状态、剩余位置参数及 usage 回调。
2. 以 `FlagSet::Parse` 实现当前 dumpling 需要的长参数、短参数、布尔短参数簇、内联值、重复 slice/array/map 参数和 `--` 终止解析行为。
3. 以强类型 getter 为 `config_flags::ParseFromFlags` 提供 `bool`、`i32`、`u64`、字符串、字符串集合、键值映射和 `Duration`，并在不存在或类型不匹配时返回诊断字符串。
4. 以 `register_runtime_collectors`、`set_default_gatherer` 及全局槽模拟 Go `main.go` 中注册 process/go collector 和设置 `prometheus.DefaultGatherer` 的可观测副作用。
5. 保留 `FlagHelp` 命名契约和 `FlagError` 标准错误外形，供 Rust 主流程与后续兼容扩展使用。

这里明确不负责真实进程指标、Go runtime 指标采集、Prometheus gather、网络访问、数据库连接或导出资源生命周期；`NewProcessCollector`/`NewGoCollector` 仅返回稳定名称，实际 `Registry::MustRegister` 行为由 `dumpling/export` 提供的 registry 实现决定。

## 主要符号

- `FlagHelp: &str = "help"`：对应 Go `export.FlagHelp` 的参数名；`config_flags::DefineFlags` 用它注册帮助开关，`entry::run_with_factory` 用它读取帮助状态。
- `FlagValue`：公开枚举，当前包含 `Bool`、`Int(i32)`、`Uint64`、`String`、`StringSlice`、`StringArray`、`StringToString` 和 `Duration`。枚举决定 `FlagSet::set_from_parse` 的解析分派和 getter 的类型检查。
- `Flag`：模块私有记录，保存 `name`、可选 `shorthand`、`usage`、当前 `value`、`changed` 与 `hidden`。`hidden` 只影响 `PrintDefaults`，不禁止解析；`changed` 区分注册默认值和用户首次赋值。
- `FlagSet`：公开的最小参数集合。`flags` 是长名到 `Flag` 的主表，`shorthand` 是短名到长名的索引，`args` 保存未消费的位置参数，`usage` 保存线程安全、可共享的回调。自定义 `Debug` 不打印回调本体，只打印其存在性。
- 定义接口：`BoolP`/`Bool`、`StringP`/`String`、`StringArray`、`StringSliceP`/`StringSlice`、`IntP`/`Int`、`Uint64P`/`Uint64`、`Duration`、`StringToString` 最终统一调用私有 `define`。带 `P` 的接口同时建立短名索引。
- 状态和输出接口：`set_usage`、`Usage`、`PrintDefaults`、`MarkHidden`、`Changed`、`NArg`、`Args`。默认帮助输出按长名排序并跳过隐藏项，以获得稳定输出。
- 取值接口：`GetBool`、`GetString`、`GetInt`、`GetUint64`、`GetStringSlice`、`GetStringArray`、`GetDuration`、`GetStringToString`。集合和字符串返回克隆，不把内部可变状态借给调用者。
- 解析接口：`Parse` 是公开入口；`set_from_parse` 按注册类型转换；`apply_value` 统一更新值并置 `changed = true`；`take_value` 处理内联值或后一 argv；`parse_bool`、`parse_duration`、`parse_csv_record` 分别复现 dumpling 所需的 Go `strconv.ParseBool`、`time.ParseDuration` 正时长子集和 CSV record 子集。
- 指标桩：`ProcessCollectorOpts`、`NewProcessCollector`、`NewGoCollector`、`register_runtime_collectors`。两个构造函数分别返回 `"process_collector"` 和 `"go_collector"`，注册顺序与 Go `main.go` 相同。
- 默认 gatherer：`DEFAULT_GATHERER`、`default_gatherer_slot`、`set_default_gatherer`、`take_default_gatherer`、`clear_default_gatherer`。槽中类型是 `Option<Arc<dyn Registry>>`。
- `FlagError(String)`：实现 `Display` 和 `std::error::Error` 的兼容占位。目前目标 crate 的主流程直接使用 `Result<_, String>`，源码引用搜索未发现它被实例化。

## 执行流程

命令行主链由 `entry::run_with_factory` 驱动：

1. `FlagSet::new` 建立空集合，`set_usage` 安装 dumpling 帮助回调，主流程先以 `BoolP` 注册 `-V/--version`。
2. `config_flags::DefineFlags` 调用本文件的各种定义接口注册 dumpling 参数，包括 `FlagHelp`；部分兼容参数通过 `MarkHidden` 从帮助输出中隐藏但仍可解析。
3. `FlagSet::Parse` 从左到右扫描不含程序名的 argv。`--` 把其后全部内容写入 `args`；`--name=value` 和 `--name value` 都进入 `set_from_parse`；短参数允许布尔簇，首个非布尔短参数可从剩余文本或下一 argv 取值，例如 `-P4001`。
4. `set_from_parse` 读取参数注册时的 `FlagValue` 类型并转换。无值布尔参数设为 `true`；整数直接解析；`StringArray` 每次追加一个原始值；`StringSlice` 解析 CSV 并在第一次显式赋值时丢弃注册默认值、以后继续追加；`StringToString` 解析 `key=value` 并合并；时长转为 `std::time::Duration`。
5. `apply_value` 写回结果并标记 `changed`。解析结束后，`main.rs` 读取 help/version，`config_flags::ParseFromFlags` 通过 getter 将值复制到 export `Config`，而 `NArg`/`Args` 让主流程拒绝残留位置参数。
6. 配置校验通过后，`main.rs` 取得 `conf.PromRegistry`，调用 `register_runtime_collectors`，依次把两个稳定名称交给 `Registry::MustRegister`，再调用 `set_default_gatherer` 保存该 registry。
7. 测试可用 `take_default_gatherer` 观察安装结果，并由 `entry::reset_cli_globals` 间接调用 `clear_default_gatherer`，隔离共享全局状态。

## 数据与状态

`FlagSet` 的状态完全由调用者持有，解析需要 `&mut self`。再次调用 `Parse` 会清空旧的 `args`，但不会把各 flag 的值或 `changed` 恢复到最初默认值；因此复用同一个集合会延续前次赋值，尤其会影响重复 `StringSlice`、`StringArray` 和 `StringToString` 的追加语义。若需要独立的一次解析，应重新构造并重新调用 `DefineFlags`。

同名长参数会覆盖 `flags` 主表中的旧记录；若定义带短名参数，`shorthand` 索引也会写入。源码没有清理旧定义所留下短名映射的逻辑，因此扩展代码不应依赖重复定义来重绑或删除旧短名。

`StringSlice` 与 `StringArray` 的差异是有意的：前者把单次输入当成 CSV record，可由一个参数生成多个元素；后者把每次输入整体当成一个元素，适合保存内联 TOML 等含逗号内容。`StringToString` 在只有一个等号时直接保留整段（去掉两端双引号），多个等号时先走 CSV 分割，再按第一个等号拆成键和值；相同键后写覆盖前写。

默认 gatherer 是进程级共享状态：`OnceLock` 只负责惰性创建槽，槽内 `Mutex<Option<Arc<dyn Registry>>>` 允许替换、读取和清空；`take_default_gatherer` 的名称虽含 `take`，实现只是克隆 `Arc` 快照，并不消费槽中的值。最后一次 `set_default_gatherer` 生效。

## 依赖与调用关系

上游直接调用者由源码和 RustCodeGraph 文件关系共同确认：

- `dumpling/cmd/dumpling/lib.rs` 声明 `pub mod stubs`，形成模块入口。
- `dumpling/cmd/dumpling/main.rs::run_with_factory` 调用 `FlagSet::new`、usage/定义/解析/getter/位置参数接口，以及 `register_runtime_collectors`、`set_default_gatherer`；`reset_cli_globals` 调用 `clear_default_gatherer`。
- `dumpling/cmd/dumpling/config_flags.rs::DefineFlags` 使用定义和隐藏接口，`ParseFromFlags` 使用所有主要 getter 与 `Changed`，把参数写入 `astersql_dumpling_export::Config`。
- `dumpling/cmd/dumpling/parity_test.rs` 直接覆盖 `FlagSet` 的 pflag 兼容边界和 `take_default_gatherer` 的全局副作用；`config_flags_test.rs` 经 `DefineFlags`/`ParseFromFlags` 间接覆盖 array、map 等参数。

下游只有标准库和一个 crate 接口：`HashMap` 保存 flag/map 值；`Arc` 保存 usage 和 registry；`Mutex`/`OnceLock` 管理进程级 gatherer；`Duration` 是解析后的时长表示；`fmt` 支撑错误展示；`astersql_dumpling_export::Registry` 提供 `MustRegister`。本文件不直接调用数据库、文件系统、网络或异步运行时。

RustCodeGraph 的文件查询确认目标文件被索引并含 67 个符号，且将其列为 `config_flags.rs`、`config_flags_test.rs`、`dumpling/export/config.rs`、`dumpling/export/dump.rs` 等 13 个文件的依赖来源；精确源码引用进一步确认本 crate 的实际主链是 `lib.rs -> main.rs/config_flags.rs -> stubs.rs`。对重名 `FlagSet` 的全局自然语言查询会混入其他 crate，因此本文不把那些重名结果当成本文件调用证据。

## 错误处理与边界

公开解析/getter 接口使用 `Result<_, String>`：未知长参数、未知短名、缺少参数值、数值转换失败、布尔/时长/CSV 格式无效、map 项缺少等号、未定义 flag 或 getter 类型不匹配都会立即返回错误。`main.rs` 把 `Parse` 错误映射为与 Go `pflag.CommandLine` 的 `ExitOnError` 相同的退出码 2；配置层语义校验错误则由 `ParseFromFlags` 处理并返回退出码 1。

`parse_bool` 只接受 Go `strconv.ParseBool` 支持的大小写组合及 `1/0`，明确不接受 `yes/no`。`parse_duration` 接受无单位的特殊值 `0`、可选前导 `+`、小数和组合单位 `ns/us/µs/μs/ms/s/m/h`；拒绝负数、缺少单位（非零）、非有限值和超出 `u64` 秒范围的值。它只实现 dumpling 使用的正时长子集，不等价于 Go `time.ParseDuration` 的完整语义。

`parse_csv_record` 支持逗号分隔、双引号字段和成对双引号转义；拒绝未闭合引号、引号字段结束后非逗号字符和裸引号。它是单 record 解析器，不提供 Go `encoding/csv` 的完整配置面。

`PrintDefaults` 是稳定、简化的帮助输出，不复刻 pflag 的全部默认值展示和排版。collector 构造函数也不产生真实指标。扩展者不能把“parity 测试通过”解释为支持通用 pflag 或 Prometheus。

所有 gatherer 槽操作都对 `Mutex::lock()` 直接 `unwrap()`；若持锁线程 panic 导致 mutex poisoned，后续访问也会 panic，而不是返回可恢复错误。`Registry::MustRegister` 的重复注册/失败策略属于下游 trait 实现，本文件不捕获或转换。

## 并发与资源生命周期

单个 `FlagSet` 没有内部锁，也没有声明供多个线程并发修改；预期生命周期是 CLI 启动阶段在一个调用栈内完成定义、解析和只读取值。usage 闭包要求 `Send + Sync + 'static`，并由 `Arc` 持有，但执行 `Usage` 时仍只是同步调用。

`DEFAULT_GATHERER` 是本文件唯一跨调用、跨测试共享的状态。`OnceLock` 保证槽只初始化一次，`Mutex` 串行化替换、读取和清空，`Arc<dyn Registry>` 延长 registry 生命周期。`set_default_gatherer` 会覆盖旧 `Arc`；若无其他引用，旧 registry 在释放锁后按引用计数销毁。`take_default_gatherer` 增加一次强引用，调用者持有期间 registry 不会销毁；`clear_default_gatherer` 只清除槽内引用，不影响其他已克隆的 `Arc`。

正常 CLI 成功或失败路径不会自动清空该全局槽，这与 Go 进程级 `prometheus.DefaultGatherer` 的生命周期相符。独立测试必须通过 `entry::reset_cli_globals` 清空，否则并发或顺序测试会相互观察到状态。`parity_test.rs::contract_resource_cleanup_close_and_gatherer` 明确验证安装后可见、reset 后为空；该测试也表明 dumper 的 `Close` 生命周期由 `main.rs` 管理，不属于本文件。

## 与 Go 版本的对应关系

Go 对照入口是 `dumpling/cmd/dumpling/main.go`：

- Go 的全局 `pflag.CommandLine` 在 Rust 中变成局部 `FlagSet`，避免隐式共享参数状态；`pflag.Usage` 对应 `set_usage`/`Usage`，`pflag.PrintDefaults` 对应 `PrintDefaults`，`pflag.NArg`/`Args` 对应同名 Rust 方法。
- `export.FlagHelp` 对应 `FlagHelp`；`config_flags.rs::DefineFlags` 和 `ParseFromFlags` 是从 Go export 配置方法拆出的 Rust 对照层。
- Go `pflag.Parse` 的关键行为由 `FlagSet::Parse` 聚焦复刻。独立测试固定了首次 `StringSlice` 赋值覆盖默认值、`-P4001` 附着值、引号 CSV、重复 map、组合时长、无单位时长错误、`yes` 布尔错误以及解析失败退出码 2。
- Go 依次调用 `registry.MustRegister(collectors.NewProcessCollector(...))`、`registry.MustRegister(collectors.NewGoCollector())`；Rust `register_runtime_collectors` 保留相同顺序，但只注册名称标识，不提供指标内容。
- Go 仅在 registry 实现 `prometheus.Gatherer` 时赋给 `prometheus.DefaultGatherer`。Rust `Config::PromRegistry` 已是 `Arc<dyn Registry>`，因此 `main.rs` 无运行时类型断言，直接调用 `set_default_gatherer`；这是结构差异，测试只承诺“已安装同一个 registry 抽象”的副作用。

Go 的相关测试提供交叉证据：`dumpling/export/main_test.go::TestMain` 使用相同顺序注册真实 collectors；`dumpling/export/http_handler_test.go::TestMetricsHandlerWithSharedDefaultGatherer` 临时把配置 registry 设置为真实 `prometheus.DefaultGatherer` 并验证 metrics handler 可读取 dumpling 指标；`dumpling/export/config_test.go::parseConfigFromArgsForTestWithErr` 使用 `pflag.NewFlagSet(..., ContinueOnError)`、`DefineFlags`、`Parse`、`ParseFromFlags` 组成配置解析对照链。

## 扩展指南

新增参数类型时，应同时修改 `FlagValue`、相应定义方法、`set_from_parse` 分支和强类型 getter，并在 `config_flags.rs::DefineFlags`/`ParseFromFlags` 接线。测试必须放在独立文件：解析器语法或 Go pflag 行为优先补 `dumpling/cmd/dumpling/parity_test.rs`，配置字段映射和语义校验优先补 `config_flags_test.rs`；若改动要对齐 Go 行为，还应核对或补充 `dumpling/export/*_test.go` 中相应场景。

修改 parser 时需要保护以下不变量：第一次显式 `StringSlice` 赋值清除默认值；后续重复项追加；`StringArray` 不按逗号拆分；每次成功写入都置 `changed`；`--` 后不再解析；短参数簇遇到非布尔项后停止并消费余串；解析失败不得静默改变为成功。若要允许同一个 `FlagSet` 多次独立解析，应显式设计默认值快照/重置接口，不能只清空 `args`。

扩展时长或 CSV 支持，应以 Go 标准库的可观察行为和独立回归测试为准，避免把当前子集随意解释成完整实现。任何新的错误类型若改用 `FlagError`，还需统一现有 `String` 错误的转换边界，防止 CLI 文案或退出码漂移。

若需要真实 Prometheus 能力，替换点是 `NewProcessCollector`、`NewGoCollector` 和 `register_runtime_collectors`，并应重新评估 Cargo 依赖、arm64-safe 约束、重复注册行为和指标性能；不能在保持名称桩的情况下声称已采集真实 runtime/process 指标。修改全局 gatherer 时，应保留测试隔离接口，并决定 poisoned mutex 是继续 panic 还是显式恢复/返回错误。

兼容风险主要是 CLI 解析字节行为和帮助/错误文案；性能风险主要来自复制集合 getter、CSV/时长逐字符解析以及全局 mutex，但它们只处于启动/测试路径，当前没有热路径证据。并发风险集中在进程级 gatherer 的测试隔离和 mutex poison。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 7,032 个 Rust 文件；`files --filter dumpling/cmd/dumpling` 确认目标及 `lib.rs`、`main.rs`、`config_flags.rs`、两份独立测试均已索引；`node --file dumpling/cmd/dumpling/stubs.rs --offset 1 --limit 500` 与 `--offset 487 --limit 260` 完整读取 686 行源码；`query FlagSet --kind struct`、`query register_runtime_collectors --kind function`、`query set_default_gatherer --kind function` 锁定本文件符号。精确 callers/callees 未返回可用边，因此调用关系以直接源码引用复核，不作推测。
- 目标与 crate 文件：`dumpling/cmd/dumpling/stubs.rs`、`Cargo.toml`、`lib.rs`、`main.rs`、`config_flags.rs`、`bin_main.rs`（由 Cargo bin 声明确认）。目标目录没有 `doc.go`。
- Rust 独立测试：`dumpling/cmd/dumpling/parity_test.rs`、`dumpling/cmd/dumpling/config_flags_test.rs`。前者直接验证解析边界、退出码与 gatherer 状态；后者验证配置映射使用的 `StringArray`、`StringToString` 等路径。
- Go 对照：`dumpling/cmd/dumpling/main.go`、`dumpling/export/main_test.go`、`dumpling/export/http_handler_test.go`、`dumpling/export/config_test.go`。这些文件分别提供 CLI 顺序、collector 注册、默认 gatherer 和 pflag 配置解析证据。
- 人工复核结论：该文件存在是为了在精简、arm64-safe 的 Rust dumpling CLI 中承接 pflag/Prometheus 边界；其运行路径从 `entry::run_with_factory` 和 `config_flags` 进入，安全扩展点及必须同步的独立测试如上，未把名称桩描述为真实采集实现。
