// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

use host_discovery::{default_dir, lock_path, read_live, socket_path, Lockfile};

pub fn write(dir: &Path, socket: &Path) -> std::io::Result<()> {
    host_discovery::write(dir, socket, crate::PROTOCOL_VERSION)
}
