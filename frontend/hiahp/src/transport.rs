// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub type Dialing =
    Pin<Box<dyn Future<Output = Result<ahp::transport::BoxedTransport, String>> + Send>>;

pub trait Connector: Send + Sync + 'static {
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing;
}

pub struct DeadLatched<T> {
    pub inner: T,
    pub dead: Arc<AtomicBool>,
}

impl<T: ahp::transport::Transport> ahp::transport::Transport for DeadLatched<T> {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        let sent = self.inner.send(msg).await;
        if sent.is_err() {
            self.dead.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        sent
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        let received = self.inner.recv().await;
        if !matches!(&received, Ok(Some(_))) {
            self.dead.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        received
    }

    async fn close(&mut self) -> Result<(), ahp::TransportError> {
        self.inner.close().await
    }
}
