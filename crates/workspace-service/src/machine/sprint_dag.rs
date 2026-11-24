//! ID-only DAG validation shared by approved-plan materialization and live edits.
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
};

pub(super) enum Error<'a> {
    DuplicateId(&'a str),
    SelfEdge(&'a str),
    Missing(&'a str, &'a str),
    DuplicateEdge,
    Cycle,
}

/// Stable topological order: among ready vertices, use input order.
pub(super) fn ordered<'a>(nodes: &[(&'a str, Vec<&'a str>)]) -> Result<Vec<usize>, Error<'a>> {
    let mut ids = HashMap::new();
    for (i, (id, _)) in nodes.iter().enumerate() {
        if ids.insert(*id, i).is_some() {
            return Err(Error::DuplicateId(id));
        }
    }
    let mut counts = Vec::with_capacity(nodes.len());
    let mut children = vec![Vec::new(); nodes.len()];
    let mut ready = BinaryHeap::new();
    for (i, (id, predecessors)) in nodes.iter().enumerate() {
        let mut seen = HashSet::new();
        for predecessor in predecessors {
            if predecessor == id {
                return Err(Error::SelfEdge(id));
            }
            let parent = *ids
                .get(predecessor)
                .ok_or(Error::Missing(id, predecessor))?;
            if !seen.insert(parent) {
                return Err(Error::DuplicateEdge);
            }
            children[parent].push(i);
        }
        counts.push(predecessors.len());
        if predecessors.is_empty() {
            ready.push(Reverse(i));
        }
    }
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(Reverse(i)) = ready.pop() {
        order.push(i);
        for &child in &children[i] {
            counts[child] -= 1;
            if counts[child] == 0 {
                ready.push(Reverse(child));
            }
        }
    }
    if order.len() != nodes.len() {
        return Err(Error::Cycle);
    }
    Ok(order)
}
