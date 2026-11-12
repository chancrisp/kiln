//! The right-click menu, drawn in the window as a Fluent menu flyout
//! (mock screen 2's `.menu`): what it holds, where it is, which item is
//! lit, and what a key or the pointer does to it. `App` opens it and acts
//! on the item chosen.

use super::*;
use crate::controls::{Key, MenuEvent, MenuState, Press};
use crate::layout::kit::{MenuLayout, MenuRow};

/// An open right-click menu.
pub(in crate::win) struct RowMenu {
    /// Its lines in order, separators among them.
    pub items: Vec<MenuItem>,
    /// Where it is and each line's rect, in layout units.
    pub layout: MenuLayout,
    /// Which items can be used (all but the separators) and which is lit.
    pub state: MenuState,
    /// The rows it is for, by file: the list can change while it is up.
    pub files: Vec<PathBuf>,
    /// The row it opened at, by file: a wheel pours from it.
    pub focus: Option<PathBuf>,
    /// Whether it opened from the keyboard: then its first item is lit.
    pub keyboard: bool,
}

/// What an input did to the open menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::win) enum MenuDid {
    /// Nothing to draw again.
    Nothing,
    /// The lit item changed.
    Lit,
    /// Do the item at this line; the menu closes.
    Chose(usize),
    /// The menu closes with nothing chosen.
    Close,
}

/// Each of `items` as the flyout lays it out: its label's and shortcut's
/// widths, as `width` measures them (the label in Body, `false`; the
/// shortcut in Caption, `true`).
pub(in crate::win) fn menu_rows(
    items: &[MenuItem],
    width: &dyn Fn(&str, bool) -> f32,
) -> Vec<MenuRow> {
    // A column for glyphs, when any line has one.
    let glyphs = items.iter().any(|i| i.glyph().is_some());
    items
        .iter()
        .map(|&item| match item {
            MenuItem::Separator => MenuRow::Separator,
            _ => {
                let (label, key) = item.parts();
                MenuRow::Item {
                    label: width(label, false),
                    accel: if key.is_empty() {
                        0.0
                    } else {
                        width(&key, true)
                    },
                    glyph: glyphs,
                }
            }
        })
        .collect()
}

/// A key as the menu reads it.
fn press_of(key: VIRTUAL_KEY) -> Option<Press> {
    Some(Press::key(match key {
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_RETURN => Key::Enter,
        VK_SPACE => Key::Space,
        VK_ESCAPE => Key::Esc,
        _ => return None,
    }))
}

impl RowMenu {
    /// A menu of `items` laid out as `rows`, opening down and right from
    /// `at` and kept inside `bounds`.
    pub fn new(
        items: Vec<MenuItem>,
        rows: &[MenuRow],
        at: (f32, f32),
        bounds: layout::Rect,
        files: Vec<PathBuf>,
        focus: Option<PathBuf>,
        keyboard: bool,
    ) -> Self {
        let layout = layout::kit::menu_flyout(at, rows, bounds);
        let enabled = items.iter().map(|&i| i != MenuItem::Separator).collect();
        RowMenu {
            items,
            layout,
            state: MenuState::new(enabled, keyboard),
            files,
            focus,
            keyboard,
        }
    }

