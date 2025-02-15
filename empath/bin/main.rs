#[cfg(not(any(target_os = "macos", unix)))]
compile_error!("Only macos and unix are currently supported");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let f = std::fs::read_to_string("./empath.config.toml")?;
    let empath: empath::controller::Empath = toml::from_str(&f)?;

    empath.run().await
}
