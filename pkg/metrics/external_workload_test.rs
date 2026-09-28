// Copyright 2026 AsterSQL.

use prometheus::core::Collector;

#[test]
fn external_workload_counter_matches_go_descriptor_and_registration() {
    if crate::main_test::run_in_isolated_process(
        "external_workload_test::external_workload_counter_matches_go_descriptor_and_registration",
    ) {
        return;
    }
    crate::main_test::ensure_test_env();
    unsafe {
        crate::metrics::InitMetrics().expect("initialize package metrics");
        let counter = crate::external_workload::ExternalWorkloadTaskCounter
            .as_ref()
            .expect("external workload counter initialized");
        let descriptors = counter.desc();
        let descriptor = descriptors
            .first()
            .expect("external workload counter descriptor");
        assert_eq!(descriptor.fq_name, "tidb_external_workload_task_total");
        assert_eq!(descriptor.variable_labels, ["type", "action"]);

        counter
            .with_label_values(&["gc", crate::external_workload::WorkerActionInit])
            .inc();
        crate::metrics::RegisterMetrics().expect("register all package metrics");
    }

    assert!(
        prometheus::gather()
            .iter()
            .any(|family| { family.name() == "tidb_external_workload_task_total" })
    );
}
