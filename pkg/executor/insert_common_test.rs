// Copyright 2026 AsterSQL.

use super::insert_common::resolve_get_row_cast_error;

#[test]
fn get_row_cast_error_follows_go_error_context_result() {
    assert_eq!(resolve_get_row_cast_error(false, "cast", Ok(())), Ok(()));
    assert_eq!(resolve_get_row_cast_error(true, "cast", Ok(())), Ok(()));
    assert_eq!(
        resolve_get_row_cast_error(false, "cast", Err("completed")),
        Err("cast")
    );
    assert_eq!(
        resolve_get_row_cast_error(true, "cast", Err("completed")),
        Err("completed")
    );
}

#[test]
fn terminal_auto_id_marker_survives_error_wrapping() {
    use super::insert_common::is_terminal_auto_id_error;
    use astersql_meta_autoid::AutoIdError;
    let marked =
        astersql_errors::SharedError::new(AutoIdError::RpcRetryLimit("last RPC failure".into()));
    assert!(is_terminal_auto_id_error(&marked));
    let ordinary = astersql_errors::SharedError::new(AutoIdError::AutoIncrementReadFailed(
        "read failed".into(),
    ));
    assert!(!is_terminal_auto_id_error(&ordinary));
}

#[test]
fn embedding_batch_preserves_task_order_errors_nulls_and_owned_text() {
    use super::insert_common::evaluate_embedding_inputs;
    use astersql_expression::EmbedTextArgs;
    let args = |text: &str| {
        Ok(Some(EmbedTextArgs {
            Model: "provider/model".into(),
            Text: text.into(),
            Opts: Default::default(),
        }))
    };
    let inputs = vec![
        args("1"),
        args("4"),
        Ok(None),
        Err("argument error".into()),
        args("bad"),
        args("9"),
    ];
    let results = evaluate_embedding_inputs(
        &|args| {
            args.Text
                .parse::<f32>()
                .map(|value| vec![value + 1.0])
                .map_err(|_| "provider error".into())
        },
        &inputs,
        &|| None,
    )
    .unwrap();
    assert_eq!(
        results,
        vec![
            Ok(Some(vec![2.0])),
            Ok(Some(vec![5.0])),
            Ok(None),
            Err("argument error".into()),
            Err("provider error".into()),
            Ok(Some(vec![10.0]))
        ]
    );
    assert!(
        evaluate_embedding_inputs(&|_| panic!("empty batch"), &[], &|| Some("canceled".into()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn embedding_batch_cancellation_precedes_null_and_argument_errors() {
    use super::insert_common::evaluate_embedding_inputs;
    let inputs = vec![Ok(None), Err("argument error".into())];
    assert_eq!(
        evaluate_embedding_inputs(
            &|_| panic!("canceled batch must not call provider"),
            &inputs,
            &|| Some("request canceled".into())
        )
        .unwrap_err(),
        "request canceled"
    );
}

#[test]
fn embedding_batch_bounds_concurrency_and_keeps_provider_errors_local() {
    use super::insert_common::evaluate_embedding_inputs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let active = AtomicUsize::new(0);
    let maximum = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let inputs = (0..805)
        .map(|index| {
            Ok(Some(astersql_expression::EmbedTextArgs {
                Model: "provider/model".into(),
                Text: index.to_string(),
                Opts: Default::default(),
            }))
        })
        .collect::<Vec<_>>();
    let results = evaluate_embedding_inputs(
        &|args| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            std::thread::yield_now();
            active.fetch_sub(1, Ordering::SeqCst);
            completed.fetch_add(1, Ordering::SeqCst);
            if args.Text == "1" {
                Err("one request failed".into())
            } else {
                Ok(vec![args.Text.parse::<f32>().unwrap()])
            }
        },
        &inputs,
        &|| None,
    )
    .unwrap();
    assert!(maximum.load(Ordering::SeqCst) <= 800);
    assert_eq!(completed.load(Ordering::SeqCst), 805);
    assert_eq!(results[1], Err("one request failed".into()));
    assert_eq!(results[804], Ok(Some(vec![804.0])));
}
