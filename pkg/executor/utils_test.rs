// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 执行器通用工具（集合字符串、批量取回）的单元测试。

use std::sync::{Arc, mpsc};
use std::time::Duration;

use astersql_extension::AuthPlugin;
use astersql_parser_ast::{AuthOption, UserSpec};
use astersql_parser_mysql::r#const::{
    AuthCachingSha2Password, AuthLDAPSimple, AuthNativePassword, AuthSocket, PWDHashLen,
    SHAPWDHashLen,
};
use astersql_parser_mysql::r#type::TypeLonglong;
use astersql_types::datum::FieldType;

use crate::utils::{
    SetFromString, addToSet, batchRetrieverHelper, deleteFromSet, encodePasswordWithPlugin,
    encodedPassword, estimateDMLChildChunkInitCap, setToString, workerPool,
};

#[test]
/// 验证集合保序增删，以及 batchRetrieverHelper 分批与出错后停止。
fn executor_set_and_batch_helpers_preserve_order_and_stop_after_error() {
    let mut set = SetFromString("a,b").unwrap();
    set = addToSet(set, "b".into());
    set = addToSet(set, "c".into());
    set = deleteFromSet(set, "a");
    assert_eq!(setToString(&set), "b,c");
    assert!(SetFromString("").is_none());

    let mut helper = batchRetrieverHelper {
        retrieved: false,
        retrieved_idx: 0,
        batch_size: 2,
        total_rows: 5,
    };
    let mut ranges = Vec::new();
    while !helper.retrieved {
        helper
            .nextBatch::<()>(|start, end| {
                ranges.push((start, end));
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 5)]);

    let mut failing = batchRetrieverHelper {
        retrieved: false,
        retrieved_idx: 0,
        batch_size: 2,
        total_rows: 3,
    };
    assert_eq!(failing.nextBatch(|_, _| Err::<(), _>("boom")), Err("boom"));
    assert!(failing.retrieved);
}

