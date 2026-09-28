// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn record_api_call_registers_metric_with_default_registry() {
    RecordAPICall(BACKEND_S3, API_CALL_LIST_OBJECTS);

    let metric = prometheus::gather()
        .into_iter()
        .find(|family| family.name() == "tidb_br_s3_api_call_total");

    assert!(
        metric.is_some(),
        "Go package init registers S3APICallCounter with the default registry"
    );
}