    /// Whether `(x, y)` is on the menu.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.layout.rect.contains(x, y)
    }

    /// The item under `(x, y)`: never a separator.
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        self.layout
            .rows
            .iter()
            .position(|r| r.contains(x, y))
            .filter(|&i| self.items[i] != MenuItem::Separator)
    }

    /// The pointer moved to `(x, y)`: the item under it is lit. Off the
    /// menu, a lit item stays lit only for the keyboard.
    pub fn hover(&mut self, x: f32, y: f32) -> MenuDid {
        let under = self.item_at(x, y);
        let lit = match under {
            Some(i) => Some(i),
            None if self.contains(x, y) => self.state.highlight,
            None => None,
        };
        if lit == self.state.highlight {
            return MenuDid::Nothing;
        }
        self.state.highlight = lit;
        self.keyboard = false;
        MenuDid::Lit
    }

    /// The button came up at `(x, y)`: the item under it is chosen. Off
    /// the menu, it closes.
    pub fn release(&self, x: f32, y: f32) -> MenuDid {
        match self.item_at(x, y) {
            Some(i) => MenuDid::Chose(i),
            None if self.contains(x, y) => MenuDid::Nothing,
            None => MenuDid::Close,
        }
    }

    /// A key: Up and Down move round the items, Home and End go to the
    /// ends, Enter and Space choose the lit one, Esc and the Menu key
    /// close it. Left and Right do nothing, as there are no submenus. Any
    /// other key closes it too, as a Windows menu does with a key it has
    /// no use for — except Shift, Ctrl and Alt alone.
    pub fn key(&mut self, key: VIRTUAL_KEY) -> MenuDid {
        if matches!(key, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LEFT | VK_RIGHT) {
            return MenuDid::Nothing;
        }
        let Some(p) = press_of(key) else {
            return MenuDid::Close;
        };
        match self.state.key(p) {
            MenuEvent::Ignored => MenuDid::Nothing,
            MenuEvent::Moved(_) => {
                self.keyboard = true;
                MenuDid::Lit
            }
            MenuEvent::Act(i) => MenuDid::Chose(i),
            MenuEvent::Close => MenuDid::Close,
        }
    }

    /// The lit item, as the tree's focus.
    pub fn lit(&self) -> Option<usize> {
        self.state.highlight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ways_in::{menu_items, RowState};

    fn width(s: &str, _caption: bool) -> f32 {
        s.chars().count() as f32 * 7.0
    }

    fn held_menu(keyboard: bool) -> RowMenu {
        let items = menu_items(&[RowState::Saved], &[WheelKind::Convert]);
        let rows = menu_rows(&items, &width);
        RowMenu::new(
            items,
            &rows,
            (100.0, 100.0),
            layout::Rect::new(0.0, 0.0, 800.0, 600.0),
            vec![PathBuf::from("a.png")],
            None,
            keyboard,
        )
    }

    #[test]
    fn a_menu_from_the_keyboard_lights_its_first_item_and_keys_move_round_it() {
        let mut m = held_menu(true);
        assert_eq!(m.lit(), Some(0));
        assert_eq!(m.items[0], MenuItem::Wheel(WheelKind::Convert));
        // Down skips the separator after Convert to…
        assert_eq!(m.key(VK_DOWN), MenuDid::Lit);
        assert_eq!(m.items[m.lit().unwrap()], MenuItem::ShowInFolder);
        // Up from the top wraps to Remove from list.
        assert_eq!(m.key(VK_HOME), MenuDid::Lit);
        assert_eq!(m.key(VK_UP), MenuDid::Lit);
        assert_eq!(m.items[m.lit().unwrap()], MenuItem::Remove);
        assert_eq!(m.key(VK_RETURN), MenuDid::Chose(m.lit().unwrap()));
        assert_eq!(m.key(VK_ESCAPE), MenuDid::Close);
        // A letter or Tab closes it, as a Windows menu does; Shift alone
        // doesn't.
        assert_eq!(m.key(VK_TAB), MenuDid::Close);
        assert_eq!(m.key(VK_SHIFT), MenuDid::Nothing);
        // With no submenus, Left and Right do nothing; the Menu key closes
        // it, as it opened it.
        let lit = m.lit();
        assert_eq!(m.key(VK_LEFT), MenuDid::Nothing);
        assert_eq!(m.key(VK_RIGHT), MenuDid::Nothing);
        assert_eq!(m.lit(), lit);
        assert_eq!(m.key(VK_APPS), MenuDid::Close);
    }

    #[test]
    fn a_menu_with_glyphs_has_a_column_for_every_line() {
        use crate::theme::fluent::glyph;
        let items = menu_items(&[RowState::Saved], &[WheelKind::Convert]);
        // Convert to…, Show in folder, Copy path and Remove from list have
        // glyphs; Make smaller too, when it applies.
        let glyphs: Vec<_> = items.iter().filter_map(|i| i.glyph()).collect();
        assert!(glyphs.contains(&glyph::CONVERT));
        assert!(glyphs.contains(&glyph::SHOW_IN_FOLDER));
        assert!(glyphs.contains(&glyph::COPY_PATH));
        assert!(glyphs.contains(&glyph::REMOVE));
        assert_eq!(MenuItem::Smaller.glyph(), Some(glyph::MAKE_SMALLER));
        // Tools… has none, and its line still has the column.
        assert_eq!(MenuItem::Wheel(WheelKind::Tools).glyph(), None);
        let rows = menu_rows(&items, &width);
        assert!(rows
            .iter()
            .all(|r| !matches!(r, MenuRow::Item { glyph: false, .. })));
        // The column makes the menu wider than the same words without it.
        let bare: Vec<MenuRow> = rows
            .iter()
            .map(|r| match *r {
                MenuRow::Item { label, accel, .. } => MenuRow::Item {
                    label,
                    accel,
                    glyph: false,
                },
                s => s,
            })
            .collect();
        let bounds = layout::Rect::new(0.0, 0.0, 800.0, 600.0);
        let with = layout::kit::menu_flyout((10.0, 10.0), &rows, bounds);
        let without = layout::kit::menu_flyout((10.0, 10.0), &bare, bounds);
        assert!(with.rect.w >= without.rect.w);
    }

    #[test]
    fn the_pointer_lights_and_chooses_an_item_and_a_click_off_the_menu_closes_it() {
        let mut m = held_menu(false);
        assert_eq!(m.lit(), None);
        let r = m.layout.rows[2];
        let (x, y) = (r.x + 8.0, r.y + r.h / 2.0);
        assert_eq!(m.hover(x, y), MenuDid::Lit);
        assert_eq!(m.lit(), Some(2));
        assert_eq!(m.release(x, y), MenuDid::Chose(2));
        // A separator is no item.
        let sep = m
            .items
            .iter()
            .position(|&i| i == MenuItem::Separator)
            .unwrap();
        let s = m.layout.rows[sep];
        assert_eq!(m.item_at(s.x + 8.0, s.y + s.h / 2.0), None);
        assert_eq!(m.release(s.x + 8.0, s.y + s.h / 2.0), MenuDid::Nothing);
        // Off the menu: nothing lit, and a release there closes it.
        assert_eq!(m.hover(5.0, 5.0), MenuDid::Lit);
        assert_eq!(m.lit(), None);
        assert_eq!(m.release(5.0, 5.0), MenuDid::Close);
    }

    #[test]
    fn the_menu_is_laid_out_inside_the_window_with_room_for_its_shortcuts() {
        let items = menu_items(&[RowState::Saved], &[WheelKind::Tools]);
        let rows = menu_rows(&items, &width);
        let bounds = layout::Rect::new(0.0, 0.0, 400.0, 300.0);
        let m = RowMenu::new(items, &rows, (390.0, 290.0), bounds, vec![], None, false);
        let r = m.layout.rect;
        assert!(r.x >= 0.0 && r.right() <= 400.0 && r.y >= 0.0 && r.bottom() <= 300.0);
        assert_eq!(m.layout.rows.len(), m.items.len());
        // Tools…'s shortcut is measured.
        assert!(matches!(rows[0], MenuRow::Item { accel, .. } if accel > 0.0));
    }
}
