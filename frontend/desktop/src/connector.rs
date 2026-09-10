use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use hiahp::transport::{Connector, DeadLatched, Dialing};

pub struct DesktopConnector;

impl Connector for DesktopConnector {
    fn dial(&self, url: String, tag: String, dead: Arc<AtomicBool>) -> Dialing {
        Box::pin(async move {
            let transport = match url.strip_prefix("unix:") {
                Some(path) => ahp::transport::BoxedTransport::new(DeadLatched {
                    inner: crate::UnixTransport::connect(path, tag, Arc::clone(&dead)).await?,
                    dead,
                }),
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
