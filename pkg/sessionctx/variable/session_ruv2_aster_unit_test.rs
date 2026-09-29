// Copyright 2026 AsterSQL.

use crate::session::SessionVars;

#[test]
fn go_merge_9_ruv2_weights_read_tikv_client_config() {
    let cfg = crate::config::get_global_config();
    let source = &cfg.tikv_client.ruv2;
    let actual = SessionVars::new().RUV2Weights();
    assert_eq!(actual.RUScale, source.ru_scale);
    assert_eq!(actual.ResultChunkCells, source.result_chunk_cells);
    assert_eq!(actual.ExecutorL1, source.executor_l1);
    assert_eq!(actual.ExecutorL2, source.executor_l2);
    assert_eq!(actual.ExecutorL3, source.executor_l3);
    assert_eq!(actual.ExecutorL5InsertRows, source.executor_l5_insert_rows);
    assert_eq!(actual.PlanCnt, source.plan_cnt);
    assert_eq!(actual.PlanDeriveStatsPaths, source.plan_derive_stats_paths);
    assert_eq!(
        actual.ResourceManagerReadCnt,
        source.resource_manager_read_cnt
    );
    assert_eq!(
        actual.ResourceManagerWriteCnt,
        source.resource_manager_write_cnt
    );
    assert_eq!(actual.WriteKeys, source.write_keys);
    assert_eq!(actual.SessionParserTotal, source.session_parser_total);
    assert_eq!(actual.TxnCnt, source.txn_cnt);
}
