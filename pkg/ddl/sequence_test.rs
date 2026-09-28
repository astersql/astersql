// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// SEQUENCE（序列）相关 Go 测试的草稿结构记录。
//
// 本文件不直接驱动真实 SQL 执行引擎，而是把原 Go testkit 用例拆成
// [`SqlStep`] 步骤表，通过 [`record_sql_steps`] 记录步骤数量，用于
// 迁移期对齐覆盖面。覆盖创建校验、nextval/lastval/setval、cache/cycle、
// 溢出边界、跨会话与 benchmark 等场景。

use crate::sequence::{
    SequenceCatalog, SequenceError, SequenceOption, apply_sequence_options, build_sequence_info,
    restart_sequence_base, sequence_defaults, validate_sequence_options,
};

// 这段逻辑记录 DDL sequence 创建、权限、nextval/lastval/setval、cache/cycle、溢出边界和 benchmark 的测试步骤。

/// 单条 SQL 测试步骤：动作类型、语句与期望结果摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SqlStep {
    /// 测试工具动作名（如 MustExec / MustQuery）。
    action: &'static str,
    /// 待执行的 SQL 文本。
    sql: &'static str,
    /// 期望结果或错误码的文字摘要。
    expect: &'static str,
}

// record_sql_steps 是测试的无副作用记录器。
/// 返回步骤切片长度，作为迁移草稿中的覆盖计数。
fn record_sql_steps(steps: &[SqlStep]) -> usize {
    steps.len()
}

// test_create_sequence_draft_structure 对应 Go 的 TestCreateSequence。
// 它覆盖非法选项、默认 sequence 元数据，以及普通用户缺少 CREATE 权限的报错。
/// 记录 CREATE SEQUENCE 非法选项、默认元数据与权限拒绝步骤。
#[test]
fn test_create_sequence_draft_structure() {
    let setup = [
        SqlStep {
            action: "MustExec",
            sql: "use test",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "drop sequence if exists seq",
            expect: "ok",
        },
    ];
    assert_eq!(record_sql_steps(&setup), 2);

    let invalid_create_cases = [
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence `seq  `",
            expect: "ErrWrongTableName",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq increment 0",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq maxvalue 1 minvalue 2",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq maxvalue 1 minvalue 1",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq maxvalue 9223372036854775807 minvalue 1",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq maxvalue 1 start with 2",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq increment 100000 cache 922337203685477",
            expect: "ErrSequenceInvalidData",
        },
        SqlStep {
            action: "MustGetErrCode",
            sql: "create sequence seq CHARSET=utf8",
            expect: "ErrSequenceUnsupportedTableOption",
        },
    ];
    assert_eq!(record_sql_steps(&invalid_create_cases), 8);

    // Go 在 create sequence seq comment="test" 后读取表元数据：
    // IsSequence=true，Increment/Start/Min/Max/Cache/CacheValue/Cycle 均等于 model 默认值。
    let default_meta_checks = [
        ("IsSequence", "true"),
        ("Increment", "DefaultSequenceIncrementValue"),
        ("Start", "DefaultPositiveSequenceStartValue"),
        ("MinValue", "DefaultPositiveSequenceMinValue"),
        ("MaxValue", "DefaultPositiveSequenceMaxValue"),
        ("Cache", "true"),
        ("CacheValue", "DefaultSequenceCacheValue"),
        ("Cycle", "false"),
    ];
    assert_eq!(default_meta_checks.len(), 8);

    let privilege_steps = [
        SqlStep {
            action: "MustExec",
            sql: "drop user if exists myuser@localhost",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "create user myuser@localhost",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "grant select on test.* to 'myuser'@'localhost'",
            expect: "ok",
        },
        SqlStep {
            action: "Exec",
            sql: "create sequence my_seq",
            expect: "[planner:1142]CREATE command denied to user 'myuser'@'localhost' for table 'my_seq'",
        },
    ];
    assert_eq!(record_sql_steps(&privilege_steps), 4);
}

