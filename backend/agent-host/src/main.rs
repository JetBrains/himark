fn main() {
    let value_of = |flag: &str| {
        std::env::args()
            .skip_while(|argument| argument != flag)
            .nth(1)
    };
    let socket = value_of("--socket").map(std::path::PathBuf::from);
    let http = value_of("--http");
    let web_root = value_of("--web-root").map(std::path::PathBuf::from);
    let code = agent_host::run_with(socket.as_deref(), http.as_deref(), web_root.as_deref());
    std::process::exit(code);
}
