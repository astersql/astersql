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

// AsterSQL 迁移补充：plancodec 编解码与二进制计划解码的行为对照测试。
//
// 覆盖物理计划 ID 往返、任务类型编码、计划节点转义、压缩哨兵，
// 以及二进制 Explain 输出中的运行时统计与访问对象格式。

use super::protobuf::Message;
use super::*;

#[test]
/// 校验全部物理计划类型字符串与 ID 双向映射，并覆盖未知类型/ID 分支。
fn physical_plan_ids_match_go_and_round_trip() {
    let mappings = [
        (TypeSel, 1),
        (TypeSet, 2),
        (TypeProj, 3),
        (TypeAgg, 4),
        (TypeStreamAgg, 5),
        (TypeHashAgg, 6),
        (TypeShow, 7),
        (TypeJoin, 8),
        (TypeUnion, 9),
        (TypeTableScan, 10),
        (TypeMemTableScan, 11),
        (TypeUnionScan, 12),
        (TypeIdxScan, 13),
        (TypeSort, 14),
        (TypeTopN, 15),
        (TypeLimit, 16),
        (TypeHashJoin, 17),
        (TypeMergeJoin, 18),
        (TypeIndexJoin, 19),
        (TypeIndexMergeJoin, 20),
        (TypeIndexHashJoin, 21),
        (TypeApply, 22),
        (TypeMaxOneRow, 23),
        (TypeExists, 24),
        (TypeDual, 25),
        (TypeLock, 26),
        (TypeInsert, 27),
        (TypeUpdate, 28),
        (TypeDelete, 29),
        (TypeIndexLookUp, 30),
        (TypeTableReader, 31),
        (TypeIndexReader, 32),
        (TypeWindow, 33),
        (TypeTiKVSingleGather, 34),
        (TypeIndexMerge, 35),
        (TypePointGet, 36),
        (TypeShowDDLJobs, 37),
        (TypeBatchPointGet, 38),
        (TypeClusterMemTableReader, 39),
        (TypeDataSource, 40),
        (TypeLoadData, 41),
        (TypeTableSample, 42),
        (TypeTableFullScan, 43),
        (TypeTableRangeScan, 44),
        (TypeTableRowIDScan, 45),
        (TypeIndexFullScan, 46),
        (TypeIndexRangeScan, 47),
        (TypeExchangeReceiver, 48),
        (TypeExchangeSender, 49),
        (TypeCTE, 50),
        (TypeCTEDefinition, 51),
        (TypeCTETable, 52),
        (TypePartitionUnion, 53),
        (TypeShuffle, 54),
        (TypeShuffleReceiver, 55),
        (TypeForeignKeyCheck, 56),
        (TypeForeignKeyCascade, 57),
        (TypeExpand, 58),
        (TypeImportInto, 59),
        (TypeScalarSubQuery, 60),
        (TypeLocalIndexLookUp, 61),
        (TypePhysicalCTESink, 62),
        (TypePhysicalCTESource, 63),
    ];
    for (plan_type, id) in mappings {
        assert_eq!(TypeStringToPhysicalID(plan_type), id);
        assert_eq!(PhysicalIDToTypeString(id), plan_type);
    }
    assert_eq!(TypeStringToPhysicalID(TypeSequence), 0);
    assert_eq!(PhysicalIDToTypeString(99), "UnknownPlanID99");
}

#[test]
/// 校验任务类型编码及规范化计划解码与 Go 行为一致。
fn task_type_encoding_and_normalized_decoding_match_go() {
    use kv::StoreType;

    assert_eq!(EncodeTaskType(true, StoreType::UnSpecified), "0");
    assert_eq!(EncodeTaskType(false, StoreType::TiKV), "1_0");
    assert_eq!(EncodeTaskType(false, StoreType::TiFlash), "1_1");
    assert_eq!(EncodeTaskType(false, StoreType::TiDB), "1_2");
    assert_eq!(EncodeTaskTypeForNormalize(false, StoreType::TiKV), "1");

    assert_eq!(
        DecodeNormalizedPlan("0\t1\t1\tpredicate").unwrap(),
        "\tSelection\tcop\tpredicate"
    );
    assert!(DecodeNormalizedPlan("0\t1\t1_x\tpredicate").is_err());
}

