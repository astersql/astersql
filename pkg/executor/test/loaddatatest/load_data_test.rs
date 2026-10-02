// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Rust counterparts of the scenarios in `load_data_test.go`.
//!
//! 本模块以轻量模型复现 Go 版 LOAD DATA 回归场景，验证格式选项、字段解码、
//! 列映射、重复键处理以及事务生命周期等可观察语义，不依赖完整 SQL 执行器。

use crate::{
    LoadDataConfig, LoadDataError, LoadDataEvent, LoadDataLifecycle, apply_replace, map_columns,
    parse_load_data, parse_unsigned_bigint, validate_load_data_options,
};

#[test]
fn test_load_data_init_param() {
    // 按执行器的校验顺序覆盖互斥格式选项；合法选项最终仍因测试未安装 reader 而失败。
    let defaults = LoadDataConfig::default();
    assert_eq!(defaults.null_def, vec![r"\N"]);
    assert_eq!(
        validate_load_data_options("", true, None, &defaults),
        Err(LoadDataError::EmptyPath)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                null_value_opt_enclosed: true,
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::MustSpecifyEnclosed)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                lines_terminated_by: String::new(),
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::EmptyLineTerminator)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            None,
            &LoadDataConfig {
                fields_terminated_by: "a".into(),
                lines_terminated_by: "a".into(),
                ..defaults.clone()
            }
        ),
        Err(LoadDataError::OverlappingTerminators)
    );
    assert_eq!(
        validate_load_data_options("/a", true, None, &defaults),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        validate_load_data_options("/a", true, Some("sql file"), &defaults),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        validate_load_data_options(
            "/a",
            true,
            Some("delimited data"),
            &defaults.clone().terminated_by("a"),
        ),
        Err(LoadDataError::ReaderNil)
    );
    assert_eq!(
        defaults.clone().null_by(["a", r"\N"]).null_def,
        vec!["a", r"\N"]
    );
}

#[test]
fn test_load_data() {
    // 同时覆盖默认 TSV 方言与自定义行前缀、多字节行终止符组合。
    let config = LoadDataConfig::default();
    let rows = parse_load_data(b"1\thello\n2\tworld\n", &config).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("hello".into())],
            vec![Some("2".into()), Some("world".into())]
        ]
    );

    let custom = config.clone().terminated_by("\\").lines("xxx", "|!#^");
    let rows = parse_load_data(b"xxx3\\2\\3\\4|!#^xxx4\\2\\3\\4|!#^", &custom).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![
                Some("3".into()),
                Some("2".into()),
                Some("3".into()),
                Some("4".into())
            ],
            vec![
                Some("4".into()),
                Some("2".into()),
                Some("3".into()),
                Some("4".into())
            ]
        ]
    );

    // Go scans forward to LINES STARTING BY within a physical record instead of
    // discarding the entire record when the prefix is not at byte zero.
    let rows = parse_load_data(b"10\\2\\3xxx11\\4\\5|!#^", &custom).unwrap();
    assert_eq!(
        rows,
        vec![vec![Some("11".into()), Some("4".into()), Some("5".into())]]
    );

    let mut lifecycle = LoadDataLifecycle::default();
    lifecycle.run(false);
    // LOAD DATA 必须先开启事务再提交，并在结束时关闭输入 reader。
    assert!(lifecycle.begin_before_commit());
    assert_eq!(lifecycle.events.last(), Some(&LoadDataEvent::CloseReader));
}

#[test]
fn test_load_data_escape() {
    // 反斜杠转义需兼容控制字符；嵌入普通文本的未知转义（如 \N）只去掉反斜杠。
    let rows = parse_load_data(
        b"1\ta string\n2\tstr \\t\n3\tstr \\n\n4\tboth \\t\\n\n5\tstr \\\\\n6\t\\r\\t\\n\\0\\Z\\b\n7\trtn0ZbN\n8\trtn0Zb\\N\n9\ttab\\ttab\n",
        &LoadDataConfig::default(),
    )
    .unwrap();
    assert_eq!(rows[0], vec![Some("1".into()), Some("a string".into())]);
    assert_eq!(rows[1], vec![Some("2".into()), Some("str \t".into())]);
    assert_eq!(rows[2], vec![Some("3".into()), Some("str \n".into())]);
    assert_eq!(rows[3], vec![Some("4".into()), Some("both \t\n".into())]);
    assert_eq!(rows[4], vec![Some("5".into()), Some("str \\".into())]);
    assert_eq!(rows[5][1], Some("\r\t\n\0\u{1a}\u{8}".into()));
    assert_eq!(rows[6][1], Some("rtn0ZbN".into()));
    assert_eq!(rows[7][1], Some("rtn0ZbN".into()));
    assert_eq!(rows[8][1], Some("tab\ttab".into()));
}

