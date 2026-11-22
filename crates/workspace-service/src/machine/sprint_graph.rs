//! Read-only sprint relationships. Workspace membership is the intersection of
//! the saved sprint roster, its task overlays and the project's bound Core rows.
use super::{native, settings};
use cortex_core::{CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug)]
pub(crate) struct Member {
    pub id: String,
    pub runtime: Option<String>,
    pub record: Value,
    dependencies: Vec<String>,
    dependents: Vec<String>,
}

pub(crate) struct Scope {
    sprint_id: String,
    task_id: String,
    order: Vec<String>,
    members: HashMap<String, Member>,
}

const INVALID: CoreError = CoreError::Refused("sprint_membership_invalid");

impl Scope {
    pub fn load(db: &Connection, state: &Path, runtime: &str) -> CoreResult<Option<Self>> {
        let binding: Option<(String, String)> = db
            .query_row(
                "SELECT w.task_id,w.project_id FROM workspace_tasks w JOIN workspace_executions e
             ON w.execution_id=e.execution_id AND w.task_id=e.task_id WHERE e.runtime_task_id=?",
                [runtime],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((task_id, project)) = binding else {
            return Ok(None);
        };
        let path = native::product_path(state, &project).map_err(|_| INVALID)?;
        let product = settings::read_json(&path, json!({"tasks":{},"sprints":[]}))
            .map_err(|_| CoreError::Refused("state_unreadable"))?;
        let record = &product["tasks"][&task_id];
        let Some(sprint_id) = record["sprintId"].as_str() else {
            return Ok(None);
        };
        let sprints = product["sprints"].as_array().ok_or(INVALID)?;
        let mut matching = sprints.iter().filter(|s| s["id"] == sprint_id);
        let sprint = matching.next().ok_or(INVALID)?;
        if matching.next().is_some() || sprint["projectId"].as_str().is_some_and(|id| id != project)
        {
            return Err(INVALID);
        }
        let order: Vec<String> =
            serde_json::from_value(sprint["taskIds"].clone()).map_err(|_| INVALID)?;
        // A duplicated roster entry in a different sprint is an ambiguous scope,
        // even if the task overlay still names this sprint.
        let roster: HashSet<&str> = order.iter().map(String::as_str).collect();
        if sprints.iter().filter(|s| s["id"] != sprint_id).any(|s| {
            s["taskIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|id| roster.contains(id))
        }) {
            return Err(INVALID);
        }
        let mut members = HashMap::new();
        let mut stmt = db.prepare("SELECT substr(w.description,1,160), w.execution_id, e.runtime_task_id, r.state,
            r.synthesis IS NOT NULL
            FROM workspace_tasks w LEFT JOIN workspace_executions e ON w.execution_id=e.execution_id AND w.task_id=e.task_id
            LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.project_id=? AND w.task_id=?")?;
        for id in &order {
            let overlay = &product["tasks"][id];
            if members.contains_key(id) || overlay["sprintId"] != sprint_id || id.len() > 128 {
                return Err(INVALID);
            }
            let (description, execution, runtime, status, has_synthesis): (
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                bool,
            ) = stmt
                .query_row((&project, id), |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .optional()?
                .ok_or(INVALID)?;
            if execution.is_some() && status.is_none() {
                return Err(INVALID);
            }
            let dependencies: Vec<String> = match overlay.get("dependencies") {
                None => Vec::new(),
                Some(deps) => serde_json::from_value(deps.clone()).map_err(|_| INVALID)?,
            };
            let title: String = overlay["title"]
                .as_str()
                .unwrap_or(&description)
                .chars()
                .take(200)
                .collect();
            let mut record = json!({"title":title,"status":status.as_deref().unwrap_or("not_started"),"has_synthesis":has_synthesis});
            for key in ["status", "closedAs", "targetBranch"] {
                if let Some(value) = overlay[key].as_str() {
                    record[key] = json!(value);
                }
            }
            if let Some(flag) = overlay["notVerifiedByCortex"].as_bool() {
                record["notVerifiedByCortex"] = json!(flag);
            }
            members.insert(
                id.clone(),
                Member {
                    id: id.clone(),
                    runtime,
                    record,
                    dependencies,
                    dependents: vec![],
                },
            );
        }
        if !members.contains_key(&task_id) {
            return Err(INVALID);
        }
        for id in &order {
            let mut unique = HashSet::new();
            for dep in members[id].dependencies.clone() {
                if dep == *id || !unique.insert(dep.clone()) {
                    return Err(INVALID);
                }
                members
                    .get_mut(&dep)
                    .ok_or(INVALID)?
                    .dependents
                    .push(id.clone());
            }
        }
        Ok(Some(Self {
            sprint_id: sprint_id.into(),
            task_id,
            order,
            members,
        }))
    }

    pub fn member(&self, id: &str) -> CoreResult<&Member> {
        self.members
            .get(id)
            .ok_or(CoreError::Refused("task_outside_sprint"))
    }

    /// Recheck the set of principals and their execution bindings before a
    /// multi-step read releases data outside the publishing task.
    pub fn same_members(&self, other: &Self) -> bool {
        self.sprint_id == other.sprint_id
            && self.members.len() == other.members.len()
            && self.members.iter().all(|(id, member)| {
                other
                    .members
                    .get(id)
                    .is_some_and(|current| current.runtime == member.runtime)
            })
    }

    pub fn graph(&self, args: &Value) -> CoreResult<Value> {
        let limit = args["limit"].as_u64().unwrap_or(25);
        if !(1..=50).contains(&limit) {
            return Err(CoreError::Refused("invalid_limit"));
        }
        let selected = args["member_id"].as_str();
        let neighbors = selected
            .map(|id| {
                self.member(id).map(|m| {
                    m.dependencies
                        .iter()
                        .chain(&m.dependents)
                        .chain(std::iter::once(&m.id))
                        .collect::<HashSet<_>>()
                })
            })
            .transpose()?;
        let view: Vec<_> = self
            .order
            .iter()
            .filter(|id| neighbors.as_ref().is_none_or(|set| set.contains(id)))
            .collect();
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(&(
            &self.sprint_id,
            &self.task_id,
            selected,
        ))?);
        for id in &self.order {
            hash.update(serde_json::to_vec(&(id, &self.members[id].dependencies))?);
        }
        let binding = format!("{:x}", hash.finalize());
        let offset = if let Some(cursor) = args["cursor"].as_str() {
            cursor
                .strip_prefix(&binding)
                .and_then(|s| s.strip_prefix('.'))
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| *n < view.len())
                .ok_or(CoreError::Refused("invalid_cursor"))?
        } else {
            0
        };
        let end = (offset + limit as usize).min(view.len());
        let rows:Vec<_>=view[offset..end].iter().map(|id|{
            let m=&self.members[*id];
            json!({"taskId":m.id,"title":m.record["title"],"status":m.record["status"],"closedAs":m.record["closedAs"],
                "dependencies":m.dependencies,"dependents":m.dependents,"result":{"member_id":m.id}})
        }).collect();
        Ok(
            json!({"sprintId":self.sprint_id,"taskId":self.task_id,"tasks":rows,"total":view.len(),
            "next_cursor":(end<view.len()).then(||format!("{binding}.{end}"))}),
        )
    }

    pub fn start(&self, db: &Connection) -> CoreResult<Value> {
        let current = self.member(&self.task_id)?;
        // IDs are bounded on admission. Fetch the remaining neighbors via sprint_graph.
        let preview = |ids: &[String]| -> CoreResult<Vec<Value>> {
            let mut items = Vec::new();
            let mut bytes = 0;
            for id in ids.iter().take(12) {
                let member = &self.members[id];
                let record = &member.record;
                let title: String = record["title"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(48)
                    .collect();
                let has_reports = match &member.runtime {
                    Some(task) => db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM reports WHERE task_id=? AND kind='report')",
                        [task],
                        |r| r.get::<_, bool>(0),
                    )?,
                    None => false,
                };
                let item = json!({"member_id":id,"title":title,"status":record["status"],"has_synthesis":record["has_synthesis"],"has_reports":has_reports});
                bytes += item.to_string().len() + 1;
                if bytes > 4000 {
                    break;
                }
                items.push(item);
            }
            Ok(items)
        };
        Ok(
            json!({"sprintId":self.sprint_id,"taskId":self.task_id,"predecessorTotal":current.dependencies.len(),
            "successorTotal":current.dependents.len(),"predecessors":preview(&current.dependencies)?,"dependents":preview(&current.dependents)?}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_degree_neighbors_and_view_bound_pages() {
        let mut members = HashMap::new();
        let order: Vec<_> = (0..500).map(|n| format!("wt_{n:032}")).collect();
        for (i, id) in order.iter().enumerate() {
            members.insert(
                id.clone(),
                Member {
                    id: id.clone(),
                    runtime: None,
                    record: json!({"title":"Д".repeat(200),"status":"not_started"}),
                    dependencies: if i == 0 {
                        vec![]
                    } else {
                        vec![order[0].clone()]
                    },
                    dependents: if i == 0 { order[1..].to_vec() } else { vec![] },
                },
            );
        }
        let s = Scope {
            sprint_id: "sp".into(),
            task_id: order[0].clone(),
            members,
            order,
        };
        let start = s.start(&Connection::open_in_memory().unwrap()).unwrap();
        assert_eq!(start["successorTotal"], 499);
        assert!(start.to_string().len() < 12_000);
        let page = s.graph(&json!({"limit":1})).unwrap();
        let next = &page["next_cursor"];
        assert_eq!(
            s.graph(&json!({"cursor":next,"limit":1})).unwrap()["tasks"][0]["taskId"],
            s.order[1]
        );
        assert!(s
            .graph(&json!({"cursor":next,"member_id":s.order[1]}))
            .is_err());
        assert_eq!(
            s.graph(&json!({"member_id":s.order[1]})).unwrap()["total"],
            2
        );
    }
}
