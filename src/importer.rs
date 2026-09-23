//! Case pack import and staged scenario replay.
//!
//! Import rule: everything lands in one transaction; a failure returns Err and
//! leaves no partial rows. Raw representations are preserved (FCS bytes in
//! `raw_blobs`, float bit tokens on each event); interpretation layers are
//! separate entities on separate versioned streams.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::domain::{Signal, UnitBasis};
use crate::fcs::parse_fcs;
use crate::id::{digest_hex, obj_id};
use crate::linalg::{judge_invert, NEAR_SINGULAR_THRESHOLD};
use crate::models::{
    ChannelRef, CompMatrix, GateHierarchy, GatePolygon, HierarchyNode, InvertRecord, RawEvent,
    RawFcsEvents, RawFile, SingleStainControl, SpilloverMatrix, SpilloverMeta, TransformParams,
};
use crate::spill::parse_spill_csv;
use crate::store::{
    BranchRow, ResultRow,
    ALL_STREAMS, STREAM_COMP, STREAM_GATES, STREAM_HIER, STREAM_RAW, STREAM_SPILL_MATRIX,
    STREAM_SPILL_META, STREAM_XFORM,
};
use crate::{db::FixedClock, db::Db};

#[derive(Debug)]
pub struct ImportedCase {
    pub case_id: String,
    pub root_branch: String,
}

fn stream_for(kind: &str, key: &str) -> String {
    format!("{}:{}", kind, key)
}

pub fn read_bytes_pub(base: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let p = base.join(rel);
    std::fs::read(&p).map_err(|e| format!("cannot read {}: {}", p.display(), e))
}

fn read_json(base: &Path, rel: &str) -> Result<Value, String> {
    let bytes = read_bytes(base, rel)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("bad json {}: {}", rel, e))
}

fn channel_refs(manifest: &Value) -> Vec<ChannelRef> {
    manifest["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let ub = c["basis"].clone();
            let basis: UnitBasis = serde_json::from_value(ub).unwrap();
            ChannelRef {
                index: c["index"].as_u64().unwrap() as usize,
                short_name: c["short_name"].as_str().unwrap().into(),
                long_name: c["long_name"].as_str().unwrap().into(),
                basis,
            }
        })
        .collect()
}

pub fn fcs_to_raw_events_pub(
    bytes: &[u8],
    file_id: &str,
    role: &str,
    channels_meta: &[ChannelRef],
) -> Result<RawFcsEvents, String> {
    let parsed = parse_fcs(bytes)?;
    let par = parsed.par;
    if channels_meta.len() != par {
        return Err(format!(
            "manifest channels {} != FCS $PAR {}",
            channels_meta.len(),
            par
        ));
    }
    let mut channels = Vec::new();
    for (i, meta) in channels_meta.iter().enumerate() {
        let n = i + 1;
        let short = parsed
            .keyword(&format!("$P{}N", n))
            .unwrap_or(&meta.short_name)
            .to_string();
        let long = parsed
            .keyword(&format!("$P{}S", n))
            .unwrap_or(&meta.long_name)
            .to_string();
        if short != meta.short_name {
            return Err(format!(
                "channel order mismatch: manifest {} but FCS $P{}N {}",
                meta.short_name, n, short
            ));
        }
        channels.push(ChannelRef {
            index: i,
            short_name: short,
            long_name: long,
            basis: meta.basis.clone(),
        });
    }
    let mut events = Vec::new();
    for ev in 0..parsed.tot {
        let mut values = Vec::new();
        let mut tokens = Vec::new();
        for ch in 0..par {
            let v = parsed.values[ev * par + ch];
            values.push(Signal::new(v));
            tokens.push(parsed.tokens[ev * par + ch].clone());
        }
        events.push(RawEvent {
            event_index: ev,
            values,
            raw_tokens: tokens,
        });
    }
    Ok(RawFcsEvents {
        file_id: file_id.into(),
        role: role.into(),
        channels,
        events,
    })
}

#[allow(clippy::too_many_arguments)]
fn put_entity(tx: &crate::store::TransactionCtx, case_id: &str, stream: &str, kind: &str, payload: &Value, event: &str) -> (String, i64) {
    let version = tx.query_row_i64(
        "SELECT COALESCE(MAX(version),0)+1 FROM entities WHERE case_id=?1 AND stream_id=?2",
        &[&case_id, &stream],
    );
    let bytes = crate::store::canonical(payload);
    let id = obj_id(
        stream.split(':').next().unwrap_or("ent"),
        &[version.to_le_bytes().as_slice(), bytes.as_slice()].concat(),
    );
    tx.execute(
        "INSERT INTO entities(case_id,id,stream_id,kind,version,payload_json,created_event)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        &[&case_id, &id, &stream, &kind, &version, &payload.to_string(), &event],
    );
    (id, version)
}