#[test]
fn batch_retriever_matches_go_empty_exact_and_oversized_batches() {
    for (batch_size, total_rows, expected) in [
        (0, 0, vec![]),
        (3, 9, vec![(0, 3), (3, 6), (6, 9)]),
        (3, 10, vec![(0, 3), (3, 6), (6, 9), (9, 10)]),
        (100, 10, vec![(0, 10)]),
    ] {
        let mut helper = batchRetrieverHelper {
            retrieved: false,
            retrieved_idx: 0,
            batch_size,
            total_rows,
        };
        let mut actual = Vec::new();
        while !helper.retrieved {
            helper
                .nextBatch::<()>(|start, end| {
                    actual.push((start, end));
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(actual, expected);
    }

    let mut already_done = batchRetrieverHelper {
        retrieved: true,
        retrieved_idx: 0,
        batch_size: 3,
        total_rows: 10,
    };
    already_done
        .nextBatch::<()>(|_, _| panic!("completed retriever must not invoke callback"))
        .unwrap();
}

#[test]
fn dml_child_chunk_capacity_matches_go_bounds_and_width_estimate() {
    assert_eq!(estimateDMLChildChunkInitCap(&[], 0, 10), 0);
    assert_eq!(estimateDMLChildChunkInitCap(&[], 10, 0), 0);
    assert_eq!(estimateDMLChildChunkInitCap(&[], 10, 7), 7);

    let fields = (0..4)
        .map(|_| {
            let mut field = FieldType::default();
            field.SetType(TypeLonglong);
            field
        })
        .collect::<Vec<_>>();
    assert_eq!(estimateDMLChildChunkInitCap(&fields, 10_000, 9_000), 8192);
    assert_eq!(estimateDMLChildChunkInitCap(&fields, 1024, 9_000), 1024);
    assert_eq!(estimateDMLChildChunkInitCap(&fields, 10_000, 512), 512);
}

fn user_with_auth(option: Option<AuthOption>) -> UserSpec {
    UserSpec {
        AuthOpt: option,
        ..UserSpec::default()
    }
}

#[test]
fn extension_password_plugin_generates_validates_and_handles_missing_auth() {
    let plugin = AuthPlugin {
        GenerateAuthString: Some(Arc::new(|input| {
            (format!("generated:{input}"), input == "plain")
        })),
        ValidateAuthString: Some(Arc::new(|input| input == "valid-hash")),
        ..AuthPlugin::default()
    };

    let plain = user_with_auth(Some(AuthOption {
        AuthString: "plain".into(),
        ByAuthString: true,
        ..AuthOption::default()
    }));
    assert_eq!(
        encodePasswordWithPlugin(&plain, Some(&plugin), ""),
        ("generated:plain".into(), true)
    );

    for (hash, expected) in [
        ("valid-hash", ("valid-hash".to_owned(), true)),
        ("invalid-hash", (String::new(), false)),
    ] {
        let user = user_with_auth(Some(AuthOption {
            HashString: hash.into(),
            ..AuthOption::default()
        }));
        assert_eq!(encodePasswordWithPlugin(&user, Some(&plugin), ""), expected);
    }
    assert_eq!(
        encodePasswordWithPlugin(&UserSpec::default(), Some(&plugin), ""),
        (String::new(), true)
    );
}

#[test]
fn builtin_password_encoding_matches_go_plugin_rules() {
    let native_hash = "*3D56A309CD04FA2EEF181462E59011F075C89548";
    assert_eq!(native_hash.len(), PWDHashLen + 1);

    let native_plain = user_with_auth(Some(AuthOption {
        AuthString: "xxx".into(),
        ByAuthString: true,
        ..AuthOption::default()
    }));
    assert_eq!(
        encodedPassword(&native_plain, AuthNativePassword),
        (native_hash.into(), true)
    );

    let caching_plain = user_with_auth(Some(AuthOption {
        AuthString: "xxx".into(),
        ByAuthString: true,
        ..AuthOption::default()
    }));
    let (caching_hash, valid) = encodedPassword(&caching_plain, AuthCachingSha2Password);
    assert!(valid);
    assert_eq!(caching_hash.len(), SHAPWDHashLen);

    let socket_plain = user_with_auth(Some(AuthOption {
        AuthPlugin: AuthSocket.into(),
        AuthString: "ignored".into(),
        ByAuthString: true,
        ..AuthOption::default()
    }));
    assert_eq!(encodedPassword(&socket_plain, ""), (String::new(), true));

    for (plugin, hash, expected) in [
        (
            AuthNativePassword,
            native_hash,
            (native_hash.to_owned(), true),
        ),
        (AuthNativePassword, "invalid", (String::new(), false)),
        (AuthLDAPSimple, "cn=user", ("cn=user".to_owned(), true)),
        (AuthSocket, "ignored", ("ignored".to_owned(), true)),
        ("unknown_plugin", "value", (String::new(), false)),
    ] {
        let user = user_with_auth(Some(AuthOption {
            AuthPlugin: plugin.into(),
            HashString: hash.into(),
            ..AuthOption::default()
        }));
        assert_eq!(encodedPassword(&user, ""), expected);
    }

    let empty = user_with_auth(Some(AuthOption {
        AuthPlugin: "unknown_plugin".into(),
        ..AuthOption::default()
    }));
    assert_eq!(encodedPassword(&empty, ""), (String::new(), true));
    assert_eq!(
        encodedPassword(&UserSpec::default(), ""),
        (String::new(), true)
    );
}

#[test]
fn worker_pool_honors_single_worker_spawn_policy() {
    let pool = Arc::new(workerPool::new(Some(Arc::new(|workers, tasks| {
        workers < 1 && tasks > 0
    }))));
    let (events_tx, events_rx) = mpsc::channel();
    let nested_pool = Arc::clone(&pool);
    pool.submit(move || {
        events_tx.send(1).unwrap();
        let nested_tx = events_tx.clone();
        nested_pool.submit(move || nested_tx.send(3).unwrap());
        events_tx.send(2).unwrap();
    });

    let events = (0..3)
        .map(|_| events_rx.recv_timeout(Duration::from_secs(2)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events, vec![1, 2, 3]);
}

#[test]
fn worker_pool_can_spawn_a_second_worker_for_pending_work() {
    let pool = Arc::new(workerPool::new(Some(Arc::new(|workers, tasks| {
        workers < 2 && tasks > 0
    }))));
    let (events_tx, events_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let nested_pool = Arc::clone(&pool);
    pool.submit(move || {
        events_tx.send(1).unwrap();
        let nested_tx = events_tx.clone();
        nested_pool.submit(move || nested_tx.send(2).unwrap());
        release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        events_tx.send(3).unwrap();
    });

    assert_eq!(events_rx.recv_timeout(Duration::from_secs(2)), Ok(1));
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(2)), Ok(2));
    release_tx.send(()).unwrap();
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(2)), Ok(3));
}
