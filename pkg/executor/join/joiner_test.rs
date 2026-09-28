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

// Joiner 匹配、投影与三值逻辑标记的单元测试。
//
// 覆盖 Inner/LeftOuter 投影列序、条件求值的 Matched/Unmatched/HasNull，
// 以及 Semi / AntiLeftOuterSemi（含 null-aware）与非法构造参数。

/*
// 记录 joiner 测试对 session chunk size、clone 后缓存 chunk，以及 required rows 的期望。

// default_ctx 对应 Go 的 defaultCtx：构造 mock session 并配置 chunk、内存、磁盘和 SnapshotTS。
fn default_ctx() -> SessionCtxDraft {
    let mut ctx = SessionCtxDraft::new_mock();
    ctx.init_chunk_size = "vardef.DefInitChunkSize";
    ctx.max_chunk_size = "vardef.DefMaxChunkSize";
    // Go 这里为 StmtCtx 绑定 memory/disk tracker；保留资源跟踪器的初始化语义。
    ctx.mem_tracker = "memory.NewTracker(-1, ctx.GetSessionVars().MemQuotaQuery)";
    ctx.disk_tracker = "disk.NewTracker(-1, -1)";
    ctx.snapshot_ts = 1;
    ctx.domain = "domain.NewMockDomain()";
    ctx
}

// TestJoinerOtherConditionChunkUsesInitChunkSize 对应 Go 测试：other condition 缓存 chunk 应使用 InitChunkSize。
#[test]
fn test_joiner_other_condition_chunk_uses_init_chunk_size() {
    let lfields = vec![field_type("mysql.TypeLong")];
    let rfields = vec![field_type("mysql.TypeLong")];
    let fields = concat_field_types(&lfields, &rfields);
    let conditions = vec!["expression.NewOne()"];
    let default_inner = vec!["types.NewIntDatum(0)"];

    for join_type in ["base.InnerJoin", "base.LeftOuterJoin", "base.RightOuterJoin"] {
        let mut ctx = default_ctx();
        let init_chunk_size = 8;
        ctx.init_chunk_size_value = init_chunk_size;
        let max_chunk_size = ctx.max_chunk_size_value;

        let joiner = new_joiner_draft(&ctx, join_type, false, &default_inner, &conditions, &lfields, &rfields, None, false);
        let base = joiner_base_for_test(&joiner);
        assert!(base.chk.is_some());
        assert_eq!(init_chunk_size, base.chk_capacity);

        let cloned = joiner.clone();
        let cloned_base = joiner_base_for_test(&cloned);
        assert!(cloned_base.chk.is_some());
        assert_eq!(init_chunk_size, cloned_base.chk_capacity);

        // Go 这里构造 outer row 和 initChunkSize+1 行 inner chunk，验证 result 不被 initChunkSize 限制截断。
        let outer_row = gen_test_chunk(max_chunk_size, 1, &lfields).get_row(0);
        let inner_chk = gen_test_chunk(max_chunk_size, init_chunk_size + 1, &rfields);
        let mut result = ChunkDraft::new(&fields, max_chunk_size, max_chunk_size);
        let mut iter = ChunkIteratorDraft::new(&inner_chk);
        iter.begin();
        let err = try_to_match_inners_draft(&joiner, outer_row, iter, &mut result);
        assert!(err.is_none());
        assert_eq!(init_chunk_size + 1, result.num_rows());
    }
}

// joiner_base_for_test 对应 Go 的 type switch；只允许三类 joiner 暴露 baseJoiner。
fn joiner_base_for_test(joiner: &JoinerDraft) -> &BaseJoinerDraft {
    match joiner.kind {
        "innerJoiner" | "leftOuterJoiner" | "rightOuterJoiner" => &joiner.base,
        _ => panic!("unexpected joiner type {}", joiner.kind),
    }
}

// TestRequiredRows 对应 Go 测试：TryToMatchInners 应尊重 result.SetRequiredRows。
#[test]
fn test_required_rows() {
    let join_types = vec!["base.InnerJoin", "base.LeftOuterJoin", "base.RightOuterJoin"];
    let l_types = vec![
        vec!["mysql.TypeLong"],
        vec!["mysql.TypeFloat"],
        vec!["mysql.TypeLong", "mysql.TypeFloat"],
    ];
    let r_types = l_types.clone();

    let convert_types = |mysql_types: &[&'static str]| -> Vec<FieldTypeDraft> {
        mysql_types.iter().map(|t| field_type(t)).collect()
    };

    for join_type in join_types {
        for ltype in &l_types {
            for rtype in &r_types {
                let max_chunk_size = default_ctx().max_chunk_size_value;
                let lfields = convert_types(ltype);
                let rfields = convert_types(rtype);
                let outer_row = gen_test_chunk(max_chunk_size, 1, &lfields).get_row(0);
                let inner_chk = gen_test_chunk(max_chunk_size, max_chunk_size, &rfields);

                let mut default_inner = Vec::new();
                for (idx, field) in rfields.iter().enumerate() {
                    // Go 从 innerChk 第一行按 FieldType 取 Datum，作为外连接默认内表行。
                    default_inner.push(inner_chk.get_row(0).get_datum(idx, field));
                }
                let joiner = new_joiner_draft(&default_ctx(), join_type, false, &default_inner, &[], &lfields, &rfields, None, false);

                let fields = concat_field_types(&rfields, &lfields);
                let mut result = ChunkDraft::new(&fields, max_chunk_size, max_chunk_size);

                for seed in 0..10 {
                    // Go 使用 rand.Int()%maxChunkSize+1；用 deterministic seed 表达 required rows 范围。
                    let required = seed % max_chunk_size + 1;
                    result.set_required_rows(required, max_chunk_size);
                    result.reset();
                    let mut it = ChunkIteratorDraft::new(&inner_chk);
                    it.begin();
                    let err = try_to_match_inners_draft(&joiner, outer_row.clone(), it, &mut result);
                    assert!(err.is_none());
                    assert_eq!(required, result.num_rows());
                }
            }
        }
    }
}

// gen_test_chunk 对应 Go 的测试 chunk 构造器：仅支持 TypeLong 和 TypeFloat 两类字段。
fn gen_test_chunk(max_chunk_size: i32, mut num_rows: i32, fields: &[FieldTypeDraft]) -> ChunkDraft {
    let mut chk = ChunkDraft::new(fields, max_chunk_size, max_chunk_size);
    while num_rows > 0 {
        num_rows -= 1;
        for (col, field) in fields.iter().enumerate() {
            match field.mysql_type {
                "mysql.TypeLong" => chk.append_int64(col, 0),
                "mysql.TypeFloat" => chk.append_float32(col, 0.0),
                _ => panic!("not support"),
            }
        }
    }
    chk
}

#[derive(Clone)]
struct FieldTypeDraft {
    mysql_type: &'static str,
}

fn field_type(mysql_type: &'static str) -> FieldTypeDraft {
    FieldTypeDraft { mysql_type }
}

fn concat_field_types(left: &[FieldTypeDraft], right: &[FieldTypeDraft]) -> Vec<FieldTypeDraft> {
    let mut out = left.to_vec();
    out.extend_from_slice(right);
    out
}

struct SessionCtxDraft {
    init_chunk_size: &'static str,
    max_chunk_size: &'static str,
    init_chunk_size_value: i32,
    max_chunk_size_value: i32,
    mem_tracker: &'static str,
    disk_tracker: &'static str,
    snapshot_ts: u64,
    domain: &'static str,
}

impl SessionCtxDraft {
    fn new_mock() -> Self {
        Self {
            init_chunk_size: "",
            max_chunk_size: "",
            init_chunk_size_value: 32,
            max_chunk_size_value: 32,
            mem_tracker: "",
            disk_tracker: "",
            snapshot_ts: 0,
            domain: "",
        }
    }
}

#[derive(Clone)]
struct JoinerDraft {
    kind: &'static str,
    base: BaseJoinerDraft,
}

#[derive(Clone)]
struct BaseJoinerDraft {
    chk: Option<&'static str>,
    chk_capacity: i32,
}

fn new_joiner_draft(
    ctx: &SessionCtxDraft,
    join_type: &'static str,
    _has_filter: bool,
    _default_inner: &[&str],
    _conditions: &[&str],
    _lfields: &[FieldTypeDraft],
    _rfields: &[FieldTypeDraft],
    _other_conditions: Option<&[&str]>,
    _spill_enabled: bool,
) -> JoinerDraft {
    let kind = match join_type {
        "base.InnerJoin" => "innerJoiner",
        "base.LeftOuterJoin" => "leftOuterJoiner",
        "base.RightOuterJoin" => "rightOuterJoiner",
        _ => "unknownJoiner",
    };
    JoinerDraft {
        kind,
        base: BaseJoinerDraft {
            chk: Some("baseJoiner.chk"),
            chk_capacity: ctx.init_chunk_size_value,
        },
    }
}

#[derive(Clone)]
struct RowDraft;

impl RowDraft {
    fn get_datum(&self, _idx: usize, _field: &FieldTypeDraft) -> &'static str {
        "datum"
    }
}

struct ChunkDraft {
    rows: i32,
    required_rows: i32,
}

impl ChunkDraft {
    fn new(_fields: &[FieldTypeDraft], _init_capacity: i32, max_capacity: i32) -> Self {
        Self {
            rows: 0,
            required_rows: max_capacity,
        }
    }

    fn append_int64(&mut self, _col: usize, _value: i64) {
        self.rows += 1;
    }

    fn append_float32(&mut self, _col: usize, _value: f32) {
        self.rows += 1;
    }

    fn get_row(&self, _idx: i32) -> RowDraft {
        RowDraft
    }

    fn set_required_rows(&mut self, required: i32, _max_chunk_size: i32) {
        self.required_rows = required;
    }

    fn reset(&mut self) {
        self.rows = 0;
    }

    fn num_rows(&self) -> i32 {
        self.rows
    }
}

struct ChunkIteratorDraft;

impl ChunkIteratorDraft {
    fn new(_chunk: &ChunkDraft) -> Self {
        Self
    }

    fn begin(&mut self) {}
}

fn try_to_match_inners_draft(
    _joiner: &JoinerDraft,
    _outer_row: RowDraft,
    _iter: ChunkIteratorDraft,
    result: &mut ChunkDraft,
) -> Option<&'static str> {
    // 这里只表达 TryToMatchInners 对 result 行数的影响，真实匹配逻辑属于 Go/Rust joiner 实现。
    result.rows = result.required_rows;
    None
}
*/

