//! The accessibility tree of the exact owned desktop process: lookups by name and role, actions, and the one tolerated case of a
//! node the page replaced while the harness held it.
//!
//! Applications are matched by the process group of the app the harness spawned, never by name, so a second window of the same
//! binary elsewhere could not be touched. A node can vanish between being found and being used; libatspi then refuses it before
//! anything is sent, so repeating the whole find-and-act is safe and nothing runs twice.

use crate::installed::ffi::Acc;
use crate::json::Res;

/// How libatspi refuses a node the page has since removed: the application object is gone once it has disposed it, or the
/// application answers that the path no longer exists.
pub const STALE_NODE: [&str; 2] = [
    "The application no longer exists",
    "Object does not exist at path",
];

pub fn is_stale_text(error: &str) -> bool {
    STALE_NODE.iter().any(|text| error.contains(text))
}

/// One node with its handle kept for an action; its name and role are read when it is listed.
pub struct Node {
    pub acc: Acc,
    pub name: String,
    pub role: String,
}

impl Node {
    pub fn enabled(&self) -> bool {
        self.acc.states().map(|s| s[2]).unwrap_or(false)
    }

    pub fn text(&self) -> String {
        self.acc.text().unwrap_or_default()
    }
}

/// Every node of the applications whose process group is `group`, depth-first. An unreadable subtree is skipped, never widened.
pub fn tree(group: i32) -> Vec<Node> {
    let Ok(desktop) = Acc::desktop() else {
        return Vec::new();
    };
    // The desktop child list itself can expire during a window relaunch: no match, and the caller's bounded wait finds the new app.
    let Ok(count) = desktop.child_count() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for index in 0..count {
        let Ok(Some(app)) = desktop.child(index) else {
            continue;
        };
        let Ok(pid) = app.process_id() else { continue };
        if pid <= 0 || unsafe { libc::getpgid(pid) } != group {
            continue;
        }
        walk(app, 0, &mut found);
    }
    found
}

fn walk(node: Acc, depth: usize, out: &mut Vec<Node>) {
    if depth > 35 {
        return;
    }
    let children = node.child_count().unwrap_or(0);
    let name = node.name().unwrap_or_default();
    let role = node.role_name().unwrap_or_default();
    let mut kids = Vec::new();
    for index in 0..children {
        if let Ok(Some(child)) = node.child(index) {
            kids.push(child);
        }
    }
    out.push(Node {
        acc: node,
        name,
        role,
    });
    for child in kids {
        walk(child, depth + 1, out);
    }
}

pub fn find(group: i32, name: &str, role: Option<&str>) -> Option<Node> {
    tree(group)
        .into_iter()
        .find(|n| n.name == name && role.is_none_or(|r| n.role == r))
}

/// The node named `name` inside the subtree of the region named `region`, only when it is the one node of that name
/// in the whole window: a stale match elsewhere (a title band, a closed menu) never stands in for it.
pub fn find_within(group: i32, region: &str, name: &str) -> Option<Node> {
    let button = |n: &Node| {
        n.name == name && matches!(n.role.as_str(), "push button" | "button" | "toggle button")
    };
    let all = tree(group);
    if all.iter().filter(|n| button(n)).count() != 1 {
        return None;
    }
    let area = all.into_iter().find(|n| n.name == region)?;
    let mut inside = Vec::new();
    walk(area.acc, 0, &mut inside);
    inside.into_iter().find(|n| button(n))
}

pub fn find_containing(group: i32, name: &str, role: &str) -> Option<Node> {
    tree(group)
        .into_iter()
        .find(|n| n.role == role && n.name.contains(name))
}

/// Whether some node's name, or its text, contains `text`.
pub fn text_seen(group: i32, text: &str) -> bool {
    tree(group).iter().any(|n| {
        n.name.contains(text)
            || n.acc
                .interfaces()
                .is_ok_and(|i| i.iter().any(|x| x == "Text"))
                && n.text().contains(text)
    })
}

/// Names and roles from the tree, for a failure diagnostic only; never raw logs.
pub fn names(group: i32, limit: usize) -> Vec<(String, String)> {
    tree(group)
        .into_iter()
        .filter(|n| !n.name.is_empty())
        .take(limit)
        .map(|n| (n.role, n.name.chars().take(180).collect()))
        .collect()
}

pub fn focus_states(acc: &Acc) -> Vec<String> {
    match acc.states() {
        Ok(s) => ["defunct", "showing", "enabled", "focused", "selected"]
            .iter()
            .zip(s)
            .filter(|(_, on)| *on)
            .map(|(n, _)| n.to_string())
            .collect(),
        Err(error) => vec![format!(
            "unreadable:{}",
            error.chars().take(40).collect::<String>()
        )],
    }
}

pub fn do_action(node: &Node) -> Res<bool> {
    node.acc.do_action(0)
}

pub fn grab_focus(node: &Node) -> Res<bool> {
    node.acc.grab_focus()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_libatspi_refusals_of_a_replaced_node_are_stale() {
        assert!(is_stale_text("The application no longer exists"));
        assert!(is_stale_text(
            "GDBus.Error: Object does not exist at path '/org/a11y/atspi/accessible/12'"
        ));
        assert!(!is_stale_text("control_missing:Add project"));
        assert!(!is_stale_text("Did not receive a reply"));
    }
}
