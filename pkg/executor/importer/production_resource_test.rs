// Copyright 2026 AsterSQL.

use super::*;
use std::sync::Arc;

struct UnusedSampler;
impl KVSizeSamplerService for UnusedSampler {
    fn NewParser(
        &self,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &KVSizeSampleConfig,
    ) -> Result<Box<dyn astersql_lightning_mydump::Parser + Send>, String> {
        unreachable!()
    }

    fn NewEncoder(
        &self,
        _: &astersql_lightning_mydump::SourceFileMeta,
        _: &KVSizeSampleConfig,
        _: Arc<dyn astersql_table::Table>,
        _: &[FieldMapping],
        _: &[Arc<astersql_table::Column>],
    ) -> Result<TableKVEncoder, String> {
        unreachable!()
    }
}

#[test]
fn handle_resource_calculator_uses_go_import_rccalc() {
    const GIB: i64 = 1024 * 1024 * 1024;
    let calculator = HandleImportResourceCalculator {
        Context: astersql_dxf_framework_handle::Context::default(),
        Sampler: Arc::new(UnusedSampler),
    };
    let result = calculator.Calculate(
        200 * GIB,
        16,
        0.0,
        ScheduleTuneFactors { AmplifyFactor: 1.0 },
    );
    assert_eq!(result.ThreadCnt, 8);
    assert_eq!(result.MaxNodeCnt, 1);
    assert_eq!(result.DistSQLScanConcurrency, 120);
}
