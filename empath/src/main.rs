use empath_common::internal;
use empath_server::Server;

#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let server_handle = tokio::spawn(Server::from_config("./empath.config.toml")?.run());

    tokio::signal::ctrl_c().await?;
    internal!(
        level = INFO,
        "Shutting down... (Send SIGINT again to force shut down)"
    );

    let resp = tokio::select! {
        biased;
        rc = server_handle => { rc?.map_err(|err| std::io::Error::new(std::io::ErrorKind::Interrupted, err.to_string())) }
        _ = Server::shutdown() => { Ok(()) }
        _ = tokio::signal::ctrl_c() => {
            internal!(level = INFO, "Forcefully shutting down");
           Ok(())
        }
    };

    internal!(level = INFO, "Reached target shutdown");

    resp
}
