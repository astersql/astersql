// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
        if opts.contains_key("delay") && cancel.load(Ordering::Acquire) {
            return Err("context canceled".into());
        }
        texts
            .iter()
            .map(|text| {
                let decoded = serde_json::from_str::<Option<Vec<Option<f64>>>>(text)
                    .map_err(|error| error.to_string())?
                    .unwrap_or_default();
                let mut vector = decoded
                    .into_iter()
                    .map(|value| {
                        let value = value.unwrap_or_default() as f32;
                        if value.is_finite() {
                            Ok(value)
                        } else {
                            Err("invalid float32 value in embedding".to_owned())
                        }
                    })
                    .collect::<Result<Vec<f32>, String>>()?;
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
    let (negative, mut rest) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    if rest == "0" {
        return Some(Duration::ZERO);
    }
    let limit = if negative {
        1u128 << 63
    } else {
        i64::MAX as u128
    };
    let mut nanos = 0u128;
    let mut parsed = false;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let integer = if digits == 0 {
            0
        } else {
            rest[..digits].parse::<u128>().ok()?
        };
        rest = &rest[digits..];
        let mut fraction = 0u128;
        let mut scale = 1u128;
        let mut fraction_digits = 0;
        if let Some(suffix) = rest.strip_prefix('.') {
            fraction_digits = suffix.bytes().take_while(u8::is_ascii_digit).count();
            for digit in suffix[..fraction_digits].bytes().take(18) {
                fraction = fraction * 10 + u128::from(digit - b'0');
                scale *= 10;
            }
            rest = &suffix[fraction_digits..];
        }
        if digits == 0 && fraction_digits == 0 {
            return None;
        }
        let (unit, factor) = [
            ("ns", 1u128),
            ("us", 1000),
            ("µs", 1000),
            ("μs", 1000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        nanos = nanos
            .checked_add(integer.checked_mul(factor)?)?
            .checked_add(fraction * factor / scale)?;
        if nanos > limit {
            return None;
        }
        rest = &rest[unit.len()..];
        parsed = true;
    }
    if !parsed {
        return None;
    }
    Some(if negative {
        Duration::ZERO
    } else {
        Duration::from_nanos(nanos as u64)
    })
}
