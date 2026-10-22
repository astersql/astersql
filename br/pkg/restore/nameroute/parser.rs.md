# `br/pkg/restore/nameroute/parser.rs`

## 文件定位

本文以冻结提交 `2c944fbcbfb2c904cc8cbddafe17954138bd8867` 为准；增量比较为 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867`，不采用工作区或索引中的后续代码。

## 核心职责

将精确名称规则解析为 `Rule`：支持 `source:target` 库规则与 `source_db.source_table:target_db.target_table` 表规则；保留标识符原始大小写，不负责重复源、目标碰撞或系统库禁用策略。

## 主要符号

`parse_rules(&[String]) -> Result<Vec<Rule>, NameRouteError>` 顺序处理规则并附加从 1 开始的规则序号及原文。私有 `SpecParser` 保存字符向量和当前位置；`parse_rule`、`parse_object_name`、`parse_identifier`、`parse_quoted_identifier` 完成递归下降。`validate_identifier` 是 crate 内共享校验入口，库名、表名上限均为 64 个字符。

## 执行流程

先跳空白，解析源对象，要求冒号，再解析目标对象并确认输入耗尽；两端必须同为库或同为表。对象最多包含库和表两个分量。未引用标识符在点、冒号或空白处停止；反引号内的双反引号还原成一个反引号，因此被引用的点和冒号属于名称内容。

## 数据与状态

解析器位置以 `Vec<char>` 下标计数，错误定位是字符位置。规则和对象持有 `String`；空表名表示库对象。ASCII 未引用字符限字母、数字、下划线、美元符号；非 ASCII 字符排除空白和控制字符。此层不做小写化。

## 依赖与调用关系

`router.rs::Router::parse` 调用 `parse_rules`，然后由 `Router::new` 校验规则集合。`router.rs` 还复用 `validate_identifier`，防止直接构造 `Rule` 绕过名称校验。任务层 `restore_name_route.rs::get_name_router` 通过 `Router::parse` 接入配置解析。模块导出证明 API 可达，不证明恢复 DDL 已使用名称计划。

## 错误处理与边界

拒绝空名称、NUL、末尾空白、超过 64 字符、未闭合反引号、未引用 ASCII 标点、缺冒号、多余输入和库/表两端类型不一致。首条失败中止整个 `collect`，不返回部分规则。引号内允许分隔符及前导空白，但仍执行公共名称校验。Rust `String` 已保证 UTF-8 合法。

## 并发与资源生命周期

纯内存同步解析，无线程、锁、I/O 或外部资源。每条规格转成字符向量，解析结果拥有自己的字符串；调用结束后解析器释放。

## 扩展指南

扩展语法时同时核对 Go 解析器和路由 canonical 表达，避免引号或分隔符产生歧义。测试继续放独立文件；当前 Rust 相关用例位于 `br/pkg/task/restore_name_route_test.rs`，覆盖引号、优先级、非法规则与指纹。
