// Copyright 2026 AsterSQL.

use crate::index_cop::Datum;
use crate::index_presplit::{get_split_keys_from_bound, get_split_keys_from_value_list};

fn go_values_list(lower: &[u8], upper: &[u8], num: usize) -> Vec<Vec<u8>> {
    let common = lower
        .iter()
        .zip(upper)
        .take_while(|(left, right)| left == right)
        .count();
    let to_u64 = |bytes: &[u8], pad: u8| {
        let mut value = [pad; 8];
        let copied = bytes.len().min(8);
        value[..copied].copy_from_slice(&bytes[..copied]);
        u64::from_be_bytes(value)
    };
    let mut start = to_u64(&lower[common..], 0);
    let end = to_u64(&upper[common..], 0xff);
    let step = end.wrapping_sub(start) / num as u64;
    (0..num.saturating_sub(1))
        .map(|_| {
            start = start.wrapping_add(step);
            let mut key = lower[..common].to_vec();
            key.extend_from_slice(&start.to_be_bytes());
            key
        })
        .collect()
}

#[test]
fn explicit_value_list_preserves_go_order_and_duplicates() {
    let keys = get_split_keys_from_value_list(
        42,
        7,
        &[
            vec![Datum::Int(2)],
            vec![Datum::Int(1)],
            vec![Datum::Int(2)],
        ],
    )
    .expect("encode explicit split values");

    assert_eq!(keys.len(), 3);
    assert!(keys[0] > keys[1]);
    assert_eq!(keys[0], keys[2]);
}

#[test]
fn bounded_split_uses_go_values_list_algorithm() {
    let table_id = 42;
    let index_id = 7;
    let lower = [Datum::Int(0)];
    let upper = [Datum::Int(100_000)];

    assert_eq!(
        get_split_keys_from_bound(table_id, index_id, &lower, &upper, 1),
        Ok(Vec::new())
    );

    let mut lower_key = b"t".to_vec();
    lower_key.extend_from_slice(&table_id.to_be_bytes());
    lower_key.push(b'i');
    lower_key.extend_from_slice(&index_id.to_be_bytes());
    lower_key.extend_from_slice(format!("{:?}", lower[0]).as_bytes());
    lower_key.push(0);

    let mut upper_key = b"t".to_vec();
    upper_key.extend_from_slice(&table_id.to_be_bytes());
    upper_key.push(b'i');
    upper_key.extend_from_slice(&index_id.to_be_bytes());
    upper_key.extend_from_slice(format!("{:?}", upper[0]).as_bytes());
    upper_key.push(0);

    let expected = go_values_list(&lower_key, &upper_key, 3);
    assert_eq!(
        get_split_keys_from_bound(table_id, index_id, &lower, &upper, 3),
        Ok(expected)
    );
}
