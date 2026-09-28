// Copyright 2026 AsterSQL.

use rand::{CryptoRng, Error, RngCore};

use crate::common::{CtrIv16, GcmIv12, IvType, NewIVFromSlice, NewIVGcm, new_iv_gcm_with_rng};

#[derive(Debug)]
struct FailingRng;

impl RngCore for FailingRng {
    fn next_u32(&mut self) -> u32 {
        unreachable!("the implementation must use fallible random filling")
    }

    fn next_u64(&mut self) -> u64 {
        unreachable!("the implementation must use fallible random filling")
    }

    fn fill_bytes(&mut self, _dest: &mut [u8]) {
        unreachable!("the implementation must use fallible random filling")
    }

    fn try_fill_bytes(&mut self, _dest: &mut [u8]) -> Result<(), Error> {
        Err(Error::new("injected random source failure"))
    }
}

impl CryptoRng for FailingRng {}

#[test]
fn new_iv_gcm_has_go_compatible_shape() {
    let iv = NewIVGcm().expect("the operating system random source should be available");
    assert_eq!(iv.Type, IvType::IvTypeGcm);
    assert_eq!(iv.AsSlice().len(), GcmIv12);
}

#[test]
fn new_iv_gcm_propagates_random_source_errors() {
    let err = new_iv_gcm_with_rng(&mut FailingRng).unwrap_err();
    assert!(err.contains("injected random source failure"));
}

#[test]
fn new_iv_from_slice_clones_and_classifies_supported_lengths() {
    for (length, expected_type) in [(GcmIv12, IvType::IvTypeGcm), (CtrIv16, IvType::IvTypeCtr)] {
        let mut source = vec![7; length];
        let iv = NewIVFromSlice(&source).unwrap();
        source[0] = 9;

        assert_eq!(iv.Type, expected_type);
        assert_eq!(iv.AsSlice(), vec![7; length]);
    }
}

#[test]
fn new_iv_from_slice_rejects_all_other_lengths_with_go_error_shape() {
    for length in [0, GcmIv12 - 1, GcmIv12 + 1, CtrIv16 - 1, CtrIv16 + 1] {
        assert_eq!(
            NewIVFromSlice(&vec![0; length]).unwrap_err(),
            format!("invalid IV length, must be 12 or 16 bytes, got {length}")
        );
    }
}