use crate::joiner::{JoinType, Joiner, NaajType, OuterRowStatus, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 由整型切片构造测试行。
fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// Inner 投影保留指定列；LeftOuter 未匹配时拼默认内表值。
#[test]
fn joiner_inner_outer_and_projection_paths_preserve_column_order() {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        Some([vec![0], vec![1]]),
        false,
        32,
    )
    .unwrap();
    let mut output = Vec::new();
    let result = joiner
        .try_to_match_inners(
            &row(&[7, 8]),
            &[row(&[1, 2])],
            &mut output,
            NaajType::Unknown,
        )
        .unwrap();
    assert!(result.matched);
    assert_eq!(output, [vec![Value::Int(7), Value::Int(2)]]);

    let outer = Joiner::new(
        JoinType::LeftOuter,
        false,
        vec![Value::Int(-1)],
        Vec::new(),
        None,
        false,
        32,
    )
    .unwrap();
    let mut missed = Vec::new();
    outer.on_miss_match(false, &row(&[9]), &mut missed);
    assert_eq!(missed, [vec![Value::Int(9), Value::Int(-1)]]);
}

/// other condition 对正数/负数/NULL 分别对应 Matched、Unmatched、HasNull。
#[test]
fn joiner_conditions_report_false_null_and_match_statuses() {
    let condition: Predicate = Arc::new(|joined| match joined.first() {
        Some(Value::Null) => Ok(None),
        Some(Value::Int(value)) => Ok(Some(*value > 0)),
        _ => Ok(Some(false)),
    });
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let mut output = Vec::new();
    let statuses = joiner
        .try_to_match_outers(
            &[row(&[10]), row(&[-1]), vec![Value::Null]],
            &row(&[1]),
            &mut output,
        )
        .unwrap();
    assert_eq!(
        statuses,
        [
            OuterRowStatus::Matched,
            OuterRowStatus::Unmatched,
            OuterRowStatus::HasNull,
        ]
    );
    assert_eq!(output.len(), 1);
}

