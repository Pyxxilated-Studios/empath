use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Server {
    hostname: String,
    listeners: Vec<Box<dyn Listener>>,
}

#[derive(Serialize, Deserialize)]
struct Esmtp {
    port: u64,
}

#[derive(Serialize, Deserialize)]
struct Http {
    path: String,
    port: u64,
}

#[typetag::serde]
trait Listener {}
#[typetag::serde]
impl Listener for Esmtp {}
#[typetag::serde]
impl Listener for Http {}

fn main() {
    let card = Server {
        hostname: "test".to_owned(),
        listeners: vec![
            Box::new(Esmtp { port: 1025 }),
            Box::new(Http {
                path: "/test".to_owned(),
                port: 80,
            }),
        ],
    };

    println!("{}", toml::to_string_pretty(&card).unwrap());

    toml::from_str::<Server>(&toml::to_string_pretty(&card).unwrap()).unwrap();
}
