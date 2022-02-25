use mailparse::parse_mail;
use smtplib::{common::extensions::Extension, server::Server, state::State, SMTPServer};

fn main() -> std::io::Result<()> {
    SMTPServer! {
        PORT 1026

        EXTENSIONS {
            STARTTLS
        }

        HANDLERS {
            DataReceived |vctx| {
                let message = vctx.message();
                let parsed = parse_mail(message.as_bytes());
                let parsed = parsed.unwrap();
                let headers = parsed.get_headers();
                println!("Headers: {headers:#?}");
                println!("FROM: {}", vctx.sender());
                println!("TO: {}", vctx.recipients());
                // let body = parsed.subparts[1].get_body();
                // println!("Data: {body:#?}");
                Ok(())
            }
        }
    }
    .run()
}
