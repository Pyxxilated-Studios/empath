use empath_smtp_proto::{
    common::{extensions::Extension, status::Status},
    context::ValidationContext,
    mailparse::parse_mail,
    phase::Phase,
    server::Server,
    SMTPError,
};

use libloading::{Library, Symbol};
use smol::future;

type InitFunc = unsafe fn(&ValidationContext) -> isize;

#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

fn main() -> std::io::Result<()> {
    let (s, ctrl_c) = async_channel::bounded(100);

    ctrlc::set_handler(move || {
        s.try_send(()).ok();
    })
    .expect("Error setting Ctrl-C handler");

    smol::block_on(async {
        future::race(
            async {
                Server::default()
                    .on_port(1026)
                    .extension(Extension::STARTTLS)
                    .handle(Phase::DataReceived, |vctx| {
                        let message = vctx.message();
                        let parsed = parse_mail(message.as_bytes())?;
                        let headers = parsed.get_headers();
                        headers.into_iter().for_each(|header| {
                            println!("{}: {}", header.get_key(), header.get_value())
                        });
                        let body = parsed.get_body()?;

                        println!("{body}");

                        Ok(())
                    })
                    .handle(Phase::DataReceived, |vctx| {
                        println!("Second handler");
                        unsafe {
                            (if cfg!(target_os = "macos") {
                                Library::new("./examples/libdll.dylib")
                            } else {
                                Library::new("./examples/libdll.so")
                            })
                            .and_then(|lib| {
                                let init: Symbol<InitFunc> = lib.get(b"init")?;
                                let response = init(vctx);

                                println!("init: {response}");

                                Ok(())
                            })
                            .map_err(|err| {
                                eprintln!("{err}");
                                SMTPError {
                                    status: Status::Error,
                                    message: String::from("5.5.1 There was an internal issue"),
                                }
                            })
                        }
                    })
                    .run()
                    .await
            },
            async {
                ctrl_c
                    .recv()
                    .await
                    .map_err(|_| std::io::ErrorKind::ConnectionAborted.into())
            },
        )
        .await
        .unwrap()
    });

    println!("Shutting down...");

    Ok(())
}
