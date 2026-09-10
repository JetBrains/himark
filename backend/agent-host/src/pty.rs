use std::io::Result;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

pub(crate) struct PtyConfig {
    pub shell: String,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

pub(crate) struct Pty {
    pub reader: std::fs::File,

    pub writer: Arc<std::fs::File>,

    pub controller: Arc<OwnedFd>,
    pub child: Arc<Mutex<Child>>,
}

pub(crate) fn spawn(config: &PtyConfig) -> Result<Pty> {
    let winsize = rustix::termios::Winsize {
        ws_row: config.rows,
        ws_col: config.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let pair = rustix_openpty::openpty(None, Some(&winsize))
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;

    let controller: OwnedFd = pair.controller;
    let user: OwnedFd = pair.user;

    if let Ok(mut termios) = rustix::termios::tcgetattr(&controller) {
        termios.input_modes |= rustix::termios::InputModes::IUTF8;
        let _ = rustix::termios::tcsetattr(
            &controller,
            rustix::termios::OptionalActions::Now,
            &termios,
        );
    }

    let mut command = Command::new(&config.shell);

    let basename = config
        .shell
        .rsplit('/')
        .next()
        .unwrap_or(config.shell.as_str());
    if matches!(basename, "zsh" | "bash" | "sh") {
        command.arg("-l");
    }
    command
        .stdin(Stdio::from(user.try_clone()?))
        .stdout(Stdio::from(user.try_clone()?))
        .stderr(Stdio::from(user.try_clone()?))
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor");
    if let Some(cwd) = &config.cwd {
        if cwd.is_dir() {
            command.current_dir(cwd);
        }
    }
    let user_raw = user.as_raw_fd();
    let controller_raw = controller.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            libc::close(user_raw);
            libc::close(controller_raw);

            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGALRM, libc::SIG_DFL);
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(user);
    let reader = std::fs::File::from(controller.try_clone()?);
    let writer = Arc::new(std::fs::File::from(controller.try_clone()?));
    Ok(Pty {
        reader,
        writer,
        controller: Arc::new(controller),
        child: Arc::new(Mutex::new(child)),
    })
}

pub(crate) fn resize(controller: &OwnedFd, cols: u16, rows: u16) {
    let winsize = rustix::termios::Winsize {
        ws_row: rows.max(2),
        ws_col: cols.max(2),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let _ = rustix::termios::tcsetwinsize(controller, winsize);
}

pub(crate) fn take_valid_utf8(tail: &mut Vec<u8>, chunk: &[u8]) -> String {
    tail.extend_from_slice(chunk);
    let bytes = std::mem::take(tail);
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes.as_slice();
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                return out;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("validated"));
                match error.error_len() {
                    Some(bad) => {
                        out.push('\u{FFFD}');
                        rest = &after[bad..];
                    }
                    None => {
                        *tail = after.to_vec();
                        return out;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::take_valid_utf8;

    #[test]
    fn multibyte_split_across_chunks_carries_over() {
        let text = "héllo — мир";
        let bytes = text.as_bytes();
        let mut tail = Vec::new();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            out.push_str(&take_valid_utf8(&mut tail, chunk));
        }
        assert_eq!(out, text);
        assert!(tail.is_empty());
    }

    #[test]
    fn midstream_garbage_degrades_and_the_stream_continues() {
        let mut tail = Vec::new();
        let out = take_valid_utf8(&mut tail, b"ok\xFF\xFEon");
        assert_eq!(out, "ok\u{FFFD}\u{FFFD}on");
        assert!(tail.is_empty());
    }
}
