use std::collections::{HashMap, HashSet};

use serde::Deserialize;

const KNOWN_COMPONENTS: &[&str] = &[
    "visualiser",
    "now-playing",
    "source-browser",
    "queue",
    "mini-spectrum",
    "command-bar",
];

#[derive(Debug, Deserialize)]
pub struct LayoutFile {
    pub layouts: HashMap<String, LayoutDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutDef {
    pub root: NodeDef,
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default = "default_min_size")]
    pub min_size: MinSize,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct MinSize {
    pub cols: u16,
    pub rows: u16,
}

fn default_min_size() -> MinSize {
    MinSize { cols: 1, rows: 1 }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum NodeDef {
    Split {
        split: SplitDirection,
        ratios: Vec<u16>,
        children: Vec<NodeDef>,
    },
    Component {
        component: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("layout '{layout}': split has {ratios} ratios but {children} children")]
    RatioMismatch {
        layout: String,
        ratios: usize,
        children: usize,
    },
    #[error("layout '{layout}': unknown component '{component}'. Available: {available}")]
    UnknownComponent {
        layout: String,
        component: String,
        available: String,
    },
    #[error("layout fallback cycle detected: {cycle}")]
    FallbackCycle { cycle: String },
}

pub fn parse_layout_file(toml_str: &str) -> Result<LayoutFile, LayoutError> {
    let file: LayoutFile = toml::from_str(toml_str)?;
    for (name, def) in &file.layouts {
        validate_node(name, &def.root)?;
    }
    detect_fallback_cycles(&file)?;
    Ok(file)
}

fn validate_node(layout_name: &str, node: &NodeDef) -> Result<(), LayoutError> {
    match node {
        NodeDef::Split {
            ratios, children, ..
        } => {
            if ratios.len() != children.len() {
                return Err(LayoutError::RatioMismatch {
                    layout: layout_name.to_string(),
                    ratios: ratios.len(),
                    children: children.len(),
                });
            }
            for child in children {
                validate_node(layout_name, child)?;
            }
        }
        NodeDef::Component { component } => {
            if !KNOWN_COMPONENTS.contains(&component.as_str()) {
                return Err(LayoutError::UnknownComponent {
                    layout: layout_name.to_string(),
                    component: component.clone(),
                    available: KNOWN_COMPONENTS.join(", "),
                });
            }
        }
    }
    Ok(())
}

fn detect_fallback_cycles(file: &LayoutFile) -> Result<(), LayoutError> {
    for start_name in file.layouts.keys() {
        let mut visited = HashSet::new();
        let mut current = start_name.as_str();
        visited.insert(current.to_string());

        while let Some(def) = file.layouts.get(current) {
            if let Some(ref fb) = def.fallback {
                if !visited.insert(fb.clone()) {
                    let cycle: Vec<_> = visited.into_iter().collect();
                    return Err(LayoutError::FallbackCycle {
                        cycle: cycle.join(" -> "),
                    });
                }
                current = fb;
            } else {
                break;
            }
        }
    }
    Ok(())
}
