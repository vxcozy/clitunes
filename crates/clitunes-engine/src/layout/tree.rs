use super::parser::{NodeDef, SplitDirection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn area(&self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

#[derive(Debug, Clone)]
pub struct PaneAssignment {
    pub component: String,
    pub rect: Rect,
}

/// Walk the layout tree and compute pane assignments for a given terminal area.
pub fn resolve_layout(root: &NodeDef, area: Rect) -> Vec<PaneAssignment> {
    let mut panes = Vec::new();
    walk(root, area, &mut panes);
    panes
}

fn walk(node: &NodeDef, area: Rect, panes: &mut Vec<PaneAssignment>) {
    match node {
        NodeDef::Component { component } => {
            panes.push(PaneAssignment {
                component: component.clone(),
                rect: area,
            });
        }
        NodeDef::Split {
            split,
            ratios,
            children,
        } => {
            let total_ratio: u16 = ratios.iter().sum();
            if total_ratio == 0 || children.is_empty() {
                return;
            }

            let (total_size, start_pos, is_horizontal) = match split {
                SplitDirection::Horizontal => (area.width, area.x, true),
                SplitDirection::Vertical => (area.height, area.y, false),
            };

            let mut pos = start_pos;
            for (i, (ratio, child)) in ratios.iter().zip(children.iter()).enumerate() {
                let is_last = i == children.len() - 1;
                let child_size = if is_last {
                    // Give remaining space to last child to avoid rounding gaps
                    let end = if is_horizontal {
                        area.x + area.width
                    } else {
                        area.y + area.height
                    };
                    end.saturating_sub(pos)
                } else {
                    (total_size as u32 * *ratio as u32 / total_ratio as u32) as u16
                };

                let child_area = if is_horizontal {
                    Rect::new(pos, area.y, child_size, area.height)
                } else {
                    Rect::new(area.x, pos, area.width, child_size)
                };

                walk(child, child_area, panes);
                pos += child_size;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::parser::SplitDirection;

    #[test]
    fn single_component_fills_area() {
        let node = NodeDef::Component {
            component: "visualiser".into(),
        };
        let area = Rect::new(0, 0, 80, 24);
        let panes = resolve_layout(&node, area);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].component, "visualiser");
        assert_eq!(panes[0].rect, area);
    }

    #[test]
    fn horizontal_split_divides_width() {
        let node = NodeDef::Split {
            split: SplitDirection::Horizontal,
            ratios: vec![3, 1],
            children: vec![
                NodeDef::Component {
                    component: "visualiser".into(),
                },
                NodeDef::Component {
                    component: "source-browser".into(),
                },
            ],
        };
        let area = Rect::new(0, 0, 80, 24);
        let panes = resolve_layout(&node, area);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].component, "visualiser");
        assert_eq!(panes[1].component, "source-browser");
        // 3/4 * 80 = 60, 1/4 * 80 = 20
        assert_eq!(panes[0].rect.width, 60);
        assert_eq!(panes[1].rect.width, 20);
        assert_eq!(panes[0].rect.height, 24);
    }

    #[test]
    fn vertical_split_divides_height() {
        let node = NodeDef::Split {
            split: SplitDirection::Vertical,
            ratios: vec![4, 1],
            children: vec![
                NodeDef::Component {
                    component: "visualiser".into(),
                },
                NodeDef::Component {
                    component: "now-playing".into(),
                },
            ],
        };
        let area = Rect::new(0, 0, 80, 25);
        let panes = resolve_layout(&node, area);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].rect.height, 20);
        assert_eq!(panes[1].rect.height, 5);
    }
}