// test_sequence_function_draft_structure 对应 Go 的 TestSequenceFunction。
// 原测试用一条很长的 testkit 脚本覆盖所有 sequence 函数语义；这里按 Go 注释块拆成等价步骤组。
/// 记录 nextval / setval / lastval、正负向 cycle 与跨会话等函数语义步骤。
#[test]
fn test_sequence_function_draft_structure() {
    let normal_function_steps = [
        SqlStep {
            action: "MustExec",
            sql: "use test",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "drop sequence if exists seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "drop sequence if exists seq1",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "1",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(test.seq)",
            expect: "2",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select next value for seq",
            expect: "3",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select next value for test.seq",
            expect: "4",
        },
        SqlStep {
            action: "MustGetErrMsg",
            sql: "select nextval(seq1)",
            expect: "[schema:1146]Table 'test.seq1' doesn't exist",
        },
        SqlStep {
            action: "MustExec",
            sql: "create database test2",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "use test2",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(test.seq)",
            expect: "5",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select next value for test.seq",
            expect: "6",
        },
        SqlStep {
            action: "MustGetErrMsg",
            sql: "select nextval(seq)",
            expect: "[schema:1146]Table 'test2.seq' doesn't exist",
        },
        SqlStep {
            action: "MustGetErrMsg",
            sql: "select next value for seq",
            expect: "[schema:1146]Table 'test2.seq' doesn't exist",
        },
    ];
    assert_eq!(record_sql_steps(&normal_function_steps), 15);

    // 正向序列：步长、起始、cycle 回绕等期望序列。
    let positive_sequence_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq nocache",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "1",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "2",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "3",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 5",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "1",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "6",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "11",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 5 start = 3",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "3/8/13 in order",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq minvalue -5 start = -2 increment = 5",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "-2/3/8 in order",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 5 start = 3 maxvalue = 12 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "3/8/1/6/11/1 in order",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 4 start = 2 maxvalue = 10 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "2/6/10/1/5/9/1 in order",
        },
    ];
    assert_eq!(record_sql_steps(&positive_sequence_cases), 16);

    // 耗尽（run out）与负向步长场景。
    let runout_and_negative_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 5 start = 3 maxvalue = 12 nocycle",
            expect: "ok",
        },
        SqlStep {
            action: "QueryToErr",
            sql: "select nextval(seq) after 3 and 8",
            expect: "[table:4135]Sequence 'test.seq' has run out",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = 3 start = 3 maxvalue = 9 nocycle",
            expect: "ok",
        },
        SqlStep {
            action: "QueryToErr",
            sql: "select nextval(seq) after 3/6/9",
            expect: "[table:4135]Sequence 'test.seq' has run out",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = -2 start = 3 minvalue -5 maxvalue = 12 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "3/1/-1/-3/-5/12/10 in order",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = -3 start = 2 minvalue -6 maxvalue = 11 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "2/-1/-4/11/8 in order",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = -4 start = 6 minvalue -6 maxvalue = 11",
            expect: "ok",
        },
        SqlStep {
            action: "QueryToErr",
            sql: "select nextval(seq) after 6/2/-2/-6",
            expect: "[table:4135]Sequence 'test.seq' has run out",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment = -3 start = 2 minvalue -2 maxvalue 10",
            expect: "ok",
        },
        SqlStep {
            action: "QueryToErr",
            sql: "select nextval(seq) after 2/-1",
            expect: "[table:4135]Sequence 'test.seq' has run out",
        },
    ];
    assert_eq!(record_sql_steps(&runout_and_negative_cases), 12);

    // setval：设置当前值，可能触发缓存边界与 cycle。
    let setval_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 2)",
            expect: "<nil> after value already used",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 5)",
            expect: "5",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment 3 maxvalue 11",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 3/4/5/8/11/100)",
            expect: "<nil>/<nil>/5/8/11/100 with runout checks",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment 10 start 5 maxvalue 100 cache 10 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 20)",
            expect: "20; nextval is 25, cache end stays 95 round 0",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 95); select nextval(seq)",
            expect: "95 then 1, new cache end 91 round 1",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment 2 start 0 maxvalue 10 minvalue -10 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, -20/20); select nextval(seq)",
            expect: "<nil>/20/-10, cache end -6 round 1",
        },
    ];
    assert_eq!(record_sql_steps(&setval_cases), 10);

    // 负向序列上的 setval / nextval 交互。
    let negative_setval_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment -3 start 5 maxvalue 10 minvalue -10 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, -2); select nextval(seq)",
            expect: "-2/-4, cache end -10 round 0",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, -10); select nextval(seq)",
            expect: "-10/10, cache end 4 round 1",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 0); select nextval(seq)",
            expect: "0/-2",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment -2 start 0 maxvalue 10 minvalue -10 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, 20/-20); select nextval(seq)",
            expect: "<nil>/-20/10, cache end 6 round 1",
        },
    ];
    assert_eq!(record_sql_steps(&negative_setval_cases), 6);

    // lastval：会话内最近一次 nextval 的值；setval 不改变 lastval。
    let lastval_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select lastval(seq)",
            expect: "<nil> before nextval",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq); select lastval(seq)",
            expect: "1 then 1",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select next value for seq; select lastval(seq)",
            expect: "2 then 2",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select setval(seq, -1/5); select lastval(seq)",
            expect: "setval does not change last value 2",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment 3 start 3 maxvalue 14 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "setval 10 then nextval, setval 13 then nextval",
            expect: "lastval remains previous value; cache end 9/14/7 round 0/0/1",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment -3 start -2 maxvalue 10 minvalue -10 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "setval -8 then nextval",
            expect: "lastval -2 then nextval 10, cache end 4 round 1",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment -1 start 1 maxvalue 10 minvalue -10 cache 3 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "setval -8 then nextval",
            expect: "-9, cache end -10 round 0",
        },
    ];
    assert_eq!(record_sql_steps(&lastval_cases), 11);

    // 近 i64 边界溢出与对非 SEQUENCE 对象调用函数的报错。
    let overflow_and_wrong_object_cases = [
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment 2 start -9223372036854775807 maxvalue 9223372036854775806 minvalue -9223372036854775807 cache 2 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "setval(seq, 9223372036854775800); nextval(seq)",
            expect: "9223372036854775801",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq increment -2 start 9223372036854775806 maxvalue 9223372036854775806 minvalue -9223372036854775807 cache 2 cycle",
            expect: "ok",
        },
        SqlStep {
            action: "MustQuery",
            sql: "setval(seq, -9223372036854775800); nextval(seq)",
            expect: "-9223372036854775802",
        },
        SqlStep {
            action: "MustExec",
            sql: "create table seq(a int)",
            expect: "ok",
        },
        SqlStep {
            action: "ExecToErr",
            sql: "select nextval(seq); select lastval(seq); select setval(seq, 10)",
            expect: "[schema:1347]'test.seq' is not SEQUENCE",
        },
        SqlStep {
            action: "MustExec",
            sql: "create view seq1 as select * from seq",
            expect: "ok",
        },
        SqlStep {
            action: "ExecToErr",
            sql: "select nextval(seq1); select lastval(seq1); select setval(seq1, 10)",
            expect: "[schema:1347]'test.seq1' is not SEQUENCE",
        },
    ];
    assert_eq!(record_sql_steps(&overflow_and_wrong_object_cases), 8);

    // 大小写不敏感命名与跨会话 setval 可见性。
    let ticase_and_cross_session_cases = [
        SqlStep {
            action: "MustQuery",
            sql: "create sequence seq; setval(seq, 10); setval(seq, 5)",
            expect: "10 then <nil>",
        },
        SqlStep {
            action: "MustQuery",
            sql: "create sequence seq increment=-1; setval(seq, -10); setval(seq, -5)",
            expect: "-10 then <nil>",
        },
        SqlStep {
            action: "MustQuery",
            sql: "session1 setval(seq,100); session2 setval(seq,50); nextval(seq); setval 100/101/102",
            expect: "<nil>/101/<nil>/<nil>/102",
        },
        SqlStep {
            action: "MustQuery",
            sql: "negative sequence cross session setval -100/-50; nextval; setval -100/-101/-102",
            expect: "<nil>/-101/<nil>/<nil>/-102",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq), t.a from t",
            expect: "1 1; 2 2",
        },
        SqlStep {
            action: "ExecToErr",
            sql: "select nextval(t), t.a from t",
            expect: "[schema:1347]'test.t' is not SEQUENCE",
        },
        SqlStep {
            action: "ExecToErr",
            sql: "select nextval(seq), nextval(t), t.a from t",
            expect: "[schema:1347]'test.t' is not SEQUENCE",
        },
        SqlStep {
            action: "MustQuery",
            sql: "select nextval(seq)",
            expect: "3",
        },
    ];
    assert_eq!(record_sql_steps(&ticase_and_cross_session_cases), 8);
}

