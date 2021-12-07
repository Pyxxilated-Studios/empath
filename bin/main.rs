use smtp_server::{server::Server, Command, SMTPServer};

fn main() -> std::io::Result<()> {
    SMTPServer! {
        LISTEN 1025

        Ehlo |context| {
            Ok(String::from("Hello! Nice to meet you!"))
        }

        Data |context| {
            Ok(String::from("Why though"))
        }

        DataReceived |context| {
            println!("Data: {}", context.message);
            Ok(String::new())
        }

        Quit |_| {
            Ok(String::new())
        }
    }
    .run()?;

    Ok(())
}
