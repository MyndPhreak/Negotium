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
    pub fn new(_proc_mask: u64, _sys_mask: u64, _groups: &[CpuGroup]) -> Self {
        Self::default()
    }

    pub fn toggle_core(&mut self, _id: u32) {}

    /// Turns every processor in the group on, or off when all of them are already on.
    pub fn toggle_group(&mut self, _index: usize) {}

    pub fn set_all(&mut self, _on: bool) {}

    pub fn mask(&self) -> u64 {
        0
    }

    pub fn selected(&self) -> usize {
        0
    }

    pub fn total(&self) -> usize {
        0
    }

    /// The groups for the dialog.
    pub fn model(&self) -> Vec<crate::CoreGroup> {
        Vec::new()
    }
}

/// "0–15" or "0–7, 16–23" for sorted processor numbers.
fn ranges(_ids: &[u32]) -> String {
    String::new()
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
