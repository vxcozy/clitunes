use std::collections::HashMap;

use clitunes_engine::layout::{parse_layout_file, select_layout, LayoutDef};

fn all_layouts() -> HashMap<String, LayoutDef> {
    let mut layouts = HashMap::new();
    for name in &["default", "compact", "minimal", "fullscreen"] {
        let path = format!("examples/layout_{}.toml", name);
        let content =
            std::fs::read_to_string(std::path::Path::new("crates/clitunes-engine").join(&path))
                .unwrap_or_else(|_| std::fs::read_to_string(&path).unwrap());
        let file = parse_layout_file(&content).unwrap();
        for (k, v) in file.layouts {
            layouts.insert(k, v);
        }
    }
    layouts
}

#[test]
fn large_terminal_selects_default() {
    let layouts = all_layouts();
    let (name, _) = select_layout(&layouts, "default", 120, 40).unwrap();
    assert_eq!(name, "default");
}

#[test]
fn resize_ladder_falls_through() {
    let layouts = all_layouts();
    // 50x15: default (80x24) fails, compact (60x18) fails, minimal (40x6) fits
    let (name, _) = select_layout(&layouts, "default", 50, 15).unwrap();
    assert_eq!(name, "minimal");
}

#[test]
fn tiny_terminal_returns_none() {
    let layouts = all_layouts();
    assert!(select_layout(&layouts, "default", 10, 3).is_none());
}
