use mailparse::parse_mail;
use smtplib::{common::extensions::Extension, server::Server, state::State};

fn main() -> std::io::Result<()> {
    Server::default()
        .on_port(1026)
        .extension(Extension::STARTTLS)
        .handle(State::DataReceived, |vctx| {
            let message = vctx.message();
            let parsed = parse_mail(message.as_bytes());
            let parsed = parsed.unwrap();
            let headers = parsed.get_headers();
            println!("Headers: {headers:#?}");
            println!("FROM: {}", vctx.sender());
            println!("TO: {}", vctx.recipients());
            let body = parsed.get_body().unwrap();
            println!("Body: {body}");
            Ok(())
        })
        .run()
}
