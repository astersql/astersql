// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Embedder, Options};

/// Deterministic JSON-vector provider used by SQL and inference tests.
pub struct MockEmbedder;

impl Embedder for MockEmbedder {
    fn create_embeddings(
        &self,
        cancel: &AtomicBool,
        model: &str,
        texts: &[String],
        opts: &Options,
    ) -> Result<Vec<Vec<f32>>, String> {
        if model != "json" {
            return Err(format!("unknown model {model}"));
        }
        for option in opts.keys() {
            if option != "plus" && option != "delay" {
                return Err(format!("unknown option {option}"));
            }
        }
        let plus = match opts.get("plus") {
            None => 0.0,
            Some(value) => value
                .as_f64()
                .ok_or_else(|| "invalid type for 'plus' option".to_owned())?,
        } as f32;
        if let Some(value) = opts.get("delay") {
            let delay = value
                .as_str()
                .ok_or_else(|| "invalid type for 'delay' option".to_owned())?;
            let duration =
                parse_go_delay(delay).ok_or_else(|| format!("invalid delay duration: {delay}"))?;
            let end = Instant::now() + duration;
            while Instant::now() < end {
                if cancel.load(Ordering::Acquire) {
                    return Err("context canceled".into());
                }
                thread::sleep(
                    Duration::from_millis(5).min(end.saturating_duration_since(Instant::now())),
                );
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Err("context canceled".into());
        }
        texts
            .iter()
            .map(|text| {
                let mut vector: Vec<f32> =
                    serde_json::from_str(text).map_err(|error| error.to_string())?;
                if plus != 0.0 {
                    for value in &mut vector {
                        *value += plus;
                    }
                }
                Ok(vector)
            })
            .collect()
    }
}

fn parse_go_delay(value: &str) -> Option<Duration> {
    if value == "0" {
        return Some(Duration::ZERO);
    }
    let (negative, value) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    let mut rest = value;
    let mut nanos = 0.0_f64;
    let mut parsed = false;
    while !rest.is_empty() {
        let number_len = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())?;
        let (number, suffix) = rest.split_at(number_len);
        let number = number.parse::<f64>().ok()?;
        let (unit, factor) = [
            ("ns", 1.0),
            ("us", 1_000.0),
            ("µs", 1_000.0),
            ("μs", 1_000.0),
            ("ms", 1_000_000.0),
            ("s", 1_000_000_000.0),
            ("m", 60_000_000_000.0),
            ("h", 3_600_000_000_000.0),
        ]
        .into_iter()
        .find(|(unit, _)| suffix.starts_with(unit))?;
        nanos += number * factor;
        rest = &suffix[unit.len()..];
        parsed = true;
    }
    if !parsed || !nanos.is_finite() || nanos > u64::MAX as f64 {
        return None;
    }
    Some(if negative {
        Duration::ZERO
    } else {
        Duration::from_nanos(nanos as u64)
    })
}
