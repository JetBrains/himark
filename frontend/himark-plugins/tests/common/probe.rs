// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The PROBE connector: the one instrument the reconnect suites
//! share. It counts dials, can hold a dial open (a slow host),
//! refuse dials (a host that is down), redirect dials to another
//! address (a host restarted behind the same name), and cut a
//! connection's socket — the things a host, a network or a
//! scheduler do to a wire.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ahp_wire::transport::{Connector, DeadLatched, Dialing};

#[derive(Default)]
pub struct Probe {
    /// Dials that went through to the socket.
    dials: AtomicUsize,
    /// Dials attempted, refused ones included.
    pub attempts: AtomicUsize,
    hold_dials: AtomicBool,
    /// The host is down: every dial fails at once.
    refuse: AtomicBool,
    /// Dials go to this url instead — another host behind the same
    /// address, as after a host restart.
    redirect: Mutex<Option<String>>,
    links: Mutex<Vec<Arc<Link>>>,
}

/// One dialed connection, in dial order.
#[derive(Default)]
pub struct Link {
    /// Inbound lines wait here before the client sees them.
    pub hold: AtomicBool,
    /// The socket breaks: reads and writes fail, the stream is
    /// dropped, the host sees EOF.
    pub cut: AtomicBool,
}

impl Probe {
    pub fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }

    pub fn gate_dials(&self, held: bool) {
        self.hold_dials.store(held, Ordering::SeqCst);
    }

    pub fn refuse_dials(&self, refused: bool) {
        self.refuse.store(refused, Ordering::SeqCst);
    }

    pub fn redirect(&self, url: String) {
        *self.redirect.lock().expect("probe redirect") = Some(url);
    }

    pub fn link(&self, index: usize) -> Arc<Link> {
        Arc::clone(&self.links.lock().expect("probe links")[index])
    }

    /// The latest dialed connection — what a restart cuts.
    pub fn last_link(&self) -> Arc<Link> {
        let links = self.links.lock().expect("probe links");
        Arc::clone(links.last().expect("a dialed connection"))
    }

    pub fn wait_for_dials(&self, count: usize, within: Duration) {
        let deadline = Instant::now() + within;
        while self.dials() < count {
            assert!(Instant::now() < deadline, "only {} dials", self.dials());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

pub struct ProbeConnector(pub Arc<Probe>);

impl Connector for ProbeConnector {
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing {
        let probe = Arc::clone(&self.0);
        Box::pin(async move {
            probe.attempts.fetch_add(1, Ordering::SeqCst);
            if probe.refuse.load(Ordering::SeqCst) {
                return Err("agent host unreachable: refused".to_owned());
            }
            while probe.hold_dials.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // A dial held open fails too when the host went down meanwhile.
            if probe.refuse.load(Ordering::SeqCst) {
                return Err("agent host unreachable: refused".to_owned());
            }
            probe.dials.fetch_add(1, Ordering::SeqCst);
            let url = probe
                .redirect
                .lock()
                .expect("probe redirect")
                .clone()
                .unwrap_or(url);
            let path = url.strip_prefix("unix:").expect("a unix url");
            let inner =
                desktop::unix_transport::UnixTransport::connect(path, tag, Arc::clone(&dead))
                    .await?;
            let link = Arc::new(Link::default());
            probe
                .links
                .lock()
                .expect("probe links")
                .push(Arc::clone(&link));
            Ok(ahp::transport::BoxedTransport::new(DeadLatched {
                inner: Probed { inner, link },
                dead,
            }))
        })
    }
}

struct Probed {
    inner: desktop::unix_transport::UnixTransport,
    link: Arc<Link>,
}

async fn cut(link: &Link) {
    while !link.cut.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

impl ahp::Transport for Probed {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        if self.link.cut.load(Ordering::SeqCst) {
            return Err(ahp::TransportError::Io("cut".to_owned()));
        }
        self.inner.send(msg).await
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        let received = tokio::select! {
            received = self.inner.recv() => received,
            () = cut(&self.link) => return Err(ahp::TransportError::Io("cut".to_owned())),
        };
        while self.link.hold.load(Ordering::SeqCst) {
            if self.link.cut.load(Ordering::SeqCst) {
                return Err(ahp::TransportError::Io("cut".to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        received
    }
}
