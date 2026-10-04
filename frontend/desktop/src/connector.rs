// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ahp_wire::transport::{Connector, DeadLatched, Dialing};

pub struct DesktopConnector;

impl Connector for DesktopConnector {
    #[cfg_attr(not(unix), allow(unused_variables))]
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing {
        Box::pin(async move {
            let transport = match url.strip_prefix("unix:") {
                #[cfg(unix)]
                Some(path) => ahp::transport::BoxedTransport::new(DeadLatched {
                    inner: crate::unix_transport::UnixTransport::connect(path, tag, Arc::clone(&dead)).await?,
                    dead,
                }),
                #[cfg(not(unix))]
                Some(_) => {
                    return Err(format!(
                        "agent host unreachable at {url}: unix sockets are not supported on this platform"
                    ));
                }
                None => ahp::transport::BoxedTransport::new(DeadLatched {
                    inner: ahp_ws::WebSocketTransport::connect(&url)
                        .await
                        .map_err(|error| format!("agent host unreachable at {url}: {error}"))?,
                    dead,
                }),
            };
            Ok(transport)
        })
    }
}
