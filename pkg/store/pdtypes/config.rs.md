# [`pkg/store/pdtypes/config.rs`](config.rs)

## 文件定位

用私有模块 `bool_string` 实现 PD JSON 所需的字符串布尔值协议：序列化为 `"true"`/`"false"`，反序列化时接受这两个字符串，并把 JSON `null` 和字符串 `"null"` 映射为 `false`。

## 核心职责

本文件承担两项职责：

1. 用公开结构体 `ReplicationConfig` 表示 Region 副本数量、Store 拓扑标签、placement rule 开关和强制隔离级别。
2. 用私有模块 `bool_string` 实现 PD JSON 所需的字符串布尔值协议：序列化为 `"true"`/`"false"`，反序列化时接受这两个字符串，并把 JSON `null` 和字符串 `"null"` 映射为 `false`。

它刻意只定义数据形状和 Serde 行为。字段组合是否合法（例如 `IsolationLevel` 是否属于 `LocationLabels`）、副本数是否合理、启用 placement rules 后哪些旧字段失效，都没有在这里校验。

## 主要符号

### `ReplicationConfig`

`pub struct ReplicationConfig` 是文件唯一公开业务类型，派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`、`Serialize` 和 `Deserialize`。`#[serde(default)]` 使缺失字段取结构体默认值：数值为 `0`，布尔值为 `false`，字符串为空，`StringSlice` 为空向量。Serde 未启用 `deny_unknown_fields`，所以未知输入字段默认被忽略。

字段与 wire 名称如下：

| Rust 字段 | Serde 名称 | 类型与含义 |
| --- | --- | --- |
| `MaxReplicas` | `max-replicas` | `u64`，每个 Region 的目标副本数 |
| `LocationLabels` | `location-labels` | `StringSlice`，按优先级排列的拓扑标签；JSON 表示是逗号分隔字符串 |
| `StrictlyMatchLabel` | `strictly-match-label` | 字符串编码的布尔值，要求 Store 标签严格匹配配置 |
| `EnablePlacementRules` | `enable-placement-rules` | 字符串编码的布尔值，控制细粒度放置规则 |
| `EnablePlacementRulesCache` | `enable-placement-rules-cache` | 字符串编码的布尔值，控制 rule checker 缓存 |
| `IsolationLevel` | `isolation-level` | `String`，显式强制隔离使用的 location label |

### `bool_string::serialize`

私有 Serde 辅助函数接收 `&bool`：真值调用 `Serializer::serialize_str("true")`，假值调用 `serialize_str("false")`。三个布尔字段通过 `#[serde(with = "bool_string")]` 共用此逻辑。

### `bool_string::deserialize`

私有 Serde 辅助函数先反序列化为 `Option<String>`，再做严格匹配：`Some("true")` 为 `true`；`Some("false")`、`Some("null")` 和 `None` 为 `false`；其他字符串通过 `serde::de::Error::custom` 返回错误。因为目标类型是 `Option<String>`，未加引号的 JSON 布尔值和其他非字符串、非空类型也会由 Serde 拒绝。

## 执行流程

本文件没有主动执行入口，流程由调用方触发的 Serde 编解码驱动。

序列化 `ReplicationConfig` 时：

1. 派生的 `Serialize` 按六个 `#[serde(rename = ...)]` 名称输出字段。
2. `MaxReplicas`、`IsolationLevel` 使用其类型的标准序列化。
3. `LocationLabels` 调用 `pkg/store/pdtypes/typeutil.rs` 中 `StringSlice::serialize`，将向量以逗号连接成一个字符串。
4. 三个布尔字段分别调用 `bool_string::serialize`，所以输出 JSON 字符串而不是 JSON 布尔字面量。

反序列化时：

1. 派生的 `Deserialize` 按连字符形式的字段名匹配输入。
2. 整体 `#[serde(default)]` 为缺失字段补默认值。
3. `StringSlice::deserialize` 将逗号分隔字符串拆成向量，空字符串得到空向量。
4. 三个布尔字段经 `bool_string::deserialize` 解析；合法字符串产生布尔值，空值按 Go 零值语义产生 `false`，非法表示立即返回反序列化错误。

## 数据与状态

`ReplicationConfig` 是拥有其数据的普通值类型：标签保存在 `StringSlice(Vec<String>)`，隔离级别保存在 `String`，其余字段是可复制的标量。它没有内部可变性、全局状态、缓存、句柄或懒加载行为；克隆会克隆两个拥有堆数据的字段。

重要数据约束来自 Go 注释而非 Rust 运行时检查：`LocationLabels` 的顺序表达放置优先级；非空 `IsolationLevel` 应当是 `LocationLabels` 中的一个值；启用 `EnablePlacementRules` 后，PD 侧不再使用 `MaxReplicas`、`LocationLabels` 和隔离配置。Rust 类型允许构造违反这些约束的值，验证责任留给调用方或 PD。

