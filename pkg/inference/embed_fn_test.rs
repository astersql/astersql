// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crate::{EmbedFn, Embedder, Options};

struct TestEmbedder {
    calls: AtomicUsize,
    texts: AtomicUsize,
}

impl Embedder for TestEmbedder {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        _model: &str,
        texts: &[String],
        _opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
        for _ in 0..8 {
            if cancel.load(Ordering::Acquire) {
                return Err("request canceled".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        texts
            .iter()
            .map(|text| serde_json::from_str::<Vec<f32>>(text).map_err(|error| error.to_string()))
            .collect()
    }
}

#[test]
fn go_merge_43_embed_fn_shares_equal_calls_and_caches_results() {
    let embed_fn = Arc::new(EmbedFn::new());
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("mock", provider.clone()).unwrap();
    let first = {
        let embed_fn = Arc::clone(&embed_fn);
        thread::spawn(move || embed_fn.embed("mock/json", "[1,2]", &Options::new(), &|| false))
    };
    let second = {
        let embed_fn = Arc::clone(&embed_fn);
        thread::spawn(move || embed_fn.embed("mock/json", "[1,2]", &Options::new(), &|| false))
    };
    assert_eq!(first.join().unwrap().unwrap(), [1.0, 2.0]);
    assert_eq!(second.join().unwrap().unwrap(), [1.0, 2.0]);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        embed_fn
            .embed("mock/json", "[1,2]", &Options::new(), &|| false)
            .unwrap(),
        [1.0, 2.0]
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    embed_fn.close();
    assert!(
        embed_fn
            .embed("mock/json", "[1,2]", &Options::new(), &|| false)
            .is_err()
    );
}

#[test]
fn go_merge_43_one_cancelled_waiter_preserves_shared_request() {
    let embed_fn = Arc::new(EmbedFn::new());
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("mock", provider.clone()).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = {
        let embed_fn = Arc::clone(&embed_fn);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            embed_fn.embed("mock/json", "[3]", &Options::new(), &|| {
                cancel.load(Ordering::Acquire)
            })
        })
    };
    thread::sleep(Duration::from_millis(5));
    let remaining = {
        let embed_fn = Arc::clone(&embed_fn);
        thread::spawn(move || embed_fn.embed("mock/json", "[3]", &Options::new(), &|| false))
    };
    thread::sleep(Duration::from_millis(5));
    cancel.store(true, Ordering::Release);
    assert!(cancelled.join().unwrap().is_err());
    assert_eq!(remaining.join().unwrap().unwrap(), [3.0]);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn go_merge_43_distinct_texts_batch_into_one_provider_call() {
    let embed_fn = Arc::new(EmbedFn::new());
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("mock", provider.clone()).unwrap();
    let jobs = ["[1]", "[2]"]
        .into_iter()
        .map(|text| {
            let embed_fn = Arc::clone(&embed_fn);
            thread::spawn(move || embed_fn.embed("mock/json", text, &Options::new(), &|| false))
        })
        .collect::<Vec<_>>();
    for job in jobs {
        assert!(job.join().unwrap().is_ok());
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.texts.load(Ordering::SeqCst), 2);
}

struct WaitingEmbedder(Arc<AtomicBool>);

impl Embedder for WaitingEmbedder {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        _model: &str,
        _texts: &[String],
        _opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.0.store(true, Ordering::Release);
        while !cancel.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        Err("request canceled".into())
    }
}

