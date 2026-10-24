//! One cancellation-safe owner for a connection's admission and native session.
use super::{ConnectionId, ConnectionRegistry};

pub(super) struct ConnectionLifecycle<'a> {
    registry: &'a ConnectionRegistry,
    admin: &'a crate::admin::Admin,
    id: ConnectionId,
    pub(super) cancelled: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(super) native: Option<mc_core::core::PlayerSession>,
    pub(super) writer: Option<tokio::task::JoinHandle<(&'static str, Option<String>)>>,
}

impl<'a> ConnectionLifecycle<'a> {
    pub(super) fn new(
        registry: &'a ConnectionRegistry,
        admin: &'a crate::admin::Admin,
        id: ConnectionId,
    ) -> Self {
        let (close, cancelled) = tokio::sync::oneshot::channel();
        registry.inner.lock().closers.insert(id, close);
        Self {
            registry,
            admin,
            id,
            cancelled: Some(cancelled),
            native: None,
            writer: None,
        }
    }

    /// Normal and delivery-policy exits wait for the socket half to be released
    /// before the accept loop can treat this connection as completed.
    pub(super) async fn stop_writer(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.abort();
            let _ = writer.await;
        }
    }
}

impl Drop for ConnectionLifecycle<'_> {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
        // The native session owns a reserved FIFO Leave credit: no await,
        // retry loop, or full-queue failure is possible during cancellation.
        drop(self.native.take());
        self.admin.remove(self.id);
        self.registry.finish(self.id);
    }
}
