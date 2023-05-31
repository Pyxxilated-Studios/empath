use entropy::{common::extensions::Extension, phase::Phase, server::Server};
use mailparse::parse_mail;

use libloading::{Library, Symbol};

type InitFunc = unsafe fn() -> isize;

fn main() -> std::io::Result<()> {
    Server::default()
        .on_port(1026)
        .extension(Extension::STARTTLS)
        .handle(Phase::DataReceived, |vctx| {
            let message = vctx.message();
            let parsed = parse_mail(message.as_bytes())?;
            let headers = parsed.get_headers();
            headers
                .into_iter()
                .for_each(|header| println!("{}: {}", header.get_key(), header.get_value()));
            let body = parsed.get_body()?;

            println!("{body}");

            Ok(())
        })
        .handle(Phase::DataReceived, |_| {
            println!("Second handler");
            unsafe {
                let lib = Library::new("./target/debug/libdll.dylib").unwrap();
                let init: Symbol<InitFunc> = lib.get(b"init").unwrap();
                let response = init();

                println!("init: {}", response);
            }

            Ok(())
        })
        .run()
}
