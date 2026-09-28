// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// sqlexec mock 迁移单元测试：键身份、期望转发、FIFO 与 verify 行为。
//
// 对照 GoMock：参数/选项透传、错误保留、未消费期望在 verify 时 panic。

use std::io;

use crate::ast::{NodeRef, SelectStmt};
use crate::chunk::Row;
use crate::context::Context;
use crate::resolve::ResultField;
use crate::sqlexec::{
    ExecOptionAnalyzeVer2, ExecOptionIgnoreWarning, GetExecOption, RestrictedSQLExecutor,
};
use crate::{Call, NewMockRestrictedSQLExecutor, RestrictedSQLExecutorKey};

/// 键的 String/Display 必须与 Go `__MockRestrictedSQLExecutor` 一致。
#[test]
fn restricted_sql_executor_key_has_the_go_context_identity() {
    let key = RestrictedSQLExecutorKey;
    assert_eq!(key.String(), "__MockRestrictedSQLExecutor");
    assert_eq!(key.to_string(), "__MockRestrictedSQLExecutor");
}

/// ExecRestrictedSQL 应将选项、SQL、参数原样交给期望处理器并返回其结果。
#[test]
fn exec_restricted_sql_forwards_options_variadic_args_and_results() {
    let mut executor = NewMockRestrictedSQLExecutor();
    executor
        .EXPECT()
        .ExecRestrictedSQL(|_ctx, opts, sql, args| {
            assert_eq!(sql, "select %?, %?");
            let option = GetExecOption(opts);
            assert!(option.IgnoreWarning);
            assert_eq!(option.AnalyzeVer, 2);
            assert_eq!(*args[0].downcast_ref::<i64>().unwrap(), 7);
            assert_eq!(args[1].downcast_ref::<String>().unwrap(), "seven");
            Ok((
                vec![Row::default(), Row::default()],
                vec![ResultField::default()],
            ))
        });

    let ctx = Context::new();
    let (rows, fields) = RestrictedSQLExecutor::ExecRestrictedSQL(
        &mut executor,
        &ctx,
        vec![
            Box::new(ExecOptionIgnoreWarning),
            Box::new(ExecOptionAnalyzeVer2),
        ],
        "select %?, %?",
        vec![Box::new(7_i64), Box::new(String::from("seven"))],
    )
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(fields.len(), 1);
    assert_eq!(
        executor.calls(),
        vec![Call::ExecRestrictedSQL {
            sql: "select %?, %?".to_owned(),
            option_count: 2,
            argument_count: 2,
        }]
    );
    executor.verify();
}

/// Stmt/Parse 期望按 FIFO 消费，错误字符串原样保留。
#[test]
fn stmt_and_parse_expectations_are_fifo_and_preserve_errors() {
    let mut executor = NewMockRestrictedSQLExecutor();
    let statement = NodeRef::new(Box::new(SelectStmt::default()));
    let expected_statement = statement.clone();
    executor
        .EXPECT()
        .ExecRestrictedStmt(move |_ctx, stmt, opts| {
            assert_eq!(stmt, expected_statement);
            assert!(opts.is_empty());
            Err(Box::new(io::Error::other("statement failed")))
        });

    let parsed = NodeRef::new(Box::new(SelectStmt::default()));
    let returned = parsed.clone();
    executor.EXPECT().ParseWithParams(move |_ctx, sql, args| {
        assert_eq!(sql, "select %?");
        assert_eq!(*args[0].downcast_ref::<u64>().unwrap(), 42);
        Ok(returned)
    });
    executor.EXPECT().ParseWithParams(|_ctx, sql, args| {
        assert_eq!(sql, "broken");
        assert!(args.is_empty());
        Err(Box::new(io::Error::other("parse failed")))
    });

    let ctx = Context::new();
    let error =
        match RestrictedSQLExecutor::ExecRestrictedStmt(&mut executor, &ctx, statement, Vec::new())
        {
            Ok(_) => panic!("ExecRestrictedStmt unexpectedly succeeded"),
            Err(error) => error,
        };
    assert_eq!(error.to_string(), "statement failed");

    let actual = RestrictedSQLExecutor::ParseWithParams(
        &mut executor,
        &ctx,
        "select %?",
        vec![Box::new(42_u64)],
    )
    .unwrap();
    assert_eq!(actual, parsed);
    let error = RestrictedSQLExecutor::ParseWithParams(&mut executor, &ctx, "broken", Vec::new())
        .unwrap_err();
    assert_eq!(error.to_string(), "parse failed");

    assert_eq!(
        executor.calls(),
        vec![
            Call::ExecRestrictedStmt { option_count: 0 },
            Call::ParseWithParams {
                sql: "select %?".to_owned(),
                argument_count: 1
            },
            Call::ParseWithParams {
                sql: "broken".to_owned(),
                argument_count: 0
            },
        ]
    );
    executor.verify();
}

/// 仍有未消费期望时 verify 必须 panic。
#[test]
fn verify_rejects_unconsumed_go_mock_expectations() {
    let executor = NewMockRestrictedSQLExecutor();
    executor
        .EXPECT()
        .ParseWithParams(|_ctx, _sql, _args| Ok(NodeRef::new(Box::new(SelectStmt::default()))));

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| executor.verify()));
    assert!(panic.is_err());
}
