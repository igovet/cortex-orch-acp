use super::{tests::*, *};
#[test]
fn denial_expansion_matches_the_oracle_path_patterns() {
    for (path, pattern, expected) in [
        ("src/a.rs", "**/*.rs", true),
        ("src/a.rs", "src", true),
        ("src", "src/**", true),
        ("a/x/y", "a/*", true),
        ("routes/[locale]/a", "routes/[locale]", true),
        ("a/z", "a/[xy]", false),
        ("a/x", "a/[xy]", true),
        ("a/z", "a/[!xy]", true),
        ("literal[", "literal[", true),
        ("abc", "z*", false),
        ("a", "**/*", true),
    ] {
        assert_eq!(policy::matches(path, pattern), expected, "{path} {pattern}");
    }
    let f = Fixture::new();
    let root = f.root();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/no.txt"), "no").unwrap();
    let found = policy::expand(&root, &[root.join("**/*.txt")]).unwrap();
    assert_eq!(found, vec![root.join("src/no.txt")]);
}

#[test]
fn linked_worktree_git_dirs_are_read_from_the_common_git_files() {
    let f = Fixture::new();
    let main = f.0.join("main");
    let linked = f.0.join("linked");
    let admin = main.join(".git/worktrees/linked");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir(&linked).unwrap();
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    let dirs = policy::parsed_git_dirs(&linked).unwrap();
    assert_eq!(
        dirs,
        vec![admin.canonicalize().unwrap(), main.join(".git").canonicalize().unwrap()]
    );
    fs::write(linked.join(".git"), "not a pointer").unwrap();
    assert!(policy::parsed_git_dirs(&linked).is_err());
}
