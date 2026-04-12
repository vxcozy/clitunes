use std::collections::{HashMap, HashSet};

use super::parser::LayoutDef;

/// Select the best layout for the given terminal size, following the fallback chain.
/// Returns the layout name and definition, or None if nothing fits.
pub fn select_layout<'a>(
    layouts: &'a HashMap<String, LayoutDef>,
    start_layout: &'a str,
    cols: u16,
    rows: u16,
) -> Option<(&'a str, &'a LayoutDef)> {
    let mut visited = HashSet::new();
    let mut current = start_layout;

    loop {
        if !visited.insert(current.to_string()) {
            return None; // cycle detected (defensive)
        }

        let def = layouts.get(current)?;

        if cols >= def.min_size.cols && rows >= def.min_size.rows {
            return Some((current, def));
        }

        match &def.fallback {
            Some(fb) => current = fb,
            None => return None,
        }
    }
}

/// Message to show when no layout fits the terminal.
pub fn too_small_message(min_cols: u16, min_rows: u16) -> String {
    format!(
        "clitunes — resize to at least {}x{} to display",
        min_cols, min_rows
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::parser::{LayoutDef, MinSize, NodeDef};

    fn make_layouts() -> HashMap<String, LayoutDef> {
        let mut layouts = HashMap::new();
        let component = NodeDef::Component {
            component: "visualiser".into(),
        };

        layouts.insert(
            "default".into(),
            LayoutDef {
                root: component.clone(),
                fallback: Some("compact".into()),
                min_size: MinSize { cols: 80, rows: 24 },
            },
        );
        layouts.insert(
            "compact".into(),
            LayoutDef {
                root: component.clone(),
                fallback: Some("minimal".into()),
                min_size: MinSize { cols: 60, rows: 18 },
            },
        );
        layouts.insert(
            "minimal".into(),
            LayoutDef {
                root: component.clone(),
                fallback: Some("fullscreen".into()),
                min_size: MinSize { cols: 40, rows: 6 },
            },
        );
        layouts.insert(
            "fullscreen".into(),
            LayoutDef {
                root: component,
                fallback: None,
                min_size: MinSize { cols: 20, rows: 5 },
            },
        );
        layouts
    }

    #[test]
    fn large_terminal_selects_default() {
        let layouts = make_layouts();
        let (name, _) = select_layout(&layouts, "default", 120, 40).unwrap();
        assert_eq!(name, "default");
    }

    #[test]
    fn medium_terminal_falls_back_to_compact() {
        let layouts = make_layouts();
        let (name, _) = select_layout(&layouts, "default", 70, 20).unwrap();
        assert_eq!(name, "compact");
    }

    #[test]
    fn small_terminal_falls_through_to_minimal() {
        let layouts = make_layouts();
        let (name, _) = select_layout(&layouts, "default", 50, 10).unwrap();
        assert_eq!(name, "minimal");
    }

    #[test]
    fn tiny_terminal_returns_none() {
        let layouts = make_layouts();
        assert!(select_layout(&layouts, "default", 10, 3).is_none());
    }
}