#[test]
fn go_merge_43_close_cancels_provider_after_batch_dispatch() {
    let embed_fn = Arc::new(EmbedFn::new());
    let entered = Arc::new(AtomicBool::new(false));
    embed_fn
        .register("waiting", Arc::new(WaitingEmbedder(Arc::clone(&entered))))
        .unwrap();
    let caller = {
        let embed_fn = Arc::clone(&embed_fn);
        thread::spawn(move || embed_fn.embed("waiting/model", "text", &Options::new(), &|| false))
    };
    for _ in 0..200 {
        if entered.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(entered.load(Ordering::Acquire));
    embed_fn.close();
    assert!(caller.join().unwrap().is_err());
}

#[test]
fn go_merge_43_last_cancelled_waiter_cancels_provider() {
    let embed_fn = Arc::new(EmbedFn::new());
    let entered = Arc::new(AtomicBool::new(false));
    embed_fn
        .register("waiting", Arc::new(WaitingEmbedder(Arc::clone(&entered))))
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let callers = (0..2)
        .map(|_| {
            let embed_fn = Arc::clone(&embed_fn);
            let cancel = Arc::clone(&cancel);
            thread::spawn(move || {
                embed_fn.embed("waiting/model", "text", &Options::new(), &|| {
                    cancel.load(Ordering::Acquire)
                })
            })
        })
        .collect::<Vec<_>>();
    for _ in 0..200 {
        if entered.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(entered.load(Ordering::Acquire));
    cancel.store(true, Ordering::Release);
    for caller in callers {
        assert!(caller.join().unwrap().is_err());
    }
    embed_fn.close();
}

#[test]
fn go_merge_43_cache_version_and_duplicate_registration_follow_provider_state() {
    let embed_fn = EmbedFn::new();
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("Mock", provider.clone()).unwrap();
    assert!(
        embed_fn
            .register(" mock ", Arc::new(crate::MockEmbedder))
            .is_err()
    );
    assert!(embed_fn.has_embedder("MOCK"));
    embed_fn
        .embed("mock/json", "[1]", &Options::new(), &|| false)
        .unwrap();
    embed_fn.set_config_version(1);
    embed_fn
        .embed("mock/json", "[1]", &Options::new(), &|| false)
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn go_merge_43_mock_provider_validates_model_options_and_applies_offset() {
    let embed_fn = EmbedFn::new();
    embed_fn
        .register("mock", Arc::new(crate::MockEmbedder))
        .unwrap();
    let opts = Options::from([("plus".into(), serde_json::json!(2.5))]);
    assert_eq!(
        embed_fn
            .embed("mock/json", "[1,2]", &opts, &|| false)
            .unwrap(),
        [3.5, 4.5]
    );
    assert!(
        embed_fn
            .embed("mock/unknown", "[1]", &Options::new(), &|| false)
            .is_err()
    );
    assert!(
        embed_fn
            .embed(
                "mock/json",
                "[1]",
                &Options::from([("unexpected".into(), serde_json::json!(true))]),
                &|| false
            )
            .is_err()
    );
    assert_eq!(
        embed_fn
            .embed(
                "mock/json",
                "[5]",
                &Options::from([("delay".into(), serde_json::json!("1ms"))]),
                &|| false
            )
            .unwrap(),
        [5.0]
    );
}

#[test]
fn go_merge_43_new_request_avoids_cancelled_batch() {
    let embed_fn = EmbedFn::new();
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("mock", provider.clone()).unwrap();
    let checks = AtomicUsize::new(0);
    assert!(
        embed_fn
            .embed("mock/json", "[1]", &Options::new(), &|| {
                checks.fetch_add(1, Ordering::SeqCst) > 0
            })
            .is_err()
    );
    assert_eq!(
        embed_fn
            .embed("mock/json", "[2]", &Options::new(), &|| false)
            .unwrap(),
        [2.0]
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn go_merge_43_full_batch_dispatches_before_window_expires() {
    let embed_fn = Arc::new(EmbedFn::new_with_config(Duration::from_secs(1), 2));
    let provider = Arc::new(TestEmbedder {
        calls: AtomicUsize::new(0),
        texts: AtomicUsize::new(0),
    });
    embed_fn.register("mock", provider.clone()).unwrap();
    let started = std::time::Instant::now();
    let jobs = ["[1]", "[2]"]
        .into_iter()
        .map(|text| {
            let embed_fn = Arc::clone(&embed_fn);
            thread::spawn(move || embed_fn.embed("mock/json", text, &Options::new(), &|| false))
        })
        .collect::<Vec<_>>();
    for job in jobs {
        assert!(job.join().unwrap().is_ok());
    }
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.texts.load(Ordering::SeqCst), 2);
}

#[test]
fn mock_json_null_is_an_empty_vector() {
    let provider = crate::MockEmbedder;
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "json",
                &["null".into()],
                &Options::new()
            )
            .unwrap(),
        vec![Vec::<f32>::new()]
    );
}

#[derive(Default)]
struct RecordingProvider {
    requests: std::sync::Mutex<Vec<(String, Vec<String>, Options)>>,
    error: std::sync::Mutex<Option<String>>,
    panic: AtomicBool,
    wrong_count: AtomicUsize,
    block: AtomicBool,
    entered: AtomicBool,
    observed_cancel: AtomicBool,
}

impl Embedder for RecordingProvider {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        self.requests
            .lock()
            .unwrap()
            .push((model.into(), texts.to_vec(), opts.clone()));
        self.entered.store(true, Ordering::Release);
        while self.block.load(Ordering::Acquire) {
            if cancel.load(Ordering::Acquire) {
                self.observed_cancel.store(true, Ordering::Release);
                return Err("context canceled".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.panic.load(Ordering::Acquire), "provider panic");
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        if self.wrong_count.load(Ordering::Acquire) == 1 {
            return Ok(vec![]);
        }
        if self.wrong_count.load(Ordering::Acquire) == 2 {
            return Ok(vec![vec![1.0]; texts.len() + 1]);
        }
        Ok(texts
            .iter()
            .map(|text| vec![text.parse::<f32>().unwrap_or(text.len() as f32)])
            .collect())
    }
}

fn batch_fixture(max: usize, window: Duration) -> (Arc<EmbedFn>, Arc<RecordingProvider>) {
    let batch = Arc::new(EmbedFn::new_with_config(window, max));
    let provider = Arc::new(RecordingProvider::default());
    batch.register("test", provider.clone()).unwrap();
    (batch, provider)
}

// The second cancellation poll happens after enqueuing, making cancellation,
// options reuse, and cross-caller ordering deterministic without sleep guesses.
fn start_batch_call(
    batch: Arc<EmbedFn>,
    texts: Vec<String>,
    opts: Options,
    cancel: Arc<AtomicBool>,
) -> (
    thread::JoinHandle<Result<Vec<Vec<f32>>, String>>,
    std::sync::mpsc::Receiver<()>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let job = thread::spawn(move || {
        let polls = AtomicUsize::new(0);
        batch.create_embeddings("test/model", &texts, &opts, &|| {
            if polls.fetch_add(1, Ordering::SeqCst) == 1 {
                tx.send(()).unwrap();
            }
            cancel
                .load(Ordering::Acquire)
                .then(|| "caller cause".into())
        })
    });
    (job, rx)
}

#[test]
fn batch_multi_text_chunks_preserve_caller_boundaries_and_isolation() {
    let (batch, provider) = batch_fixture(3, Duration::from_secs(60));
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["1".into(), "2".into()],
        Options::new(),
        Arc::new(AtomicBool::new(false)),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = batch
        .create_embeddings(
            " TEST / model ",
            &["3".into(), "4".into()],
            &Options::new(),
            &|| None,
        )
        .unwrap();
    let mut first = first.join().unwrap().unwrap();
    assert_eq!(first, vec![vec![1.0], vec![2.0]]);
    assert_eq!(second, vec![vec![3.0], vec![4.0]]);
    first.push(vec![999.0]);
    first[0][0] = 999.0;
    assert_eq!(second[0], [3.0]);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1, ["1", "2", "3"]);
    assert_eq!(requests[1].1, ["4"]);
}

#[test]
fn batch_exact_limit_and_large_single_request_dispatch_immediately() {
    for size in [3, 5] {
        let (batch, provider) = batch_fixture(3, Duration::from_secs(60));
        let texts = (1..=size).map(|n| n.to_string()).collect::<Vec<_>>();
        let begin = std::time::Instant::now();
        let result = batch
            .create_embeddings("test/model", &texts, &Options::new(), &|| None)
            .unwrap();
        assert!(begin.elapsed() < Duration::from_secs(2));
        assert_eq!(result.len(), size);
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), size.div_ceil(3));
        assert_eq!(
            requests
                .iter()
                .flat_map(|r| r.1.clone())
                .collect::<Vec<_>>(),
            texts
        );
        assert!(requests.iter().all(|r| r.1.len() <= 3));
    }
}

