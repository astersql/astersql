// Copyright 2026 AsterSQL.

use crate::{Options, base::*};

#[test]
fn float_decoder_preserves_empty_and_little_endian_values() {
    assert_eq!(decode_float32_array_bytes(&[]).unwrap(), Vec::<f32>::new());
    for data in [vec![0; 3], vec![0; 5]] {
        assert_eq!(
            decode_float32_array_bytes(&data).unwrap_err(),
            "invalid embedding data"
        );
    }
    assert_eq!(
        decode_float32_array_bytes(&1.5f32.to_le_bytes()).unwrap(),
        [1.5]
    );
}

#[test]
fn fixed_request_fields_override_owned_options() {
    let opts = Options::from([
        ("model".into(), serde_json::json!("wrong")),
        ("dimensions".into(), serde_json::json!(512)),
    ]);
    let fields = Options::from([("model".into(), serde_json::json!("right"))]);
    let merged = json_fields_with_options(fields, &opts);
    assert_eq!(merged["model"], "right");
    assert_eq!(merged["dimensions"], 512);
    assert_eq!(opts["model"], "wrong");
}

#[test]
fn credentials_are_redacted_before_error_truncation() {
    let text = r#"{"authorization":"Bearer secret-token","api_key":"plain-key","message":"Bearer another-secret sk-proj-super-secret-value dash\"secret"}"#;
    let sanitized = sanitize_error_text(text, &[r#"dash\"secret"#]);
    for secret in [
        "secret-token",
        "plain-key",
        "another-secret",
        "sk-proj-super-secret-value",
        r#"dash\"secret"#,
    ] {
        assert!(!sanitized.contains(secret), "{sanitized}");
    }
    assert!(sanitized.contains("[REDACTED]"));
    for field in ["TOKEN", "access_token", "api-key", "credentials"] {
        assert_eq!(
            sanitize_error_text(&format!(r#"{{"{field}":"secret"}}"#), &[]),
            format!(r#"{{"{field}":"[REDACTED]"}}"#)
        );
    }
    assert!(
        sanitize_error_text(&format!(r#"{{"api_key":"{}"}}"#, "s".repeat(4224)), &[]).len() < 100
    );
    let long = sanitize_error_text(&"s".repeat(4224), &[]);
    assert_eq!(long.len(), 4096 + "...[truncated]".len());
    assert!(long.ends_with("...[truncated]"));
}
