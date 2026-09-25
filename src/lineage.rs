//! 门控系谱：节点（门/群）与父子边分开版本化。
//! - 改名（label 变化）不改变节点稳定身份。
//! - 上游修订时，用依赖图计算失效范围；只影响下游闭包（局部失效，不全局替换）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LineageNode {
    /// 根群：全部已补偿事件。
    Root { id: String, label: String },
    /// 门控群。
    Gate {
        id: String,
        label: String,
        gate_material_version_id: String,
    },
}

impl LineageNode {
    pub fn id(&self) -> &str {
        match self {
            LineageNode::Root { id, .. } | LineageNode::Gate { id, .. } => id,
        }
    }
    pub fn label(&self) -> &str {
        match self {
            LineageNode::Root { label, .. } | LineageNode::Gate { label, .. } => label,
        }
    }
    pub fn with_label(self, label: String) -> Self {
        match self {
            LineageNode::Root { id, .. } => LineageNode::Root { id, label },
            LineageNode::Gate { id, gate_material_version_id, .. } => {
                LineageNode::Gate { id, label, gate_material_version_id }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageEdge {
    pub parent: String,
    pub child: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageDoc {
    /// 顺序是稳定顺序（导出/导入后保持）。
    pub nodes: Vec<LineageNode>,
    pub edges: Vec<LineageEdge>,
}

impl LineageDoc {
    pub fn node(&self, id: &str) -> Option<&LineageNode> {
        self.nodes.iter().find(|n| n.id() == id)
    }

    pub fn parents(&self, id: &str) -> Vec<&str> {
        self.edges.iter().filter(|e| e.child == id).map(|e| e.parent.as_str()).collect()
    }

    pub fn children(&self, id: &str) -> Vec<&str> {
        self.edges.iter().filter(|e| e.parent == id).map(|e| e.child.as_str()).collect()
    }

    /// 改名：仅 label 变化，节点 id 与下游引用保持不变。
    pub fn rename(&mut self, id: &str, new_label: String) -> bool {
        if let Some(slot) = self.nodes.iter_mut().find(|n| n.id() == id) {
            let old = slot.clone();
            *slot = old.with_label(new_label);
            true
        } else {
            false
        }
    }

    /// 受影响闭包：从修订节点出发（含自身）的全部下游节点。
    /// 用于"上游过期传播"——范围被限制在真实依赖内。
    pub fn affected_scope(&self, changed_id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![changed_id.to_string()];
        while let Some(cur) = stack.pop() {
            if out.contains(&cur) {
                continue;
            }
            out.push(cur.clone());
            for ch in self.children(&cur) {
                stack.push(ch.to_string());
            }
        }
        out.sort();
        out
    }

    pub fn validate(&self) -> Result<(), String> {
        for e in &self.edges {
            if self.node(&e.parent).is_none() {
                return Err(format!("edge references missing parent {}", e.parent));
            }
            if self.node(&e.child).is_none() {
                return Err(format!("edge references missing child {}", e.child));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> LineageDoc {
        LineageDoc {
            nodes: vec![
                LineageNode::Root { id: "root".into(), label: "All".into() },
                LineageNode::Gate { id: "lym".into(), label: "Lymphocytes".into(), gate_material_version_id: "g1".into() },
                LineageNode::Gate { id: "cd3".into(), label: "CD3+".into(), gate_material_version_id: "g2".into() },
                LineageNode::Gate { id: "cd4".into(), label: "CD4+".into(), gate_material_version_id: "g3".into() },
            ],
            edges: vec![
                LineageEdge { parent: "root".into(), child: "lym".into() },
                LineageEdge { parent: "lym".into(), child: "cd3".into() },
                LineageEdge { parent: "cd3".into(), child: "cd4".into() },
            ],
        }
    }

    #[test]
    fn rename_keeps_identity_and_edges() {
        let mut d = doc();
        assert!(d.rename("cd3", "T cells".into()));
        let n = d.node("cd3").unwrap();
        assert_eq!(n.label(), "T cells");
        assert_eq!(d.children("lym"), vec!["cd3"]);
    }

    #[test]
    fn affected_scope_is_downstream_closure_only() {
        let d = doc();
        assert_eq!(d.affected_scope("lym"), vec!["cd3", "cd4", "lym"]);
        // 修订叶子只影响自己
        assert_eq!(d.affected_scope("cd4"), vec!["cd4"]);
    }
}