#[test]
fn test_load_data_specified_columns() {
    // 输入列只写入显式目标位置，未映射列保持空值，\N 仍映射为 SQL NULL。
    let rows =
        parse_load_data(b"7\ta string\n\\N\ta string\n", &LoadDataConfig::default()).unwrap();
    let mapped = map_columns(&rows, &[1, 2], None, false).unwrap();
    assert_eq!(
        mapped[0],
        vec![None, Some("7".into()), Some("a string".into())]
    );
    assert_eq!(mapped[1], vec![None, None, Some("a string".into())]);
}

#[test]
fn test_load_data_ignore_lines() {
    // IGNORE LINES 按解析完成的记录计数，被跳过的记录不应影响后续记录边界。
    let config = LoadDataConfig {
        ignore_lines: 1,
        ..LoadDataConfig::default()
    };
    assert_eq!(
        parse_load_data(b"1\tline1\n2\tline2\n3\tline3\n", &config).unwrap(),
        vec![
            vec![Some("2".into()), Some("line2".into())],
            vec![Some("3".into()), Some("line3".into())]
        ]
    );
}

#[test]
fn test_load_data_null() {
    // 是否被引号包围会改变 NULL 判定：裸 NULL/\N 为空，引号内 NULL 保留为文本。
    let config = LoadDataConfig::default()
        .terminated_by(",")
        .enclosed(b'"', false)
        .lines("", "\n");
    let rows = parse_load_data(b"NULL,\"NULL\"\n\\N,\"\\N\"\n\"\\\\N\"", &config).unwrap();
    assert_eq!(
        rows,
        vec![
            vec![None, Some("NULL".into())],
            vec![None, None],
            vec![Some("\\N".into())]
        ]
    );
}

#[test]
fn test_load_data_replace() {
    // REPLACE 以首列为键删除较早的重复行，保留输入中最后出现的值并报告删除数。
    let mut rows = vec![
        vec![Some("1".into()), Some("line1".into())],
        vec![Some("2".into()), Some("line2".into())],
        vec![Some("2".into()), Some("new line2".into())],
        vec![Some("3".into()), Some("new line3".into())],
    ];
    assert_eq!(apply_replace(&mut rows, 0, true), 1);
    assert_eq!(
        rows,
        vec![
            vec![Some("1".into()), Some("line1".into())],
            vec![Some("2".into()), Some("new line2".into())],
            vec![Some("3".into()), Some("new line3".into())]
        ]
    );
}

#[test]
fn test_load_data_overflow_bigint_unsigned() {
    // 无符号 BIGINT 转换对负数钳制为 0，对正向溢出钳制为 u64 上界，并产生警告标记。
    let values = ["-1", "-18446744073709551615", "-18446744073709551616"];
    for value in values {
        assert_eq!(parse_unsigned_bigint(value), (0, true));
    }
    assert_eq!(parse_unsigned_bigint("-9223372036854775809"), (0, true));
    assert_eq!(
        parse_unsigned_bigint("18446744073709551616"),
        (u64::MAX, true)
    );
}

#[test]
fn test_load_data_with_uppercase_user_vars() {
    // 用户变量名大小写不影响 SET 表达式；这里用倍率模型验证逐行求值结果。
    let rows = parse_load_data(b"1\n2\n", &LoadDataConfig::default()).unwrap();
    let mapped = map_columns(&rows, &[0], Some(100), false).unwrap();
    assert_eq!(
        mapped,
        vec![vec![Some("100".into())], vec![Some("200".into())]]
    );
}

#[test]
fn test_load_data_into_partitioned_table() {
    // 模拟 RANGE 分区上界路由，边界值 7 应进入下一个“小于 11”的分区。
    let config = LoadDataConfig::default().terminated_by(",");
    let rows = parse_load_data(b"1,2\n3,4\n5,6\n7,8\n9,10\n", &config).unwrap();
    let partitions: Vec<usize> = rows
        .iter()
        .map(|row| {
            let value = row[0].as_ref().unwrap().parse::<i64>().unwrap();
            [4, 7, 11].iter().position(|limit| value < *limit).unwrap()
        })
        .collect();
    assert_eq!(partitions, vec![0, 0, 1, 2, 2]);
}

