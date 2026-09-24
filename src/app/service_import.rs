//! 内置“流式边界室”案例包的导入流水线（FCS → 领域对象 → 单一 ImportCommitted 事件）。

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::app::service::{err, Service, SrvResult};
use crate::casepack;
use crate::db;
use crate::domain::events::{EventKind, Judgement};
use crate::domain::fcs::parse_fcs;
use crate::domain::gate::{CoordBasis, Point, PolygonGate};
use crate::domain::ids::{content_rev, content_rev_parts, stable_id};
use crate::domain::matrix::{invert_spillover, InversionStatus};
use crate::domain::model::*;
use crate::domain::derive::derive_tree;
use crate::domain::project::*;
use crate::domain::transform::TransformSpec;

const CASE_AT: &str = "2026-09-19T00:33:12+09:00";

impl Service {
    /// 载入仓库内置案例包（生成真实 FCS 字节后走解析路径），原子提交到 main。
    pub fn load_builtin_case(&self) -> SrvResult<ImportSummary> {
        let pack = casepack::boundary_chamber();
        let mut conn = self.conn.lock().unwrap();
        self.import_pack(&mut conn, &pack, None, MAIN_BRANCH, "system", CASE_AT)
    }

    pub(crate) fn import_pack(
        &self,
        conn: &mut Connection,
        pack: &casepack::CasePack,
        filename_override: Option<&str>,
        branch: &str,
        actor: &str,
        at: &str,
    ) -> SrvResult<ImportSummary> {
        // 若分支已有事件则拒绝，避免半份/重复导入（调用方应在空库或新分支执行）。
        if db::head_hash(conn, branch).map_err(|e| err("db", e.0))?.is_some() {
            return Err(err("IMPORT_CONFLICT", format!("分支 {branch} 已有数据，拒绝覆盖式导入")));
        }

        // 1) 原始字节进 blob 表（FCS + 单染对照 + spillover 草案 + 门控草案）。
        // 解析必须成功，否则整体回滚（事务由 append_event/insert_blob 在同一连接顺序执行；
        // 这里先做解析验证，任何错误都在提交事件之前返回）。
        let fcs = parse_fcs(&pack.fcs_bytes)
            .map_err(|e| err("FCS_PARSE", format!("FCS 无法解析，导入整体回滚：{e}")))?;
        if fcs.events.is_empty() {
            return Err(err("FCS_EMPTY", "FCS 无事件，导入回滚".to_string()));
        }

        // blob 元信息
        let mut blob_infos = Vec::new();
        let fcs_blob_id = pack.fcs_blob_id.clone();
        let fcs_sha = content_rev(&String::from_utf8_lossy(&pack.fcs_bytes));
        blob_infos.push(BlobInfo {
            blob_id: fcs_blob_id.clone(),
            kind: "fcs".into(),
            filename: filename_override.map(|s| s.to_string()).unwrap_or_else(|| pack.fcs_filename.clone()),
            byte_len: pack.fcs_bytes.len(),
            sha256: fcs_sha,
        });
        for b in &pack.extra_blobs {
            blob_infos.push(BlobInfo {
                blob_id: b.blob_id.clone(),
                kind: b.kind.clone(),
                filename: b.filename.clone(),
                byte_len: b.bytes.len(),
                sha256: content_rev(&String::from_utf8_lossy(&b.bytes)),
            });
        }

        // 2) 通道（保留原始表示 + 单位）
        let channels: Vec<ChannelSpec> = fcs
            .channels
            .iter()
            .zip(&fcs.channel_labels)
            .map(|(name, label)| {
                let unit = pack.channel_units.get(name).cloned().unwrap_or_else(|| "a.u.".into());
                ChannelSpec { name: name.clone(), raw_label: label.clone(), unit, range_max: 262_144.0 }
            })
            .collect();

        // 3) 原始事件（NaN/Inf 原样；诊断随行）
        let mut raw_events = Vec::new();
        let mut event_order = Vec::new();
        for (i, row) in fcs.events.iter().enumerate() {
            let eid = format!("event#{}", i + 1);
            let mut vals = BTreeMap::new();
            for (c, v) in fcs.channels.iter().zip(row) {
                vals.insert(c.clone(), *v as f64);
            }
            raw_events.push(RawEventRow {
                event_id: eid.clone(),
                values: vals,
                diagnostics: fcs.event_diagnostics[i].clone(),
            });
            event_order.push(eid);
        }

        // 4) spillover 元数据（与补偿矩阵分开版本化）
        let spill_rev = content_rev_parts(&[
            "spillover",
            &pack.spillover.channels.join(","),
            &canon_matrix(&pack.spillover.matrix),
            &pack.spillover.single_stain_blob_ids.join(","),
        ]);
        let spill = SpilloverMeta {
            stable_id: pack.spillover.stable_id.clone(),
            ordinal: 1,
            rev: spill_rev.clone(),
            channels: pack.spillover.channels.clone(),
            single_stain_blobs: pack.spillover.single_stain_blob_ids.clone(),
            source_note: pack.spillover.note.clone(),
        };

        // 5) 补偿矩阵：求逆判定（近奇异 → 多候选，不默默选）
        let inv = invert_spillover(&pack.spillover.matrix);
        let comp_status = match inv.status {
            InversionStatus::Ok => "ok",
            InversionStatus::NeedsDecision => "needs_decision",
            InversionStatus::Rejected => "rejected",
        };
        let matrix_rev = content_rev_parts(&[
            "compmatrix",
            &pack.matrix_stable_id,
            &spill_rev,
            &canon_matrix(&pack.spillover.matrix),
            comp_status,
        ]);
        let matrix = CompensationMatrix {
            stable_id: pack.matrix_stable_id.clone(),
            ordinal: 1,
            rev: matrix_rev.clone(),
            channels: pack.spillover.channels.clone(),
            spillover: pack.spillover.matrix.clone(),
            source_spillover_rev: spill_rev.clone(),
            rcond: inv.rcond,
            status: comp_status.into(),
            chosen_candidate: if inv.status == InversionStatus::Ok {
                Some("exact_inverse".into())
            } else {
                None
            },
            candidates: inv.candidates,
            diagnostics: inv.diagnostics,
        };

        // 6) 变换参数（独立版本化）
        let mut transforms = Vec::new();
        for (channel, spec_in) in &pack.transforms {
            let spec: TransformSpec = match spec_in {
                casepack::TransformIn::Linear { offset, scale, unit } => {
                    TransformSpec::Linear { offset: *offset, scale: *scale, unit: unit.clone() }
                }
                casepack::TransformIn::ArcSinh { cofactor, unit } => {
                    TransformSpec::ArcSinh { cofactor: *cofactor, unit: unit.clone() }
                }
            };
            let rev = content_rev_parts(&["transform", channel, &serde_json::to_string(&spec).unwrap_or_default()]);
            transforms.push(TransformParam {
                stable_id: stable_id("transform", channel),
                ordinal: 1,
                rev,
                channel: channel.clone(),
                spec,
            });
        }

        // 7) 门草案（多边形 + 父子关系）
        let mut gate_versions = Vec::new();
        for g in &pack.gates {
            let rev = gate_content_rev(&g.gate_id, g.ordinal, &g.name, g.parent.as_deref(), &g.polygon);
            let gate = PolygonGate {
                stable_id: g.gate_id.clone(),
                name: g.name.clone(),
                parent_gate_id: g.parent.clone(),
                basis: CoordBasis {
                    x_channel: g.x_channel.clone(),
                    y_channel: g.y_channel.clone(),
                    x_transform_rev: transforms.iter().find(|t| t.channel == g.x_channel).map(|t| t.rev.clone()),
                    y_transform_rev: transforms.iter().find(|t| t.channel == g.y_channel).map(|t| t.rev.clone()),
                },
                polygon: g.polygon.iter().map(|(x, y)| Point { x: *x, y: *y }).collect(),
                ordinal: g.ordinal,
                rev,
            };
            gate_versions.push(GateVersion { rev: gate.rev.clone(), gate });
        }

        // 8) 初始派生（用当前状态重放一次，以门树推导初始群结果）
        let mut seed = BranchState::new(branch);
        seed.channels = channels.iter().cloned().map(|c| (c.name.clone(), c)).collect();
        seed.channel_order = channels.iter().map(|c| c.name.clone()).collect();
        seed.raw_events = raw_events.iter().cloned().map(|r| (r.event_id.clone(), r)).collect();
        seed.event_order = event_order.clone();
        seed.spillovers.insert(spill.stable_id.clone(), spill.clone());
        seed.spillover_order.push(spill.stable_id.clone());
        seed.matrices.insert(matrix.stable_id.clone(), matrix.clone());
        seed.matrix_order.push(matrix.stable_id.clone());
        seed.transforms = transforms.iter().cloned().map(|t| (t.stable_id.clone(), t)).collect();
        seed.transform_order = transforms.iter().map(|t| t.stable_id.clone()).collect();
        seed.gates = gate_versions.iter().cloned().map(|gv| (gv.gate.stable_id.clone(), vec![gv])).collect();
        seed.gate_order = gate_versions.iter().map(|gv| gv.gate.stable_id.clone()).collect();
        seed.gate_head_rev = gate_versions.iter().map(|gv| (gv.gate.stable_id.clone(), gv.rev.clone())).collect();

        let initial_populations = if matrix.comp_matrix().is_some() {
            derive_tree(&seed, &BTreeMap::new())
        } else {
            // 近奇异/拒绝时不能补偿：不产出荧光群结果，保留门定义与候选，等待裁定。
            Vec::new()
        };

        // 判定事件（机器可读）
        let mut judgements = Vec::new();
        judgements.push(Judgement {
            rule_id: "rule.separate-versioning".into(),
            status: "satisfied".into(),
            message: "原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形与父子关系分开版本化".into(),
            affected: vec![fcs_blob_id.clone(), spill.stable_id.clone(), matrix.stable_id.clone()],
        });
        match inv.status {
            InversionStatus::Ok => {}
            s => {
                judgements.push(Judgement {
                    rule_id: "rule.no-silent-inverse".into(),
                    status: if s == InversionStatus::Rejected { "rejected".into() } else { "blocked_pending_adjudication".into() },
                    message: if s == InversionStatus::Rejected {
                        "矩阵奇异：拒绝求逆，不产生补偿".into()
                    } else {
                        "近奇异：保留 exact_inverse 与 ridge_regularized 全部合法候选，等待显式裁定".into()
                    },
                    affected: vec![matrix.stable_id.clone()],
                });
            }
        }
        for pop in &initial_populations {
            judgements.push(Judgement {
                rule_id: "rule.denominator-pinned-parent".into(),
                status: "satisfied".into(),
                message: format!(
                    "初始派生：{} 分母钉住 {}@{}",
                    pop.gate_id, pop.denominator.parent_gate_id, pop.denominator.parent_gate_rev
                ),
                affected: vec![pop.gate_id.clone()],
            });
        }

        let payload = json!({
            "case_id": pack.case_id,
            "case_title": pack.title,
            "fcs_blob_id": fcs_blob_id,
            "blobs": serde_json::to_value(&blob_infos).unwrap(),
            "channels": serde_json::to_value(&channels).unwrap(),
            "raw_events": serde_json::to_value(&raw_events).unwrap(),
            "spillovers": [serde_json::to_value(&spill).unwrap()],
            "matrices": [serde_json::to_value(&matrix).unwrap()],
            "transforms": serde_json::to_value(&transforms).unwrap(),
            "gates": serde_json::to_value(&gate_versions).unwrap(),
            "populations": serde_json::to_value(&initial_populations).unwrap(),
        });

        // 9) 落库：先写 blob 字节，再追加事件；整个调用链上任何失败都不提交事件。
        for (info, bytes) in blob_infos.iter().zip(
            std::iter::once(&pack.fcs_bytes).chain(pack.extra_blobs.iter().map(|b| &b.bytes)),
        ) {
            db::insert_blob(conn, &info.blob_id, &info.kind, &info.filename, bytes, &info.sha256, at)
                .map_err(|e| err("db", e.0))?;
        }
        let out = self.append(
            conn, branch, EventKind::ImportCommitted, at, payload, judgements, None, actor,
        )?;

        Ok(ImportSummary {
            event_id: out.event_id,
            seq: out.seq,
            branch: branch.into(),
            fcs_blob_id,
            channels: channels.iter().map(|c| c.name.clone()).collect(),
            events: raw_events.len(),
            gates: gate_versions.len(),
            populations: initial_populations.len(),
            matrix_status: matrix.status.clone(),
            matrix_candidates: matrix.candidates.iter().map(|c| c.kind.clone()).collect(),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportSummary {
    pub event_id: String,
    pub seq: i64,
    pub branch: String,
    pub fcs_blob_id: String,
    pub channels: Vec<String>,
    pub events: usize,
    pub gates: usize,
    pub populations: usize,
    pub matrix_status: String,
    pub matrix_candidates: Vec<String>,
}

pub fn canon_matrix(m: &[Vec<f64>]) -> String {
    m.iter()
        .map(|r| r.iter().map(|v| format!("{v:.12}")).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join(";")
}

pub fn gate_content_rev(
    id: &str, ordinal: u64, name: &str, parent: Option<&str>, polygon: &[(f64, f64)],
) -> String {
    let poly = polygon.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(";");
    content_rev_parts(&["gate", id, &ordinal.to_string(), name, parent.unwrap_or(""), &poly])
}
