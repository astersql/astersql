# `pkg/config/keyspace_observability.rs`

## 文件定位

本文件属于 `astersql-config` crate；crate 根 `pkg/config/lib.rs` 以私有模块 `mod keyspace_observability` 装载它，再通过 `pub use keyspace_observability::*` 导出其公开类型、常量和方法。crate 边界及依赖见 `pkg/config/Cargo.toml`：本文件直接使用标准库 `HashMap`/`HashSet` 和 `serde` 派生，不启动任务，也不访问网络或存储。

它把配置中的 keyspace metadata 映射规则分成两个阶段：`KeyspaceObservability::Valid` 校验静态规则；`Config::ResolveKeyspaceObservability` 用一次实际 metadata 快照计算运行时值并存入 `Config::keyspace_observability_values`。`Config` 中配置字段、运行时缓存和默认值分别定义在 `pkg/config/config.rs:1049-1052`、`pkg/config/config.rs:1126-1127`；模块自身不负责取得 metadata，也不直接写指标或日志。

当前 Rust 应用入口是 `cmd/tidb-server/main.rs::prepareKeyspaceObservabilityForStarter`：TiKV starter 启动路径复制全局配置、调用解析方法，再把结果与内置 `keyspace_name` 指标标签合并并通过 `config::UpdateGlobal` 发布。非 TiKV store 在入口层跳过这一流程。

## 核心职责

1. `KeyspaceObservability`/`KeyspaceObservabilityField` 描述 TOML/JSON 中 `keyspace-observability.fields` 的来源键、三个输出名和必填标志。
2. `KeyspaceObservability::Valid` 拒绝无来源、无输出、非法名称/前缀以及同一输出类别内大小写不敏感的重名。
3. `Config::ResolveKeyspaceObservability` 从 metadata 取值，分别生成 Prometheus label、慢日志字段和 statement log 字段，并将慢日志字段按名称排序。
4. 三个 `GetKeyspaceObservability*` 方法提供缓存只读借用；`KeyspaceObservabilityValues::Clone`提供与 Go 显式深拷贝 API 对应的独立副本。

本文件不校验部署模式。只有 starter 才能配置非空规则的约束位于 `pkg/config/config.rs:1350-1355` 的 `Config::valid`；它先检查部署模式，再调用本文件的 `Valid`。

## 主要符号

- `KeyspaceObservability { Fields }`：规则集合。`#[serde(default, rename_all = "kebab-case")]` 允许缺省字段，并将结构映射到配置命名；`Fields` 又显式映射为 `fields`。
- `KeyspaceObservabilityField`：单条规则。`Source` 是 metadata 键；`MetricLabel`、`SlowLogField`、`StmtLogField` 可各自为空但不能全空；`Required` 控制来源缺失是报错还是跳过。
- `KeyspaceObservabilityValues`：解析后的运行时缓存，包含两个 `HashMap<String, String>` 和一个 `Vec<KeyspaceObservabilityLogField>`。它在 `Config` 上带 `#[serde(skip)]`，不会作为普通配置持久化。
- `KeyspaceObservabilityLogField { Name, Value }`：慢日志需要的有序键值项。
- `keyspaceObservabilityMetricLabelPrefix`：`"keyspace_meta_"`。指标前缀检查先把候选名转成小写，因此前缀接受大小写变体。
- `keyspaceObservabilitySlowLogFieldPrefix`：`"Keyspace_meta_"`。慢日志前缀使用原字符串检查，大小写敏感。
- `KeyspaceObservability::Valid(&self) -> Result<(), String>`：规则静态校验入口。
- `validPrometheusLabelName(&str) -> bool`：实现传统 `[A-Za-z_:][A-Za-z0-9_:]*` 文法；`validKeyspaceObservabilityLogFieldName` 当前直接复用它。
- `Config::ResolveKeyspaceObservability(&mut self, HashMap<String, String>) -> Result<(), String>`：解析并原子式替换缓存。
- `Config::GetKeyspaceObservabilityMetricLabels`、`GetKeyspaceObservabilitySlowLogFields`、`GetKeyspaceObservabilityStmtLogFields`：分别返回缓存集合的不可变引用。
- `KeyspaceObservabilityValues::Clone(&self)`：仅对非空集合分配并深拷贝；与派生的 Rust `Clone` 结果等价，但保留 Go 风格方法名以对齐移植接口。

文件中没有 trait、enum、宏定义或条件编译项。

## 执行流程

静态配置流程如下：

1. `serde` 将 `[[keyspace-observability.fields]]` 解码为规则列表，未给出的字符串/布尔字段采用默认值。
2. `Config::valid` 限制非空规则只能用于 starter，并调用 `KeyspaceObservability::Valid`。
3. `Valid` 为三种输出各建一个 `HashSet`，按规则顺序检查。它先验证 `Source` 和“至少一个输出”，再检查每个非空输出。
4. 指标名和慢日志名必须符合传统 Prometheus label 文法；指标名还需具有大小写不敏感的 `keyspace_meta_` 前缀，慢日志名则必须精确以 `Keyspace_meta_` 开头。三个输出类别分别按小写键去重，类别之间互不冲突。

