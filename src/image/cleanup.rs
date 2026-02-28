use std::sync::Arc;

pub struct ShutdownNotif {
    rx: Arc<tokio::sync::Notify>,
}

pub struct Cleanup {
    tx: Arc<tokio::sync::Notify>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.tx.notify_one();
    }
}

pub fn channel() -> (Cleanup, ShutdownNotif) {
    let notify = Arc::new(tokio::sync::Notify::new());
    let rx = ShutdownNotif { rx: notify.clone() };
    let tx = Cleanup { tx: notify.clone() };

    (tx, rx)
}

impl ShutdownNotif {
    pub async fn sleep(&self, duration: std::time::Duration) -> bool {
        tokio::select! {
            _ = self.rx.notified() => true,
            _ = tokio::time::sleep(duration) => false,
        }
    }

    pub async fn shutdown_or<T>(&self, f: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::select! {
            _ = self.rx.notified() => None,
            done = f => Some(done)
        }
    }
}
