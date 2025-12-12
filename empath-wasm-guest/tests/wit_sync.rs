#![allow(clippy::expect_used)]

use empath_wasm_guest::EMPATH_WIT;

#[test]
fn wit_is_in_sync_with_host() {
    let host_wit = std::fs::read_to_string("../empath-wasm/wit/empath.wit")
        .expect("host WIT must exist for sync check");

    assert_eq!(EMPATH_WIT, host_wit, "guest WIT drifted from host copy");
}
