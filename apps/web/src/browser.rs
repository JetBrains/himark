use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[allow(non_camel_case_types)]
type EMSCRIPTEN_WEBSOCKET_T = std::ffi::c_int;

#[repr(C)]
struct EmscriptenWebSocketCreateAttributes {
    url: *const std::ffi::c_char,
    protocols: *const std::ffi::c_char,
    create_on_main_thread: bool,
}

#[repr(C)]
struct EmscriptenWebSocketOpenEvent {
    socket: EMSCRIPTEN_WEBSOCKET_T,
}

#[repr(C)]
struct EmscriptenWebSocketMessageEvent {
    socket: EMSCRIPTEN_WEBSOCKET_T,
    data: *const u8,
    number_of_bytes: u32,
    is_text: bool,
}

#[repr(C)]
struct EmscriptenWebSocketErrorEvent {
    socket: EMSCRIPTEN_WEBSOCKET_T,
}

#[repr(C)]
struct EmscriptenWebSocketCloseEvent {
    socket: EMSCRIPTEN_WEBSOCKET_T,
    was_clean: bool,
    code: u16,
    reason: [std::ffi::c_char; 512],
}

const MAIN_THREAD: *mut std::ffi::c_void = 0x1 as *mut std::ffi::c_void;

extern "C" {
    fn emscripten_websocket_new(
        attributes: *const EmscriptenWebSocketCreateAttributes,
    ) -> EMSCRIPTEN_WEBSOCKET_T;
    fn emscripten_websocket_set_onopen_callback_on_thread(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        user_data: *mut std::ffi::c_void,
        callback: Option<
            unsafe extern "C" fn(
                std::ffi::c_int,
                *const EmscriptenWebSocketOpenEvent,
                *mut std::ffi::c_void,
            ) -> bool,
        >,
        thread: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
    fn emscripten_websocket_set_onmessage_callback_on_thread(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        user_data: *mut std::ffi::c_void,
        callback: Option<
            unsafe extern "C" fn(
                std::ffi::c_int,
                *const EmscriptenWebSocketMessageEvent,
                *mut std::ffi::c_void,
            ) -> bool,
        >,
        thread: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
    fn emscripten_websocket_set_onerror_callback_on_thread(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        user_data: *mut std::ffi::c_void,
        callback: Option<
            unsafe extern "C" fn(
                std::ffi::c_int,
                *const EmscriptenWebSocketErrorEvent,
                *mut std::ffi::c_void,
            ) -> bool,
        >,
        thread: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
    fn emscripten_websocket_set_onclose_callback_on_thread(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        user_data: *mut std::ffi::c_void,
        callback: Option<
            unsafe extern "C" fn(
                std::ffi::c_int,
                *const EmscriptenWebSocketCloseEvent,
                *mut std::ffi::c_void,
            ) -> bool,
        >,
        thread: *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
    fn emscripten_websocket_send_utf8_text(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        text: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn emscripten_websocket_close(
        socket: EMSCRIPTEN_WEBSOCKET_T,
        code: u16,
        reason: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
}

#[derive(Default)]
struct Shared {
    open: bool,
    failed: bool,
    closed: bool,
    inbox: VecDeque<String>,

    waker: Option<Waker>,
}

impl Shared {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

pub struct BrowserTransport {
    socket: EMSCRIPTEN_WEBSOCKET_T,
    shared: Arc<Mutex<Shared>>,

    anchor: *const Mutex<Shared>,
}

unsafe impl Send for BrowserTransport {}

unsafe extern "C" fn on_open(
    _event_type: std::ffi::c_int,
    _event: *const EmscriptenWebSocketOpenEvent,
    user_data: *mut std::ffi::c_void,
) -> bool {
    let shared = &*(user_data as *const Mutex<Shared>);
    if let Ok(mut held) = shared.lock() {
        held.open = true;
        held.wake();
    }
    true
}

unsafe extern "C" fn on_message(
    _event_type: std::ffi::c_int,
    event: *const EmscriptenWebSocketMessageEvent,
    user_data: *mut std::ffi::c_void,
) -> bool {
    let event = &*event;
    if !event.is_text {
        return true;
    }
    let bytes = std::slice::from_raw_parts(event.data, event.number_of_bytes as usize);

    let bytes = match bytes.last() {
        Some(0) => &bytes[..bytes.len() - 1],
        _ => bytes,
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return true;
    };
    let shared = &*(user_data as *const Mutex<Shared>);
    if let Ok(mut held) = shared.lock() {
        held.inbox.push_back(text.to_owned());
        held.wake();
    }
    true
}

unsafe extern "C" fn on_error(
    _event_type: std::ffi::c_int,
    _event: *const EmscriptenWebSocketErrorEvent,
    user_data: *mut std::ffi::c_void,
) -> bool {
    let shared = &*(user_data as *const Mutex<Shared>);
    if let Ok(mut held) = shared.lock() {
        held.failed = true;
        held.wake();
    }
    true
}

unsafe extern "C" fn on_close(
    _event_type: std::ffi::c_int,
    _event: *const EmscriptenWebSocketCloseEvent,
    user_data: *mut std::ffi::c_void,
) -> bool {
    let shared = &*(user_data as *const Mutex<Shared>);
    if let Ok(mut held) = shared.lock() {
        held.closed = true;
        held.wake();
    }
    true
}

struct SharedWait<F> {
    shared: Arc<Mutex<Shared>>,
    ready: F,
}

impl<T, F: Fn(&mut Shared) -> Option<T> + Unpin> Future for SharedWait<F> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        let Ok(mut held) = self.shared.lock() else {
            panic!("browser websocket state poisoned");
        };
        if let Some(answer) = (self.ready)(&mut held) {
            return Poll::Ready(answer);
        }
        held.waker = Some(context.waker().clone());
        Poll::Pending
    }
}

impl BrowserTransport {
    pub async fn connect(url: &str) -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let anchor = Arc::into_raw(Arc::clone(&shared));
        let url_c = std::ffi::CString::new(url).map_err(|_| "url with NUL".to_owned())?;
        let socket = unsafe {
            let attributes = EmscriptenWebSocketCreateAttributes {
                url: url_c.as_ptr(),
                protocols: std::ptr::null(),
                create_on_main_thread: true,
            };
            let socket = emscripten_websocket_new(&attributes);
            if socket <= 0 {
                drop(Arc::from_raw(anchor));
                return Err(format!("websocket create failed ({socket})"));
            }
            let user_data = anchor as *mut std::ffi::c_void;
            emscripten_websocket_set_onopen_callback_on_thread(
                socket,
                user_data,
                Some(on_open),
                MAIN_THREAD,
            );
            emscripten_websocket_set_onmessage_callback_on_thread(
                socket,
                user_data,
                Some(on_message),
                MAIN_THREAD,
            );
            emscripten_websocket_set_onerror_callback_on_thread(
                socket,
                user_data,
                Some(on_error),
                MAIN_THREAD,
            );
            emscripten_websocket_set_onclose_callback_on_thread(
                socket,
                user_data,
                Some(on_close),
                MAIN_THREAD,
            );
            socket
        };
        let opened = SharedWait {
            shared: Arc::clone(&shared),
            ready: |state: &mut Shared| {
                if state.open {
                    Some(Ok(()))
                } else if state.failed || state.closed {
                    Some(Err("websocket refused".to_owned()))
                } else {
                    None
                }
            },
        }
        .await;
        match opened {
            Ok(()) => Ok(Self {
                socket,
                shared,
                anchor,
            }),
            Err(error) => {
                unsafe {
                    drop(Arc::from_raw(anchor));
                }
                Err(error)
            }
        }
    }
}

impl Drop for BrowserTransport {
    fn drop(&mut self) {
        unsafe {
            let _ = emscripten_websocket_close(self.socket, 1000, std::ptr::null());
            drop(Arc::from_raw(self.anchor));
        }
    }
}

impl ahp::Transport for BrowserTransport {
    async fn send(
        &mut self,
        msg: ahp::transport::TransportMessage,
    ) -> Result<(), ahp::TransportError> {
        use ahp::transport::TransportMessage;
        let text = match msg {
            TransportMessage::Text(text) => text,
            TransportMessage::Parsed(message) => serde_json::to_string(&message)
                .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?,
            TransportMessage::Binary(bytes) => String::from_utf8(bytes)
                .map_err(|error| ahp::TransportError::Protocol(error.to_string()))?,
        };
        tracing::info!(target: "ahp_wire", line = %host_discovery::logging::brief(&text), "->");
        let text_c = std::ffi::CString::new(text)
            .map_err(|_| ahp::TransportError::Protocol("message with NUL".to_owned()))?;
        let sent = unsafe { emscripten_websocket_send_utf8_text(self.socket, text_c.as_ptr()) };
        match sent {
            0 => Ok(()),
            code => Err(ahp::TransportError::Io(format!("ws send failed ({code})"))),
        }
    }

    async fn recv(
        &mut self,
    ) -> Result<Option<ahp::transport::TransportMessage>, ahp::TransportError> {
        let received = SharedWait {
            shared: Arc::clone(&self.shared),
            ready: |state: &mut Shared| {
                if let Some(line) = state.inbox.pop_front() {
                    Some(Some(line))
                } else if state.closed || state.failed {
                    Some(None)
                } else {
                    None
                }
            },
        }
        .await;
        match received {
            Some(line) => {
                tracing::info!(target: "ahp_wire", line = %host_discovery::logging::brief(&line), "<-");
                Ok(Some(ahp::transport::TransportMessage::Text(line)))
            }
            None => Ok(None),
        }
    }
}

pub struct BrowserConnector;

impl hiahp::transport::Connector for BrowserConnector {
    fn dial(
        &self,
        url: String,
        _tag: String,
        dead: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> hiahp::transport::Dialing {
        Box::pin(async move {
            let inner = BrowserTransport::connect(&url)
                .await
                .map_err(|error| format!("agent host unreachable at {url}: {error}"))?;
            Ok(ahp::transport::BoxedTransport::new(
                hiahp::transport::DeadLatched { inner, dead },
            ))
        })
    }
}
