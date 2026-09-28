// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 扩展 bootstrap SQL 注册顺序的规范回归测试。

use crate::{
    BootstrapContext, ExtensionContext, ExtensionError, SessionPool, SessionResource, WithBootstrap,
};
use serial_test::serial;

struct MockSessionPool;

impl SessionPool for MockSessionPool {
    fn Get(&self) -> Result<Box<dyn SessionResource>, ExtensionError> {
        Ok(Box::new(()))
    }

    fn Put(&self, _resource: Box<dyn SessionResource>) {}
}

#[derive(Default)]
struct MockBootstrapContext {
    executed_sql: Vec<String>,
    table_values: Vec<i64>,
}

impl ExtensionContext for MockBootstrapContext {}

impl BootstrapContext for MockBootstrapContext {
    fn ExecuteSQL(&mut self, sql: &str) -> Result<Vec<crate::chunk::Row>, ExtensionError> {
        self.executed_sql.push(sql.to_owned());
        match sql {
            "create table test.t1 (a int)" => Ok(Vec::new()),
            "insert into test.t1 values(1)" => {
                self.table_values.push(1);
                Ok(Vec::new())
            }
            "select * from test.t1 where a=1" => Ok(self
                .table_values
                .iter()
                .copied()
                .filter(|value| *value == 1)
                .map(|value| {
                    chunk_dependency::mutrow::MutRowFromValues(vec![
                        chunk_dependency::mutrow::GoAny::Int64(value),
                    ])
                    .ToRow()
                    .CopyConstruct()
                })
                .collect()),
            _ => Err(ExtensionError::new(format!(
                "unexpected bootstrap SQL: {sql}"
            ))),
        }
    }

    fn EtcdClient(&self) -> Option<&etcd_client::Client> {
        None
    }

    fn SessionPool(&self) -> &dyn SessionPool {
        &MockSessionPool
    }
}

/// 与 Go `TestBootstrap` 一致：注册的 bootstrap 按扩展名顺序执行并传播 SQL。
#[test]
#[serial]
fn canonical_bootstrap_sql_preserves_registration_order() {
    crate::Reset();
    crate::Register(
        "test1".into(),
        vec![crate::WithBootstrapSQL(vec![
            "create table test.t1 (a int)".into(),
        ])],
    )
    .unwrap();
    crate::Register(
        "test2".into(),
        vec![WithBootstrap(|context| {
            context.ExecuteSQL("insert into test.t1 values(1)")?;
            let rows = context.ExecuteSQL("select * from test.t1 where a=1")?;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].GetInt64(0), 1);
            Ok(())
        })],
    )
    .unwrap();
    crate::Setup().unwrap();

    let extensions = crate::GetExtensions().unwrap().unwrap();
    let mut context = MockBootstrapContext::default();
    extensions.Bootstrap(&mut context).unwrap();
    assert_eq!(
        context.executed_sql,
        [
            "create table test.t1 (a int)",
            "insert into test.t1 values(1)",
            "select * from test.t1 where a=1",
        ]
    );
    assert_eq!(context.table_values, [1]);
    crate::Reset();
}
