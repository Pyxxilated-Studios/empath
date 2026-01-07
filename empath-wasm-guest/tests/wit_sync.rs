use empath_wasm_guest::EMPATH_WIT;

#[test]
#[cfg_attr(miri, ignore)]
fn wit_is_in_sync_with_host() -> Result<(), Box<dyn std::error::Error>> {
    let host_wit = std::fs::read_to_string("../empath-ffi/wit/empath.wit")?;

    assert_eq!(EMPATH_WIT, host_wit, "guest WIT drifted from host copy");
    Ok(())
}
