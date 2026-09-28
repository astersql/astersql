// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! benchdb command — parse jobs, open a TiKV-backed session (stubbed on arm64),
//! and run SQL benchmark workloads. Logic matches `cmd/benchdb/main.go`.
//! 该模块承接 `benchdb` 命令的主流程，把命令行参数解析、TiKV 会话初始化、
//! 基准作业拆分与执行时序统计集中在一个入口文件中。
//! Rust 版本刻意维持与 Go 实现同名函数和相近控制流，方便对照迁移时确认
//! 作业语义、错误路径以及事务边界都没有被“Rust 化”后意外改变。
//! 这里的 `stubs` 抽象并不试图隐藏行为，而是把真实依赖替换成可记录调用的
//! 轻量实现，使 parity test 可以验证 SQL 文本、参数和执行顺序是否与 Go 对齐。

use std::time::{Duration, Instant};

use crate::stubs::{
    self, Flags, RecordingSession, Session, SessionFactory, SqlArg, Storage, StoreType, TiKVDriver,
};

/// Package-level flags (Go `var` block); overwritten by `flag.Parse` in `main`.
/// 中文补充：保持一个独立的默认值构造函数，便于二进制入口和对照测试都从
/// 同一组旗标基线开始，避免测试自行拼装默认值时和 Go `flag` 注册块漂移。
pub fn default_flags() -> Flags {
    Flags::default()
}

/// Entry matching Go `main`.
/// 中文补充：入口只负责从环境提取参数、解析旗标并打印帮助，再把控制权交给
/// `run_with_flags`。这样测试可以绕过真实进程参数，直接覆盖作业调度逻辑。
pub fn main() {
    let args = stubs::args_from_env();
    let flags = stubs::parse_flags(&args);
    stubs::print_defaults();
    run_with_flags(flags, SessionFactory::default());
}

/// Run job pipeline after flags are known (used by binary and parity tests).
/// 中文补充：日志初始化与 store 注册在这里完成，而不是散落到各个作业内部，
/// 目的是与 Go 一样在执行任何 SQL 前先固定运行环境。
/// `run_jobs` 使用 `|` 分隔多个阶段，便于把建表、写入、更新、查询串成一次
/// 可复现实验；未知作业直接打印并返回，保持原实现的“失败即停止”策略。
pub fn run_with_flags(flags: Flags, factory: SessionFactory) {
    let err = stubs::init_logger(stubs::new_log_config(
        flags.log_level.clone(),
        stubs::DEFAULT_LOG_FORMAT,
        "",
        "",
        stubs::EMPTY_FILE_LOG_CONFIG.clone(),
        false,
    ));
    stubs::must_nil(err);
    let err = stubs::store_register(StoreType::TiKV, &TiKVDriver);
    stubs::must_nil(err);

    let mut ut = new_bench_db(&flags, &factory);
    let works: Vec<String> = flags.run_jobs.split('|').map(|s| s.to_string()).collect();
    for v in &works {
        let work = v.trim().to_lowercase();
        let (name, spec) = ut.must_parse_work(&work);
        match name.as_str() {
            "create" => ut.create_table(),
            "truncate" => ut.truncate_table(),
            "insert" => ut.insert_rows(&spec),
            "update-random" | "update_random" => ut.update_random_rows(&spec),
            "update-range" | "update_range" => ut.update_range_rows(&spec),
            "select" => ut.select_rows(&spec),
            "query" => ut.query(&spec),
            _ => {
                c_log(&format!("Unknown job {v}"));
                return;
            }
        }
    }
}

/// Go `benchDB`.
/// 中文补充：该结构体同时保存底层 store、用于执行内部 SQL 的会话，以及一份
/// 旗标快照。后续所有作业都只依赖这里的状态，避免在多个函数间重复传参。
/// 保留 `store` 字段即使当前文件未直接读取，是为了与 Go 版资源布局一致，
/// 也方便测试断言初始化顺序没有被重排。
pub struct BenchDB {
    pub store: Storage,
    pub session: RecordingSession,
    pub flags: Flags,
}

