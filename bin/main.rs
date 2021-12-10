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

        DataReceived |_context| {
            // let parsed = parse_mail(context.message.as_bytes());
            // println!("Data: {:#?}", parsed);
            Ok(())
        }

        Quit |_| {
            Ok(())
        }
    }
    .run()?;

    Ok(())
}