运行时解析流程如下：

1. `ResolveKeyspaceObservability` 创建空的局部 `resolved`，不立即改动 `Config` 缓存。
2. 对每条规则，以 `Source` 查询输入 metadata。缺失且 `Required=true` 时立即返回错误；缺失且非必填时整条规则不产生任何输出。
3. 找到值后，把它按非空输出名克隆到相应集合；同一来源可以同时进入三条输出通道。
4. 全部规则处理完成后，按 `Name` 升序排列 `SlowLogFields`，再一次性赋给 `self.keyspace_observability_values`。
5. starter 入口将这些配置结果与内置 `keyspace_name` label 合并后发布到全局配置；显式配置的同名指标键会覆盖内置值（`cmd/tidb-server/main.rs:2148-2171`）。

## 数据与状态

规则数据存放在 `Config::keyspace_observability`，解析结果存放在 `Config::keyspace_observability_values`。两者生命周期不同：前者来自配置反序列化，后者从当前 keyspace activation metadata 派生且被 `serde` 跳过。

`ResolveKeyspaceObservability` 接管输入 `HashMap`，但只借用其中的值并将字符串克隆进缓存。它在局部变量中完成计算，因此成功时完整替换旧缓存；若遇到缺失的必填来源，赋值点尚未执行，旧缓存保持不变，不会暴露部分更新。若前置 `Valid` 被绕过而存在同名输出，`HashMap::insert` 会以后出现的规则覆盖先前值；慢日志 `Vec` 则会保留重复项，所以调用方应遵守先校验后解析的不变量。

慢日志使用 `Vec` 是为了提供确定顺序；指标和 statement log 使用 `HashMap`，本文件不承诺遍历顺序。三个 getter 返回内部集合的不可变引用，生命周期受 `Config` 借用约束；`Clone` 用于需要脱离原配置所有权或跨全局更新边界的场景。

## 依赖与调用关系

上游关系：

- `pkg/config/config.rs::Config::valid` 调用 `KeyspaceObservability::Valid`，同时承担 starter-only 约束。
- `cmd/tidb-server/main.rs::prepareKeyspaceObservabilityForStarter` 调用 `ResolveKeyspaceObservability` 和 `KeyspaceObservabilityValues::Clone`，是 Rust 主程序已确认的运行时入口。
- `pkg/config/config_test.rs`、`pkg/config/const_3_aster_unit_test.rs` 和 `cmd/tidb-server/main_test.rs` 直接调用本文件 API；RustCodeGraph 的目标文件视图也列出了这些引用。

下游关系：

- `Valid` 只依赖标准库集合、字符串转换和两个本地名称校验函数。
- `ResolveKeyspaceObservability` 只依赖配置字段、标准库集合、`KeyspaceObservabilityLogField` 和字符串排序。
- `serde` 是唯一直接外部 crate 依赖，用于四个数据结构的序列化/反序列化派生。

Go 完整消费端还包括 `pkg/util/metricsutil/common.go`（指标标签）、`pkg/sessionctx/variable/slow_log.go`（慢日志）和 `pkg/util/stmtsummary/v2/logger.go`（statement log）。在检索到的 Rust 源码中，真实非测试接线目前集中于 server starter 对缓存的构造/发布；未检索到上述三个 Go 消费点的同路径 Rust 直接调用，因此不能据此文档声称 Rust 三类最终输出链均已完整接通。

## 错误处理与边界

所有失败都以 `Result<(), String>` 返回，不 panic，也不包装结构化错误。`Valid` 的消息含零基规则下标，能够定位 `keyspace-observability.fields.N`；解析错误包含缺失的 `Source`。`Config::valid` 会把字符串转换为配置层错误。

关键边界如下：

- 空规则列表合法，并解析为空缓存。
- `Source` 只要求非空；本文件不限制 metadata 键语法，也不禁止多条规则读取同一来源。
- 三个输出全空非法；任一非空即可。
- 指标/慢日志字段拒绝空格、连字符、非 ASCII 以及数字开头，但传统文法允许冒号。statement log 字段除非为空或与同类字段大小写不敏感重名，否则没有语法/前缀限制。
- 指标前缀校验大小写不敏感，慢日志前缀校验大小写敏感；三类重名检查均大小写不敏感。
- 可选来源缺失时不会产生空字符串字段，而是跳过该规则的所有输出。
- 必填来源缺失会保留上一次成功缓存；调用者若不希望旧值继续可见，必须在更高层决定是否清空或拒绝发布。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、文件句柄或网络连接。解析需要 `&mut Config`，Rust 借用规则保证同一个 `Config` 在调用期间不被并发读写；getter 仅提供 `&` 引用。