#[test]
/// Go 先按 int 解析 store type，再转换为 uint8，因此越界值按模 256 环绕。
fn task_type_decoding_matches_go_uint8_conversion() {
    let decoded = DecodeNormalizedPlan("0\t1\t1_256\tpredicate").unwrap();
    assert_eq!(decoded.split('\t').nth(2), Some("cop[tikv]"));

    let decoded = DecodeNormalizedPlan("0\t1\t1_-1\tpredicate").unwrap();
    assert_eq!(decoded.split('\t').nth(2), Some("cop[unspecified]"));
}

#[test]
/// 校验计划节点字段转义编码，并经压缩后可解码出表头与内容。
fn plan_node_encoding_escapes_fields_and_decodes_with_header() {
    let mut encoded = Vec::new();
    EncodePlanNode(
        0,
        "7",
        TypeTableScan,
        3.25,
        "0",
        "a\tb\nc",
        "4",
        "time:1ms",
        "1 KB",
        "N/A",
        &mut encoded,
    );
    assert_eq!(
        encoded,
        b"0\t10_7\t0\t3.25\ta\\tb\\nc\t4\ttime:1ms\t1 KB\tN/A\n"
    );

    let decoded = DecodePlan(&Compress(&encoded)).unwrap();
    assert!(decoded.contains("id"));
    assert!(decoded.contains("TableScan_7"));
    assert!(decoded.contains("a\\tb\\nc"));
    assert!(decoded.contains("time:1ms"));
}

#[test]
/// strconv.FormatFloat 对非有限值使用 Go 固定拼写。
fn plan_node_non_finite_row_counts_match_go_format_float() {
    for (row_count, expected) in [
        (f64::INFINITY, "+Inf"),
        (f64::NEG_INFINITY, "-Inf"),
        (f64::NAN, "NaN"),
    ] {
        let mut encoded = Vec::new();
        EncodePlanNode(
            0,
            "1",
            TypeTableScan,
            row_count,
            "0",
            "",
            "",
            "",
            "",
            "",
            &mut encoded,
        );
        assert_eq!(encoded, format!("0\t10_1\t0\t{expected}\t\n").into_bytes());
    }
}

#[test]
/// 校验压缩往返，以及计划过长丢弃哨兵的解码文案。
fn compression_round_trip_and_discard_sentinels_match_go() {
    let input = b"0\t1\t0\t1\troot";
    let compressed = Compress(input);
    assert_ne!(compressed, String::from_utf8_lossy(input));
    assert_eq!(Decompress(&compressed).unwrap(), input);
    assert!(Decompress("not base64").is_err());
    assert_eq!(
        DecodePlan(PlanDiscardedEncoded).unwrap(),
        "(plan discarded because too long)"
    );
    assert_eq!(
        DecodeBinaryPlan(&BinaryPlanDiscardedEncoded()).unwrap(),
        "(plan discarded because too long)"
    );
}

#[test]
/// Go 的 base64.StdEncoding 解码会忽略编码中的 CR/LF。
fn decompression_accepts_base64_line_breaks_like_go() {
    let input = b"line-wrapped encoded plan";
    let encoded = Compress(input);
    let midpoint = encoded.len() / 2;
    let wrapped = format!("{}\r\n{}", &encoded[..midpoint], &encoded[midpoint..]);
    assert_eq!(Decompress(&wrapped).unwrap(), input);
}

