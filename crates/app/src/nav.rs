//! Keyboard navigation through the app list (ruling R4): one list-level focus stop and a
//! "current row" moved by the keys. Pure: it decides; the bridge applies.
use crate::view::RowView;
use std::collections::HashSet;

pub const PAGE_ROWS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Enter,
    Delete,
}

impl NavKey {
    /// The navigation key a Slint key event's text stands for (Space acts like Enter).
    pub fn from_text(text: &str) -> Option<NavKey> {
        use slint::platform::Key;
        let is = |k: Key| text == slint::SharedString::from(k).as_str();
        let key = if is(Key::UpArrow) {
            NavKey::Up
        } else if is(Key::DownArrow) {
            NavKey::Down
        } else if is(Key::Home) {
            NavKey::Home
        } else if is(Key::End) {
            NavKey::End
        } else if is(Key::PageUp) {
            NavKey::PageUp
        } else if is(Key::PageDown) {
            NavKey::PageDown
        } else if is(Key::LeftArrow) {
            NavKey::Left
        } else if is(Key::RightArrow) {
            NavKey::Right
        } else if is(Key::Return) || text == " " {
            NavKey::Enter
        } else if is(Key::Delete) {
            NavKey::Delete
        } else {
            return None;
        };
        Some(key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavOutcome {
    /// Make this row current.
    Move(i32),
    /// Expand or collapse this app (and make it current).
    Toggle(i32),
    /// Act on this row, exactly as its button or menu would.
    Action(&'static str, i32),
    Nothing,
}

pub fn navigate(rows: &[RowView], expanded: &HashSet<i32>, current: Option<i32>, key: NavKey) -> NavOutcome {
    let Some(first) = rows.first() else { return NavOutcome::Nothing };
    let Some(i) = current.and_then(|id| rows.iter().position(|r| r.id == id)) else {
        // No current row yet, or it went away: any key starts at the top.
        return NavOutcome::Move(first.id);
    };
    let r = &rows[i];
    let last = rows.len() - 1;
    let to = |j: usize| if j == i { NavOutcome::Nothing } else { NavOutcome::Move(rows[j].id) };
    match key {
        NavKey::Up => to(i.saturating_sub(1)),
        NavKey::Down => to((i + 1).min(last)),
        NavKey::Home => to(0),
        NavKey::End => to(last),
        NavKey::PageUp => to(i.saturating_sub(PAGE_ROWS)),
        NavKey::PageDown => to((i + PAGE_ROWS).min(last)),
        NavKey::Right if r.expandable && !expanded.contains(&r.id) => NavOutcome::Toggle(r.id),
        NavKey::Right if r.expandable => match rows.get(i + 1) {
            Some(c) if c.parent == r.id => NavOutcome::Move(c.id),
            _ => NavOutcome::Nothing,
        },
        NavKey::Left if r.depth > 0 => NavOutcome::Move(r.parent),
        NavKey::Left if r.expandable && expanded.contains(&r.id) => NavOutcome::Toggle(r.id),
        NavKey::Enter if r.expandable => NavOutcome::Toggle(r.id),
        NavKey::Delete if r.actionable && !r.exited => NavOutcome::Action("end-task", r.id),
        _ => NavOutcome::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i32, parent: i32, expandable: bool, actionable: bool, exited: bool) -> RowView {
        RowView {
            id, parent, depth: if parent < 0 { 0 } else { 1 }, name: format!("r{id}"), expandable, actionable, exited,
            ..Default::default()
        }
    }

    /// A (expanded: a1, a2), B, C (exited), W (the never-actionable Windows group).
    fn rows() -> Vec<RowView> {
        vec![
            row(1, -1, true, true, false),
            row(2, 1, false, true, false),
            row(3, 1, false, true, false),
            row(4, -1, false, true, false),
            row(5, -1, false, false, true),
            row(6, -1, true, false, false),
        ]
    }

    fn go(current: i32, key: NavKey, expanded: &[i32]) -> NavOutcome {
        navigate(&rows(), &expanded.iter().copied().collect(), Some(current), key)
    }

    #[test]
    fn without_a_current_row_any_key_starts_at_the_top() {
        let none = HashSet::new();
        assert_eq!(navigate(&rows(), &none, None, NavKey::Delete), NavOutcome::Move(1));
        assert_eq!(navigate(&rows(), &none, Some(99), NavKey::Down), NavOutcome::Move(1), "the current row went away");
        assert_eq!(navigate(&[], &none, None, NavKey::Down), NavOutcome::Nothing);
    }

    #[test]
    fn arrows_home_end_and_paging_move_and_clamp() {
        assert_eq!(go(1, NavKey::Down, &[1]), NavOutcome::Move(2));
        assert_eq!(go(1, NavKey::Up, &[1]), NavOutcome::Nothing);
        assert_eq!(go(1, NavKey::End, &[1]), NavOutcome::Move(6));
        assert_eq!(go(6, NavKey::Home, &[1]), NavOutcome::Move(1));
        assert_eq!(go(1, NavKey::PageDown, &[1]), NavOutcome::Move(6));
        assert_eq!(go(6, NavKey::PageUp, &[1]), NavOutcome::Move(1));
        assert_eq!(go(6, NavKey::Down, &[1]), NavOutcome::Nothing);
    }

    #[test]
    fn right_expands_then_enters_and_left_returns_then_collapses() {
        assert_eq!(go(1, NavKey::Right, &[]), NavOutcome::Toggle(1));
        assert_eq!(go(1, NavKey::Right, &[1]), NavOutcome::Move(2));
        assert_eq!(go(2, NavKey::Left, &[1]), NavOutcome::Move(1));
        assert_eq!(go(1, NavKey::Left, &[1]), NavOutcome::Toggle(1));
        assert_eq!(go(1, NavKey::Left, &[]), NavOutcome::Nothing);
        assert_eq!(go(4, NavKey::Right, &[]), NavOutcome::Nothing);
        assert_eq!(go(1, NavKey::Enter, &[]), NavOutcome::Toggle(1));
        assert_eq!(go(4, NavKey::Enter, &[]), NavOutcome::Nothing);
    }

    #[test]
    fn delete_ends_only_rows_that_can_be_acted_on() {
        assert_eq!(go(4, NavKey::Delete, &[]), NavOutcome::Action("end-task", 4));
        assert_eq!(go(2, NavKey::Delete, &[1]), NavOutcome::Action("end-task", 2), "a single process");
        assert_eq!(go(5, NavKey::Delete, &[]), NavOutcome::Nothing, "an exited row held in place is inert");
        assert_eq!(go(6, NavKey::Delete, &[]), NavOutcome::Nothing, "the Windows group is never a target");
    }

    #[test]
    fn slint_key_texts_map_to_navigation_keys() {
        use slint::platform::Key;
        let text = |k: Key| slint::SharedString::from(k);
        assert_eq!(NavKey::from_text(&text(Key::DownArrow)), Some(NavKey::Down));
        assert_eq!(NavKey::from_text(&text(Key::PageUp)), Some(NavKey::PageUp));
        assert_eq!(NavKey::from_text(&text(Key::Return)), Some(NavKey::Enter));
        assert_eq!(NavKey::from_text(" "), Some(NavKey::Enter));
        assert_eq!(NavKey::from_text(&text(Key::Delete)), Some(NavKey::Delete));
        assert_eq!(NavKey::from_text("a"), None);
    }
}