fn make_event_id(case_id: &str, branch: &str, seq: i64, etype: &str, payload: &Value) -> String {
    let mut bytes = case_id.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(branch.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&seq.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(etype.as_bytes());
    bytes.extend_from_slice(&crate::store::canonical(payload));
    obj_id("event", &bytes)
}

/// Import the stage-0 baseline of a case pack in one transaction.
pub fn import_case_pack(db: &Db, case_dir: &Path) -> Result<ImportedCase, String> {
    let manifest_bytes = read_bytes(case_dir, "case.json")?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes).map_err(|e| format!("case.json: {}", e))?;
    let slug = manifest["case_pack"].as_str().unwrap();
    if db.get_case_meta_by_slug(slug).is_some() {
        return Err(format!("case pack {:?} already imported", slug));
    }
    let case_id = obj_id("case", &digest_hex(&[&manifest_bytes]));
    let stage0 = manifest["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "import_baseline")
        .ok_or("manifest has no import_baseline stage")?;

    let clock = FixedClock(1_700_000_000_000);
    let ts = clock.now_ms();
    let root_branch = obj_id("branch", &[case_id.as_bytes(), b"root"].concat());

    let outcome: Result<ImportedCase, String> = db.with_transaction(|tx| {
        // Raw FCS bytes (sample).
        let fcs_rel = stage0["fcs"].as_str().unwrap();
        let fcs_bytes = read_bytes(case_dir, fcs_rel)?;
        let file_sha = digest_hex(&[&fcs_bytes]);
        let file_id = obj_id("rawfile", &fcs_bytes);
        tx.execute(
            "INSERT INTO raw_blobs(case_id,blob_id,filename,media_type,bytes,sha256)
             VALUES(?1,?2,?3,'application/vnd.isac.fcs',?4,?5)",
            &[&case_id, &file_id, &fcs_rel, &fcs_bytes, &file_sha],
        );
        let raw_file = RawFile {
            filename: fcs_rel.into(),
            media_type: "application/vnd.isac.fcs".into(),
            sha256: file_sha,
            byte_len: fcs_bytes.len(),
        };
        let _rf_id = put_entity(tx, &case_id, "raw_file:sample", "raw_file", &json!(raw_file), "ev_import");

        // Single stain controls: their raw bytes stay readable too.
        let channels_meta = channel_refs(&manifest);
        let mut single_stains = Vec::new();
        for ctrl in stage0["single_stains"].as_array().unwrap() {
            let rel = ctrl["fcs"].as_str().unwrap();
            let bytes = read_bytes(case_dir, rel)?;
            let sha = digest_hex(&[&bytes]);
            let blob = obj_id("rawfile", &bytes);
            tx.execute(
                "INSERT INTO raw_blobs(case_id,blob_id,filename,media_type,bytes,sha256)
                 VALUES(?1,?2,?3,'application/vnd.isac.fcs',?4,?5)",
                &[&case_id, &blob, &rel, &bytes, &sha],
            );
            let ctrl_raw = fcs_to_raw_events_pub(&bytes, &blob, "single_stain", &channels_meta)?;
            let stream = format!("raw_fcs_events:control:{}", ctrl["fluorochrome"].as_str().unwrap());
            put_entity(tx, &case_id, &stream, "raw_fcs_events", &json!(ctrl_raw), "ev_import");
            single_stains.push(SingleStainControl {
                fluorochrome: ctrl["fluorochrome"].as_str().unwrap().into(),
                detector_channel: ctrl["detector_channel"].as_str().unwrap().into(),
                events_id: blob,
            });
        }

        // Raw channel values layer (no normalization dropped).
        let raw_events = fcs_to_raw_events_pub(&fcs_bytes, &file_id, "sample", &channels_meta)?;
        let raw_stream = stream_for(STREAM_RAW, "baseline");
        let (raw_id, raw_ver) = put_entity(tx, &case_id, &raw_stream, STREAM_RAW, &json!(raw_events), "ev_import");

        // Spillover metadata layer.
        let spill_rel = stage0["spill_csv"].as_str().unwrap();
        let spill_text = String::from_utf8(read_bytes(case_dir, spill_rel)?).map_err(|e| e.to_string())?;
        let table = parse_spill_csv(&spill_text)?;
        let spill_meta = SpilloverMeta {
            source_file_id: file_id.clone(),
            detectors: table.detectors.clone(),
            fluorochromes: table.fluorochromes.clone(),
            single_stains,
            note: format!("parsed from {} (FCS $SPILL shape)", spill_rel),
        };
        let meta_stream = stream_for(STREAM_SPILL_META, "baseline");
        let (meta_id, meta_ver) = put_entity(tx, &case_id, &meta_stream, STREAM_SPILL_META, &json!(spill_meta), "ev_import");

        // Spillover matrix layer.
        let spill_matrix = SpilloverMatrix {
            channel_order: table.detectors.clone(),
            matrix: table.matrix.clone(),
            meta_id: meta_id.clone(),
        };
        let sm_stream = stream_for(STREAM_SPILL_MATRIX, "baseline");
        let (sm_id, sm_ver) = put_entity(tx, &case_id, &sm_stream, STREAM_SPILL_MATRIX, &json!(spill_matrix), "ev_import");

        // Compensation matrix layer: inversion judgement is explicit.
        let judgement = judge_invert(&table.matrix);
        let inv_rec = InvertRecord {
            status: match judgement.status {
                crate::linalg::InvertStatus::Ok => "ok".into(),
                crate::linalg::InvertStatus::Singular => "singular".into(),
                crate::linalg::InvertStatus::NearSingular => "near_singular".into(),
            },
            min_pivot: judgement.min_pivot,
            condition_estimate: judgement.condition_estimate,
            threshold: NEAR_SINGULAR_THRESHOLD,
            message: match judgement.status {
                crate::linalg::InvertStatus::Ok => "inverted; comp matrix available".into(),
                crate::linalg::InvertStatus::Singular =>
                    "exact singular spillover: no inverse exists, compensation blocked".into(),
                crate::linalg::InvertStatus::NearSingular =>
                    "near-singular spillover: inverse withheld pending explicit human ruling".into(),
            },
        };
        let comp = CompMatrix {
            channel_order: table.detectors.clone(),
            matrix: judgement.inverse,
            source_spill_matrix_id: sm_id.clone(),
            invert: inv_rec,
        };
        let comp_stream = stream_for(STREAM_COMP, "baseline");
        let (comp_id, comp_ver) = put_entity(tx, &case_id, &comp_stream, STREAM_COMP, &json!(comp), "ev_import");

        // Transform parameters layer.
        let xf_val = read_json(case_dir, stage0["transform"].as_str().unwrap())?;
        let _xform: TransformParams = serde_json::from_value(xf_val.clone()).map_err(|e| e.to_string())?;
        let xf_stream = stream_for(STREAM_XFORM, "baseline");
        let (xf_id, xf_ver) = put_entity(tx, &case_id, &xf_stream, STREAM_XFORM, &xf_val, "ev_import");

        // Gate polygons: each polygon is its own versioned stream.
        let mut gate_refs = Vec::new();
        for grel in stage0["gates"].as_array().unwrap() {
            let gv = read_json(case_dir, grel.as_str().unwrap())?;
            let gate: GatePolygon = serde_json::from_value(gv.clone()).map_err(|e| e.to_string())?;
            let gs = format!("{}:{}", STREAM_GATES, gate.gate_key);
            let (gid, gver) = put_entity(tx, &case_id, &gs, STREAM_GATES, &gv, "ev_import");
            gate_refs.push((gs, gid, gver));
        }

        // Parent/child hierarchy layer (separate from polygons).
        let hv = read_json(case_dir, stage0["hierarchy"].as_str().unwrap())?;
        let _hier: GateHierarchy = serde_json::from_value(hv.clone()).map_err(|e| e.to_string())?;
        let hier_stream = stream_for(STREAM_HIER, "baseline");
        let (hier_id, hier_ver) = put_entity(tx, &case_id, &hier_stream, STREAM_HIER, &hv, "ev_import");

        // Case + branch rows.
        tx.execute(
            "INSERT INTO cases(case_id,key_slug,title,manifest_json,created_event,created_at_ts)
             VALUES(?1,?2,?3,?4,'ev_import',?5)",
            &[&case_id, &slug, &manifest["title"].as_str().unwrap_or(slug), &manifest.to_string(), &ts],
        );
        tx.execute(
            "INSERT INTO branches(case_id,branch_id,key_name,parent_branch_id,base_event_id,label,revision,created_at_ts,archived)
             VALUES(?1,?2,'baseline',NULL,NULL,'基线草案',0,?3,0)",
            &[&case_id, &root_branch, &ts],
        );

        // Root refs.
        let refs: Vec<(&str, String, i64)> = vec![
            (raw_stream.as_str(), raw_id, raw_ver),
            (meta_stream.as_str(), meta_id, meta_ver),
            (sm_stream.as_str(), sm_id, sm_ver),
            (comp_stream.as_str(), comp_id, comp_ver),
            (xf_stream.as_str(), xf_id, xf_ver),
            (hier_stream.as_str(), hier_id, hier_ver),
        ];
        for (s, id, ver) in refs {
            tx.execute(
                "INSERT INTO branch_refs(case_id,branch_id,stream_id,entity_id,version) VALUES(?1,?2,?3,?4,?5)",
                &[&case_id, &root_branch, &s, &id, &ver],
            );
        }
        for (s, id, ver) in &gate_refs {
            tx.execute(
                "INSERT INTO branch_refs(case_id,branch_id,stream_id,entity_id,version) VALUES(?1,?2,?3,?4,?5)",
                &[&case_id, &root_branch, &s, &id, &ver],
            );
        }

        // Machine-readable judgement events + import event.
        let mut seq = 0i64;
        let emit = |tx: &crate::store::TransactionCtx, etype: &str, payload: Value, seq: i64| {
            let eid = make_event_id(&case_id, &root_branch, seq, etype, &payload);
            tx.execute(
                "INSERT INTO events(case_id,branch_id,event_id,seq,event_type,payload_json,created_at_ts)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                &[&case_id, &root_branch, &eid, &seq, &etype, &payload.to_string(), &(ts + seq)],
            );
            eid
        };
        let import_payload = json!({
            "kind": "import_baseline",
            "fcs": {"file_id": file_id, "path": fcs_rel, "sha256": file_sha},
            "streams": {
                STREAM_RAW: {"id": raw_id, "version": raw_ver},
                STREAM_SPILL_META: {"id": meta_id, "version": meta_ver},
                STREAM_SPILL_MATRIX: {"id": sm_id, "version": sm_ver},
                STREAM_COMP: {"id": comp_id, "version": comp_ver},
                STREAM_XFORM: {"id": xf_id, "version": xf_ver},
                STREAM_HIER: {"id": hier_id, "version": hier_ver},
                "gates": gate_refs.iter().map(|(s,id,v)| json!({"stream":s,"id":id,"version":v})).collect::<Vec<_>>()
            }
        });
        emit(tx, "import", import_payload, seq);
        seq += 1;
        // ref_updated events make time-travel reconstruct refs.
        for (stream, id, ver) in &[
            (raw_stream.clone(), raw_id.clone(), raw_ver),
            (meta_stream.clone(), meta_id.clone(), meta_ver),
            (sm_stream.clone(), sm_id.clone(), sm_ver),
            (comp_stream.clone(), comp_id.clone(), comp_ver),
            (xf_stream.clone(), xf_id.clone(), xf_ver),
            (hier_stream.clone(), hier_id.clone(), hier_ver),
        ] {
            emit(tx, "ref_updated", json!({"stream_id": stream, "entity_id": id, "version": ver}), seq);
            seq += 1;
        }
        for (s, id, ver) in &gate_refs {
            emit(tx, "ref_updated", json!({"stream_id": s, "entity_id": id, "version": ver}), seq);
            seq += 1;
        }
        // Judgement record: matrices must not be silently inverted.
        emit(tx, "judgement", json!({
            "judgement_id": "matrix_not_inverted",
            "rule": "near_singular_or_singular_matrix_must_not_be_silently_inverted",
            "status": comp.invert.status,
            "min_pivot": comp.invert.min_pivot,
            "threshold": NEAR_SINGULAR_THRESHOLD,
            "affected": if comp.matrix.is_some() { [] } else { ["compensation", "gates", "population_pct"] },
        }), seq);

        let _ = (raw_file, HierarchyNode::clone); // keep imports used
        Ok(ImportedCase { case_id, root_branch })
    })?;

    // Compute baseline results outside the import transaction.
    let branch = db.get_branch(&case_id, &root_branch).unwrap();
    let last_seq = db.next_event_seq(&case_id, &root_branch) - 1;
    let event = make_event_id(&case_id, &root_branch, last_seq + 1, "recompute", &json!({"reason": "initial"}));
    db.append_event(&case_id, &root_branch, &event, last_seq + 1, "recompute",
        &json!({"reason": "initial population"}), ts + last_seq);
    let (_created, blocking) = db.populate_results(&case_id, &branch, &event, last_seq + 1);
    if !blocking.is_empty() {
        // baseline well-conditioned; a block here indicates a bad case pack.
        return Err(format!("baseline blocked unexpectedly: {:?}", blocking));
    }
    Ok(outcome)
}

impl Db {
    pub fn get_case_meta_by_slug(&self, slug: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT case_id FROM cases WHERE key_slug=?1", rusqlite::params![slug], |r| r.get::<_, String>(0))
            .ok()
    }
}
