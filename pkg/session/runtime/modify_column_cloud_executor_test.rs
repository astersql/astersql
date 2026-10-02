// Copyright 2026 AsterSQL.

use super::*;
use executor::StepExecutor;
use sort::Storage;

fn run_merge(duplicate: bool, cancelled: bool) {
    let domain = super::super::super::import_sst_test::domain();
    let directory =
        std::env::temp_dir().join(format!("astersql-cloud-step-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let uri = format!("local://{}", directory.display());
    let store = CloudStore::open(&uri, Arc::new(AtomicBool::new(false))).unwrap();
    let encode = |pairs: &[(&[u8], &[u8])]| {
        let mut data = Vec::new();
        for (key, value) in pairs {
            data.extend_from_slice(&(key.len() as u64).to_be_bytes());
            data.extend_from_slice(&(value.len() as u64).to_be_bytes());
            data.extend_from_slice(key);
            data.extend_from_slice(value);
        }
        data
    };
    store
        .write("first", encode(&[(b"a", b"1"), (b"c", b"3")]))
        .unwrap();
    store
        .write(
            "second",
            encode(&[(if duplicate { b"a" } else { b"b" }, b"2")]),
        )
        .unwrap();
    let fields = wire::ExternalFields {
        data_files: vec!["first".into(), "second".into()],
        ele_ids: vec![7],
        ..Default::default()
    };
    let mut internal = serde_json::json!({"physical_table_id": 99, "ts": 500});
    let meta = wire::write(store.as_ref(), &mut internal, &fields, "plan.json".into()).unwrap();
    let base = ReadIndex::new(
        domain.clone(),
        storage::TaskManager::new(),
        Default::default(),
        vec![7],
        false,
        uri,
    )
    .with_resource(
        executor::StepResource {
            CPU: 2,
            Memory: 1024 * 1024,
        },
        0,
    );
    let step = CloudStep::new(base, astersql_dxf_framework_proto::BackfillStepMergeSort).unwrap();
    let mut subtask = executor::Subtask {
        Meta: meta.clone(),
        ..Default::default()
    };
    subtask.SubtaskBase.TaskID = 88;
    subtask.SubtaskBase.ID = 9;
    let context = executor::Context::Background();
    step.Init(&context).unwrap();
    if cancelled {
        context.Cancel();
    }
    let result = step.RunSubtask(&context, &mut subtask);
    if duplicate || cancelled {
        assert!(
            result.is_err(),
            "failed or cancelled merge must return an error"
        );
        assert_eq!(
            subtask.Meta, meta,
            "failed merge must not publish successful metadata"
        );
    } else {
        result.unwrap();
        let output = wire::read(store.as_ref(), &subtask.Meta).unwrap();
        assert_eq!(output["physical_table_id"], 99);
        assert_eq!(output["ts"], 500);
        let fields: wire::ExternalFields = serde_json::from_value(output).unwrap();
        assert_eq!(fields.ele_ids, vec![7]);
        assert_eq!(fields.meta_groups.len(), 1);
        assert_eq!(fields.meta_groups[0].count, 3);
        let paths = fields.meta_groups[0]
            .files
            .iter()
            .flat_map(|group| group.filenames.iter().map(|pair| pair[0].clone()))
            .collect::<Vec<_>>();
        let pairs = sort::reader::ReadKVFilesAsync(Default::default(), store.clone(), paths)
            .collect::<sort::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            pairs
                .iter()
                .map(|pair| pair.key.as_slice())
                .collect::<Vec<_>>(),
            vec![b"a".as_slice(), b"b", b"c"]
        );
    }
    assert!(step.active.lock().unwrap().is_none());
    assert!(
        step.ResourceModified(
            &context,
            &executor::StepResource {
                CPU: 1,
                Memory: 1024
            }
        )
        .is_err()
    );
    step.Cleanup(&context).unwrap();
    drop(step);
    domain.close();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cloud_merge_step_publishes_actual_sorted_object_metadata() {
    run_merge(false, false);
}

#[test]
fn cloud_merge_step_duplicate_does_not_publish_success_metadata() {
    run_merge(true, false);
}

#[test]
fn cloud_merge_step_cancelled_context_does_not_publish_success_metadata() {
    run_merge(false, true);
}
