// Copyright 2026 AsterSQL.

use super::*;

struct DefaultApi;

impl S3API for DefaultApi {}

#[test]
fn in_flight_s3_request_stops_on_context_cancellation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let context = storeapi::Context::default();
    let canceller = {
        let context = context.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            context.cancel();
        })
    };
    let error = crate::interface::run_cancellable(&runtime, &context, std::future::pending::<()>())
        .err()
        .unwrap();
    canceller.join().unwrap();
    assert!(error.to_string().contains("operation cancelled"));
}

#[test]
fn s3_api_exposes_go_list_objects_v1_contract() {
    let error = DefaultApi
        .list_objects(
            &storeapi::Context::default(),
            &ListObjectsInput {
                bucket: "bucket".to_owned(),
                prefix: "prefix/".to_owned(),
                max_keys: 1,
                marker: Some("marker".to_owned()),
            },
            RequestOptions::default(),
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "S3 operation ListObjects is not implemented"
    );
}
