use std::path::Path;

pub use host_discovery::{default_dir, lock_path, read_live, socket_path, Lockfile};

pub fn write(dir: &Path, socket: &Path) -> std::io::Result<()> {
    host_discovery::write(dir, socket, crate::PROTOCOL_VERSION)
}
