fn test() {
    println!("Hello world!");
}

#[no_mangle]
pub extern "C" fn init() -> isize {
    test();
    return 0;
}
