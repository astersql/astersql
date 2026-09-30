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