pub(crate) fn server_file_uses_shared_store(session: &astersql_session::runtime::ConcreteSession) {
    session
        .execute("use test; drop table if exists load_data_test")
        .unwrap();
    session
        .execute("create table load_data_test (a int)")
        .unwrap();
    let error = match session.execute("load data infile 'remote.csv' into table load_data_test") {
        Ok(_) => panic!("server disk must be rejected"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("ERROR 8154 (HY000)"), "{error}");
    assert!(
        error
            .to_string()
            .contains("Don't support load data from tidb-server's disk.")
    );
}

fn query_rows(session: &astersql_session::runtime::ConcreteSession, sql: &str) -> Vec<String> {
    let mut sets = session.execute(sql).unwrap();
    let mut set = sets.pop().expect("query result");
    let mut result = Vec::new();
    while let Some(row) = set.next_row().unwrap() {
        result.push(row.join("|"));
    }
    set.close().unwrap();
    result
}

fn load_fixture(
    session: &astersql_session::runtime::ConcreteSession,
    table: &str,
    bytes: &[u8],
    fields: &str,
) {
    let path = std::env::temp_dir().join(format!(
        "astersql_shared_load_data_{}_{}.csv",
        std::process::id(),
        table
    ));
    std::fs::write(&path, bytes).unwrap();
    let result = session.execute(&format!(
        "load data local infile '{}' replace into table {table} {fields}",
        path.display()
    ));
    std::fs::remove_file(path).unwrap();
    result.unwrap();
}

pub(crate) fn replace_uses_shared_store(session: &astersql_session::runtime::ConcreteSession) {
    session
        .execute("use test; drop table if exists load_data_replace")
        .unwrap();
    session
        .execute(
            "create table load_data_replace (id int not null primary key, value text not null)",
        )
        .unwrap();
    session
        .execute("insert into load_data_replace values (1, 'val 1'), (2, 'val 2')")
        .unwrap();
    load_fixture(session, "load_data_replace", b"1\tline1\n2\tline2\n", "");
    assert_eq!(
        session.LastMessage(),
        "Records: 2  Deleted: 2  Skipped: 0  Warnings: 0"
    );
    assert_eq!(
        query_rows(session, "select * from load_data_replace order by id"),
        ["1|line1", "2|line2"]
    );
    load_fixture(
        session,
        "load_data_replace",
        b"2\tnew line2\n3\tnew line3\n",
        "",
    );
    assert_eq!(
        session.LastMessage(),
        "Records: 2  Deleted: 1  Skipped: 0  Warnings: 0"
    );
    assert_eq!(
        query_rows(session, "select * from load_data_replace order by id"),
        ["1|line1", "2|new line2", "3|new line3"]
    );
    // Clustered handles also keep identical rows and count them as skipped.
    load_fixture(
        session,
        "load_data_replace",
        b"2\tnew line2\n3\tnew line3\n",
        "",
    );
    assert_eq!(
        session.LastMessage(),
        "Records: 2  Deleted: 0  Skipped: 2  Warnings: 0"
    );
    assert_eq!(session.protocol_state().affected_rows, 2);
    assert_eq!(
        query_rows(session, "select * from load_data_replace order by id"),
        ["1|line1", "2|new line2", "3|new line3"]
    );
}

pub(crate) fn repeated_nonclustered_keys_use_shared_store(
    session: &astersql_session::runtime::ConcreteSession,
) {
    session.execute("use test; drop table if exists a").unwrap();
    session.execute("create table a(id int,name varchar(20),addr varchar(100),primary key (id) nonclustered)").unwrap();
    load_fixture(session, "a", b"1|aa|beijing\n1|aa|beijing\n1|aa|beijing\n1|aa|beijing\n2|bb|shanghai\n2|bb|shanghai\n2|bb|shanghai\n3|cc|guangzhou\n", "fields terminated by '|'");
    assert_eq!(
        session.LastMessage(),
        "Records: 8  Deleted: 0  Skipped: 5  Warnings: 0"
    );
    assert_eq!(
        query_rows(session, "select * from a order by id"),
        ["1|aa|beijing", "2|bb|shanghai", "3|cc|guangzhou"]
    );
    session.execute("admin check table a").unwrap();
}

#[test]
fn test_load_data_auto_random_error() {
    // LOAD DATA 显式写入 AUTO_RANDOM 列必须沿用执行器的拒绝契约。
    let rows = parse_load_data(b"1,2\n", &LoadDataConfig::default().terminated_by(",")).unwrap();
    assert_eq!(
        map_columns(&rows, &[0, 1], None, true),
        Err(LoadDataError::InvalidAutoRandom)
    );
}

#[derive(Clone, Default)]
pub(crate) struct PriorityChecker {
    enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<astersql_store_mockstore_unistore::CommandType>>>,
}

impl PriorityChecker {
    pub(crate) fn interceptor(&self) -> astersql_store_mockstore_unistore::rpc::RequestInterceptor {
        let checker = self.clone();
        std::sync::Arc::new(move |request| {
            use astersql_store_mockstore_unistore::{CommandType, RpcError};
            let Some(context) = request.context() else {
                return Ok(());
            };
            if context.request_marker != Some(42)
                || !checker.enabled.load(std::sync::atomic::Ordering::Acquire)
            {
                return Ok(());
            }
            let kind = request.command_type();
            if matches!(
                kind,
                CommandType::BatchGet
                    | CommandType::Get
                    | CommandType::Scan
                    | CommandType::Prewrite
                    | CommandType::Commit
                    | CommandType::Cleanup
                    | CommandType::BatchRollback
            ) {
                if context.priority != astersql_kv::PriorityLow {
                    return Err(RpcError::Server("unexpected kv request priority".into()));
                }
                checker.seen.lock().unwrap().push(kind);
            }
            Ok(())
        })
    }
}

pub(crate) fn low_priority_uses_shared_store(
    session: &astersql_session::runtime::ConcreteSession,
    client: &std::sync::Arc<astersql_store_mockstore_unistore::RPCClient>,
    checker: &PriorityChecker,
) {
    use astersql_store_mockstore_unistore::CommandType;
    session
        .execute("use test; drop table if exists load_data_low_prio")
        .unwrap();
    session
        .execute("create table load_data_low_prio (a int primary key, b int unique)")
        .unwrap();
    checker.seen.lock().unwrap().clear();
    struct Reader {
        input: std::io::Cursor<Vec<u8>>,
        closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl std::io::Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            std::io::Read::read(&mut self.input, bytes)
        }
    }
    impl Drop for Reader {
        fn drop(&mut self) {
            self.closed
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = Reader {
        input: std::io::Cursor::new(b"1\t10\n".to_vec()),
        closed: closed.clone(),
    };
    checker
        .enabled
        .store(true, std::sync::atomic::Ordering::Release);
    let result = client.with_request_marker(42, || session.execute_with_load_data_reader("load data low_priority local infile '/tmp/nonexistence.csv' into table load_data_low_prio", reader));
    checker
        .enabled
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(
        closed.load(std::sync::atomic::Ordering::Acquire),
        "reader must close on success or error"
    );
    result.unwrap();
    let seen = checker.seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|kind| matches!(
            kind,
            CommandType::Get | CommandType::BatchGet | CommandType::Scan
        )),
        "must observe actual conflict reads: {seen:?}"
    );
    assert!(
        seen.contains(&CommandType::Prewrite),
        "must observe actual prewrite: {seen:?}"
    );
    assert!(
        seen.contains(&CommandType::Commit),
        "must observe actual commit: {seen:?}"
    );
    assert_eq!(
        query_rows(session, "select * from load_data_low_prio"),
        ["1|10"]
    );
    assert_eq!(client.request_marker(), None);
    let checked_count = checker.seen.lock().unwrap().len();
    checker
        .enabled
        .store(true, std::sync::atomic::Ordering::Release);
    // Unmarked and differently marked normal-priority SQL must be forwarded.
    session
        .execute("insert into load_data_low_prio values (2,20)")
        .unwrap();
    client
        .with_request_marker(7, || {
            session.execute("insert into load_data_low_prio values (3,30)")
        })
        .unwrap();
    assert_eq!(checker.seen.lock().unwrap().len(), checked_count);
    let rejected_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = Reader {
        input: std::io::Cursor::new(b"4\t40\n".to_vec()),
        closed: rejected_closed.clone(),
    };
    let rejected = client.with_request_marker(42, || {
        session.execute_with_load_data_reader(
            "load data local infile '/tmp/nonexistence.csv' into table load_data_low_prio",
            reader,
        )
    });
    checker
        .enabled
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(
        rejected
            .err()
            .unwrap()
            .to_string()
            .contains("unexpected kv request priority")
    );
    assert!(rejected_closed.load(std::sync::atomic::Ordering::Acquire));
    client
        .with_request_marker(42, || {
            session.execute("insert into load_data_low_prio values (5,50)")
        })
        .unwrap();
    assert_eq!(
        query_rows(session, "select * from load_data_low_prio order by a"),
        ["1|10", "2|20", "3|30", "5|50"]
    );
    assert_eq!(client.request_marker(), None);
}

