//! Process-tree ordering for "End process tree": children before parents.
use std::collections::{HashMap, HashSet};

/// `procs` are `(pid, create_time, parent_pid)`. Returns the root's descendants deepest
/// first with the root last. A child counts only if it was created no earlier than its
/// parent (guards against reused PIDs); cycles terminate.
pub fn descendants_deepest_first(procs: &[(u32, i64, u32)], root: (u32, i64)) -> Vec<(u32, i64)> {
    let mut children: HashMap<u32, Vec<(u32, i64)>> = HashMap::new();
    for &(pid, ct, parent) in procs {
        if pid != parent {
            children.entry(parent).or_default().push((pid, ct));
        }
    }
    let mut seen: HashSet<(u32, i64)> = HashSet::from([root]);
    let mut levels: Vec<Vec<(u32, i64)>> = vec![vec![root]];
    loop {
        let mut next = Vec::new();
        for &(pid, ct) in levels.last().unwrap() {
            for &(cpid, cct) in children.get(&pid).map(Vec::as_slice).unwrap_or(&[]) {
                if cct >= ct && seen.insert((cpid, cct)) {
                    next.push((cpid, cct));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        levels.push(next);
    }
    levels.into_iter().rev().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_descendants_deepest_first_with_root_last() {
        // 10 → 11 → 12, 10 → 13; 14 unrelated; 15 claims parent 10 but is older (reused PID)
        let procs = [(10, 100, 1), (11, 110, 10), (12, 120, 11), (13, 130, 10), (14, 140, 1), (15, 50, 10)];
        let order = descendants_deepest_first(&procs, (10, 100));
        assert_eq!(order.last(), Some(&(10, 100)));
        assert_eq!(order[0], (12, 120));
        assert_eq!(order.len(), 4);
        assert!(!order.contains(&(14, 140)) && !order.contains(&(15, 50)));
    }

    #[test]
    fn cycles_terminate() {
        let procs = [(1, 5, 2), (2, 5, 1)];
        let order = descendants_deepest_first(&procs, (1, 5));
        assert_eq!(order.last(), Some(&(1, 5)));
        assert!(order.len() <= 2);
    }
}
