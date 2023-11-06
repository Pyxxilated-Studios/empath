use empath_common::internal;
use empath_server::Server;

#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

#[tokio::main]
async fn main() -> std::io::Result<()> {
    #[allow(clippy::redundant_pub_crate)]
    let resp = tokio::select! {
        biased;
        rc = Server::from_config("./empath.config.toml")?.run() => {
            rc.map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))
        }
        rc = tokio::signal::ctrl_c() => {
            rc
        }
    };

    internal!("Shutting down...");

    resp
}
