pub trait Controller {
    ///
    /// Initialise this controller
    ///
    /// # Errors
    /// Any errors initialising this controller
    ///
    fn init() -> anyhow::Result<()>;

    fn run() -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}