#[test]
fn test_load_data_low_priority_sets_kv_low_priority() {
    crate::main_test::with_shared_load_data_store(|domain, client, checker| {
        low_priority_uses_shared_store(
            &astersql_session::runtime::ConcreteSession::new(domain.clone()),
            client,
            checker,
        );
    });
}

#[test]
fn shared_priority_checker_filters_markers_enabled_state_and_command_types() {
    use astersql_store_mockstore_unistore::tikv::{
        mvcc::PrewriteRequest, region::RequestContext, server::RpcContext,
    };
    use astersql_store_mockstore_unistore::{New, Request, RpcError};
    let (client, _, cluster) = New(
        "",
        Vec::new(),
        astersql_store_mockstore_unistore::NULL_KEYSPACE_ID,
        Vec::new(),
    )
    .unwrap();
    let region = cluster.region_manager().scan_regions(b"", b"", 1).remove(0);
    let address = cluster.region_manager().all_stores()[0].address.clone();
    let context = RpcContext {
        region: RequestContext {
            region_id: region.meta.id,
            ..Default::default()
        },
        priority: astersql_kv::PriorityNormal,
        request_marker: Some(42),
        ..Default::default()
    };
    let checker = PriorityChecker::default();
    client.set_request_interceptor(Some(checker.interceptor()));
    checker
        .enabled
        .store(true, std::sync::atomic::Ordering::Release);
    let keys = vec![b"load-data-key".to_vec()];
    let requests = vec![
        Request::BatchGet {
            context: context.clone(),
            keys: keys.clone(),
            version: 10,
        },
        Request::Get {
            context: context.clone(),
            key: keys[0].clone(),
            version: 10,
        },
        Request::Scan {
            context: context.clone(),
            start: keys[0].clone(),
            end: b"load-data-z".to_vec(),
            version: 10,
            limit: 10,
            reverse: false,
            key_only: false,
        },
        Request::Prewrite {
            context: context.clone(),
            request: PrewriteRequest {
                start_ts: 10,
                primary_lock: keys[0].clone(),
                ..Default::default()
            },
        },
        Request::Commit {
            context: context.clone(),
            keys: keys.clone(),
            start_ts: 10,
            commit_ts: 11,
        },
        Request::Cleanup {
            context: context.clone(),
            key: keys[0].clone(),
            start_ts: 10,
            current_ts: 11,
        },
        Request::BatchRollback {
            context: context.clone(),
            keys: keys.clone(),
            start_ts: 10,
        },
    ];
    for request in requests {
        let result = client.send_request(&address, request, std::time::Duration::from_secs(2));
        assert!(
            matches!(result, Err(RpcError::Server(ref message)) if message == "unexpected kv request priority")
        );
    }
    // Non-KV commands still reach the server even while enabled.
    assert!(
        client
            .send_request(
                &address,
                Request::DebugGetRegionProperties {
                    region_id: region.meta.id
                },
                std::time::Duration::from_secs(2)
            )
            .is_ok()
    );
    for marker in [None, Some(7)] {
        let request = Request::Get {
            context: RpcContext {
                request_marker: marker,
                ..context.clone()
            },
            key: keys[0].clone(),
            version: 10,
        };
        assert!(
            client
                .send_request(&address, request, std::time::Duration::from_secs(2))
                .is_ok()
        );
    }
    checker
        .enabled
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(
        client
            .send_request(
                &address,
                Request::Get {
                    context,
                    key: keys[0].clone(),
                    version: 10
                },
                std::time::Duration::from_secs(2)
            )
            .is_ok()
    );
    client.close().unwrap();
}
