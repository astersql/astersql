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

// `util` 运行时辅助与 Go 边界行为的对等单元测试。
//
// 覆盖数值前缀截取、二进制时间/时长解码、容量/纳秒格式化、时区与校对定位，
// 以及 SQL digest 检索器对本地/全局 statements_summary 的查询路径。

use crate::util_kernel::*;
use crate::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// 模拟 `SQLExecutor`：记录调用参数，并按 local/global 返回假 digest 文本。
struct DigestExecutor {
    global_only: bool,
    calls: Mutex<Vec<(bool, usize)>>,
}

impl DigestExecutor {
    /// 构造执行器；`global_only` 为真时本地查询返回空结果，迫使走全局表。
    fn new(global_only: bool) -> Self {
        Self {
            global_only,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl expropt::SQLExecutor for DigestExecutor {
    type Context = kv::Context;
    type OptionFuncAlias = ();
    type Row = chunk::Row;
    type ResultField = ();

    /// 校验内部请求来源后，按 SQL 是否含 cluster_statements_summary 分支返回行。
    fn exec_restricted_sql(
        &self,
        context: &Self::Context,
        _options: &[Self::OptionFuncAlias],
        sql: &str,
        args: &[Box<dyn std::any::Any>],
    ) -> anyhow::Result<(Vec<Self::Row>, Vec<Self::ResultField>)> {
        assert_eq!(
            context
                .RequestSource()
                .map(|source| source.RequestSourceType.as_str()),
            Some(kv::InternalTxnOthers)
        );
        // 根据 SQL 文本判断是否命中集群级 statements_summary。
        let global = sql.contains("cluster_statements_summary");
        self.calls.lock().unwrap().push((global, args.len()));
        if self.global_only && !global {
            return Ok((Vec::new(), Vec::new()));
        }
        let digest = args
            .first()
            .and_then(|arg| arg.downcast_ref::<String>())
            .cloned()
            .unwrap_or_else(|| "digest-all".to_owned());
        let fields = vec![
            *types::NewFieldType(mysql::TypeVarchar),
            *types::NewFieldType(mysql::TypeVarchar),
        ];
        let mut rows = chunk::NewChunkWithCapacity(fields, 1);
        rows.AppendString(0, &digest);
        rows.AppendString(1, if global { "global sql" } else { "local sql" });
        Ok((vec![rows.GetRow(0).CopyConstruct()], Vec::new()))
    }
}

/// 用固定的本地/全局 digest 数据复现 Go `TestSQLDigestTextRetriever` 的部分命中场景。
struct MappingDigestExecutor {
    local: HashMap<String, String>,
    global: HashMap<String, String>,
    calls: Mutex<Vec<(bool, usize)>>,
}

impl MappingDigestExecutor {
    fn new() -> Self {
        Self {
            local: [
                ("digest1", "text1"),
                ("digest2", "text2"),
                ("digest6", "text6"),
            ]
            .into_iter()
            .map(|(digest, text)| (digest.to_owned(), text.to_owned()))
            .collect(),
            global: [
                ("digest2", "text2"),
                ("digest3", "text3"),
                ("digest4", "text4"),
                ("digest7", "text7"),
            ]
            .into_iter()
            .map(|(digest, text)| (digest.to_owned(), text.to_owned()))
            .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl expropt::SQLExecutor for MappingDigestExecutor {
    type Context = kv::Context;
    type OptionFuncAlias = ();
    type Row = chunk::Row;
    type ResultField = ();

    fn exec_restricted_sql(
        &self,
        context: &Self::Context,
        _options: &[Self::OptionFuncAlias],
        sql: &str,
        args: &[Box<dyn std::any::Any>],
    ) -> anyhow::Result<(Vec<Self::Row>, Vec<Self::ResultField>)> {
        assert_eq!(
            context
                .RequestSource()
                .map(|source| source.RequestSourceType.as_str()),
            Some(kv::InternalTxnOthers)
        );
        let global = sql.contains("cluster_statements_summary");
        self.calls.lock().unwrap().push((global, args.len()));
        let data = if global { &self.global } else { &self.local };
        let requested = args
            .iter()
            .map(|arg| arg.downcast_ref::<String>().unwrap().as_str())
            .collect::<Vec<_>>();
        let matches = data
            .iter()
            .filter(|(digest, _)| requested.is_empty() || requested.contains(&digest.as_str()));
        let fields = vec![
            *types::NewFieldType(mysql::TypeVarchar),
            *types::NewFieldType(mysql::TypeVarchar),
        ];
        let mut rows = Vec::new();
        for (digest, text) in matches {
            let mut row = chunk::NewChunkWithCapacity(fields.clone(), 1);
            row.AppendString(0, digest);
            row.AppendString(1, text);
            rows.push(row.GetRow(0).CopyConstruct());
        }
        Ok((rows, Vec::new()))
    }
}

/// 校验 `getValidPrefix` 在进制边界与非法进制下与 Go 一致。
#[test]
fn test_valid_numeric_prefix_matches_go_boundaries() {
    assert_eq!(getValidPrefix("+1fZ", 16), "1f");
    assert_eq!(getValidPrefix("-1012", 2), "-101");
    assert_eq!(getValidPrefix("z!", 36), "z");
    assert_eq!(getValidPrefix("123", 1), "");
    assert_eq!(getValidPrefix("123", 37), "");
}

/// 校验 MySQL 二进制协议日期/时间戳/时长解码的偏移与文本格式。
#[test]
fn test_binary_temporal_helpers_preserve_protocol_offsets_and_text() {
    let date = [0xe8, 0x07, 0x0c, 0x1f];
    assert_eq!(binaryDate(0, &date), (4, "2024-12-31".to_owned()));

    let timestamp_with_tz = [
        0xe8, 0x07, 0x0c, 0x1f, 0x17, 0x3b, 0x3a, 0x40, 0xe2, 0x01, 0x00, 0x3e, 0xfe,
    ];
    assert_eq!(
        binaryTimestampWithTZ(0, &timestamp_with_tz),
        (13, "2024-12-31 23:59:58.123456-7:30".to_owned())
    );

    let duration = [1, 2, 0, 0, 0, 3, 4, 5, 64, 226, 1, 0];
    assert_eq!(
        binaryDurationWithMS(1, &duration, duration[0]),
        (12, "-2 03:04:05.123456".to_owned())
    );
}

/// 校验字节与纳秒格式化阈值与 Go 阈值一致。
#[test]
fn test_format_units_match_go_thresholds() {
    assert_eq!(GetFormatBytes(0.0), "0 bytes");
    assert_eq!(GetFormatBytes(1024.0), "1.00 KiB");
    assert_eq!(GetFormatNanoTime(1_000.0), "1.00 us");
    assert_eq!(GetFormatNanoTime(60_000_000_000.0), "1.00 min");
}

/// 校验时区秒偏移与空 needle 的校对定位边界。
#[test]
fn test_timezone_and_empty_collation_search_boundaries() {
    assert_eq!(timeZone2int("+13:00"), 46_800);
    assert_eq!(timeZone2int("-05:30"), -19_800);
    assert_eq!(locateStringWithCollation("abc", "", "utf8mb4_bin"), 1);
}

/// 校验 digest 检索器先本地、必要时再全局查询，并写回 SQLDigestsMap。
#[test]
fn test_digest_retriever_executes_local_and_global_internal_queries() {
    let local = DigestExecutor::new(false);
    let mut retriever = NewSQLDigestTextRetriever();
    retriever
        .SQLDigestsMap
        .insert("digest-local".to_owned(), String::new());
    retriever
        .RetrieveLocal(kv::Context::default(), &local)
        .unwrap();
    assert_eq!(retriever.SQLDigestsMap["digest-local"], "local sql");
    assert_eq!(*local.calls.lock().unwrap(), vec![(false, 1)]);

    let global = DigestExecutor::new(true);
    let mut retriever = NewSQLDigestTextRetriever();
    retriever
        .SQLDigestsMap
        .insert("digest-global".to_owned(), String::new());
    retriever
        .RetrieveGlobal(kv::Context::default(), &global)
        .unwrap();
    assert_eq!(retriever.SQLDigestsMap["digest-global"], "global sql");
    assert_eq!(*global.calls.lock().unwrap(), vec![(false, 1), (true, 1)]);
}

/// 逐项复现 Go 测试的部分命中、全局补齐、未知 digest 与“大集合查全量”分支。
#[test]
fn test_digest_retriever_matches_go_partial_and_fetch_all_contract() {
    let executor = MappingDigestExecutor::new();
    let requested = || {
        (1..=5)
            .map(|index| (format!("digest{index}"), String::new()))
            .collect::<HashMap<_, _>>()
    };

    let mut local = NewSQLDigestTextRetriever();
    local.SQLDigestsMap = requested();
    local
        .RetrieveLocal(kv::Context::default(), &executor)
        .unwrap();
    assert_eq!(local.SQLDigestsMap["digest1"], "text1");
    assert_eq!(local.SQLDigestsMap["digest2"], "text2");
    assert_eq!(local.SQLDigestsMap["digest3"], "");
    assert_eq!(local.SQLDigestsMap["digest4"], "");
    assert_eq!(local.SQLDigestsMap["digest5"], "");
    assert!(!local.SQLDigestsMap.contains_key("digest6"));

    let mut global = NewSQLDigestTextRetriever();
    global.SQLDigestsMap = requested();
    global
        .SQLDigestsMap
        .insert("digest2".to_owned(), "known".to_owned());
    global
        .RetrieveGlobal(kv::Context::default(), &executor)
        .unwrap();
    assert_eq!(global.SQLDigestsMap["digest1"], "text1");
    assert_eq!(global.SQLDigestsMap["digest2"], "known");
    assert_eq!(global.SQLDigestsMap["digest3"], "text3");
    assert_eq!(global.SQLDigestsMap["digest4"], "text4");
    assert_eq!(global.SQLDigestsMap["digest5"], "");
    assert!(!global.SQLDigestsMap.contains_key("digest7"));

    let mut fetch_all = NewSQLDigestTextRetriever();
    fetch_all.SQLDigestsMap = (0..513)
        .map(|index| (format!("missing-{index}"), String::new()))
        .collect();
    fetch_all
        .RetrieveLocal(kv::Context::default(), &executor)
        .unwrap();

    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls[0], (false, 5));
    assert_eq!(calls[1], (false, 5));
    assert_eq!(calls[2], (true, 3));
    assert_eq!(calls[3], (false, 0));
}
