use empath_server::Server;

use smol::future;

#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

fn main() -> std::io::Result<()> {
    let (s, ctrl_c) = async_channel::bounded(100);

    smol::block_on(async {
        ctrlc::set_handler(move || {
            s.try_send(()).ok();
        })
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;

        let server = Server::from_config("./empath.config.toml")?;
        println!("{}", toml::to_string(&server).unwrap());
        if let Err(err) = future::race(server.run(), async {
            ctrl_c
                .recv()
                .await
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::ConnectionAborted).into())
        })
        .await
        {
            eprintln!("{err:#?}",);
        }

        println!("Shutting down...");

        Ok(())
    })
}
