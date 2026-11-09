//! The folder search through the real socket and helper, and what stops it.
use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use workspace_service::{connect, read_frame, write_frame, Config, LaunchSpec, Service};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "workspace-directories-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(p.join("projects")).unwrap();
        Self(p)
    }
    fn config(&self, tools: &Path) -> Config {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("projects"),
            launch: LaunchSpec::new(PathBuf::from("/usr/bin/python3"), vec!["-B".into(), root.join("tests/fake_acp.py").display().to_string()])
                .unwrap()
                .with_tools(tools.to_path_buf())
                .unwrap(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 2,
        }
    }
    fn real(&self) -> Config {
        self.config(&Path::new(env!("CARGO_MANIFEST_DIR")).join("machine_tools.py"))
    }
    /// A helper that says nothing and records its process, as a read on a dead mount would.
    fn stuck(&self) -> Config {
        let tools = self.0.join("tools");
        std::fs::create_dir(&tools).unwrap();
        std::fs::write(tools.join("machine_tools.py"), "").unwrap();
        let pids = self.0.join("pids");
        std::fs::create_dir(&pids).unwrap();
        std::fs::write(
            tools.join("directory_search.py"),
            format!("import os, time\nopen({:?} + '/' + str(os.getpid()), 'w').close()\ntime.sleep(120)\n", pids.display().to_string()),
        )
        .unwrap();
        self.config(&tools.join("machine_tools.py"))
    }
    fn pids(&self) -> Vec<u32> {
        std::fs::read_dir(self.0.join("pids")).unwrap().map(|e| e.unwrap().file_name().to_str().unwrap().parse().unwrap()).collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("chmod").args(["-R", "u+rwx"]).arg(&self.0).status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn call(service: &Service, method: &str, params: Value) -> Value {
    let mut stream = connect(service.socket_path()).await.unwrap();
    write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":"1","method":method,"params":params})).await.unwrap();
    read_frame(&mut stream).await.unwrap()
}
async fn found(service: &Service, query: &str) -> Vec<String> {
    let reply = call(service, "directories.search", json!({"session":"pick-1","query":query})).await;
    assert_eq!(reply["result"]["query"], query, "{reply}");
    reply["result"]["entries"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_owned()).collect()
}
async fn serving(config: Config) -> (Arc<Service>, tokio::task::JoinHandle<std::io::Result<()>>) {
    let service = Service::bind(config).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    (service, runner)
}
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
fn ended(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat.rsplit(')').next().is_some_and(|tail| tail.trim_start().starts_with('Z')),
    }
}

