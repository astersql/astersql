// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
//
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 统计信息会话运行时的集成测试。
//
// 验证分析会话完成 Domain 与统计组件初始化后，能够通过 SQL 直接读写
// `mysql.stats_top_n` 元数据表，并按结果集协议返回持久化的统计字段。

use super::*;

#[test]
/// 覆盖分析会话初始化、Top-N 统计元数据写入及查询结果读取的完整链路。
fn analyze_session_initializes_and_executes_against_mysql_stats_top_n() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("analyze session");
    session
        .execute(
            "insert into mysql.stats_top_n \
             (table_id,is_index,hist_id,value,count) \
             values (874,0,1,x'04000000000000042a',3)",
        )
        .expect("insert into real mysql.stats_top_n metadata");
    let mut result = session
        .execute("select table_id,is_index,hist_id,count from mysql.stats_top_n")
        .expect("select mysql.stats_top_n")
        .pop()
        .expect("stats_top_n record set");
    assert_eq!(
        result.Next().expect("read stats_top_n row"),
        Some(vec![
            "874".to_owned(),
            "0".to_owned(),
            "1".to_owned(),
            "3".to_owned(),
        ])
    );
    // 再次读取必须得到空值，以确认结果集只包含刚插入的一条统计记录。
    assert_eq!(result.Next().expect("stats_top_n exhausted"), None);
}
