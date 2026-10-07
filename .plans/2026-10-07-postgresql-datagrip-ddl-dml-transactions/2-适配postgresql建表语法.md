# 任务 2: 适配 PostgreSQL 常用建表语法

批次：【批次 2】 依赖批次 1

状态：未开始

目的：让 PG 简单/扩展协议用同一有界适配器创建 DataGrip 可识别的表，不影响 MySQL listener。

来源任务：无

预计会话范围：实现 CREATE TABLE 的 schema/标识符、常用类型和默认值映射；不处理 ALTER、RETURNING 或 PostgreSQL 专属约束语义。

## 文件

- 新建：`pkg/server/pg_sql.rs`
- 新建：`pkg/server/pg_sql_test.rs`
- 修改：`pkg/server/lib.rs`
- 修改：`pkg/server/pg_conn.rs`
- 修改：`pkg/server/pg_extended.rs`
- 测试：`pkg/server/pg_query_test.rs`
- 测试：`pkg/server/pg_extended_test.rs`

## 上下文

- 适配器只在 PG listener 调用；MySQL SQL 文本不得经过它。
- 支持 `public."CaseSensitiveTable"` 这类 schema 限定和双引号标识符，以及 `smallint/integer/bigint/real/double precision/numeric/decimal/boolean/char/varchar/text/bytea/date/time/timestamp` 的无损原生映射。
- `serial/bigserial` 仅在能映射为真实自增整数并由目录正确呈现默认/identity 信息时支持；否则以 `0A000` 明确拒绝，不得静默降级。

## Cargo 共享槽位规则（仅 Rust/Cargo 任务）

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 测试计划

- 行为：简单 Query 与 Parse/Bind/Execute 接收同一组 PG CREATE TABLE，并由 RetrieveTables/RetrieveColumns 返回真实名称、类型、长度、NULL 和默认值。
- 失败验证测试：新增 `pg_sql_test::postgres_create_table_mapping`，扩展两条协议测试覆盖真实执行。
- 失败验证命令：`cargo test -p astersql-server --lib postgres_create_table -- --test-threads=1`
- 预期失败原因：canonical parser 不接受至少一种 PG 专属类型/语法，或两条协议路径适配不一致。
- 通过验证命令：同失败验证命令。
- 模拟策略：语法单元测试验证拒绝边界；协议测试使用真实 canonical session 和目录查询。

## 步骤

1. 先加入简单与扩展协议失败测试，验证 MySQL listener 隔离。
2. 实现有长度/深度限制、识别引号和注释边界的适配器；禁止字符串替换式误改字面量。
3. 在 Query 与 Parse 的执行前统一调用，错误映射为 `42601`、`0A000` 或合适 SQLSTATE。
4. 用完整 DataGrip 列查询确认原生元数据结果，不只检查 CREATE 命令标签。

## 验证

- 运行：`cargo fmt --all`，然后运行 `cargo test -p astersql-server --lib postgres_create_table -- --test-threads=1`
- 预期：PG 两条协议路径通过，非法/不可映射语法稳定拒绝，MySQL 现有协议测试不受影响。
- 所需证据：红绿测试输出、每种支持类型的目录结果、拒绝 SQLSTATE、测试数量、退出码、槽位和差异检查。

## 完成

完成时列出确切支持类型与拒绝项；不得把计划范围扩展成完整 PostgreSQL parser。状态变为 `已完成` 后使用 `$git-commit` 提交本任务且删除任务文件。
