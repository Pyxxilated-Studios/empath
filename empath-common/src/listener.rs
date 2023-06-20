#[typetag::serde]
#[async_trait::async_trait]
pub trait Listener: Send + Sync {
    async fn spawn(&self);
}
