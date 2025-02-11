use empath_tracing::traced;
use serde::{Deserialize, Serialize};

use empath_common::Signal;

use crate::{listener::Listener, Smtp};

#[derive(Default, Deserialize, Serialize)]
pub struct Server {
    #[serde(alias = "listener")]
    listeners: Vec<Listener<Smtp>>,
}

impl Server {
    #[traced(instrument(level = tracing::Level::TRACE, skip_all), timing(precision = "us"))]
    pub async fn serve(
        &self,
        shutdown: tokio::sync::broadcast::Receiver<Signal>,
    ) -> anyhow::Result<()> {
        futures_util::future::join_all(
            self.listeners
                .iter()
                .map(|l| l.serve(shutdown.resubscribe())),
        )
        .await;

        Ok(())
    }
}