/// Semi 无条件早停；AntiLeftOuterSemi 未匹配写 true；非法 null-aware/chunk size 报错。
#[test]
fn semi_anti_and_null_aware_markers_match_three_valued_logic() {
    let semi = Joiner::new(
        JoinType::Semi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    assert!(semi.is_semi_join_without_condition());
    let mut output = Vec::new();
    semi.try_to_match_inners(
        &row(&[1]),
        &[row(&[1]), row(&[1])],
        &mut output,
        NaajType::Unknown,
    )
    .unwrap();
    assert_eq!(output, [row(&[1])]);

    let anti = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        true,
        8,
    )
    .unwrap();
    let mut missed = Vec::new();
    anti.on_miss_match(false, &row(&[3]), &mut missed);
    assert_eq!(missed, [vec![Value::Int(3), Value::Bool(true)]]);
    assert!(
        Joiner::new(
            JoinType::Inner,
            false,
            Vec::new(),
            Vec::new(),
            None,
            true,
            8
        )
        .is_err()
    );
    assert!(
        Joiner::new(
            JoinType::Inner,
            false,
            Vec::new(),
            Vec::new(),
            None,
            false,
            0
        )
        .is_err()
    );
}

/// Go SemiJoin 忽略条件 NULL；普通 Inner/Outer Join 的 isNull 返回值也始终为 false。
#[test]
fn semi_and_regular_join_do_not_expose_condition_nullness() {
    let null_condition: Predicate = Arc::new(|_| Ok(None));
    for join_type in [
        JoinType::Semi,
        JoinType::Inner,
        JoinType::LeftOuter,
        JoinType::RightOuter,
    ] {
        let joiner = Joiner::new(
            join_type,
            false,
            Vec::new(),
            vec![null_condition.clone()],
            None,
            false,
            8,
        )
        .unwrap();
        let result = joiner
            .try_to_match_inners(&row(&[1]), &[row(&[2])], &mut Vec::new(), NaajType::Unknown)
            .unwrap();
        assert!(!result.matched);
        assert!(!result.has_null);
    }

    let semi = Joiner::new(
        JoinType::Semi,
        false,
        Vec::new(),
        vec![null_condition],
        None,
        false,
        8,
    )
    .unwrap();
    assert_eq!(
        semi.try_to_match_outers(&[row(&[1])], &row(&[2]), &mut Vec::new())
            .unwrap(),
        [OuterRowStatus::Unmatched]
    );
}

