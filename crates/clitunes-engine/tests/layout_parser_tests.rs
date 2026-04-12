use clitunes_engine::layout::{parse_layout_file, resolve_layout, Rect};

#[test]
fn parse_default_layout() {
    let toml = include_str!("../examples/layout_default.toml");
    let file = parse_layout_file(toml).expect("valid layout");
    assert!(file.layouts.contains_key("default"));

    let def = &file.layouts["default"];
    assert_eq!(def.min_size.cols, 80);
    assert_eq!(def.min_size.rows, 24);
    assert_eq!(def.fallback.as_deref(), Some("compact"));
}

#[test]
fn default_layout_visualiser_gets_largest_rect() {
    let toml = include_str!("../examples/layout_default.toml");
    let file = parse_layout_file(toml).unwrap();
    let panes = resolve_layout(&file.layouts["default"].root, Rect::new(0, 0, 120, 40));

    let vis_pane = panes.iter().find(|p| p.component == "visualiser").unwrap();
    for pane in &panes {
        assert!(
            vis_pane.rect.area() >= pane.rect.area(),
            "visualiser area {} should be >= {} area {}",
            vis_pane.rect.area(),
            pane.component,
            pane.rect.area()
        );
    }
}

#[test]
fn parse_all_example_layouts() {
    for name in &["default", "compact", "minimal", "pure", "fullscreen"] {
        let path = format!("examples/layout_{}.toml", name);
        let toml =
            std::fs::read_to_string(std::path::Path::new("crates/clitunes-engine").join(&path))
                .unwrap_or_else(|_| {
                    // Try from the crate root (when tests run from crate dir)
                    std::fs::read_to_string(&path).unwrap()
                });
        parse_layout_file(&toml).unwrap_or_else(|e| panic!("{} failed: {}", name, e));
    }
}

#[test]
fn unknown_component_error() {
    let toml = r#"
[layouts.test]
[layouts.test.min_size]
cols = 20
rows = 5
[layouts.test.root]
component = "fnord"
"#;
    let err = parse_layout_file(toml).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("fnord"),
        "error should mention the component: {}",
        msg
    );
    assert!(
        msg.contains("visualiser"),
        "error should list available components: {}",
        msg
    );
}

#[test]
fn ratio_mismatch_error() {
    let toml = r#"
[layouts.test]
[layouts.test.min_size]
cols = 20
rows = 5
[layouts.test.root]
split = "horizontal"
ratios = [1, 2, 3]
[[layouts.test.root.children]]
component = "visualiser"
[[layouts.test.root.children]]
component = "now-playing"
"#;
    let err = parse_layout_file(toml).unwrap_err();
    assert!(err.to_string().contains("ratios"), "error: {}", err);
}

#[test]
fn fallback_cycle_detected() {
    let toml = r#"
[layouts.a]
fallback = "b"
[layouts.a.min_size]
cols = 80
rows = 24
[layouts.a.root]
component = "visualiser"

[layouts.b]
fallback = "a"
[layouts.b.min_size]
cols = 40
rows = 12
[layouts.b.root]
component = "visualiser"
"#;
    let err = parse_layout_file(toml).unwrap_err();
    assert!(err.to_string().contains("cycle"), "error: {}", err);
}

#[test]
fn missing_min_size_defaults() {
    let toml = r#"
[layouts.test]
[layouts.test.root]
component = "visualiser"
"#;
    let file = parse_layout_file(toml).unwrap();
    assert_eq!(file.layouts["test"].min_size.cols, 1);
    assert_eq!(file.layouts["test"].min_size.rows, 1);
}
