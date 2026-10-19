//! A batch's task graph (§2.10, batches): a task with `after: #n` starts
//! once task n has ended, on its accepted work.

use std::collections::{BTreeMap, BTreeSet};

use super::input::BatchTask;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("task #{task} is after #{after}, which is not a task of this batch")]
    Unknown { task: u32, after: u32 },
    #[error("task #{0} is after itself")]
    SelfRef(u32),
    #[error("the tasks {} are after each other in a cycle, so none could start", cycle_text(.0))]
    Cycle(Vec<u32>),
}

fn cycle_text(cycle: &[u32]) -> String {
    let mut names: Vec<String> = cycle.iter().map(|n| format!("#{n}")).collect();
    if let Some(first) = names.first().cloned() {
        names.push(first);
    }
    names.join(" -> ")
}

/// Which tasks of a batch build on which, in the batch's task order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGraph {
    order: Vec<u32>,
    after: BTreeMap<u32, Vec<u32>>,
}

impl TaskGraph {
    pub fn new(tasks: &[BatchTask]) -> Result<Self, GraphError> {
        let numbers: BTreeSet<u32> = tasks.iter().map(|t| t.number).collect();
        let mut after = BTreeMap::new();
        for t in tasks {
            for &a in &t.after {
                if a == t.number {
                    return Err(GraphError::SelfRef(a));
                }
                if !numbers.contains(&a) {
                    return Err(GraphError::Unknown {
                        task: t.number,
                        after: a,
                    });
                }
            }
            after.insert(t.number, t.after.clone());
        }
        let graph = Self {
            order: tasks.iter().map(|t| t.number).collect(),
            after,
        };
        if let Some(cycle) = graph.cycle() {
            return Err(GraphError::Cycle(cycle));
        }
        Ok(graph)
    }

    /// A cycle, as the tasks along it, starting from the lowest-ordered.
    fn cycle(&self) -> Option<Vec<u32>> {
        // 0 unvisited, 1 on the current path, 2 done.
        let mut mark: BTreeMap<u32, u8> = BTreeMap::new();
        let mut path: Vec<u32> = Vec::new();
        fn visit(
            g: &TaskGraph,
            n: u32,
            mark: &mut BTreeMap<u32, u8>,
            path: &mut Vec<u32>,
        ) -> Option<Vec<u32>> {
            match mark.get(&n).copied().unwrap_or(0) {
                2 => return None,
                1 => {
                    let at = path.iter().position(|p| *p == n).expect("on the path");
                    return Some(path[at..].to_vec());
                }
                _ => {}
            }
            mark.insert(n, 1);
            path.push(n);
            for &p in g.prerequisites(n) {
                if let Some(c) = visit(g, p, mark, path) {
                    return Some(c);
                }
            }
            path.pop();
            mark.insert(n, 2);
            None
        }
        self.order
            .iter()
            .find_map(|&n| visit(self, n, &mut mark, &mut path))
            .map(|mut c| {
                // The path follows prerequisites; a cycle reads forwards, in
                // the order the tasks would run.
                c.reverse();
                let pos = |n: &u32| self.order.iter().position(|o| o == n);
                let first = (0..c.len()).min_by_key(|&i| pos(&c[i])).unwrap_or(0);
                c.rotate_left(first);
                c
            })
    }

    /// The tasks `n` builds on directly.
    pub fn prerequisites(&self, n: u32) -> &[u32] {
        self.after.get(&n).map_or(&[], Vec::as_slice)
    }

    /// Every task `n` builds on, directly or through others.
    pub fn ancestors(&self, n: u32) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        let mut todo: Vec<u32> = self.prerequisites(n).to_vec();
        while let Some(p) = todo.pop() {
            if out.insert(p) {
                todo.extend_from_slice(self.prerequisites(p));
            }
        }
        out
    }

    /// The tasks that build on `n` directly, in task order.
    pub fn dependents(&self, n: u32) -> Vec<u32> {
        self.order
            .iter()
            .copied()
            .filter(|t| self.prerequisites(*t).contains(&n))
            .collect()
    }

    /// The tasks not in `done` whose prerequisites all are, in task order.
    pub fn ready(&self, done: &BTreeSet<u32>) -> Vec<u32> {
        self.order
            .iter()
            .copied()
            .filter(|t| !done.contains(t))
            .filter(|t| self.prerequisites(*t).iter().all(|p| done.contains(p)))
            .collect()
    }

    /// Whether any task builds on another.
    pub fn has_edges(&self) -> bool {
        self.after.values().any(|a| !a.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(number: u32, after: &[u32]) -> BatchTask {
        BatchTask {
            number,
            title: format!("task {number}"),
            prompt: format!("task {number}"),
            criteria: Vec::new(),
            protected: Vec::new(),
            issue: None,
            after: after.to_vec(),
            touches: Vec::new(),
        }
    }

    #[test]
    fn ready_follows_the_edges_in_task_order() {
        let g =
            TaskGraph::new(&[task(1, &[]), task(2, &[1]), task(3, &[]), task(4, &[2, 3])]).unwrap();
        assert!(g.has_edges());
        assert_eq!(g.ready(&BTreeSet::new()), [1, 3]);
        assert_eq!(g.ready(&BTreeSet::from([1])), [2, 3]);
        assert_eq!(g.ready(&BTreeSet::from([1, 2])), [3]);
        assert_eq!(g.ready(&BTreeSet::from([1, 2, 3])), [4]);
        assert_eq!(g.prerequisites(4), [2, 3]);
        assert_eq!(g.dependents(2), [4]);
        assert_eq!(g.dependents(4), Vec::<u32>::new());
        assert_eq!(g.ancestors(4), BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn unknown_self_and_cyclic_edges_are_refused() {
        assert_eq!(
            TaskGraph::new(&[task(1, &[]), task(2, &[7])]),
            Err(GraphError::Unknown { task: 2, after: 7 })
        );
        assert_eq!(
            TaskGraph::new(&[task(1, &[1])]),
            Err(GraphError::SelfRef(1))
        );
        let e = TaskGraph::new(&[task(1, &[3]), task(2, &[1]), task(3, &[2]), task(4, &[])])
            .unwrap_err();
        assert_eq!(e, GraphError::Cycle(vec![1, 2, 3]));
        assert_eq!(
            e.to_string(),
            "the tasks #1 -> #2 -> #3 -> #1 are after each other in a cycle, so none could start"
        );
        assert!(!TaskGraph::new(&[task(1, &[]), task(2, &[])])
            .unwrap()
            .has_edges());
    }
}
