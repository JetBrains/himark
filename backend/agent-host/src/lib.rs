pub mod catalog;
mod changes;
pub mod claude;
pub mod codex;
mod documents;
mod history;
pub(crate) mod http;
pub mod lock;
mod lsp;
mod pty;
mod rpc;
mod server;
mod store;
pub mod testing;
mod trace;
mod uris;

pub use server::{Host, HostConfig, LanguageServer};

pub(crate) fn uuid_v4() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let stack = &seed as *const _ as usize as u128;
    let mut bits = seed ^ stack.rotate_left(64) ^ (std::process::id() as u128) << 96;
    let mut nibbles = String::with_capacity(36);
    for index in 0..32 {
        let nibble = (bits & 0xf) as u32;
        bits = bits >> 4 | (u128::from(nibble.wrapping_mul(2654435769)) << 100);
        match index {
            8 | 12 | 16 | 20 => nibbles.push('-'),
            _ => {}
        }
        let value = match index {
            12 => 4,
            16 => 8 | (nibble & 0x3),
            _ => nibble,
        };
        nibbles.push(char::from_digit(value, 16).expect("nibble"));
    }
    nibbles
}

pub const PROTOCOL_VERSION: &str = "0.7.0";

pub const SERVER_NAME: &str = "himark-agent-host";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run(socket: Option<&std::path::Path>) -> i32 {
    run_with(socket, None, None)
}

pub fn run_with(
    socket: Option<&std::path::Path>,
    http: Option<&str>,
    web_root: Option<&std::path::Path>,
) -> i32 {
    let Some(dir) = lock::default_dir() else {
        eprintln!("[agent-host] no HOME — nowhere to put the socket");
        return 1;
    };
    let socket = socket
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| lock::socket_path(&dir));

    if lock::read_live(&dir).is_some() {
        eprintln!("[agent-host] a live host already holds {}", dir.display());
        return 0;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("agent-host")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("[agent-host] tokio runtime: {error}");
            return 1;
        }
    };
    if let Err(error) = lock::write(&dir, &socket) {
        eprintln!("[agent-host] lockfile: {error}");
        return 1;
    }

    host_discovery::logging::init("agent-host");
    tracing::info!(
        target: "ahp_host",
        socket = %socket.display(),
        build = %host_discovery::build_stamp(),
        protocol = PROTOCOL_VERSION,
        "serving"
    );
    let host = Host::new(HostConfig::default());
    if let Ok(mut held) = host.web_root.lock() {
        *held = crate::http::resolve_web_root(web_root);
    }
    if let Some(addr) = http {
        let root = host.web_root.lock().ok().and_then(|held| held.clone());
        match runtime.block_on(crate::http::start(&host, addr, root)) {
            Ok(url) => {
                host_discovery::update_http(Some(url.clone()));
                eprintln!("[agent-host] serving http on {url}");
            }
            Err(error) => {
                eprintln!("[agent-host] http bind {addr}: {error}");
                return 1;
            }
        }
    }
    eprintln!("[agent-host] serving on {}", socket.display());

    let served = runtime.block_on(async move {
        let socket = socket.clone();
        tokio::spawn(async move { host.bind(&socket).await }).await
    });
    match served {
        Ok(Ok(())) => {
            tracing::warn!(target: "ahp_host", "bind task ended cleanly (listener gone?)");
            0
        }
        Ok(Err(error)) => {
            tracing::error!(target: "ahp_host", %error, "serve error");
            eprintln!("[agent-host] serve: {error}");
            1
        }
        Err(join) => {
            tracing::error!(target: "ahp_host", error = %join, "bind task DIED");
            eprintln!("[agent-host] bind task died: {join}");
            1
        }
    }
}