全局配置的同步与发布不由本文件管理。server 入口先在局部副本上解析，再通过 `config::UpdateGlobal` 更新全局状态，这使可能失败的解析发生在发布之前。`Clone` 切断 `HashMap`、`Vec` 和 `String` 的所有权共享，修改副本不会影响原缓存，测试 `keyspace_observability_resolves_sorts_and_clones_deeply` 对此有直接断言。

性能上，校验为字段数的线性期望时间并持有三个去重集合；解析为线性扫描加慢日志字段的 `O(L log L)` 排序，其中 `L` 是生成的慢日志字段数。字符串克隆使结果独立，但字段或 metadata 很大时会带来对应分配成本。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/config/keyspace_observability.go`。四个结构、两个前缀常量、校验顺序、错误文本、解析分支、稳定排序、深拷贝和三个 getter 均有一一对应实现。Rust `HashMap`/`HashSet` 对应 Go map，`Vec` 对应 slice；Rust getter 返回借用，而 Go 返回 map/slice 句柄，这是所有权接口差异。

Go 的 `validPrometheusLabelName` 同时调用 Prometheus `LabelName.IsValid()` 与 `IsValidLegacy()`；两者交集在当前 Go 实现中落到 legacy ASCII 文法。Rust 直接逐字节实现该交集，避免引入 Prometheus crate。Go 使用 `sort.SliceStable`，Rust 使用稳定的 slice `sort_by` 并按 `Name` 比较；即使合法配置因去重而不会出现同名慢日志字段，排序语义仍保持一致。

Go 的运行时消费者证据见 `pkg/util/metricsutil/common.go:119`、`pkg/sessionctx/variable/slow_log.go:586`、`pkg/util/stmtsummary/v2/logger.go:136`。Rust 当前已验证 starter 构造/发布路径，但没有从本次直接证据确认上述最终消费链全部移植，因此该差异应视为当前接线状态，而不是由本文件补齐的职责。

## 扩展指南

- 新增输出通道时，应同时扩展 `KeyspaceObservabilityField`、`KeyspaceObservabilityValues`、`Valid`、`ResolveKeyspaceObservability`、深拷贝与 getter，并同步 Go 对照或明确偏离原因。测试应放在独立文件，优先扩展 `pkg/config/const_3_aster_unit_test.rs` 和 `pkg/config/config_test.rs`，不要内嵌到生产源文件。
- 修改命名规则时，应同步检查两个本地校验函数、前缀常量、大小写策略及 Go 的 Prometheus 语义；尤其避免无意放宽 Unicode 或改变冒号处理。
- 修改解析失败语义时，应保留“先局部构造、成功后替换”的事务式边界，或明确评估旧缓存/半成品暴露风险。
- 修改排序或容器类型时，应保留慢日志确定顺序，并评估 API 从借用到复制的兼容性与额外分配。
- 调整 starter 接线时，应同步 `cmd/tidb-server/main_test.rs` 中 TiKV 和非 TiKV 两组测试，并确认内置 `keyspace_name` 与显式 label 的覆盖优先级。
- 若继续接通最终消费端，应以 Go 三个消费文件为行为依据，分别新增独立 Rust 测试验证指标注册、slow log 字段和 statement log 字段；这属于消费模块任务，不应把逻辑塞回本配置文件。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、目标文件已索引；`files --filter pkg/config/keyspace_observability.rs` 报告 13 个符号；`node --file ... --offset 1/229` 核对完整 323 行实现；`query`/`explore` 确认 Go/Rust 同名符号以及 `prepareKeyspaceObservabilityForStarter`、配置测试和 getter 的引用。精确 `callers`/`callees` 命令在本环境 30 秒内未返回，调用边因此又用直接引用检索复核。
- Rust 源与装配：`pkg/config/keyspace_observability.rs`、`pkg/config/lib.rs`、`pkg/config/config.rs`、`pkg/config/Cargo.toml`、`cmd/tidb-server/main.rs`。
- Rust 独立测试：`pkg/config/const_3_aster_unit_test.rs` 覆盖命名、前缀、大小写去重、解析、排序、必填缺失和深拷贝；`pkg/config/config_test.rs` 覆盖 TOML 与配置层限制；`cmd/tidb-server/main_test.rs` 覆盖 TiKV starter 合并及非 TiKV 跳过。
- Go 对照与测试：`pkg/config/keyspace_observability.go`、`pkg/config/config_test.go`、`cmd/tidb-server/main.go`、`cmd/tidb-server/main_test.go`；消费端引用由 `pkg/util/metricsutil/common.go`、`pkg/sessionctx/variable/slow_log.go`、`pkg/util/stmtsummary/v2/logger.go` 复核。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付时执行固定十一章节结构校验，并人工复核本文没有把未确认的 Rust 最终消费链描述为已支持。