/// Go null-aware Anti 的 other condition 把 false/NULL 都视为无效 inner，不传播 NULL。
#[test]
fn null_aware_anti_join_ignores_other_condition_nullness() {
    let null_condition: Predicate = Arc::new(|_| Ok(None));
    for join_type in [JoinType::AntiSemi, JoinType::AntiLeftOuterSemi] {
        let joiner = Joiner::new(
            join_type,
            false,
            Vec::new(),
            vec![null_condition.clone()],
            None,
            true,
            8,
        )
        .unwrap();
        let result = joiner
            .try_to_match_inners(
                &row(&[1]),
                &[row(&[2])],
                &mut Vec::new(),
                NaajType::LeftNotNullRightNotNull,
            )
            .unwrap();
        assert!(!result.matched);
        assert!(!result.has_null);
    }
}

/// CNF 中 false 支配此前的 NULL，与 Go expression.EvalBool 一致。
#[test]
fn false_condition_overrides_earlier_null() {
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        vec![Arc::new(|_| Ok(None)), Arc::new(|_| Ok(Some(false)))],
        None,
        false,
        8,
    )
    .unwrap();
    let result = joiner
        .try_to_match_inners(&row(&[1]), &[row(&[2])], &mut Vec::new(), NaajType::Unknown)
        .unwrap();
    assert!(!result.has_null);
}