`Default` 只表达 Go 式零值兼容，不代表一组可以直接投入生产的 PD 参数，尤其不能把 `MaxReplicas == 0` 解读为有效的副本策略。

## 依赖与调用关系

直接内部依赖是 `crate::typeutil::StringSlice`；其自定义 Serde 实现决定 `LocationLabels` 的逗号分隔字符串格式。直接外部依赖是 `serde::{Serialize, Deserialize}`，而测试用 `serde_json` 实例化这套协议。`pkg/store/pdtypes/Cargo.toml` 声明 crate 名为 `astersql-store-pdtypes`，并直接依赖 `serde`（derive feature）和 `serde_json`；其中本文件生产代码只直接使用 `serde`。

模块与公开路径为：

`pkg/store/pdtypes/lib.rs` → `config` → `ReplicationConfig`

工作区 facade 路径为：

`Cargo.toml` 的 `facade_store_pdtypes` → `pkg/lib.rs::store::pdtypes` → 整体再导出该 crate

RustCodeGraph 的文件节点报告 `config.rs` 被 `config_test.rs` 和 `migration_aster_unit_test.rs` 两个文件使用。精确仓库搜索同样只得到这两个 Rust 使用点，未得到生产 caller；结构体也没有方法，因此不存在业务级 callee。实际下游调用发生在派生的 Serde 实现与 `bool_string`/`StringSlice` 编解码辅助之间。

## 错误处理与边界

`bool_string::serialize` 不自行制造错误，只透传具体 Serializer 的错误。`bool_string::deserialize` 有两层失败来源：输入不能被解成 `Option<String>` 时透传 Deserializer 类型错误；输入是字符串但不是 `"true"`、`"false"` 或 `"null"` 时返回带原值的 `invalid boolean string ...` 自定义错误。

边界行为由源码和测试共同限定：

- 缺失字段由 `#[serde(default)]` 补零值；`migration_json_missing_fields_use_go_zero_values` 验证空对象等于 `ReplicationConfig::default()`。
- JSON `null` 和字符串 `"null"` 对三个字符串布尔字段映射为 `false`；`replication_config_accepts_null_for_string_encoded_booleans` 覆盖该行为。
- 标准输出始终包含六个字段，因为没有 `skip_serializing_if`；迁移测试验证了完整 JSON shape。
- 未知字段没有显式拒绝策略；Serde 默认忽略它们，有利于前向兼容，但也可能掩盖拼写错误。
- 本文件不验证副本数、标签名称、标签重复、空隔离级别或字段间一致性。

`#[serde(with = "bool_string")]` 对所有 Serde 数据格式生效，不只对 JSON。Go 结构为 JSON 使用 `,string`，但 TOML tag 中布尔字段不是字符串；当前 Rust crate 没有 TOML 依赖或 TOML 测试，因此不能据此声称 Rust 的 TOML 表示与 Go 完全一致。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或外部资源。`ReplicationConfig` 的生命周期就是普通 Rust 所有权生命周期：创建或反序列化后由调用方持有，克隆产生独立副本，离开作用域时释放字符串和向量。

所有编解码函数只使用参数和局部值，没有共享可变状态，因此在具体 Serializer/Deserializer 和所含数据满足相应线程约束时可以由调用方并行使用；本文件自身不提供也不需要并发协调。

## 与 Go 版本的对应关系

需要明确的差异和未验证点：

- Go 同时声明 TOML tag，Rust 只声明通用 Serde rename；仓库证据只验证 JSON，未验证 TOML 兼容。
- Go 字段注释描述了 placement rule 和 isolation 的语义约束，Rust 仅保留字段，不实现校验。

## 扩展指南

新增 PD 配置字段时，首先在 `ReplicationConfig` 增加字段并使用与 Go/PD API 完全一致的 `serde(rename)`；然后在独立测试 `pkg/store/pdtypes/migration_aster_unit_test.rs` 的 JSON shape 断言中加入该字段。若新增字段需要特殊 wire 表示，应像 `bool_string` 或 `StringSlice` 一样把转换逻辑集中在辅助类型/模块，并在独立的 `pkg/store/pdtypes/config_test.rs` 增加合法值、缺失值、`null`、非法类型和非法字符串测试，不要把测试写回生产源文件。

修改现有布尔协议时必须同时评估 Go `encoding/json` 的 `,string` 行为和 PD HTTP 响应兼容性，避免将 JSON 字符串改成布尔字面量。修改 `LocationLabels` 时要同步检查 `typeutil.rs`，否则结构字段和实际 wire 格式会脱节。

如果要增加业务校验，宜提供显式验证函数或经验证的构造入口，而不是改变 `Deserialize` 的零值兼容行为；这能区分“忠实接收 PD/Go 数据”与“允许应用该配置”。性能风险总体较低，但标签编解码会分配/连接字符串；新增大集合字段时应评估 JSON 分配成本。兼容风险主要来自 wire key、默认值、未知字段策略和跨格式 Serde 行为。
