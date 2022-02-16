use mailparse::parse_mail;
use smtplib::{server::Server, SMTPServer, State};

fn main() -> std::io::Result<()> {
    let s1 = std::thread::spawn(|| {
        SMTPServer! {
            PORT 1026

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
                println!("Headers: {headers:#?}");
                // let body = parsed.subparts[1].get_body();
                // println!("Data: {body:#?}");
                Ok(())
            }

            Quit |_| {
                Ok(())
            }
        }
        .run()
    });

    let s2 = std::thread::spawn(|| {
        SMTPServer! {
            PORT 1027

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
                println!("Headers: {headers:#?}");
                // let body = parsed.subparts[1].get_body();
                // println!("Data: {body:#?}");
                Ok(())
            }

            Quit |_| {
                Ok(())
            }
        }
        .run()
    });

    s1.join().expect("")?;
    s2.join().expect("")
}
