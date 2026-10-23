//! The CPU affinity dialog's selection: which logical processors are on, grouped by CCD or core
//! class so a whole group can be turned on or off at once.
use negotium_collector::sysinfo::CpuGroup;

#[derive(Clone, Debug, PartialEq)]
struct Group {
    name: String,
    detail: String,
    /// (logical processor, on) in processor order.
    cores: Vec<(u32, bool)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoreSelection {
    groups: Vec<Group>,
}

impl CoreSelection {
    /// The processors in `sys_mask`, on where `proc_mask` has them, in the order of `groups`;
    /// processors no group covers go in a last group, "Other" (unnamed when there are no groups).
    pub fn new(proc_mask: u64, sys_mask: u64, groups: &[CpuGroup]) -> Self {
        let cores = |mask: u64| -> Vec<(u32, bool)> {
            (0..64).filter(|i| mask & sys_mask & (1u64 << i) != 0).map(|i| (i, proc_mask & (1u64 << i) != 0)).collect()
        };
        let mut out: Vec<Group> = groups
            .iter()
            .map(|g| {
                let cores = cores(g.mask);
                let ids: Vec<u32> = cores.iter().map(|&(id, _)| id).collect();
                let mut detail = format!("CPU {}", ranges(&ids));
                if let Some(b) = g.l3_bytes {
                    detail += &format!(" · {} L3", crate::format::bytes(b as f64));
                }
                Group { name: g.name.clone(), detail, cores }
            })
            .filter(|g| !g.cores.is_empty())
            .collect();
        let covered = groups.iter().fold(0, |m, g| m | g.mask);
        let rest = cores(!covered);
        if !rest.is_empty() {
            let (name, detail) = if out.is_empty() {
                (String::new(), String::new())
            } else {
                let ids: Vec<u32> = rest.iter().map(|&(id, _)| id).collect();
                ("Other".to_string(), format!("CPU {}", ranges(&ids)))
            };
            out.push(Group { name, detail, cores: rest });
        }
        Self { groups: out }
    }

    fn cores_mut(&mut self) -> impl Iterator<Item = &mut (u32, bool)> {
        self.groups.iter_mut().flat_map(|g| g.cores.iter_mut())
    }

    fn cores(&self) -> impl Iterator<Item = &(u32, bool)> {
        self.groups.iter().flat_map(|g| g.cores.iter())
    }

    pub fn toggle_core(&mut self, id: u32) {
        if let Some(core) = self.cores_mut().find(|c| c.0 == id) {
            core.1 = !core.1;
        }
    }

    /// Turns every processor in the group on, or off when all of them are already on.
    pub fn toggle_group(&mut self, index: usize) {
        if let Some(g) = self.groups.get_mut(index) {
            let on = !g.cores.iter().all(|c| c.1);
            g.cores.iter_mut().for_each(|c| c.1 = on);
        }
    }

    pub fn set_all(&mut self, on: bool) {
        self.cores_mut().for_each(|c| c.1 = on);
    }

    pub fn mask(&self) -> u64 {
        self.cores().filter(|c| c.1).fold(0, |m, c| m | 1u64 << c.0)
    }

    pub fn selected(&self) -> usize {
        self.cores().filter(|c| c.1).count()
    }

    pub fn total(&self) -> usize {
        self.cores().count()
    }

    /// The groups for the dialog.
    pub fn model(&self) -> Vec<crate::CoreGroup> {
        self.groups
            .iter()
            .map(|g| crate::CoreGroup {
                name: g.name.as_str().into(),
                detail: g.detail.as_str().into(),
                checked: g.cores.iter().all(|c| c.1),
                cores: slint::ModelRc::new(slint::VecModel::from(
                    g.cores.iter().map(|&(id, checked)| crate::CoreItem { id: id as i32, checked }).collect::<Vec<_>>(),
                )),
            })
            .collect()
    }
}

/// "0–15" or "0–7, 16–23" for sorted processor numbers.
fn ranges(ids: &[u32]) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &id in ids {
        match runs.last_mut() {
            Some(run) if run.1 + 1 == id => run.1 = id,
            _ => runs.push((id, id)),
        }
    }
    runs.iter().map(|&(a, b)| if a == b { a.to_string() } else { format!("{a}–{b}") }).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn ccds() -> Vec<CpuGroup> {
        vec![
            CpuGroup { name: "CCD 0".into(), mask: 0x0F, l3_bytes: Some(96 << 20) },
            CpuGroup { name: "CCD 1".into(), mask: 0xF0, l3_bytes: Some(32 << 20) },
        ]
    }

    #[test]
    fn processors_are_split_into_their_groups_with_details() {
        let s = CoreSelection::new(0x3F, 0xFF, &ccds());
        let m = s.model();
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].name.as_str(), m[0].detail.as_str()), ("CCD 0", "CPU 0–3 · 96.0 MB L3"));
        assert_eq!((m[1].name.as_str(), m[1].detail.as_str()), ("CCD 1", "CPU 4–7 · 32.0 MB L3"));
        let ids: Vec<_> = m[1].cores.iter().map(|c| (c.id, c.checked)).collect();
        assert_eq!(ids, [(4, true), (5, true), (6, false), (7, false)]);
        assert!(m[0].checked && !m[1].checked);
        assert_eq!((s.selected(), s.total(), s.mask()), (6, 8, 0x3F));
    }

    #[test]
    fn without_groups_every_processor_is_in_one_unnamed_group() {
        let s = CoreSelection::new(0b101, 0b111, &[]);
        let m = s.model();
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].name.as_str(), m[0].detail.as_str()), ("", ""));
        assert_eq!(m[0].cores.row_count(), 3);
    }

    #[test]
    fn processors_no_group_covers_go_in_other() {
        let groups = [CpuGroup { name: "P-cores".into(), mask: 0b11, l3_bytes: None }];
        let m = CoreSelection::new(0b1111, 0b1111, &groups).model();
        assert_eq!((m[0].name.as_str(), m[0].detail.as_str()), ("P-cores", "CPU 0–1"));
        assert_eq!((m[1].name.as_str(), m[1].detail.as_str()), ("Other", "CPU 2–3"));
    }

    #[test]
    fn toggling_a_core_a_group_and_all_changes_the_mask() {
        let mut s = CoreSelection::new(0xFF, 0xFF, &ccds());
        s.toggle_core(5);
        assert_eq!(s.mask(), 0xDF);
        // CCD 1 is partly on: the first toggle turns all of it on, the second turns it off.
        s.toggle_group(1);
        assert_eq!(s.mask(), 0xFF);
        s.toggle_group(1);
        assert_eq!(s.mask(), 0x0F);
        s.set_all(false);
        assert_eq!((s.mask(), s.selected()), (0, 0));
        s.set_all(true);
        assert_eq!(s.mask(), 0xFF);
    }

    #[test]
    fn ranges_join_runs() {
        assert_eq!(ranges(&[0, 1, 2, 3]), "0–3");
        assert_eq!(ranges(&[0, 1, 4, 5, 7]), "0–1, 4–5, 7");
        assert_eq!(ranges(&[3]), "3");
    }
}
