#[typetag::serde]
#[async_trait::async_trait]
pub trait Listener {
    async fn spawn(&self);
}
