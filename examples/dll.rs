use cached::proc_macro::cached;
use serde_json::{Result, Value};

#[cached(size = 2, time = 60, result = true, time_refresh = true)]
fn untyped_example(data: String) -> Result<Value> {
    // Parse the string of data into serde_json::Value.
    let v: Value = serde_json::from_str(data.as_str())?;

    // Access parts of the data by indexing with square brackets.
    println!("Line {}", v);

    Ok(v)
}

fn main() {
    let data = String::from("[ \"21112123\", null, \"data\" ]");
    let other = String::from("[ \"21112123\", null, \"other\" ]");
    let last = String::from("[ \"21112123\", null, \"last\" ]");

    println!("{}", untyped_example(data.clone()).unwrap());
    println!("{}", untyped_example(other.clone()).unwrap());
    println!("{}", untyped_example(data.clone()).unwrap());
    println!("{}", untyped_example(data.clone()).unwrap());
    println!("{}", untyped_example(last.clone()).unwrap());
    println!("{}", untyped_example(other.clone()).unwrap());
}
