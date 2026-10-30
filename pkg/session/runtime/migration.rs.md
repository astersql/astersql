# `pkg/session/runtime/migration.rs`

## 文件定位

ConcreteSession 的读时间戳及依赖系统变量迁移片段，对应来源 f137844d4ff6fb8dc5cfa6963c23778ab15681db；复用既有 AST、SessionStates JSON、时区、SET 与历史读实现。

## 核心职责

导出有效 snapshot/尚未消费的 tx_read_ts，按变量依赖顺序恢复，确保迁移后的首次与后续查询沿用既有历史读生命周期。

## 主要符号

parse_session_read_datetime 解析当前会话墙钟时间；show_migration_system_vars 导出本增量涉及的系统变量；restore_migration_system_vars 解码并复用 execute_set。

## 执行流程

SHOW SESSION_STATES 从 RuntimeState 读取 snapshot_read_ts/pending_stale_read_ts，通过 session_vars 的规范 hook 导出并编码 sys-vars。SET SESSION_STATES 解码既有 SessionStates 类型，OrderByDependency 保证时区、字符集和隔离检查开关优先，使用既有 SQL parser 生成 SET AST 再执行真实 setter。

## 数据与状态

RuntimeState 是时间戳消费的权威来源；hint变量缓存保存客户端 SET 拼写及校验信息。两者在导出边界桥接，不复制存储、事务或历史读实现。JSON 使用既有 sys-vars 字段。

## 依赖与调用关系

从 dispatch 的既有AST分支接入；依赖 sessionctx-variable、sessionctx-sessionstates、chrono/chrono-tz、既有 parser 和 control::execute_set。真实查询仍由原查询/事务模块负责。

## 错误处理与边界

解析和 setter 错误直接传播；无效 DST 墙钟时间拒绝解析，回拨歧义采用与 Go 相同的首个时刻，状态导出再校验精确往返，无法无损表达则拒绝迁移。JSON 和SQL字符串转义使用既有序列化与解析路径。

## 并发与资源生命周期

ConcreteSession 在线程内使用 RefCell 状态；读取有效时间戳后释放借用再锁规范变量缓存，不在锁内执行SQL。一次性读消费继续由现有事务/查询入口管理，无新增线程、连接或外部资源。

## 与 Go 版本的对应关系

本模块是 TestSessionStatesReadTimestamps 和 testSystemVars 新增分支所需的局部真实入口，涵盖有效时间戳、依赖恢复和连续读取。本文件不是完整会话迁移框架；用户变量、预编译、绑定与认证迁移不在本来源增量内，原有相应模块保留。没有用 JSON 形状测试替代真实读取。

## 扩展指南

其他迁移能力应在其独立来源任务中复用此边界；不得让导出从缓存字符串推断已消费时间戳。新增系统变量必须在注册表中声明依赖并用真实 SET/读取验证副作用。PostgreSQL保护回归与原调用路径保持一致。
