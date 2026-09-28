// Copyright 2026 AsterSQL.
// `EXPLAIN FOR CONNECTION` 权限与存活连接查找的单元测试。
//
// `EXPLAIN FOR CONNECTION`：查看指定连接上正在执行语句的执行计划。
// 需连接仍存活，且调用方为连接属主或具备超级权限。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::explain::{ExplainForConnection, ExplainForConnectionProvider, ExplainForProcess};
use astersql_errors as errors;

/// 内存中的进程表：按 connection_id 提供进程，并记录渲染调用。
struct LiveProcessProvider {
    processes: HashMap<u64, ExplainForProcess>,
    rendered: Mutex<Vec<(u64, String)>>,
}

impl ExplainForConnectionProvider for LiveProcessProvider {
    fn GetProcess(&self, connection_id: u64) -> Option<ExplainForProcess> {
        self.processes.get(&connection_id).cloned()
    }

    fn RenderProcessPlan(
        &self,
        process: &ExplainForProcess,
        format: &str,
    ) -> Result<Vec<Vec<String>>, errors::SharedError> {
        self.rendered
            .lock()
            .unwrap()
            .push((process.connection_id, format.to_owned()));
        Ok(vec![vec![
            "TableReader".to_owned(),
            process.sql.clone(),
            format.to_owned(),
        ]])
    }
}

#[test]
/// 验证属主可看计划、非属主被拒、超级用户可看、未知连接报错。
fn explain_for_connection_checks_live_owner_before_rendering() {
    let provider = LiveProcessProvider {
        processes: HashMap::from([(
            42,
            ExplainForProcess {
                connection_id: 42,
                user: "alice".to_owned(),
                sql: "select * from t where a > 1".to_owned(),
            },
        )]),
        rendered: Mutex::new(Vec::new()),
    };

    let own = ExplainForConnection(&provider, 42, "alice", false, "row").unwrap();
    assert_eq!(
        own,
        vec![vec![
            "TableReader".to_owned(),
            "select * from t where a > 1".to_owned(),
            "row".to_owned(),
        ]]
    );

    let denied = ExplainForConnection(&provider, 42, "bob", false, "brief").unwrap_err();
    assert!(denied.to_string().contains("Access denied"));
    assert_eq!(provider.rendered.lock().unwrap().len(), 1);

    ExplainForConnection(&provider, 42, "root", true, "brief").unwrap();
    assert_eq!(
        provider.rendered.lock().unwrap().as_slice(),
        &[(42, "row".to_owned()), (42, "brief".to_owned())]
    );

    let missing = ExplainForConnection(&provider, 404, "root", true, "row").unwrap_err();
    assert!(missing.to_string().contains("Unknown thread id: 404"));
}
