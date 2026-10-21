//! Logical-CPU topology (SMT siblings, hybrid core classes, L3 groups / CCDs) and the grid the
//! CPU affinity dialog lays it out in. Pure byte parsing: `sysinfo` reads the raw
//! `GetLogicalProcessorInformationEx` buffers and hands them here.

/// One logical processor in processor group 0 (the only group the affinity mask can address).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogicalCpu {
    pub id: u32,
    /// The second (or later) hardware thread of a core whose first thread is a lower id.
    pub smt_sibling: bool,
    /// Higher is faster; all equal on a non-hybrid CPU.
    pub efficiency_class: u8,
    /// Index of the L3 cache this CPU shares (one per CCD on Ryzen), in id order.
    pub l3_group: Option<u32>,
    pub l3_bytes: u64,
}

/// A named subset of CPUs shown together in the dialog (a CCD, or one hybrid core class).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Heading in the grid, e.g. "CCD 0 · 96 MB L3 (V-Cache)".
    pub label: String,
    /// Short chip name, e.g. "CCD 0 (V-Cache)".
    pub short: String,
    pub cpus: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub label: String,
    pub mask: u64,
}

pub const GRID_COLS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub id: u32,
    pub row: u32,
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub label: String,
    pub row: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub cells: Vec<Cell>,
    pub headers: Vec<Header>,
    pub rows: u32,
}

/// Walks the variable-length records of a `GetLogicalProcessorInformationEx` buffer.
fn records(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut offset = 0usize;
    std::iter::from_fn(move || {
        let head = buf.get(offset..offset + 8)?;
        let size = u32::from_ne_bytes(head[4..8].try_into().ok()?) as usize;
        if size < 8 {
            return None;
        }
        let rec = buf.get(offset..offset + size)?;
        offset += size;
        Some(rec)
    })
}