#[test]
fn concurrent_batch_requests_share_one_provider_request() {
    let (batch, provider) = batch_fixture(128, Duration::from_secs(60));
    let barrier = Arc::new(std::sync::Barrier::new(128));
    let jobs = (0..128)
        .map(|n| {
            let batch = batch.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                assert_eq!(
                    batch
                        .create_embeddings("test/model", &[n.to_string()], &Options::new(), &|| {
                            None
                        })
                        .unwrap(),
                    vec![vec![n as f32]]
                );
            })
        })
        .collect::<Vec<_>>();
    for job in jobs {
        job.join().unwrap();
    }
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[test]
fn batch_models_options_and_providers_are_partitioned() {
    for different in ["model", "opts", "provider", "numeric_type"] {
        let (batch, provider) = batch_fixture(10, Duration::from_millis(80));
        batch.register("other", provider.clone()).unwrap();
        let first_opts = Options::from([("plus".into(), serde_json::json!(1))]);
        let (first, ready) = start_batch_call(
            batch.clone(),
            vec!["1".into()],
            first_opts.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let model = match different {
            "model" => "test/other",
            "provider" => "other/model",
            _ => "test/model",
        };
        let opts = match different {
            "opts" => Options::from([("plus".into(), serde_json::json!(2))]),
            "numeric_type" => Options::from([("plus".into(), serde_json::json!(1.0))]),
            _ => first_opts,
        };
        batch
            .create_embeddings(model, &["2".into()], &opts, &|| None)
            .unwrap();
        first.join().unwrap().unwrap();
        assert_eq!(provider.requests.lock().unwrap().len(), 2, "{different}");
    }
    let (batch, provider) = batch_fixture(2, Duration::from_secs(60));
    let opts = Options::from([("nested".into(), serde_json::json!({"dimensions":128}))]);
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["1".into()],
        opts.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    batch
        .create_embeddings("test/model", &["2".into()], &opts, &|| None)
        .unwrap();
    first.join().unwrap().unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[test]
fn batch_snapshot_survives_cancellation_and_reused_inputs() {
    let (batch, provider) = batch_fixture(3, Duration::from_millis(100));
    let mut opts = Options::from([("nested".into(), serde_json::json!({"dimensions":128}))]);
    let cancel = Arc::new(AtomicBool::new(false));
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["private".into()],
        opts.clone(),
        cancel.clone(),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let (second, ready) = start_batch_call(
        batch.clone(),
        vec!["2".into()],
        opts.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    // dispatch can happen before this receive, but the snapshot is owned in both paths.
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.store(true, Ordering::Release);
    let _ = first.join().unwrap();
    opts.get_mut("nested").unwrap()["dimensions"] = serde_json::json!(512);
    assert_eq!(second.join().unwrap().unwrap(), vec![vec![2.0]]);
    assert!(
        provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.2["nested"]["dimensions"] == 128)
    );
}

#[test]
fn batch_cancellation_filters_private_texts_and_skips_empty_batches() {
    let (batch, provider) = batch_fixture(3, Duration::from_millis(100));
    let cancel = Arc::new(AtomicBool::new(false));
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["private".into()],
        Options::new(),
        cancel.clone(),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let (second, ready) = start_batch_call(
        batch.clone(),
        vec!["2".into()],
        Options::new(),
        Arc::new(AtomicBool::new(false)),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap().unwrap_err(), "caller cause");
    assert_eq!(second.join().unwrap().unwrap(), vec![vec![2.0]]);
    assert_eq!(provider.requests.lock().unwrap()[0].1, ["2"]);
    let (batch, provider) = batch_fixture(3, Duration::from_secs(60));
    let cancel = Arc::new(AtomicBool::new(false));
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["private".into()],
        Options::new(),
        cancel.clone(),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    cancel.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap().unwrap_err(), "caller cause");
    batch.close();
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[test]
fn batch_provider_cancels_only_after_all_callers_cancel() {
    let (batch, provider) = batch_fixture(2, Duration::from_secs(60));
    provider.block.store(true, Ordering::Release);
    let first_cancel = Arc::new(AtomicBool::new(false));
    let second_cancel = Arc::new(AtomicBool::new(false));
    let (first, ready) = start_batch_call(
        batch.clone(),
        vec!["1".into()],
        Options::new(),
        first_cancel.clone(),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let (second, ready) = start_batch_call(
        batch.clone(),
        vec!["2".into()],
        Options::new(),
        second_cancel.clone(),
    );
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..1000 {
        if provider.entered.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(provider.entered.load(Ordering::Acquire));
    first_cancel.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap().unwrap_err(), "caller cause");
    assert!(!provider.observed_cancel.load(Ordering::Acquire));
    second_cancel.store(true, Ordering::Release);
    assert_eq!(second.join().unwrap().unwrap_err(), "caller cause");
    batch.close();
    assert!(provider.observed_cancel.load(Ordering::Acquire));
}

#[test]
fn batch_provider_panics_errors_and_wrong_counts_complete_all_callers() {
    for mode in ["panic", "error", "empty", "count"] {
        let (batch, provider) = batch_fixture(2, Duration::from_secs(60));
        match mode {
            "panic" => provider.panic.store(true, Ordering::Release),
            "error" => *provider.error.lock().unwrap() = Some("API error".into()),
            "empty" => provider.wrong_count.store(1, Ordering::Release),
            _ => provider.wrong_count.store(2, Ordering::Release),
        }
        let (first, ready) = start_batch_call(
            batch.clone(),
            vec!["1".into()],
            Options::new(),
            Arc::new(AtomicBool::new(false)),
        );
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = batch
            .create_embeddings("test/model", &["2".into()], &Options::new(), &|| None)
            .unwrap_err();
        let first = first.join().unwrap().unwrap_err();
        assert_eq!(first, second);
        let expected = match mode {
            "panic" => "embedding batch processing panicked",
            "error" => "API error",
            "empty" => "no embeddings returned for model model",
            _ => "embedding provider returned 3 embeddings for 2 texts",
        };
        assert_eq!(second, expected);
    }
}

#[test]
fn batch_validation_and_custom_cause_precedence_match_provider_contract() {
    let batch = EmbedFn::new();
    let provider = Arc::new(RecordingProvider::default());
    assert!(batch.register("", provider.clone()).is_err());
    assert!(batch.register("bad/name", provider.clone()).is_err());
    batch.register("zeta", provider.clone()).unwrap();
    batch.register(" ALPHA ", provider.clone()).unwrap();
    assert!(batch.has_embedder("alpha"));
    assert!(batch.register("alpha", provider.clone()).is_err());
    assert_eq!(
        batch
            .create_embeddings("unknown/model", &["1".into()], &Options::new(), &|| None)
            .unwrap_err(),
        "unknown embedding provider 'unknown', available providers: alpha, zeta"
    );
    assert!(
        batch
            .create_embeddings("invalid", &["1".into()], &Options::new(), &|| None)
            .unwrap_err()
            .contains("format 'provider/model'")
    );
    assert!(
        batch
            .create_embeddings("invalid", &[], &Options::new(), &|| Some("cause".into()))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        batch
            .create_embeddings("invalid", &["1".into()], &Options::new(), &|| Some(
                "custom cause".into()
            ))
            .unwrap_err(),
        "custom cause"
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    let (batch, provider) = batch_fixture(1, Duration::from_secs(60));
    let result = batch.create_embeddings("test/model", &["1".into()], &Options::new(), &|| {
        provider
            .entered
            .load(Ordering::Acquire)
            .then(|| "custom cause".into())
    });
    assert_eq!(result.unwrap_err(), "custom cause");
}

#[test]
fn batch_window_closes_and_options_keys_have_fixed_size_digest() {
    use crate::embed_fn::new_batch_key;
    let first = Options::from([
        ("a".into(), serde_json::json!(1)),
        ("b".into(), serde_json::json!(2)),
    ]);
    let second = Options::from([
        ("b".into(), serde_json::json!(2)),
        ("a".into(), serde_json::json!(1)),
    ]);
    assert_eq!(
        new_batch_key("test", "model", &first).unwrap(),
        new_batch_key("test", "model", &second).unwrap()
    );
    let large = Options::from([("value".into(), serde_json::json!("x".repeat(65536)))]);
    assert_eq!(
        new_batch_key("test", "model", &large)
            .unwrap()
            .options_digest
            .len(),
        32
    );
    let (batch, provider) = batch_fixture(10, Duration::from_millis(10));
    for _ in 0..2 {
        batch
            .create_embeddings("test/model", &["1".into()], &Options::new(), &|| None)
            .unwrap();
    }
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[test]
fn mock_provider_rejects_invalid_inputs_and_handles_duration_boundaries() {
    let provider = crate::MockEmbedder;
    for (opts, expected) in [
        (
            Options::from([("unknown".into(), serde_json::json!(true))]),
            "unknown option",
        ),
        (
            Options::from([("plus".into(), serde_json::json!("x"))]),
            "invalid type for 'plus'",
        ),
        (
            Options::from([("delay".into(), serde_json::json!(1))]),
            "invalid type for 'delay'",
        ),
        (
            Options::from([("delay".into(), serde_json::json!("no"))]),
            "invalid delay duration",
        ),
        (
            Options::from([("delay".into(), serde_json::json!("999999999999999999999h"))]),
            "invalid delay duration",
        ),
    ] {
        assert!(
            provider
                .create_embeddings(&AtomicBool::new(false), "json", &["[1]".into()], &opts)
                .unwrap_err()
                .contains(expected)
        );
    }
    for delay in ["0", "-1s", "+1us", "1ms2us", ".5us"] {
        assert_eq!(
            provider
                .create_embeddings(
                    &AtomicBool::new(false),
                    "json",
                    &["[1]".into()],
                    &Options::from([("delay".into(), serde_json::json!(delay))])
                )
                .unwrap(),
            vec![vec![1.0]]
        );
    }
    assert!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "json",
                &["bad".into()],
                &Options::new()
            )
            .is_err()
    );
    assert!(
        provider
            .create_embeddings(
                &AtomicBool::new(true),
                "json",
                &["[1]".into()],
                &Options::from([("delay".into(), serde_json::json!("1s"))])
            )
            .is_err()
    );
}

#[test]
fn panicking_cancellation_observer_releases_pending_provider() {
    let (batch, provider) = batch_fixture(2, Duration::from_secs(60));
    let polls = AtomicUsize::new(0);
    let result = batch.create_embeddings("test/model", &["1".into()], &Options::new(), &|| {
        assert_eq!(polls.fetch_add(1, Ordering::SeqCst), 0, "observer panic");
        None
    });
    assert_eq!(result.unwrap_err(), "context canceled");
    batch.close();
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[test]
fn mock_duration_parse_observes_signed_nanosecond_limits() {
    let provider = crate::MockEmbedder;
    for delay in ["9223372036854775808ns", "-9223372036854775809ns"] {
        assert!(
            provider
                .create_embeddings(
                    &AtomicBool::new(true),
                    "json",
                    &["[1]".into()],
                    &Options::from([("delay".into(), serde_json::json!(delay))])
                )
                .unwrap_err()
                .contains("invalid delay duration")
        );
    }
    for delay in [
        "9223372036854775807ns",
        "-9223372036854775808ns",
        "-0",
        "+0",
    ] {
        let result = provider.create_embeddings(
            &AtomicBool::new(true),
            "json",
            &["[1]".into()],
            &Options::from([("delay".into(), serde_json::json!(delay))]),
        );
        assert_eq!(result.unwrap_err(), "context canceled");
    }
}

#[test]
fn mock_without_delay_does_not_check_context_like_go_provider() {
    assert_eq!(
        crate::MockEmbedder
            .create_embeddings(
                &AtomicBool::new(true),
                "json",
                &["[1]".into()],
                &Options::new()
            )
            .unwrap(),
        vec![vec![1.0]]
    );
}

#[test]
fn mock_json_null_elements_decode_as_zero_and_float_overflow_fails() {
    let provider = crate::MockEmbedder;
    assert_eq!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "json",
                &["[null,1]".into()],
                &Options::new()
            )
            .unwrap(),
        vec![vec![0.0, 1.0]]
    );
    assert!(
        provider
            .create_embeddings(
                &AtomicBool::new(false),
                "json",
                &["[1e100]".into()],
                &Options::new()
            )
            .is_err()
    );
}

#[test]
fn shared_embedding_call_keeps_first_context_values_and_cancellation_cause() {
    struct TraceProvider(std::sync::mpsc::Sender<String>, Arc<AtomicBool>);
    impl Embedder for TraceProvider {
        fn create_embeddings(
            &self,
            _: &AtomicBool,
            _: &str,
            _: &[String],
            _: &Options,
        ) -> Result<Vec<Vec<f32>>, String> {
            panic!("context boundary required")
        }
        fn create_embeddings_with_values(
            &self,
            cancel: &AtomicBool,
            _: &str,
            texts: &[String],
            _: &Options,
            values: &crate::embed_fn::ContextValues,
        ) -> Result<Vec<Vec<f32>>, String> {
            let trace = values
                .get("trace")
                .and_then(|value| value.downcast_ref::<String>())
                .cloned()
                .unwrap_or_default();
            self.0.send(trace).unwrap();
            while !self.1.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    return Err("context canceled".into());
                }
                thread::sleep(Duration::from_millis(1));
            }
            Ok(texts.iter().map(|_| vec![1.0, 2.0]).collect())
        }
    }
    let runtime = Arc::new(EmbedFn::new());
    let (sent, received) = std::sync::mpsc::channel();
    let release = Arc::new(AtomicBool::new(false));
    runtime
        .register("trace", Arc::new(TraceProvider(sent, release.clone())))
        .unwrap();
    let first_runtime = runtime.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let first_cancel = cancel.clone();
    let first = thread::spawn(move || {
        let mut values = crate::embed_fn::ContextValues::new();
        values.insert("trace".into(), Arc::new("first-caller-trace".to_owned()));
        first_runtime.embed_with_context_values(
            "trace/model",
            "text",
            &Options::new(),
            &|| {
                first_cancel
                    .load(Ordering::Acquire)
                    .then(|| "caller cause".into())
            },
            &values,
        )
    });
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        "first-caller-trace"
    );
    let second_runtime = runtime.clone();
    let polls = Arc::new(AtomicUsize::new(0));
    let second_polls = polls.clone();
    let second = thread::spawn(move || {
        let mut values = crate::embed_fn::ContextValues::new();
        values.insert("trace".into(), Arc::new("second-caller-trace".to_owned()));
        second_runtime.embed_with_context_values(
            "trace/model",
            "text",
            &Options::new(),
            &|| {
                second_polls.fetch_add(1, Ordering::AcqRel);
                None
            },
            &values,
        )
    });
    // The second cancellation poll happens after joining the in-flight call.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while polls.load(Ordering::Acquire) < 2 {
        assert!(std::time::Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    cancel.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap().unwrap_err(), "caller cause");
    release.store(true, Ordering::Release);
    assert_eq!(second.join().unwrap().unwrap(), vec![1.0, 2.0]);
    assert!(
        received.try_recv().is_err(),
        "both callers must share one provider call"
    );
    runtime.close();
}