#[test]
/// 校验二进制计划展示运行时统计，且 Build 子树排在 Probe 之前。
fn binary_plan_formats_runtime_stats_and_reorders_build_before_probe() {
    let mut probe = tipb::ExplainOperator::new();
    probe.set_name("Probe".to_owned());
    probe.mut_labels().push(tipb::OperatorLabel::ProbeSide);
    probe.set_task_type(tipb::TaskType::Cop);
    probe.set_store_type(tipb::StoreType::Tikv);

    let mut build = tipb::ExplainOperator::new();
    build.set_name("Build".to_owned());
    build.mut_labels().push(tipb::OperatorLabel::BuildSide);
    build.set_task_type(tipb::TaskType::Root);

    let mut root = tipb::ExplainOperator::new();
    root.set_name("HashJoin".to_owned());
    root.set_est_rows(2.5);
    root.set_cost(3.5);
    root.set_act_rows(2);
    root.set_task_type(tipb::TaskType::Root);
    root.set_root_basic_exec_info("time:1ms".to_owned());
    root.mut_root_group_exec_info().push("loops:1".to_owned());
    root.set_cop_exec_info("cop_task:1".to_owned());
    root.set_memory_bytes(1024);
    root.set_disk_bytes(-1);
    root.mut_children().push(probe);
    root.mut_children().push(build);

    let mut data = tipb::ExplainData::new();
    data.set_main(root);
    data.set_with_runtime_stats(true);
    let encoded = Compress(&data.write_to_bytes().unwrap());
    let decoded = DecodeBinaryPlan(&encoded).unwrap();

    assert!(decoded.contains("| HashJoin"));
    assert!(decoded.contains("time:1ms, loops:1, cop_task:1"));
    assert!(decoded.contains("1024 Bytes"));
    assert!(decoded.contains("N/A"));
    assert!(decoded.find("Build(Build)").unwrap() < decoded.find("Probe(Probe)").unwrap());
}

#[test]
/// 校验访问对象、brief 列与子查询在连接侧解码中的裁剪行为。
fn access_objects_brief_columns_and_subqueries_match_go() {
    let mut index = tipb::IndexAccess::new();
    index.set_name("PRIMARY".to_owned());
    index.mut_cols().push("id".to_owned());
    index.set_is_clustered_index(true);

    let mut scan = tipb::ScanAccessObject::new();
    scan.set_table("t".to_owned());
    scan.mut_partitions().push("p0".to_owned());
    scan.mut_indexes().push(index);

    let mut access = tipb::AccessObject::new();
    access.set_scan_object(scan);

    let mut main = tipb::ExplainOperator::new();
    main.set_name("TableReader_1".to_owned());
    main.set_brief_name("TableReader".to_owned());
    main.set_task_type(tipb::TaskType::Root);
    main.set_operator_info("full info".to_owned());
    main.set_brief_operator_info("brief info".to_owned());
    main.mut_access_objects().push(access);

    let mut subquery = tipb::ExplainOperator::new();
    subquery.set_name("Subquery_2".to_owned());
    subquery.set_task_type(tipb::TaskType::Root);

    let mut data = tipb::ExplainData::new();
    data.set_main(main);
    data.mut_subqueries().push(subquery);
    let encoded = Compress(&data.write_to_bytes().unwrap());

    let display = DecodeBinaryPlan(&encoded).unwrap();
    assert!(display.contains("table:t, partition:p0, clustered index:PRIMARY(id)"));
    assert!(display.contains("Subquery_2"));

    let rows = DecodeBinaryPlan4Connection(&encoded, types::ExplainFormatBrief, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "connection decoding intentionally omits subqueries"
    );
    assert_eq!(rows[0].len(), 5);
    assert_eq!(rows[0][0], "TableReader");
    assert_eq!(rows[0][4], "brief info");
}

// These fixtures contain UTF-8; conversion is asserted, never lossy.
fn DecodePlan(input: &str) -> Result<String, plancodec_dependency::Error> {
    plancodec_dependency::DecodePlan(input).map(|bytes| String::from_utf8(bytes).unwrap())
}
fn DecodeNormalizedPlan(input: &str) -> Result<String, plancodec_dependency::Error> {
    plancodec_dependency::DecodeNormalizedPlan(input).map(|bytes| String::from_utf8(bytes).unwrap())
}