#[tokio::test]
async fn the_machine_finds_folders_for_the_text_the_owner_typed() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    std::fs::create_dir_all(root.join("code/cedar-notes/.git")).unwrap();
    std::fs::create_dir_all(root.join("code/maple")).unwrap();
    let (s, runner) = serving(f.real()).await;
    let reply = call(&s, "directories.search", json!({"session":"pick-1","query":"cedar"})).await;
    assert_eq!(reply["result"]["entries"], json!([{"path":root.join("code/cedar-notes"),"git":true}]));
    assert_eq!(reply["result"]["complete"], true);
    let code = root.join("code");
    let listed = found(&s, &format!("{}/", code.display())).await;
    assert_eq!(listed, [code.display().to_string(), root.join("code/cedar-notes").display().to_string(), root.join("code/maple").display().to_string()]);
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_blank_query_answers_at_once_with_the_home_and_starts_nothing() {
    let f = Fixture::new();
    let (s, runner) = serving(f.stuck()).await;
    let reply = call(&s, "directories.search", json!({"session":"pick-1","query":"  "})).await;
    assert_eq!(reply["result"]["entries"], json!([]));
    assert_eq!(reply["result"]["home"], std::env::var("HOME").unwrap());
    assert!(f.pids().is_empty());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn unreadable_folders_and_link_loops_cost_nothing() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    std::fs::create_dir_all(root.join("a/cedar")).unwrap();
    std::fs::create_dir_all(root.join("locked/cedar-hidden")).unwrap();
    symlink(&root, root.join("a/loop")).unwrap();
    symlink(root.join("a/self"), root.join("a/self")).unwrap();
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0)).unwrap();
    let (s, runner) = serving(f.real()).await;
    let reply = call(&s, "directories.search", json!({"session":"pick-1","query":"cedar"})).await;
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["entries"], json!([{"path":root.join("a/cedar"),"git":false}]));
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o700)).unwrap();
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_path_through_a_parent_that_cannot_be_listed_still_resolves() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    let repo = root.join("gate/inner/repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::set_permissions(root.join("gate"), std::fs::Permissions::from_mode(0o311)).unwrap();
    let (s, runner) = serving(f.real()).await;
    assert!(!found(&s, "repo").await.contains(&repo.display().to_string()));
    let typed = found(&s, &format!("{}/gate/inner/re", root.display())).await;
    assert_eq!(typed, [repo.display().to_string()]);
    assert_eq!(s.register(&repo, "Repo").await.unwrap()["canonicalRoot"], repo.to_str().unwrap());
    std::fs::set_permissions(root.join("gate"), std::fs::Permissions::from_mode(0o700)).unwrap();
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_private_state_of_the_service_is_never_offered() {
    let f = Fixture::new();
    let mut config = f.real();
    config.allowed_root = f.0.clone();
    let (s, runner) = serving(config).await;
    assert_eq!(found(&s, &format!("{}/", f.0.join("state").display())).await, Vec::<String>::new());
    assert_eq!(found(&s, &format!("{}/", f.0.join("runtime").display())).await, Vec::<String>::new());
    assert!(found(&s, "state").await.is_empty());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelling_ends_the_process_and_answers_the_waiting_search() {
    let f = Fixture::new();
    let (s, runner) = serving(f.stuck()).await;
    let waiting = {
        let s = s.clone();
        tokio::spawn(async move { call(&s, "directories.search", json!({"session":"pick-1","query":"cedar"})).await })
    };
    until("the search to start", || !f.pids().is_empty()).await;
    let pid = f.pids()[0];
    let started = Instant::now();
    assert_eq!(call(&s, "directories.cancel", json!({"session":"pick-1"})).await["result"]["outcome"], "cancelled");
    let reply = waiting.await.unwrap();
    assert_eq!(reply["result"]["cancelled"], true);
    assert!(started.elapsed() < Duration::from_secs(2));
    until("the helper to end", || ended(pid)).await;
    assert_eq!(call(&s, "directories.cancel", json!({"session":"pick-1"})).await["result"]["outcome"], "not_running");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_newer_search_replaces_the_one_before_it() {
    let f = Fixture::new();
    let (s, runner) = serving(f.stuck()).await;
    let first = {
        let s = s.clone();
        tokio::spawn(async move { call(&s, "directories.search", json!({"session":"pick-1","query":"ce"})).await })
    };
    until("the first search to start", || f.pids().len() == 1).await;
    let pid = f.pids()[0];
    let second = {
        let s = s.clone();
        tokio::spawn(async move { call(&s, "directories.search", json!({"session":"pick-1","query":"ced"})).await })
    };
    let replaced = first.await.unwrap();
    assert_eq!(replaced["result"]["cancelled"], true);
    assert_eq!(replaced["result"]["query"], "ce");
    until("the first helper to end", || ended(pid)).await;
    until("the second search to start", || f.pids().len() == 2).await;
    assert_eq!(call(&s, "directories.cancel", json!({"session":"pick-1"})).await["result"]["outcome"], "cancelled");
    assert_eq!(second.await.unwrap()["result"]["cancelled"], true);
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_helper_that_never_answers_is_killed_and_the_machine_stays_up() {
    let f = Fixture::new();
    let (s, runner) = serving(f.stuck()).await;
    let waiting = {
        let s = s.clone();
        tokio::spawn(async move { call(&s, "directories.search", json!({"session":"pick-1","query":"cedar"})).await })
    };
    until("the search to start", || !f.pids().is_empty()).await;
    let pid = f.pids()[0];
    assert_eq!(call(&s, "service.status", json!({})).await["result"]["stopping"], false);
    let reply = waiting.await.unwrap();
    assert_eq!(reply["error"]["message"], "directory_search_timeout");
    until("the helper to end", || ended(pid)).await;
    assert_eq!(call(&s, "service.status", json!({})).await["result"]["stopping"], false);
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn stopping_the_service_ends_a_running_search() {
    let f = Fixture::new();
    let (s, runner) = serving(f.stuck()).await;
    let mut stream = connect(s.socket_path()).await.unwrap();
    write_frame(&mut stream, &json!({"jsonrpc":"2.0","id":"1","method":"directories.search","params":{"session":"pick-1","query":"cedar"}})).await.unwrap();
    until("the search to start", || !f.pids().is_empty()).await;
    let pid = f.pids()[0];
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    until("the helper to end", || ended(pid)).await;
}

#[tokio::test]
async fn a_request_that_names_more_than_text_is_refused_and_changes_nothing() {
    let f = Fixture::new();
    let (s, runner) = serving(f.real()).await;
    for params in [
        json!({"session":"pick-1"}),
        json!({"session":"pick-1","query":"x","root":"/"}),
        json!({"session":"pick-1","query":"x","deadlineMs":999999}),
        json!({"session":"pick-1","query":"x","limit":101}),
    ] {
        assert_eq!(call(&s, "directories.search", params).await["error"]["message"], "invalid_directory_search");
    }
    assert_eq!(call(&s, "service.status", json!({})).await["result"]["stopping"], false);
    let methods = call(&s, "service.capabilities", json!({})).await;
    assert!(methods["result"]["methods"].as_array().unwrap().contains(&json!("directories.search")));
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_new_folder_is_made_once_and_never_over_an_existing_one() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    std::fs::create_dir_all(root.join("taken")).unwrap();
    let (s, runner) = serving(f.real()).await;
    let made = s.register_new_folder(&root.join("fresh"), "Fresh").await.unwrap();
    assert_eq!(made["canonicalRoot"], root.join("fresh").to_str().unwrap());
    assert!(root.join("fresh").is_dir());
    assert_eq!(s.register_new_folder(&root.join("taken"), "T").await.unwrap_err().to_string(), "project_folder_exists");
    assert_eq!(s.register_new_folder(&root.join("missing/child"), "M").await.unwrap_err().to_string(), "project_folder_missing");
    assert_eq!(s.register_new_folder(&root.join(".cortex"), "P").await.unwrap_err().to_string(), "project_path_refused");
    assert!(!root.join(".cortex").exists());
    assert_eq!(s.register_new_folder(&f.0.join("state/inside"), "S").await.unwrap_err().to_string(), "project_path_refused");
    let reply = call(&s, "projects.register", json!({"root":root.join("taken"),"displayName":"T","newFolder":true})).await;
    assert_eq!(reply["error"]["message"], "project_folder_exists");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_project_keeps_a_new_name_across_a_restart() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.0.join("projects/cedar")).unwrap();
    let (s, runner) = serving(f.real()).await;
    let id = s.register(&f.0.join("projects/cedar"), "cedar").await.unwrap()["projectId"].as_str().unwrap().to_owned();
    let reply = call(&s, "projects.rename", json!({"projectId":id,"displayName":"Cedar Notes"})).await;
    assert_eq!(reply["result"]["displayName"], "Cedar Notes");
    assert_eq!(call(&s, "projects.rename", json!({"projectId":id,"displayName":" "})).await["error"]["message"], "invalid_display_name");
    assert_eq!(call(&s, "projects.rename", json!({"projectId":"wp_missing","displayName":"X"})).await["error"]["message"], "project_not_found");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    drop(s);
    let (s, runner) = serving(f.real()).await;
    assert_eq!(s.projects().await[0]["displayName"], "Cedar Notes");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