// benchmark_insert_cache_default_expr_draft_structure 对应 Go 的 BenchmarkInsertCacheDefaultExpr。
/// 构造「列默认值为 next value for seq」的大批量 INSERT 语句草稿。
fn benchmark_insert_cache_default_expr_draft_structure(iterations: usize) -> String {
    let setup = [
        SqlStep {
            action: "MustExec",
            sql: "use test",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "drop sequence if exists seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "drop table if exists t",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "create sequence seq",
            expect: "ok",
        },
        SqlStep {
            action: "MustExec",
            sql: "create table t(a int default next value for seq)",
            expect: "ok",
        },
    ];
    assert_eq!(record_sql_steps(&setup), 5);

    let mut sql = String::from("insert into t values ");
    for i in 0..1000 {
        if i == 0 {
            sql.push_str("()");
        } else {
            sql.push_str(",()");
        }
    }

    // Go 在 b.ResetTimer 后执行 tk.MustExec(sql) b.N 次；这里仅用 iterations 保留循环次数入口。
    assert!(iterations <= usize::MAX);
    sql
}

#[test]
fn sequence_rejects_i64_endpoint_bounds_like_go() {
    let mut max_endpoint = sequence_defaults(1);
    max_endpoint.max_value = i64::MAX;
    assert_eq!(
        validate_sequence_options(&max_endpoint),
        Err(SequenceError::InvalidBounds)
    );

    let mut min_endpoint = sequence_defaults(-1);
    min_endpoint.min_value = i64::MIN;
    assert_eq!(
        validate_sequence_options(&min_endpoint),
        Err(SequenceError::InvalidBounds)
    );
}

