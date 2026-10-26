# PostgreSQL interval 符号与输出样式

任务33，2026-10-26，IV-SIGNS-STYLE。复用任务32提交
`bb0bc2024846f88b02b9ddab1e76cf83ca56884f` 的三分量身份和实际完成证据。
用户追加授权允许补齐本项原依赖大计划11的IntervalStyle会话与生命周期；
没有建设通用GUC、时间运算或其他类型族。

## 实现与契约依据

- `pkg/types/pg_temporal.rs`：新增 `PgIntervalStyle`、`PgIntervalSetting`，
  `PgInterval::parse_with_style` 与 `format_with_style`。按第3章间隔契约保留
  月/日/微秒，支持独立符号、SQL标准统一负号、四种样式输出及回读；原始binary布局不变。
- `pkg/session/runtime/pg_temporal.rs`、`session.rs`：真实会话的缺省、SESSION、LOCAL
  有效值与事务快照；复用原有准备期AST常量机制冻结interval字面量，text参数cast在执行期读取设置。
- `pkg/session/runtime/control.rs`、`transaction.rs`：复用现有事务和保存点路径，
  COMMIT保留SESSION并清LOCAL，ROLLBACK恢复事务前设置，保存点回滚恢复相应设置；
  提交失败按回滚恢复。仅增加IntervalStyle状态的保存/恢复。
- `pkg/session/runtime/query.rs`：真实interval cast读取有效样式；interval转文本使用有效样式，
  继续调用现有文本转换校验以保留varchar长度等目标约束。
- `pkg/server/conn.rs`、`runtime.rs`：设置/读取请求送到拥有ConcreteSession的原worker。
- `pkg/server/pg_session.rs`、`pg_conn.rs`：Q及扩展协议使用同一SET/SHOW/RESET路径，
  接受SESSION/LOCAL，startup缺省供RESET恢复；事务外LOCAL返回25P01警告且不生效。
- `pkg/server/pg_extended.rs`：OID1186文本参数在Bind按有效样式解析；binary不受样式影响。
- `pkg/server/pg_result.rs`：simple/extended结果在Execute读取样式，仅text显示变化，
  OID1186/16/-1及binary不变。仅interval结果增加一次worker读取，其他结果不增加请求。
  为生命周期测试开放已有真实SavepointStmt/ReleaseSavepointStmt的命令门禁，
  没有重写协议或事务状态机。

独立测试位于 `pkg/types/pg_temporal_test.rs`、
`pkg/session/runtime/pg_temporal_test.rs`、`pkg/server/pg_temporal_test.rs`、
`pkg/server/pg_session_test.rs`；生产文件不包含测试函数。

## 行为与参考

混合输入 `-1 month +2 days -03:04:05.123456` 的分量为
`(-1,2,-11045123456)`，16字节网络序为
`fffffffd6da8ca8000000002ffffffff`。对应输出：

| 样式 | 文本 |
| --- | --- |
| postgres | `-1 mons +2 days -03:04:05.123456` |
| postgres_verbose | `@ 1 mon -2 days 3 hours 4 mins 5.123456 secs ago` |
| sql_standard | `-0-1 +2 -3:04:05.123456` |
| iso_8601 | `P-1M2DT-3H-4M-5.123456S` |

`-1 2:03:04` 在postgres中是 `(0,-1,7384000000)`，在sql_standard中是
`(0,-1,-7384000000)`，不是同一个规范值。零值分别显示
`00:00:00`、`@ 0`、`0`、`PT0S`，NULL仍为长度-1。

参考采样使用隔离临时initdb，PostgreSQL **18.4 (Homebrew)**，
aarch64-apple-darwin25.4.0，Apple clang 21.0.0，UTF8。
逐样式执行SET后，对混合、纯负、零和短负号输入查询 `v::interval` 与
`encode(interval_send(v::interval),'hex')`；输出保留于 `target/iv33-reference.log`。
参考进程已停止，自己的临时数据已删除。该采样不冒充18.6或完整libpq差分；
本族真实客户端差分仍由任务39承担。

## 验证

红灯 `target/iv33-red.log`：测试编译成功并实际运行1项，SET IntervalStyle返回42601，
测试失败，退出101。首次核心实现后同一过滤器实际运行1项通过。
边界测试另发现SQL标准输出i32最小日分量回读误判语法，修正并保留断言。
生命周期扩展还发现SAVEPOINT被PG命令门禁拒绝0A000，按已有真实AST/执行器最小接通。
夹具曾将事务状态字节误传为u8导致编译失败，修正后重跑；不将其算作行为红灯。

WIP用于迭代，Ready按根协议用于交付；`.agents/skills/tidb-verify-profile` 不存在，
没有声称技能已通过。所有Cargo阶段动态原子领取槽位1，设置：

    CARGO_TARGET_DIR=/Users/Shared/work/dir/data/codes/astersql-tidb/target/rust-slot-1

每次在锁内记录计划、任务、会话与PID，EXIT/INT/TERM清理自己的锁；保留编译缓存。
命令均从仓库根运行，Cargo继承上述环境：

    cargo fmt --all
    env -u LDFLAGS cargo test --locked -p astersql-types --lib pg_interval -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-session --lib interval_style_lifecycle -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_signs_style -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_temporal_iv_components -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_types_test -- --nocapture
    env -u LDFLAGS cargo test --locked -p astersql-server --lib pg_session_test -- --nocapture
    make lint
    git diff --check
    git diff --cached --check

没有Go源、import、依赖或Bazel元数据变更，不触发bazel_prepare；没有运行
make bazel_lint_changed。Rust failpoint功能未额外启用，没有运行Go包单测或改写Go failpoints。

## 限制与风险

正确性风险集中在尚未独立验收的其他interval输入、fields/p和特殊值规则，仍交后续任务。
本项覆盖符号和样式，不宣称全interval兼容。兼容性测试保留任务32基线与元数据回归。
四样式格式化增加有界标量字符串分配，interval结果多一次worker状态读取，未做性能基准。
未验证18.6、持久TiKV或任务39的完整客户端差分。

## 最终结果

最终fmt后的 `target/iv33-green.log`：上述六个测试命令全部退出0，分别运行
2、1、1、1、7、3项，共15项有效Rust测试通过。真实TCP测试覆盖四样式的
IT→OT、IT→OB、IB→OT、IB→OB，混合/纯负/零/NULL、1186/16/-1与原始字节，
以及多行、首行NULL、全NULL、零行、重复Execute和两种结果格式。
Q/扩展协议均覆盖设置、事务结束与保存点恢复，独立连接startup/reset、
事务外LOCAL警告与无副作用，以及非法文本/编码/二进制长度和错误后SELECT 1恢复。
独立会话测试还覆盖准备期常量与执行期text参数分流、RELEASE保存点及提交失败恢复。
`target/iv33-lint.log` 的make lint退出0，diff空白检查通过。
所有本任务槽位锁已释放，缓存保留，参考PG进程和临时数据已清理。
