use super::*;
use hicode::FindDefinitionEffect;
use himark::ResourceType;

fn lsp_backend() -> (tempfile::TempDir, Arc<SeatDirectory>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("backend home");
    let socket = dir.path().join("backend.sock");
    let serving = socket.clone();
    let config = agent_host::HostConfig {
        agents: Vec::new(),
        data_dir: dir.path().join("data"),
        claude_binary: "false".to_owned(),
        codex_binary: "false".to_owned(),
        claude_home: dir.path().join("dot-claude"),
        codex_home: dir.path().join("dot-codex"),
        shell: "/bin/sh".to_owned(),
        language_servers: vec![agent_host::LanguageServer {
            extensions: vec!["rs".to_owned()],
            command: agent_host::testing::fake_ls_command(dir.path()),
        }],
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("backend runtime");
        let backend = agent_host::Host::new(config);
        let _ = runtime.block_on(backend.bind(&serving));
    });
    let mut waited = 0;
    while !socket.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 500, "the backend never bound its socket");
    }
    std::fs::create_dir_all(dir.path().join("code")).expect("mkdir");
    let root = dir.path().join("code").canonicalize().expect("canonical");
    std::fs::write(root.join("lib.rs"), "fn answer() -> u32 { 42 }\n").unwrap();

    let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(), crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));

    block_on(seat.dispatch_action(
        host_discovery::LOCAL_FS_SESSION.to_owned(),
        himark::higent::ahp_types::actions::StateAction::SessionWorkingDirectorySet(
            himark::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                directory: format!("file://{}", root.display()),
            },
        ),
    ))
    .expect("the directory dispatches");

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let (server, _) = himark::higent::seat::parse(&himark::higent::seat::authority(
        {
            let (server, _) = himark::higent::seat::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    ))
    .expect("parses");
    directory.record(server, seat);
    directory.set_local(server);
    (dir, directory, root)
}

fn block_on<T>(mut future: himark::higent::SeatFuture<T>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "future never answered"
                );
                std::thread::park_timeout(std::time::Duration::from_millis(100));
            }
        }
    }
}

#[test]
fn definition_routes_over_the_seat() {
    let (_dir, directory, root) = lsp_backend();
    let location = ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("local"),
        root.join("lib.rs")
            .to_str()
            .unwrap()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>(),
    );
    let handler = DefinitionRoute {
        directory: Arc::clone(&directory),
        uris: Arc::new(crate::uris::FileUris),
    };
    let targets = block_on(Box::pin(async move {
        handler
            .handle(FindDefinitionEffect {
                folders: Vec::new(),
                location: location.clone(),
                position: LineCol { line: 0, col: 3 },
            })
            .await
    }))
    .expect("the route answers");
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].location.authority().as_str(), "local");
    assert!(targets[0]
        .location
        .path()
        .join("/")
        .ends_with("code/lib.rs"));
    assert_eq!(targets[0].range.start, LineCol { line: 1, col: 2 });
    assert_eq!(targets[0].range.end, LineCol { line: 1, col: 5 });
}

#[test]
fn unserved_locations_answer_nothing() {
    let (_dir, directory, root) = lsp_backend();
    let foreign = ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("remote:box"),
        vec!["x.rs".to_owned()],
    );
    let handler = DefinitionRoute {
        directory: Arc::clone(&directory),
        uris: Arc::new(crate::uris::FileUris),
    };
    let answered = block_on(Box::pin(async move {
        handler
            .handle(FindDefinitionEffect {
                folders: Vec::new(),
                location: foreign,
                position: LineCol { line: 0, col: 0 },
            })
            .await
    }));
    assert!(answered.is_none());

    let markdown = ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("local"),
        root.join("notes.md")
            .to_str()
            .unwrap()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>(),
    );
    let handler = DefinitionRoute {
        directory,
        uris: Arc::new(crate::uris::FileUris),
    };
    let answered = block_on(Box::pin(async move {
        handler
            .handle(FindDefinitionEffect {
                folders: Vec::new(),
                location: markdown,
                position: LineCol { line: 0, col: 0 },
            })
            .await
    }));
    assert!(answered.is_none(), "no language server serves .md");
}
