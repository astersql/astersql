// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

use super::extract_partition_value;

#[test]
fn list_partition_write_explain_only_adds_lock_for_locking_reads() {
    let (_, session) = crate::runtime::CreateAnalyzeSession().unwrap();
    let statements = [
        "explain format='plan_tree' delete from tlist where a in (2)",
        "explain format='plan_tree' update tlist set a=3 where a in (2)",
    ];
    let check_plans = |phase: &str, expect_lock: bool| {
        for sql in statements {
            let plan = session.explain_partition_integration_plan(sql).unwrap();
            println!("{phase}: {sql}: {:?}", plan.rows);
            let has_lock = plan.rows.iter().any(|row| row[0].contains("SelectLock"));
            assert_eq!(has_lock, expect_lock, "{phase}: {sql}");
        }
    };
    check_plans("ordinary", astersql_config_kerneltype::IsNextGen());
    session.execute("begin pessimistic").unwrap();
    assert!(session.TransactionIsPessimistic());
    check_plans("pessimistic", true);
    session.execute("rollback").unwrap();
    assert!(!session.TransactionIsPessimistic());
    check_plans("after rollback", astersql_config_kerneltype::IsNextGen());
}

#[test]
fn extract_partition_value_distinguishes_date_only_from_time_only_literals() {
    assert_eq!(
        extract_partition_value("year", "'1999-01-02'", 6),
        Some(1999)
    );
    assert_eq!(extract_partition_value("month", "'1999-01-02'", 6), Some(1));
    assert_eq!(extract_partition_value("day", "'1999-01-02'", 6), Some(2));
    assert_eq!(extract_partition_value("hour", "'12:34:56'", 6), Some(12));
    assert_eq!(extract_partition_value("hour", "'-12:34:56'", 6), Some(-12));
    assert_eq!(extract_partition_value("minute", "'12:34:56'", 6), Some(34));
    assert_eq!(extract_partition_value("second", "'12:34:56'", 6), Some(56));
}