/// Go `newBenchDB`.
/// 中文补充：初始化顺序严格贴近 Go 版本，先建 TiKV store，再设置全局 store
/// 类型、启动 owner manager、bootstrap session，最后创建执行 SQL 的 session。
/// `disableGC=true` 不是优化细节，而是基准程序的运行约束：写入压测期间避免
/// 自动 GC 干扰时延统计，需要时再由外部流程手动触发。
pub fn new_bench_db(flags: &Flags, factory: &SessionFactory) -> BenchDB {
    // Create TiKV store and disable GC as we will trigger GC manually.
    let path = format!("tikv://{}?disableGC=true", flags.addr);
    let (store, err) = stubs::store_new(&path);
    stubs::must_nil(err);

    // maybe close below components, but it's for test anyway.
    stubs::set_global_store(StoreType::TiKV);
    let err = stubs::start_owner_manager(&store);
    stubs::must_nil(err);
    let err = stubs::bootstrap_session(&store);
    stubs::must_nil(err);
    let (mut se, err) = (factory.create)(&store);
    stubs::must_nil(err);
    // Go discards the result set (`_`) and only checks err.
    let (_rs, err) = se.ExecuteInternal("use test", &[]);
    stubs::must_nil(err);

    BenchDB {
        store,
        session: se,
        flags: flags.clone(),
    }
}

impl BenchDB {
    /// Go `mustExec` — execute internal SQL and drain the result set.
    ///
    /// Go uses `log.Fatal` → `os.Exit`, which skips defers; on execute/Next
    /// failure we likewise Fatal without Close. Close runs only on the success path
    /// (equivalent to the deferred Close after a normal return).
    /// 中文补充：这里统一封装“执行内部 SQL 并把结果集读空”的模式，原因不是
    /// 需要结果，而是与 Go 版一样确保语句真正完成，避免懒执行把耗时记到后续
    /// 操作上。对 `SELECT` 和 `DDL` 都走同一路径，可以让基准日志反映完整成本。
    /// 失败时立即 `fatal`，故意不做恢复；这保持了 benchmark 工具的诊断风格：
    /// 一旦环境或 SQL 不符合预期，立刻终止并保留首个错误现场。
    pub fn must_exec(&mut self, sql: &str, args: &[SqlArg]) {
        let (mut rs, err) = self.session.ExecuteInternal(sql, args);
        if let Some(e) = err {
            stubs::fatal(e.Error());
        }
        if let Some(result_set) = rs.as_mut() {
            let mut req = result_set.NewChunk();
            loop {
                if let Some(e) = result_set.Next(&mut req) {
                    stubs::fatal(e.Error());
                }
                if req.NumRows() == 0 {
                    break;
                }
            }
            if let Some(e) = result_set.Close() {
                stubs::fatal(e.Error());
            }
        }
    }

    /// Go `mustParseWork`.
    /// 中文补充：作业名与规格使用第一个 `:` 分隔，后续部分原样拼回，允许
    /// `query` 之类的规格中继续包含冒号而不破坏解析。
    pub fn must_parse_work(&self, work: &str) -> (String, String) {
        let strs: Vec<&str> = work.split(':').collect();
        if strs.len() == 1 {
            return (strs[0].to_string(), String::new());
        }
        (strs[0].to_string(), strs[1..].join(":"))
    }

    /// Go `mustParseInt`.
    /// 中文补充：统一把字符串到整数的转换失败收敛到 fatal 路径，避免调用者在
    /// 每个解析点都处理错误，保持与 Go 的命令行工具风格一致。
    pub fn must_parse_int(&self, s: &str) -> i64 {
        match s.parse::<i64>() {
            Ok(i) => i,
            Err(e) => stubs::fatal(e.to_string()),
        }
    }

    /// Go `mustParseRange`.
    /// 中文补充：范围采用 `start_end` 约定，既校验格式也校验边界方向。
    /// `start < 0` 或 `end < start` 都会被视为输入无效，因为后续 SQL 生成依赖
    /// 一个非负且单调的区间，放过非法值只会把错误推迟到更难定位的执行阶段。
    pub fn must_parse_range(&self, s: &str) -> (i64, i64) {
        let strs: Vec<&str> = s.split('_').collect();
        if strs.len() != 2 {
            stubs::fatal(format!("parse range failed: invalid range {s}"));
        }
        let start = self.must_parse_int(strs[0]);
        let end = self.must_parse_int(strs[1]);
        if start < 0 || end < start {
            stubs::fatal(format!("parse range failed: invalid range {s}"));
        }
        (start, end)
    }

    /// Go `mustParseSpec`.
    /// 中文补充：规格默认把第三段次数视为 `1`，使诸如 `insert:0_10000` 这种
    /// 常见写法保持简洁；显式次数主要用于查询和更新等重复执行场景。
    pub fn must_parse_spec(&self, s: &str) -> (i64, i64, i64) {
        let strs: Vec<&str> = s.split(':').collect();
        let (start, end) = self.must_parse_range(strs[0]);
        if strs.len() == 1 {
            return (start, end, 1);
        }
        let count = self.must_parse_int(strs[1]);
        (start, end, count)
    }

