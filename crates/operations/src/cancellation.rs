use std::sync::Arc;

use tokio::sync::watch;

#[derive(Debug, Clone)]
pub struct Cancellation(Arc<watch::Sender<bool>>);

impl Default for Cancellation {
    fn default() -> Self {
        Self(Arc::new(watch::Sender::new(false)))
    }
}

impl Cancellation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    #[must_use]
    pub fn is_canceled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn canceled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|canceled| *canceled).await;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Cancellation;

    #[tokio::test]
    async fn clones_share_the_cancellation() {
        let cancellation = Cancellation::new();
        let waiting = cancellation.clone();
        let task = tokio::spawn(async move { waiting.canceled().await });

        cancellation.cancel();

        let finished = tokio::time::timeout(Duration::from_secs(5), task).await;
        assert!(matches!(finished, Ok(Ok(()))));
        assert!(cancellation.is_canceled());
    }

    #[tokio::test]
    async fn a_cancellation_requested_before_waiting_is_not_lost() {
        let cancellation = Cancellation::new();
        cancellation.cancel();

        let finished = tokio::time::timeout(Duration::from_secs(5), cancellation.canceled()).await;

        assert!(finished.is_ok());
    }
}
