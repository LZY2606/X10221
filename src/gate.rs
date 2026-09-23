//! Gate membership geometry. Boundary points are first-class: callers pass a
//! boundary rule and both inclusive/exclusive counts are derivable.

use crate::domain::{GateDoc, GateShape, Vertex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryRule {
    /// Point on an edge counts as inside.
    Inclusive,
    /// Point on an edge counts as outside (disjoint sibling gates).
    Exclusive,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Membership {
    /// "in" / "out" / "boundary" (boundary resolved by rule to in/out).
    pub status: String,
    pub on_boundary: bool,
}

pub fn classify(gate: &GateDoc, x: f64, y: f64, rule: BoundaryRule) -> Membership {
    let on = match gate.shape {
        GateShape::Polygon => point_on_polygon(&gate.vertices, x, y),
        GateShape::Rectangle => point_on_rect(&gate.vertices, x, y),
    };
    if on {
        return Membership {
            status: match rule {
                BoundaryRule::Inclusive => "in".into(),
                BoundaryRule::Exclusive => "out".into(),
            },
            on_boundary: true,
        };
    }
    let inside = match gate.shape {
        GateShape::Polygon => point_in_polygon(&gate.vertices, x, y),
        GateShape::Rectangle => point_in_rect(&gate.vertices, x, y),
    };
    Membership {
        status: if inside { "in" } else { "out" }.into(),
        on_boundary: false,
    }
}

fn bbox(verts: &[Vertex]) -> (f64, f64, f64, f64) {
    let mut xmin = f64::INFINITY;
    let mut xmax = f64::NEG_INFINITY;
    let mut ymin = f64::INFINITY;
    let mut ymax = f64::NEG_INFINITY;
    for v in verts {
        xmin = xmin.min(v.x);
        xmax = xmax.max(v.x);
        ymin = ymin.min(v.y);
        ymax = ymax.max(v.y);
    }
    (xmin, ymin, xmax, ymax)
}

fn point_on_rect(verts: &[Vertex], x: f64, y: f64) -> bool {
    if verts.len() < 2 {
        return false;
    }
    let (xmin, ymin, xmax, ymax) = bbox(verts);
    let in_box = x >= xmin - 1e-9 && x <= xmax + 1e-9 && y >= ymin - 1e-9 && y <= ymax + 1e-9;
    if !in_box {
        return false;
    }
    let on_x = (x - xmin).abs() < 1e-9 || (x - xmax).abs() < 1e-9;
    let on_y = (y - ymin).abs() < 1e-9 || (y - ymax).abs() < 1e-9;
    on_x || on_y
}

fn point_on_segment(a: &Vertex, b: &Vertex, x: f64, y: f64) -> bool {
    let cross = (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x);
    if cross.abs() > 1e-9 {
        return false;
    }
    let dot = (x - a.x) * (x - b.x) + (y - a.y) * (y - b.y);
    dot <= 1e-9
}

fn point_on_polygon(verts: &[Vertex], x: f64, y: f64) -> bool {
    if verts.len() < 2 {
        return false;
    }
    for i in 0..verts.len() {
        let a = &verts[i];
        let b = &verts[(i + 1) % verts.len()];
        if point_on_segment(a, b, x, y) {
            return true;
        }
    }
    false
}

fn point_in_polygon(verts: &[Vertex], x: f64, y: f64) -> bool {
    if verts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let n = verts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (verts[i].x, verts[i].y);
        let (xj, yj) = (verts[j].x, verts[j].y);
        if (yi > y) != (yj > y) {
            let xcross = xi + (xj - xi) * (y - yi) / (yj - yi);
            if x < xcross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}