    /// Go `createTable`.
    /// 中文补充：建表 SQL 与 Go 版文本保持一致，便于 parity test 逐字比较，
    /// 也避免迁移时因 DDL 细节差异导致基准数据分布变化。
    pub fn create_table(&mut self) {
        c_log("create table");
        let create_sql = r#"CREATE TABLE IF NOT EXISTS %n (
  id bigint(20) NOT NULL,
  name varchar(32) NOT NULL,
  exp bigint(20) NOT NULL DEFAULT '0',
  data blob,
  PRIMARY KEY (id),
  UNIQUE KEY name (name)
)"#;
        self.must_exec(create_sql, &[SqlArg::Ident(self.flags.table_name.clone())]);
    }

    /// Go `truncateTable`.
    /// 中文补充：清表单独作为一个作业阶段，方便在同一条 `run_jobs` 流水线里
    /// 复用已有表结构并重置数据，而不是每次都重新建库建表。
    pub fn truncate_table(&mut self) {
        c_log("truncate table");
        self.must_exec(
            "truncate table %n",
            &[SqlArg::Ident(self.flags.table_name.clone())],
        );
    }

    /// Go `runCountTimes`.
    /// 中文补充：所有需要重复执行的 benchmark 作业都经由这里统计耗时。
    /// 它记录首轮、末轮、最小、最大和总耗时，而不是只给平均值，原因是压测中
    /// 冷启动、尾部抖动和极端峰值都可能揭示与缓存、事务提交或网络波动有关的问题。
    /// `minv`/`maxv` 采用非零初始值，复刻 Go 的 `time.Minute` / `time.Nanosecond`
    /// 哨兵写法，避免首轮之前就需要额外的 `Option` 状态。
    pub fn run_count_times<F>(&mut self, name: &str, count: i64, mut f: F)
    where
        F: FnMut(&mut BenchDB),
    {
        let mut sum = Duration::ZERO;
        let mut first = Duration::ZERO;
        let mut last = Duration::ZERO;
        let mut minv = Duration::from_secs(60); // time.Minute
        let mut maxv = Duration::from_nanos(1); // time.Nanosecond

        c_log_f(&format!("{name} started"));
        for _ in 0..count {
            let before = Instant::now();
            f(self);
            let dur = before.elapsed();
            if first == Duration::ZERO {
                first = dur;
            }
            last = dur;
            if dur < minv {
                minv = dur;
            }
            if dur > maxv {
                maxv = dur;
            }
            sum += dur;
        }
        // A negative Go integer range executes zero iterations. Its zero sum
        // divided by the negative count remains zero; keep that behavior while
        // preserving Go's divide-by-zero panic for count == 0.
        let avg = if count < 0 {
            Duration::ZERO
        } else {
            let avg_nanos = sum.as_nanos() / u128::try_from(count).unwrap();
            Duration::from_nanos(u64::try_from(avg_nanos).unwrap())
        };
        c_log_f(&format!(
            "{name} done, avg {avg:?}, count {count}, sum {sum:?}, first {first:?}, last {last:?}, max {maxv:?}, min {minv:?}\n\n"
        ));
    }

    /// Go `insertRows` (#nosec G404).
    /// 中文补充：插入流程按批次开启事务，每批提交一次，以模拟批量写入而不是
    /// 单条 autocommit。循环次数使用区间长度和批大小推导，保证最后一批即使
    /// 未满也会被执行。
    /// blob 只分配一半旗标长度，保持与 Go 版相同的数据体量假设；随机填充发生
    /// 在每行插入前，使每条记录的 payload 都不同，减少存储层压缩带来的偏差。
    pub fn insert_rows(&mut self, spec: &str) {
        let (start, end, _) = self.must_parse_spec(spec);
        let batch = self.flags.batch_size;
        let loop_count = (end - start + batch - 1) / batch;
        let mut id = start;
        let blob_half = usize::try_from(self.flags.blob_size / 2).unwrap();
        let table = self.flags.table_name.clone();
        self.run_count_times("insert", loop_count, |ut| {
            ut.must_exec("begin", &[]);
            let mut buf = vec![0u8; blob_half];
            for _ in 0..batch {
                if id == end {
                    break;
                }
                stubs::rand_read(&mut buf);
                let insert_query = "insert %n (id, name, data) values(%?, %?, %?)";
                ut.must_exec(
                    insert_query,
                    &[
                        SqlArg::Ident(table.clone()),
                        SqlArg::from(id),
                        SqlArg::from(id),
                        SqlArg::from(buf.clone()),
                    ],
                );
                id += 1;
            }
            ut.must_exec("commit", &[]);
        });
    }

    /// Go `updateRandomRows` (#nosec G404).
    /// 中文补充：随机更新并不是遍历区间，而是在给定范围内随机挑选主键并累加
    /// `exp`。`run_count` 独立于外层循环次数，是为了在最后一批不足 `batch` 时
    /// 仍然精确执行 `total_count` 次更新。
    /// 这里沿用 Go 的随机数策略，不尝试做去重或均匀性修正，因为 benchmark
    /// 关注的是与原工具相同的访问模式，而不是统计学上的完美采样。
    pub fn update_random_rows(&mut self, spec: &str) {
        let (start, end, total_count) = self.must_parse_spec(spec);
        let batch = self.flags.batch_size;
        let loop_count = (total_count + batch - 1) / batch;
        let mut run_count = 0;
        let table = self.flags.table_name.clone();
        self.run_count_times("update-random", loop_count, |ut| {
            ut.must_exec("begin", &[]);
            for _ in 0..batch {
                if run_count == total_count {
                    break;
                }
                let id = stubs::rand_intn(end - start) + start;
                let update_query = "update %n set exp = exp + 1 where id = %?";
                ut.must_exec(
                    update_query,
                    &[SqlArg::Ident(table.clone()), SqlArg::from(id)],
                );
                run_count += 1;
            }
            ut.must_exec("commit", &[]);
        });
    }

    /// Go `updateRangeRows`.
    /// 中文补充：区间更新把同一条 SQL 执行 `count` 次，每次都在独立事务中提交，
    /// 目的是观察固定热点范围被反复写入时的吞吐和波动。
    pub fn update_range_rows(&mut self, spec: &str) {
        let (start, end, count) = self.must_parse_spec(spec);
        let table = self.flags.table_name.clone();
        self.run_count_times("update-range", count, |ut| {
            ut.must_exec("begin", &[]);
            let update_query = "update %n set exp = exp + 1 where id >= %? and id < %?";
            ut.must_exec(
                update_query,
                &[
                    SqlArg::Ident(table.clone()),
                    SqlArg::from(start),
                    SqlArg::from(end),
                ],
            );
            ut.must_exec("commit", &[]);
        });
    }

    /// Go `selectRows`.
    /// 中文补充：查询阶段不显式开启事务，保持只读路径尽量轻量；但仍通过
    /// `must_exec` 读空结果集，确保统计到真实扫描和传输成本。
    pub fn select_rows(&mut self, spec: &str) {
        let (start, end, count) = self.must_parse_spec(spec);
        let table = self.flags.table_name.clone();
        self.run_count_times("select", count, |ut| {
            let select_query = "select * from %n where id >= %? and id < %?";
            ut.must_exec(
                select_query,
                &[
                    SqlArg::Ident(table.clone()),
                    SqlArg::from(start),
                    SqlArg::from(end),
                ],
            );
        });
    }

    /// Go `query` — spec is `sql:count`.
    /// 中文补充：自定义查询允许把任意 SQL 直接纳入同一套计时框架，适合对比
    /// 特定语句模板的性能。这里假定规格至少包含一次冒号，因为它面向受控输入
    /// 的 benchmark 作业配置，而不是对外暴露的通用解析器。
    pub fn query(&mut self, spec: &str) {
        let strs: Vec<&str> = spec.split(':').collect();
        let sql = strs[0];
        let count = match strs[1].parse::<i64>() {
            Ok(c) => c,
            Err(e) => {
                stubs::must_nil(Some(stubs::Error::new(e.to_string())));
                unreachable!()
            }
        };
        self.run_count_times("query", count, |ut| {
            ut.must_exec(sql, &[]);
        });
    }
}

/// Go `cLogf`.
/// 中文补充：彩色日志只是命令行可读性增强，不参与逻辑判断；保留绿色输出是
/// 为了与原工具的人工观测体验一致，便于在大量 benchmark 日志中快速定位阶段。
pub fn c_log_f(msg: &str) {
    println!("\u{1b}[0;32m{msg}\u{1b}[0m\n");
}

/// Go `cLog`.
/// 中文补充：纯消息版本与格式化版本拆开，保持调用点与 Go 版函数名一一对应，
/// 让迁移审查时更容易逐段比对。
pub fn c_log(msg: &str) {
    println!("\u{1b}[0;32m{msg}\u{1b}[0m\n");
}
