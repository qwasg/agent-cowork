use crate::{ElementType, Geo};
use serde_json::{json, Map, Value};

pub const SLIDE_WIDTH: f64 = 10.0;
pub const SLIDE_HEIGHT: f64 = 5.625;

#[derive(Debug, Clone)]
pub struct SeedElement {
    pub role: &'static str,
    pub element_type: ElementType,
    pub geo: Geo,
    pub props: Map<String, Value>,
}

fn props(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Return layout seed elements. Mirrors
/// `docforge/packages/doc-core/src/layouts.ts`.
pub fn get_layout_seeds(layout: &str) -> Vec<SeedElement> {
    match layout {
        "title" => vec![
            SeedElement {
                role: "title",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 1.0,
                    y: 2.0,
                    w: 8.0,
                    h: 1.2,
                    rot: None,
                },
                props: props(json!({
                    "text": "标题",
                    "fontSize": 40,
                    "bold": true,
                    "align": "center"
                })),
            },
            SeedElement {
                role: "subtitle",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 1.5,
                    y: 3.3,
                    w: 7.0,
                    h: 0.8,
                    rot: None,
                },
                props: props(json!({
                    "text": "副标题",
                    "fontSize": 20,
                    "color": "666666",
                    "align": "center"
                })),
            },
        ],
        "titleBody" => vec![
            SeedElement {
                role: "title",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 0.6,
                    y: 0.4,
                    w: 8.8,
                    h: 1.0,
                    rot: None,
                },
                props: props(json!({
                    "text": "标题",
                    "fontSize": 32,
                    "bold": true
                })),
            },
            SeedElement {
                role: "body",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 0.6,
                    y: 1.6,
                    w: 8.8,
                    h: 3.5,
                    rot: None,
                },
                props: props(json!({
                    "text": "正文内容",
                    "fontSize": 18
                })),
            },
        ],
        "twoContent" => vec![
            SeedElement {
                role: "title",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 0.6,
                    y: 0.4,
                    w: 8.8,
                    h: 1.0,
                    rot: None,
                },
                props: props(json!({
                    "text": "标题",
                    "fontSize": 32,
                    "bold": true
                })),
            },
            SeedElement {
                role: "left",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 0.6,
                    y: 1.6,
                    w: 4.2,
                    h: 3.5,
                    rot: None,
                },
                props: props(json!({
                    "text": "左侧",
                    "fontSize": 16
                })),
            },
            SeedElement {
                role: "right",
                element_type: ElementType::Text,
                geo: Geo {
                    x: 5.2,
                    y: 1.6,
                    w: 4.2,
                    h: 3.5,
                    rot: None,
                },
                props: props(json!({
                    "text": "右侧",
                    "fontSize": 16
                })),
            },
        ],
        _ => Vec::new(),
    }
}
