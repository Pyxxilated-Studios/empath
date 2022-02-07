use mailparse::parse_mail;
use smtp::{server::Server, SMTPServer, State};

fn main() -> std::io::Result<()> {
    SMTPServer! {
        LISTEN 1025

        Ehlo |_context| {
            Ok(())
        }

        Data |_context| {
            Ok(())
        }

        DataReceived |context| {
            let parsed = parse_mail(context.message.as_bytes());
            let parsed = parsed.unwrap();
            let headers = parsed.get_headers();
            println!("Data: {headers:#?}", );
            Ok(())
        }

        Quit |_| {
            Ok(())
        }
    }
    .run()?;

    Ok(())
}
