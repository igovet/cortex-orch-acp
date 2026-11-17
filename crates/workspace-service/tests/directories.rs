//! Native folder search through the real socket. Queue cancellation/replacement tests live in directories.rs.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::Arc,
    time::Duration,
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
    fn config(&self) -> Config {
        Config {
            runtime_dir: self.0.join("runtime"),
            state_dir: self.0.join("state"),
            allowed_root: self.0.join("projects"),
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_millis(150),
            event_capacity: 2,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::process::Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.0)
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn call(service: &Service, method: &str, params: Value) -> Value {
    let mut stream = connect(service.socket_path()).await.unwrap();
    write_frame(
        &mut stream,
        &json!({"jsonrpc":"2.0","id":"1","method":method,"params":params}),
    )
    .await
    .unwrap();
    read_frame(&mut stream).await.unwrap()
}
async fn found(service: &Service, query: &str) -> Vec<String> {
    let reply = call(
        service,
        "directories.search",
        json!({"session":"pick-1","query":query}),
    )
    .await;
    assert_eq!(reply["result"]["query"], query, "{reply}");
    reply["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect()
}
async fn serving(config: Config) -> (Arc<Service>, tokio::task::JoinHandle<std::io::Result<()>>) {
    let service = Service::bind(config).await.unwrap();
    let runner = tokio::spawn(service.clone().serve());
    (service, runner)
}

#[tokio::test]
async fn the_machine_finds_folders_for_the_text_the_owner_typed() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    std::fs::create_dir_all(root.join("code/cedar-notes/.git")).unwrap();
    std::fs::create_dir_all(root.join("code/maple")).unwrap();
    let (s, runner) = serving(f.config()).await;
    let reply = call(
        &s,
        "directories.search",
        json!({"session":"pick-1","query":"cedar"}),
    )
    .await;
    assert_eq!(
        reply["result"]["entries"],
        json!([{"path":root.join("code/cedar-notes"),"git":true}])
    );
    assert_eq!(reply["result"]["complete"], true);
    let code = root.join("code");
    let listed = found(&s, &format!("{}/", code.display())).await;
    assert_eq!(
        listed,
        [
            code.display().to_string(),
            root.join("code/cedar-notes").display().to_string(),
            root.join("code/maple").display().to_string()
        ]
    );
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_blank_query_answers_at_once_with_the_home() {
    let f = Fixture::new();
    let (s, runner) = serving(f.config()).await;
    let reply = call(
        &s,
        "directories.search",
        json!({"session":"pick-1","query":"  "}),
    )
    .await;
    assert_eq!(reply["result"]["entries"], json!([]));
    assert_eq!(reply["result"]["home"], std::env::var("HOME").unwrap());
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
    let (s, runner) = serving(f.config()).await;
    let reply = call(
        &s,
        "directories.search",
        json!({"session":"pick-1","query":"cedar"}),
    )
    .await;
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(
        reply["result"]["entries"],
        json!([{"path":root.join("a/cedar"),"git":false}])
    );
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
    let (s, runner) = serving(f.config()).await;
    assert!(!found(&s, "repo")
        .await
        .contains(&repo.display().to_string()));
    let typed = found(&s, &format!("{}/gate/inner/re", root.display())).await;
    assert_eq!(typed, [repo.display().to_string()]);
    assert_eq!(
        s.register(&repo, "Repo").await.unwrap()["canonicalRoot"],
        repo.to_str().unwrap()
    );
    std::fs::set_permissions(root.join("gate"), std::fs::Permissions::from_mode(0o700)).unwrap();
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_private_state_of_the_service_is_never_offered() {
    let f = Fixture::new();
    let mut config = f.config();
    config.allowed_root = f.0.clone();
    let (s, runner) = serving(config).await;
    assert_eq!(
        found(&s, &format!("{}/", f.0.join("state").display())).await,
        Vec::<String>::new()
    );
    assert_eq!(
        found(&s, &format!("{}/", f.0.join("runtime").display())).await,
        Vec::<String>::new()
    );
    assert!(found(&s, "state").await.is_empty());
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn native_search_answers_and_the_service_stays_up() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.0.join("projects/cedar")).unwrap();
    let (s, runner) = serving(f.config()).await;
    let reply = call(
        &s,
        "directories.search",
        json!({"session":"pick-1","query":"cedar"}),
    )
    .await;
    assert_eq!(reply["result"]["complete"], true);
    assert_eq!(
        reply["result"]["entries"],
        json!([{"path":f.0.join("projects/cedar"),"git":false}])
    );
    assert_eq!(
        call(&s, "service.status", json!({})).await["result"]["stopping"],
        false
    );
    assert_eq!(
        call(&s, "directories.cancel", json!({"session":"pick-1"})).await["result"]["outcome"],
        "not_running"
    );
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn sequential_searches_keep_their_query() {
    let f = Fixture::new();
    let (s, runner) = serving(f.config()).await;
    for query in ["ce", "ced", "cedar"] {
        let reply = call(
            &s,
            "directories.search",
            json!({"session":"pick-1","query":query}),
        )
        .await;
        assert_eq!(reply["result"]["query"], query);
    }
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
#[tokio::test]
async fn a_request_that_names_more_than_text_is_refused_and_changes_nothing() {
    let f = Fixture::new();
    let (s, runner) = serving(f.config()).await;
    for params in [
        json!({"session":"pick-1"}),
        json!({"session":"pick-1","query":"x","root":"/"}),
        json!({"session":"pick-1","query":"x","deadlineMs":999999}),
        json!({"session":"pick-1","query":"x","limit":101}),
    ] {
        assert_eq!(
            call(&s, "directories.search", params).await["error"]["message"],
            "invalid_directory_search"
        );
    }
    assert_eq!(
        call(&s, "service.status", json!({})).await["result"]["stopping"],
        false
    );
    let methods = call(&s, "service.capabilities", json!({})).await;
    assert!(methods["result"]["methods"]
        .as_array()
        .unwrap()
        .contains(&json!("directories.search")));
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_new_folder_is_made_once_and_never_over_an_existing_one() {
    let f = Fixture::new();
    let root = f.0.join("projects");
    std::fs::create_dir_all(root.join("taken")).unwrap();
    let (s, runner) = serving(f.config()).await;
    let made = s
        .register_new_folder(&root.join("fresh"), "Fresh")
        .await
        .unwrap();
    assert_eq!(made["canonicalRoot"], root.join("fresh").to_str().unwrap());
    assert!(root.join("fresh").is_dir());
    assert_eq!(
        s.register_new_folder(&root.join("taken"), "T")
            .await
            .unwrap_err()
            .to_string(),
        "project_folder_exists"
    );
    assert_eq!(
        s.register_new_folder(&root.join("missing/child"), "M")
            .await
            .unwrap_err()
            .to_string(),
        "project_folder_missing"
    );
    assert_eq!(
        s.register_new_folder(&root.join(".cortex"), "P")
            .await
            .unwrap_err()
            .to_string(),
        "project_path_refused"
    );
    assert!(!root.join(".cortex").exists());
    assert_eq!(
        s.register_new_folder(&f.0.join("state/inside"), "S")
            .await
            .unwrap_err()
            .to_string(),
        "project_path_refused"
    );
    let reply = call(
        &s,
        "projects.register",
        json!({"root":root.join("taken"),"displayName":"T","newFolder":true}),
    )
    .await;
    assert_eq!(reply["error"]["message"], "project_folder_exists");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_project_keeps_a_new_name_across_a_restart() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.0.join("projects/cedar")).unwrap();
    let (s, runner) = serving(f.config()).await;
    let id = s
        .register(&f.0.join("projects/cedar"), "cedar")
        .await
        .unwrap()["projectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let reply = call(
        &s,
        "projects.rename",
        json!({"projectId":id,"displayName":"Cedar Notes"}),
    )
    .await;
    assert_eq!(reply["result"]["displayName"], "Cedar Notes");
    assert_eq!(
        call(
            &s,
            "projects.rename",
            json!({"projectId":id,"displayName":" "})
        )
        .await["error"]["message"],
        "invalid_display_name"
    );
    assert_eq!(
        call(
            &s,
            "projects.rename",
            json!({"projectId":"wp_missing","displayName":"X"})
        )
        .await["error"]["message"],
        "project_not_found"
    );
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
    drop(s);
    let (s, runner) = serving(f.config()).await;
    assert_eq!(s.projects().await[0]["displayName"], "Cedar Notes");
    s.stop().await.unwrap();
    runner.await.unwrap().unwrap();
}