fn u16_at(r: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(r.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(r: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(r.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(r: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(r.get(at..at + 8)?.try_into().ok()?))
}

/// The group-0 affinity mask at the given offset of a record, if the record's group is 0.
fn group0_mask(r: &[u8], mask_at: usize) -> Option<u64> {
    (u16_at(r, mask_at + 8)? == 0).then(|| u64_at(r, mask_at)).flatten()
}

/// `core_buf` is the `RelationProcessorCore` buffer, `cache_buf` the `RelationCache` buffer.
pub fn parse_cpus(core_buf: &[u8], cache_buf: &[u8]) -> Vec<LogicalCpu> {
    let mut cpus: Vec<LogicalCpu> = Vec::new();
    for r in records(core_buf) {
        // PROCESSOR_RELATIONSHIP: EfficiencyClass at 9, GroupCount at 30, GROUP_AFFINITY at 32.
        let (Some(&class), Some(count)) = (r.get(9), u16_at(r, 30)) else { continue };
        if count == 0 {
            continue;
        }
        let Some(mask) = group0_mask(r, 32) else { continue };
        let first = mask.trailing_zeros();
        for id in (0..64).filter(|i| mask & (1u64 << i) != 0) {
            cpus.push(LogicalCpu { id, smt_sibling: id != first, efficiency_class: class, l3_group: None, l3_bytes: 0 });
        }
    }
    cpus.sort_by_key(|c| c.id);

    let mut l3s: Vec<(u64, u64)> = Vec::new(); // (mask, bytes)
    for r in cache_buf_l3(cache_buf) {
        l3s.push(r);
    }
    l3s.sort_by_key(|&(mask, _)| mask.trailing_zeros());
    for (group, (mask, bytes)) in l3s.iter().enumerate() {
        for cpu in cpus.iter_mut().filter(|c| mask & (1u64 << c.id) != 0) {
            cpu.l3_group = Some(group as u32);
            cpu.l3_bytes = *bytes;
        }
    }
    cpus
}

/// `(group-0 mask, size in bytes)` of every L3 cache record.
fn cache_buf_l3(buf: &[u8]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for r in records(buf) {
        // CACHE_RELATIONSHIP: Level at 8, CacheSize at 12, Type at 16, GROUP_AFFINITY at 40.
        if r.get(8) != Some(&3) {
            continue;
        }
        // Skip the instruction/data halves of a split cache; L3 is unified (0).
        if u32_at(r, 16) != Some(0) {
            continue;
        }
        let (Some(size), Some(mask)) = (u32_at(r, 12), group0_mask(r, 40)) else { continue };
        out.push((mask, size as u64));
    }
    out
}

fn mb(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

/// Splits the CPUs into what the user thinks of as "which cores": CCDs when the CPU has several L3
/// caches, hybrid core classes when it has several efficiency classes, otherwise a single unlabelled
/// section.
pub fn sections(cpus: &[LogicalCpu]) -> Vec<Section> {
    let mut l3: Vec<u32> = cpus.iter().filter_map(|c| c.l3_group).collect();
    l3.sort_unstable();
    l3.dedup();
    if l3.len() > 1 {
        let bytes_of = |g: u32| cpus.iter().find(|c| c.l3_group == Some(g)).map_or(0, |c| c.l3_bytes);
        let biggest = l3.iter().map(|&g| bytes_of(g)).max().unwrap_or(0);
        let uneven = l3.iter().any(|&g| bytes_of(g) != biggest);
        return l3
            .iter()
            .enumerate()
            .map(|(n, &g)| {
                let bytes = bytes_of(g);
                let cache = if uneven && bytes == biggest { " (V-Cache)" } else { "" };
                Section {
                    label: format!("CCD {n} · {} MB L3{cache}", mb(bytes)),
                    short: format!("CCD {n}{cache}"),
                    cpus: cpus.iter().filter(|c| c.l3_group == Some(g)).map(|c| c.id).collect(),
                }
            })
            .collect();
    }

    let mut classes: Vec<u8> = cpus.iter().map(|c| c.efficiency_class).collect();
    classes.sort_unstable_by(|a, b| b.cmp(a));
    classes.dedup();
    if classes.len() > 1 {
        return classes
            .iter()
            .enumerate()
            .map(|(rank, &class)| {
                let name = match (classes.len(), rank) {
                    (_, 0) => "P-cores".to_string(),
                    (2, _) | (3, 1) => "E-cores".to_string(),
                    (3, _) => "LP E-cores".to_string(),
                    _ => format!("Class {class}"),
                };
                Section {
                    label: name.clone(),
                    short: name,
                    cpus: cpus.iter().filter(|c| c.efficiency_class == class).map(|c| c.id).collect(),
                }
            })
            .collect();
    }

    vec![Section { label: String::new(), short: String::new(), cpus: cpus.iter().map(|c| c.id).collect() }]
}

fn mask_of(ids: impl IntoIterator<Item = u32>) -> u64 {
    ids.into_iter().filter(|&i| i < 64).fold(0, |m, i| m | (1u64 << i))
}

/// Quick-select chips: all CPUs, each section when there are several, and one thread per core when
/// the CPU has SMT.
pub fn presets(cpus: &[LogicalCpu], sections: &[Section]) -> Vec<Preset> {
    let mut out = vec![Preset { label: "All".into(), mask: mask_of(cpus.iter().map(|c| c.id)) }];
    if sections.len() > 1 {
        out.extend(sections.iter().map(|s| Preset { label: s.short.clone(), mask: mask_of(s.cpus.iter().copied()) }));
    }
    if cpus.iter().any(|c| c.smt_sibling) {
        out.push(Preset {
            label: "Physical cores only".into(),
            mask: mask_of(cpus.iter().filter(|c| !c.smt_sibling).map(|c| c.id)),
        });
    }
    out
}

/// Lays sections out in [`GRID_COLS`]-wide rows, each headed by a one-row label when it has one.
pub fn grid(sections: &[Section]) -> Grid {
    let mut grid = Grid::default();
    let mut row = 0;
    for section in sections {
        if !section.label.is_empty() {
            grid.headers.push(Header { label: section.label.clone(), row });
            row += 1;
        }
        for (i, &id) in section.cpus.iter().enumerate() {
            grid.cells.push(Cell { id, row: row + i as u32 / GRID_COLS, col: i as u32 % GRID_COLS });
        }
        row += (section.cpus.len() as u32).div_ceil(GRID_COLS);
    }
    grid.rows = row;
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_rec(class: u8, mask: u64) -> Vec<u8> {
        let mut r = vec![0u8; 48];
        r[0..4].copy_from_slice(&0u32.to_ne_bytes());
        r[4..8].copy_from_slice(&48u32.to_ne_bytes());
        r[9] = class;
        r[30..32].copy_from_slice(&1u16.to_ne_bytes());
        r[32..40].copy_from_slice(&mask.to_ne_bytes());
        r
    }

    fn l3_rec(bytes: u32, mask: u64) -> Vec<u8> {
        let mut r = vec![0u8; 56];
        r[0..4].copy_from_slice(&2u32.to_ne_bytes());
        r[4..8].copy_from_slice(&56u32.to_ne_bytes());
        r[8] = 3;
        r[12..16].copy_from_slice(&bytes.to_ne_bytes());
        r[38..40].copy_from_slice(&1u16.to_ne_bytes());
        r[40..48].copy_from_slice(&mask.to_ne_bytes());
        r
    }

    /// 2 CCDs x 4 cores x 2 threads, the first CCD with extra cache.
    fn dual_ccd() -> Vec<LogicalCpu> {
        let cores: Vec<u8> = (0..8).flat_map(|c| core_rec(0, 0b11u64 << (c * 2))).collect();
        let cache: Vec<u8> = [l3_rec(96 << 20, 0x00FF), l3_rec(32 << 20, 0xFF00)].concat();
        parse_cpus(&cores, &cache)
    }

    #[test]
    fn parses_smt_and_l3_groups() {
        let cpus = dual_ccd();
        assert_eq!(cpus.len(), 16);
        assert!(!cpus[0].smt_sibling && cpus[1].smt_sibling && !cpus[2].smt_sibling);
        assert_eq!(cpus[7].l3_group, Some(0));
        assert_eq!(cpus[8].l3_group, Some(1));
        assert_eq!(cpus[0].l3_bytes, 96 << 20);
    }

    #[test]
    fn ccd_sections_flag_the_big_cache() {
        let secs = sections(&dual_ccd());
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].label, "CCD 0 · 96 MB L3 (V-Cache)");
        assert_eq!(secs[1].label, "CCD 1 · 32 MB L3");
        assert_eq!(secs[0].cpus, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn presets_cover_ccds_and_physical_cores() {
        let cpus = dual_ccd();
        let p = presets(&cpus, &sections(&cpus));
        let labels: Vec<_> = p.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["All", "CCD 0 (V-Cache)", "CCD 1", "Physical cores only"]);
        assert_eq!(p[0].mask, 0xFFFF);
        assert_eq!(p[1].mask, 0x00FF);
        assert_eq!(p[3].mask, 0b0101_0101_0101_0101);
    }

    #[test]
    fn hybrid_cpu_splits_by_efficiency_class() {
        let cores = [core_rec(1, 0b11), core_rec(1, 0b1100), core_rec(0, 0b1_0000), core_rec(0, 0b10_0000)].concat();
        let cpus = parse_cpus(&cores, &[]);
        let secs = sections(&cpus);
        assert_eq!(secs.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["P-cores", "E-cores"]);
        assert_eq!(secs[1].cpus, [4, 5]);
    }

    #[test]
    fn plain_cpu_is_one_unlabelled_section_and_just_all() {
        let cores = (0..4).flat_map(|c| core_rec(0, 1u64 << c)).collect::<Vec<u8>>();
        let cpus = parse_cpus(&cores, &[]);
        let secs = sections(&cpus);
        assert_eq!(secs.len(), 1);
        assert!(secs[0].label.is_empty());
        assert_eq!(presets(&cpus, &secs).len(), 1);
        assert!(grid(&secs).headers.is_empty());
    }

    #[test]
    fn grid_wraps_and_reserves_header_rows() {
        let g = grid(&sections(&dual_ccd()));
        // header + 1 row of 8 per CCD
        assert_eq!(g.rows, 4);
        assert_eq!(g.headers.iter().map(|h| h.row).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(g.cells[0], Cell { id: 0, row: 1, col: 0 });
        assert_eq!(g.cells[8], Cell { id: 8, row: 3, col: 0 });
    }

    #[test]
    fn truncated_buffers_yield_nothing() {
        assert!(parse_cpus(&[1, 2, 3], &[0; 7]).is_empty());
    }
}
