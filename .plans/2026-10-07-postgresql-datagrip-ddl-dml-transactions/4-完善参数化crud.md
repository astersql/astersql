# 任务 4: 完善 PostgreSQL 参数化 CRUD

批次：【批次 3】 依赖批次 2

状态：已完成，待回归

目的：让 PG JDBC/DataGrip 以 PreparedStatement 完成 INSERT、SELECT、UPDATE、DELETE，并返回正确结果类型和影响行数。

来源任务：无

预计会话范围：聚焦扩展协议参数、DML 执行和命令标签；不实现 COPY、批量多语句、RETURNING 或新数据类型体系。

## 文件

- 修改：`pkg/server/pg_extended.rs`
- 修改：`pkg/server/pg_result.rs`
- 新建：`pkg/server/pg_dml_test.rs`
- 修改：`pkg/server/lib.rs`
- 测试：`pkg/server/pg_extended_test.rs`
- 测试：`pkg/server/pg_client_integration_test.rs`

## 上下文

- 当前已有 `$N` marker、部分文本/二进制 OID 和基础 JDBC INSERT/SELECT；本任务补齐 UPDATE/DELETE 的参数化执行、NULL、重复/乱序参数、影响行数和错误恢复。
- 仅支持已有结果编码可无损表达的 OID；未知 OID 返回 `0A000`，坏值返回 `22P02/22P03`。
- DML `RETURNING` 不在本阶段；客户端若请求必须明确拒绝且不重复执行写入。

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

- 行为：PG JDBC 在自动提交模式下参数化插入、按参数查询、更新、删除，分别得到 1 行影响计数；NULL、重复/乱序 `$N` 和二进制整数参数正确，错误后 Sync 可恢复。
- 失败验证测试：新增 `pg_dml_test::prepared_crud_roundtrip`，扩展 JDBC 工作流覆盖全部四类操作。
- 失败验证命令：`cargo test -p astersql-server --lib prepared_crud_roundtrip -- --test-threads=1`
- 预期失败原因：现有客户端回归未证明参数化 UPDATE/DELETE、完整格式组合或命令影响行数。
- 通过验证命令：同失败验证命令。
- 模拟策略：协议单测使用真实 TCP listener；JDBC 测试调用本机已安装驱动，不 mock 数据库响应。

## 步骤

1. 写覆盖文本/二进制参数、NULL、乱序 marker 与错误恢复的失败测试。
2. 只补测试暴露的 Bind/Execute/命令标签缺口，保证 portal 生命周期和 Sync 规则不退化。
3. 确认写操作只执行一次，Describe 不执行 SQL，错误的 RETURNING 不产生副作用。
4. 运行现有 extended、types、error 与 JDBC 客户端回归。

## 验证

- 运行：`cargo fmt --all`，然后运行 `cargo test -p astersql-server --lib pg_ -- --test-threads=1`，再运行 `cargo test -p astersql-server --lib pg_client_integration_test::pg_introspection_clients -- --exact --test-threads=1 --nocapture`
- 预期：PreparedStatement CRUD、影响行数、错误恢复全部通过，JDBC 两个受支持版本均执行有效断言。
- 所需证据：四类 DML 的行内容/计数、测试数量、JDBC 版本输出、退出码、槽位及差异检查。

## 完成

完成时列出支持的参数 OID 与结果格式；不得用执行后补查猜测影响行数。状态变为 `已完成` 后使用 `$git-commit` 提交本任务且删除任务文件。

## 完成记录（2026-10-09）

- 支持的普通 DML 参数 OID：`16`/bool、`17`/bytea、`20`/int8、`21`/int2、`23`/int4、`25`/text、`700`/float4、`701`/float8、`1042`/bpchar、`1043`/varchar、`1082`/date、`1083`/time、`1114`/timestamp、`1700`/numeric；文本与二进制 Bind 均通过现有严格值校验。
- 结果格式：查询 portal 支持逐列文本/二进制格式；INSERT/UPDATE/DELETE 使用引擎 `affected_rows` 直接生成 `INSERT 0 n`/`UPDATE n`/`DELETE n`，不执行后补查。
- 新增真实 TCP listener 回归，覆盖参数化 INSERT/SELECT/UPDATE/DELETE、NULL、文本/二进制混合格式、重复/乱序 `$N`、影响行数、错误后 Sync 恢复，以及 INSERT/UPDATE/DELETE RETURNING 在 Parse 阶段以 `0A000` 拒绝且无写入副作用。
- 执行 `cargo fmt --all`，退出码 0。
- 执行 `cargo test -p astersql-server --lib prepared_crud_roundtrip -- --test-threads=1`，退出码 0：1 passed，0 failed、226 filtered out。
- 执行 `cargo test -p astersql-server --lib pg_client_integration_test::pg_introspection_clients -- --exact --test-threads=1 --nocapture`，退出码 0：1 passed，0 failed、226 filtered out；JDBC 42.7.13 与 42.7.3 均通过 27 条 DataGrip SQL 及 PreparedStatement CRUD，libpq PG 3.0/3.2 通过。
- 任务 3 提交后重跑 `cargo test -p astersql-server --lib pg_ -- --test-threads=1`：先修复其适配器对既有表级 `KEY`/`INDEX` 和 `ALTER TABLE ... RENAME TO` 的误拦截，对应 `pg_catalog_query_test::pg_introspection_oid_live_casts` 已从失败变为 1 passed。随后全量回归在无关的旧目录断言处停止：`pg_catalog_test::pg_introspection_relations_live` 要求 `SELECT xmin FROM pg_catalog.pg_class` 返回 `0A000`，而当前目录实现已明确为 `pg_class.xmin` 提供 NULL 投影；单独运行该测试退出码 101，0 passed，1 failed，226 filtered out。
- Cargo 初始领取槽位 1，并行锁冲突后重新领取槽位 2；任务 3 提交后的最终回归阶段再次动态领取槽位 1，最终 `CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1`。
- 任务 3 提交后，再次执行本任务定向测试与精确客户端回归，均退出码 0；结果仍为各 1 passed、0 failed、226 filtered out，JDBC 42.7.13/42.7.3 和 libpq PG 3.0/3.2 全部通过。
- 未验证：完整 `pg_` 回归全绿及 `make lint`。阻塞是上述 `pg_class.xmin` 实现与旧测试期望之间的无关目录契约冲突，需单独确认应保留 NULL 投影还是恢复 `0A000`。
