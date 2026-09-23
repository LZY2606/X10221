//! Read-model: replay a branch's immutable event log up to an optional seq and
//! report the visible versions, staleness, provenance and candidates.

use super::engine::Engine;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, serde::Serialize)]
pub struct VersionView {
    pub node_kind: String,
    pub node_id: String,
    pub version_uid: String,
    pub event_id: i64,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub stale: bool,
    /// (upstream kind,node,version-uid) this version was computed from.
    pub deps: Vec<(String, String, String)>,
}

#[derive(Debug, serde::Serialize)]
pub struct View {
    pub branch_id: String,
    pub at_seq: i64,
    pub head_seq: i64,
    pub nodes: Vec<VersionView>,
    pub candidates: Vec<CandidateView>,
    pub decision: Option<DecisionView>,
    pub gate_tree: Vec<TreeNode>,
}

#[derive(Debug, serde::Serialize)]
pub struct CandidateView {
    pub candidate_id: String,
    pub node_id: String,
    pub label: String,
    pub payload: Value,
    pub produced_event_id: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct DecisionView {
    pub node_id: String,
    pub active: bool,
    pub chosen_candidate_id: Option<String>,
    pub decided_event_id: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct TreeNode {
    pub gate_id: String,
    pub label: String,
    pub parent: Option<String>,
    pub population_version: Option<String>,
    pub percent: Option<f64>,
    pub denominator_version: Option<String>,
    pub stale: bool,
}

impl Engine {
    pub fn view(&self, branch: &str, at_seq: Option<i64>) -> Result<View, String> {
        let c = self.db.lock_conn();
        let head: i64 = c
            .query_row(
                "SELECT head_seq FROM branches WHERE branch_id=?1",
                rusqlite::params![branch],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let limit = at_seq.unwrap_or(head).min(head);

        let mut latest: HashMap<(String, String), (i64, i64, String, String, Value)> =
            HashMap::new();
        let mut deps_map: HashMap<i64, Vec<(String, String, String)>> = HashMap::new();

        let mut stmt = c
            .prepare(
                "SELECT event_id,seq,kind,node_kind,node_id,version_uid,payload
                 FROM events WHERE branch_id=?1 AND seq<=?2 ORDER BY seq",
            )
            .unwrap();
        let rows = stmt
            .query_map(rusqlite::params![branch, limit], |r| {
                let eid: i64 = r.get(0)?;
                let seq: i64 = r.get(1)?;
                let kind: String = r.get(2)?;
                let nk: Option<String> = r.get(3)?;
                let nid: Option<String> = r.get(4)?;
                let vu: Option<String> = r.get(5)?;
                let pl: String = r.get(6)?;
                Ok((eid, seq, kind, nk, nid, vu, pl))
            })
            .unwrap();
        for r in rows {
            let (eid, seq, kind, nk, nid, vu, pl) = r.map_err(|e| e.to_string())?;
            if let (Some(nk), Some(nid), Some(vu)) = (nk, nid, vu) {
                // A blocked matrix is a version but a later real version wins.
                latest.insert(
                    (nk, nid),
                    (eid, seq, vu, kind, serde_json::from_str(&pl).unwrap_or(Value::Null)),
                );
            }
        }
        let mut dstmt = c
            .prepare(
                "SELECT e.event_id, d.upstream_kind, d.upstream_node, d.upstream_version_uid
                 FROM deps d JOIN events e ON e.event_id=d.producer_event_id
                 WHERE e.branch_id=?1 AND e.seq<=?2",
            )
            .unwrap();
        let drows = dstmt
            .query_map(rusqlite::params![branch, limit], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
            })
            .unwrap();
        for r in drows.flatten() {
            deps_map.entry(r.0).or_default().push((r.1, r.2, r.3));
        }

        // Stale: any dependency points at a version that is not the current one.
        let current_vuid = |kind: &str, node: &str| -> Option<String> {
            latest.get(&(kind.to_string(), node.to_string())).map(|v| v.2.clone())
        };
        let mut nodes = Vec::new();
        for ((nk, nid), (eid, seq, vu, kind, payload)) in latest.iter() {
            let deps = deps_map.remove(eid).unwrap_or_default();
            let stale = deps.iter().any(|(k, n, v)| current_vuid(k, n).as_ref() != Some(v));
            nodes.push(VersionView {
                node_kind: nk.clone(),
                node_id: nid.clone(),
                version_uid: vu.clone(),
                event_id: *eid,
                seq: *seq,
                kind: kind.clone(),
                payload: payload.clone(),
                stale,
                deps,
            });
        }
        nodes.sort_by(|a, b| (a.node_kind.clone(), a.node_id.clone(), a.seq).cmp(&(b.node_kind.clone(), b.node_id.clone(), b.seq)));

        let mut candidates = Vec::new();
        let mut cstmt = c
            .prepare(
                "SELECT candidate_id,node_id,label,payload,produced_event_id
                 FROM candidates WHERE branch_id=?1",
            )
            .unwrap();
        for r in cstmt
            .query_map(rusqlite::params![branch], |r| {
                let pl: String = r.get(3)?;
                Ok(CandidateView {
                    candidate_id: r.get(0)?,
                    node_id: r.get(1)?,
                    label: r.get(2)?,
                    payload: serde_json::from_str(&pl).unwrap_or(Value::Null),
                    produced_event_id: r.get(4)?,
                })
            })
            .unwrap()
            .flatten()
        {
            candidates.push(r);
        }

        let decision = c
            .query_row(
                "SELECT node_id,active,chosen_candidate_id,decided_event_id
                 FROM decisions WHERE branch_id=?1 AND active=1",
                rusqlite::params![branch],
                |r| {
                    Ok(DecisionView {
                        node_id: r.get(0)?,
                        active: r.get(1)?,
                        chosen_candidate_id: r.get(2)?,
                        decided_event_id: r.get(3)?,
                    })
                },
            )
            .ok();

        // gate tree from current edges + gates + populations
        let mut tree = Vec::new();
        let edges = nodes
            .iter()
            .find(|n| n.node_kind == "gate_edges")
            .and_then(|n| serde_json::from_value::<super::model::GateEdges>(n.payload.clone()).ok());
        if let Some(edges) = edges {
            for edge in &edges.edges {
                let gate = nodes
                    .iter()
                    .find(|n| n.node_kind == "gate_polygon" && n.node_id == edge.child_gate)
                    .and_then(|n| n.payload.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()));
                let pop = nodes.iter().find(|n| {
                    n.node_kind == "population"
                        && n.node_id == format!("pop#{}", edge.child_gate)
                });
                tree.push(TreeNode {
                    gate_id: edge.child_gate.clone(),
                    label: gate.unwrap_or_else(|| edge.child_gate.clone()),
                    parent: edge.parent_gate.clone(),
                    population_version: pop.map(|p| p.version_uid.clone()),
                    percent: pop.and_then(|p| p.payload.get("percent").and_then(|v| v.as_f64())),
                    denominator_version: pop.and_then(|p| {
                        p.payload
                            .get("denominator_version")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    }),
                    stale: pop.map(|p| p.stale).unwrap_or(false),
                });
            }
        }
        tree.sort_by(|a, b| a.gate_id.cmp(&b.gate_id));

        Ok(View {
            branch_id: branch.to_string(),
            at_seq: limit,
            head_seq: head,
            nodes,
            candidates,
            decision,
            gate_tree: tree,
        })
    }
}