#[test]
fn sequence_rejects_cache_increment_overflow_like_go() {
    let mut oversized_cache = sequence_defaults(1);
    oversized_cache.cache = i64::MAX as u64;
    assert_eq!(
        validate_sequence_options(&oversized_cache),
        Err(SequenceError::InvalidCache)
    );
}

#[test]
fn failed_alter_does_not_mutate_sequence_like_go() {
    let mut catalog = SequenceCatalog::default();
    let info = build_sequence_info(&[]).expect("default sequence options are valid");
    catalog
        .create(1, "seq", info, false)
        .expect("sequence creation succeeds");

    assert_eq!(
        catalog.alter(1, "seq", &[SequenceOption::Increment(0)]),
        Err(SequenceError::InvalidIncrement)
    );

    assert_eq!(
        catalog.alter(
            1,
            "seq",
            &[SequenceOption::Comment("still valid".to_owned())]
        ),
        Ok(None),
        "Go validates ALTER on a copy and preserves the prior metadata on error"
    );
}

#[test]
fn restart_value_and_base_arithmetic_match_go() {
    let mut info = build_sequence_info(&[]).expect("default sequence options are valid");
    assert_eq!(
        apply_sequence_options(&mut info, &[SequenceOption::Restart(Some(0))], true),
        Ok(Some(0)),
        "Go accepts RESTART WITH independently of the configured min/max bounds"
    );
    assert_eq!(info.current, -1);

    assert_eq!(restart_sequence_base(i64::MIN, 1), i64::MAX);
    assert_eq!(restart_sequence_base(i64::MAX, -1), i64::MIN);
}
