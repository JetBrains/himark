use crate::hiahp::find::NativeFindHandler;
use crate::hiahp::fs::SeatDirectory;
use himark::{FindEffect, FindTarget, ResourceLocation};
use imba::effect::EffectHandler;
use std::sync::Arc;
use himark::higent::seat as ahp;
use himark::ResourceType;

fn wire_backend() -> (tempfile::TempDir, Arc<dyn himark::higent::AhpServer>) {
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
        language_servers: Vec::new(),
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
    let seat: Arc<dyn himark::higent::AhpServer> = Arc::new(crate::hiahp::wire::WireHost::at(
        crate::hiahp::wire::test_runtime(), crate::test_connector(),
        format!("unix:{}", socket.display()),
    ));
    (dir, seat)
}

fn block_on<T>(mut future: std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>) -> T {
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

fn located(authority: &str, path: &std::path::Path) -> ResourceLocation {
    let segments: Vec<String> = path
        .to_str()
        .expect("utf8 path")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    ResourceLocation::new(
        ResourceType::directory(),
        himark::Authority::new(authority),
        segments,
    )
}

fn find(
    directory: &Arc<SeatDirectory>,
    folders: Vec<ResourceLocation>,
    term: &str,
    target: FindTarget,
) -> Vec<ResourceLocation> {
    let handler = NativeFindHandler {
        directory: Arc::clone(directory),
    };
    let effect = FindEffect {
        folders,
        term: term.to_owned(),
        target,
    };
    block_on(Box::pin(async move { handler.handle(effect).await }))
}

#[test]
fn session_folders_ask_their_seat() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("files/src")).expect("mkdir");
    let files = dir.path().join("files").canonicalize().expect("canonical");
    std::fs::write(files.join("src/lib.rs"), "fn main() {}\n").expect("write");
    std::fs::write(files.join("README.md"), "conflation is delivery\n").expect("write");
    std::fs::write(dir.path().join("loose.md"), "conflation outside\n").expect("write");

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let encoded = ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    );
    let (server, _) = ahp::parse(&encoded).expect("round-trips");
    directory.record(server, Arc::clone(&seat));

    let base = located(&encoded, &files);
    let hits = find(
        &directory,
        vec![base.clone()],
        "conflation",
        FindTarget::Text,
    );
    assert_eq!(hits.len(), 1, "only the in-folder hit: {hits:?}");
    assert_eq!(hits[0].authority(), base.authority(), "authority inherited");
    assert!(hits[0].kind().is_document());
    assert!(hits[0].path().join("/").ends_with("README.md"), "{hits:?}");
}

#[test]
fn local_folders_ask_the_designated_backend() {
    let (dir, seat) = wire_backend();
    std::fs::create_dir_all(dir.path().join("notes")).expect("mkdir");
    let files = dir.path().join("notes").canonicalize().expect("canonical");
    std::fs::write(files.join("a.md"), "conflation is delivery\n").expect("write");

    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let (server, _) = ahp::parse(&ahp::authority(
        {
            let (server, _) = ahp::parse("ahp:1:x").expect("id");
            server
        },
        &host_discovery::LOCAL_FS_SESSION.to_owned(),
    ))
    .expect("parses");
    directory.record(server, Arc::clone(&seat));
    directory.set_local(server);

    let base = located("local", &files);
    let hits = find(
        &directory,
        vec![base.clone()],
        "conflation",
        FindTarget::Text,
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].authority().as_str(), "local", "local identity kept");
    assert!(hits[0].path().join("/").ends_with("notes/a.md"), "{hits:?}");

    let names = find(&directory, vec![base.clone()], "am", FindTarget::Path);
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn undesignated_and_foreign_folders_answer_nothing() {
    let directory = Arc::new(SeatDirectory::new(Arc::new(|_| {})));
    let local = ResourceLocation::new(
        ResourceType::directory(),
        himark::Authority::new("local"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![local], "x", FindTarget::Text).is_empty());
    let foreign = ResourceLocation::new(
        ResourceType::directory(),
        himark::Authority::new("remote:box"),
        vec!["work".to_owned()],
    );
    assert!(find(&directory, vec![foreign], "x", FindTarget::Text).is_empty());
}
