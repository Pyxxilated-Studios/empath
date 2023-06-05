extern crate cbindgen;

use cbindgen::Config;
use std::env;
use std::path::PathBuf;

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let package_name = env::var("CARGO_PKG_NAME").unwrap();
    let package_name = package_name.split('-').collect::<Vec<_>>().join("/");
    let output_file = PathBuf::from(&crate_dir)
        .join(format!("../target/{package_name}.h"))
        .display()
        .to_string();

    let config = Config {
        language: cbindgen::Language::C,
        cpp_compat: true,
        pragma_once: true,
        autogen_warning: Some(String::from(
            "/**
 * Warning, this file is autogenerated by cbindgen. Don't modify this manually.
 * Instead, alter <crate>/empath/build.rs, or the respective rust item.
 **/",
        )),
        ..Default::default()
    };

    cbindgen::generate_with_config(crate_dir, config)
        .unwrap()
        .write_to_file(output_file);
}
