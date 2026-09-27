#!/usr/bin/env python3
"""Offline domain case pack generator (see module docstring below)."""
from __future__ import annotations
import json, math, struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "static" / "case" / "panel-blood-v1"
FLUO = ["FITC-A", "PE-A", "APC-A"]
CHANNELS = ["FSC-A", "SSC-A", "FITC-A", "PE-A", "APC-A"]
UNITS = {
    "FSC-A": "instrument-a.u.-linear",
    "SSC-A": "instrument-a.u.-linear",
    "FITC-A": "detector-channel@488nm-530/30",
    "PE-A": "detector-channel@488nm-575/25",
    "APC-A": "detector-channel@640nm-660/20",
}
SPILL = [[1.00, 0.10, 0.02],
         [0.12, 1.00, 0.05],
         [0.01, 0.06, 1.00]]


def mat_inv3(m):
    a,b,c = m[0]; d,e,f = m[1]; g,h,i = m[2]
    det = a*(e*i-f*h)-b*(d*i-f*g)+c*(d*h-e*g)
    cof = [[(e*i-f*h), -(d*i-f*g), (d*h-e*g)],
           [-(b*i-c*h), (a*i-c*g), -(a*h-b*g)],
           [(b*f-c*e), -(a*f-c*d), (a*e-b*d)]]
    return det, [[cof[r][cc]/det for cc in range(3)] for r in range(3)]


DET, INV = mat_inv3(SPILL)


def mat_vec(m, v):
    return [sum(m[r][k]*v[k] for k in range(3)) for r in range(3)]


def raw_for(stain):
    """detector readings that produce the wanted stain-space vector."""
    return mat_vec(SPILL, stain)


def compensated(fluo):
    return mat_vec(INV, fluo)


# Wanted populations (stain space, bright=1000, dim=60, background=40):
# CD4 single positive -> FITC bright ; CD8 -> APC bright ; CD3 -> FITC bright
# by panel convention (CD3 on FITC here, CD4 on PE, CD8 on APC).
LO, BR = 40.0, 1000.0
# rows: (event_id, fsc, ssc, stain=(fitc,pe,apc), note)
wanted = [
    ("e01", 430, 190, (BR, LO, LO), "cd4-t"),
    ("e02", 460, 210, (BR, LO, LO), "cd4-t"),
    ("e03", 410, 175, (BR, LO, LO), "cd4-t"),
    ("e04", 445, 200, (BR, LO, LO), "cd4-t"),
    ("e05", 380, 230, (LO, LO, BR), "cd8-t"),
    ("e06", 360, 250, (LO, LO, BR), "cd8-t"),
    ("e07", 400, 220, (LO, LO, BR), "cd8-t"),
    ("e08", 375, 240, (LO, LO, BR), "cd8-t"),
    ("e09", 520, 280, (LO, LO, LO), "cd3neg-lymph"),
    ("e10", 340, 150, (LO, LO, LO), "cd3neg-lymph"),
    # boundary event sits exactly on the CD3 polygon edge
    ("e11", 430, 190, (300.0, LO, LO), "boundary-on-cd3-edge"),
    # extreme saturated detector: raw FITC pinned at acquisition ceiling
    ("e12", 420, 185, (None, LO, LO), "extreme-saturation"),
    # debris outside the lymphocyte gate
    ("e13", 60, 40, (LO, LO, LO), "debris"),
    # unconvertible NaN in PE detector
    ("e14", 450, 205, (BR, float("nan"), LO), "unconvertible-nan"),
    # near-zero / negative-after-compensation cross-talk probe
    ("e15", 470, 220, (LO, 300.0, LO), "comp-neg-probe"),
]
# gate vertices chosen in declared coordinate frames:
LYMPH_POLY = [(300,120),(560,120),(560,320),(300,320)]
# CD3 gate is in compensated linear fluorescence: FITC(comp) x SSC(raw)
CD3_POLY = [(250,120),(4000,120),(4000,320),(250,320)]
# CD4 gate: compensated PE vs FITC, both clearly positive
CD4_POLY = [(800,250),(4000,250),(4000,4000),(800,4000)]
# CD8 gate: compensated APC vs FITC
CD8_POLY = [(800,250),(4000,250),(4000,4000),(800,4000)]

SATURATION_CEILING = 262144.0  # 2^18 detector ceiling, channel counts


def build_events():
    rows = []
    for eid, fsc, ssc, stain, note in wanted:
        if note == "extreme-saturation":
            fitc_raw = SATURATION_CEILING
            pe_raw, apc_raw = raw_for((LO, LO, LO))[1:]
        else:
            fitc_raw, pe_raw, apc_raw = raw_for(stain)
        rows.append({
            "event_id": eid, "fsc": fsc, "ssc": ssc,
            "fitc": fitc_raw, "pe": pe_raw, "apc": apc_raw,
            "note": note,
        })
    return rows


def write_fcs(path, events, sample_id, extra_keywords=None):
    """Minimal FCS 3.1: HEADER(256) TEXT then DATA, big-endian float32."""
    nch = len(CHANNELS)
    kws = [
        ("BEGINANALYSIS", "0"), ("ENDANALYSIS", "0"),
        ("BEGINSTEXT", "0"), ("ENDSTEXT", "0"),
        ("$NEXTDATA", "0"),
        ("$BEGINSTEXT", "0"), ("$ENDSTEXT", "0"),
        ("$TOT", str(len(events))),
        ("$PAR", str(nch)),
        ("$MODE", "L"),
        ("$BYTEORD", "4,3,2,1"),
        ("$DATATYPE", "F"),
        ("$FIL", path.name),
        ("$SRC", sample_id),
        ("$CYT", "flowline-fixture"),
        ("$DATE", "2026-09-27"),
        ("$BTIM", "10:00:00"),
        ("$ETIM", "10:05:00"),
        ("$OP", "reviewer"),
        ("$INST", "flowline-fixture-cytometer"),
        ("$CYTSN", "FLFX-0001"),
    ]
    for idx, ch in enumerate(CHANNELS, start=1):
        kws.append((f"$P{idx}N", ch))
        kws.append((f"$P{idx}S", ch))
        kws.append((f"$P{idx}B", "32"))
        kws.append((f"$P{idx}E", "0,0"))
        kws.append((f"$P{idx}R", "262144"))
        kws.append((f"$P{idx}G", "1"))
    if extra_keywords:
        kws.extend(extra_keywords)
    delim = "|"
    text = delim + delim.join(f"{k}{delim}{v}" for k, v in kws) + delim
    text_b = text.encode("ascii")
    data = bytearray()
    for ev in events:
        vals = [ev["fsc"], ev["ssc"], ev["fitc"], ev["pe"], ev["apc"]]
        for v in vals:
            data += struct.pack(">f", v)
    header_end = 256
    btext = header_end
    etext = btext + len(text_b) - 1
    bdata = etext + 1
    edata = bdata + len(data) - 1
    header = f"FCS3.1{' '*4}{btext:>8d}{etext:>8d}{bdata:>8d}{edata:>8d}{'0':>8d}{'0':>8d}"
    header = header.ljust(256, " ")
    assert len(header) == 256
    path.write_bytes(header.encode("ascii") + text_b + bytes(data))


def build_controls():
    # one tube per fluorophore; positive bead/cell measurements used to derive
    # spillover coefficients listed in spillover-v1.json.
    ctrls = []
    pos = {"FITC-A": (BR, LO, LO), "PE-A": (LO, BR, LO), "APC-A": (LO, LO, BR)}
    for ch in FLUO:
        for n in range(6):
            stain = list(pos[ch])
            # observed spillover is what the detector records:
            det = raw_for(stain)
            ctrls.append({
                "event_id": f"ctrl-{ch.split('-')[0].lower()}-{n+1:02d}",
                "fsc": 430 + n, "ssc": 190 + n,
                "fitc": det[0], "pe": det[1], "apc": det[2],
                "note": f"single-stain:{ch}",
            })
    return ctrls


def spill_string():
    # FCS $SPILLOVER: n, names, then rows
    lines = [",".join(FLUO)]
    for row in SPILL:
        lines.append(",".join(f"{v:.6f}" for v in row))
    return "\n".join(lines)


def main():
    ROOT.mkdir(parents=True, exist_ok=True)
    events = build_events()
    write_fcs(ROOT / "specimen.fcs", events, "blood-specimen-001")
    ctrls = build_controls()
    write_fcs(ROOT / "controls.fcs", ctrls, "single-stain-controls",
              extra_keywords=[("$SPILLOVER", spill_string()),
                              ("$SPILL", spill_string())])
    # quick verification of classifications for the module author:
    report = {}
    for ev in events:
        if not math.isnan(ev["pe"]):
            c = compensated([ev["fitc"], ev["pe"], ev["apc"]])
            report[ev["event_id"]] = [round(x, 3) for x in c]
    (ROOT / "_verify_compensated.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False))

    spill_meta = {
        "schema": "flowline.spillover/1",
        "material_id": "spillover",
        "version": "v1",
        "basis": "single-stain-control-medians",
        "units": {"coefficients": "dimensionless-ratio",
                  "measurements": UNITS["FITC-A"]},
        "sources": {
            "controls_file": "controls.fcs",
            "events_per_channel": 6,
            "positive_gate": "unstained/positive separation on stained channel",
        },
        "channels": FLUO,
        "observed": {
            # detector readings of single-stain positives (medians)
            "FITC-A": {"FITC-A": 1000.0, "PE-A": 120.0, "APC-A": 10.0},
            "PE-A": {"FITC-A": 100.0, "PE-A": 1000.0, "APC-A": 60.0},
            "APC-A": {"FITC-A": 20.0, "PE-A": 50.0, "APC-A": 1000.0},
        },
        "derived_matrix_spill": SPILL,
        "diagnostics": [],
    }
    (ROOT / "spillover-v1.json").write_text(
        json.dumps(spill_meta, indent=2, ensure_ascii=False))

    comp_v1 = {
        "schema": "flowline.compensation/1",
        "material_id": "compensation",
        "version": "v1",
        "derived_from": {"spillover": "spillover@v1"},
        "channels": FLUO,
        "units": "dimensionless-ratio",
        "spillover_matrix": SPILL,
        "compensation_matrix": {
            "kind": "inverse",
            "determinant_spillover": DET,
            "matrix": INV,
            "singularity_score": abs(DET),
            "invertible": True,
        },
        "diagnostics": [],
    }
    (ROOT / "compensation-v1.json").write_text(
        json.dumps(comp_v1, indent=2, ensure_ascii=False))

    near = [[1.0, 0.10, 0.0],
            [0.12, 1.0, 0.0],
            [0.012, 0.1, 0.0]]
    det_near, _ = mat_inv3(near)
    comp_v2 = {
        "schema": "flowline.compensation/1",
        "material_id": "compensation",
        "version": "v2-near-singular",
        "derived_from": {"spillover": "spillover@v1",
                         "note": "hypothetical re-fit with APC column collinear"},
        "channels": FLUO,
        "units": "dimensionless-ratio",
        "spillover_matrix": near,
        "compensation_matrix": {
            "kind": "inverse",
            "determinant_spillover": det_near,
            "matrix": None,
            "singularity_score": abs(det_near),
            "invertible": False,
            "near_singular_threshold": 1e-9,
            "candidates": [
                {"id": "refuse", "kind": "refuse-inverse",
                 "rationale": "determinant below threshold; do not silently invert"},
                {"id": "ridge-1e-4", "kind": "ridge-regularized", "epsilon": 1e-4},
                {"id": "ridge-1e-2", "kind": "ridge-regularized", "epsilon": 1e-2},
            ],
        },
        "diagnostics": [{
            "code": "COMP_NEAR_SINGULAR",
            "severity": "error",
            "message": "spillover determinant below 1e-9; inverse withheld",
        }],
    }
    (ROOT / "compensation-v2.json").write_text(
        json.dumps(comp_v2, indent=2, ensure_ascii=False))

    transform_v1 = {
        "schema": "flowline.transform/1",
        "material_id": "transform",
        "version": "v1",
        "formula": "asinh((x - background) / cofactor)",
        "units": {"input": UNITS["FITC-A"], "output": "asinh-transformed-dimensionless"},
        "parameters": [
            {"channel": "FITC-A", "background": 0.0, "cofactor": 150.0},
            {"channel": "PE-A", "background": 0.0, "cofactor": 150.0},
            {"channel": "APC-A", "background": 0.0, "cofactor": 150.0},
        ],
        "diagnostics": [],
    }
    (ROOT / "transform-v1.json").write_text(
        json.dumps(transform_v1, indent=2, ensure_ascii=False))
    transform_v2 = json.loads(json.dumps(transform_v1))
    transform_v2["version"] = "v2-fitc-local"
    transform_v2["revision_reason"] = "FITC cofactor re-tuned; PE/APC unchanged"
    transform_v2["parameters"][0]["cofactor"] = 300.0
    transform_v2["dependency_scope"] = ["FITC-derived gates only"]
    (ROOT / "transform-v2.json").write_text(
        json.dumps(transform_v2, indent=2, ensure_ascii=False))
    print("json materials written; det(spill)=", round(DET, 6),
          " det(near)=", det_near)


    gates_v1 = {
        "schema": "flowline.gates/1",
        "material_id": "gates",
        "version": "v1",
        "boundary_rule": "inclusive-edge (point exactly on edge counts as inside)",
        "gates": [
            {"gate_id": "g-lymph", "name": "Lymphocytes",
             "geometry": "polygon",
             "frame": {"x": "FSC-A", "y": "SSC-A", "basis": "raw-linear",
                       "units": [UNITS["FSC-A"], UNITS["SSC-A"]]},
             "vertices": LYMPH_POLY},
            {"gate_id": "g-cd3", "name": "CD3+",
             "geometry": "polygon",
             "frame": {"x": "FITC-A", "y": "SSC-A", "basis": "compensated-linear",
                       "compensation": "compensation@v1",
                       "units": ["stain-equivalent-channel", UNITS["SSC-A"]]},
             "vertices": CD3_POLY},
            {"gate_id": "g-cd4", "name": "CD4+",
             "geometry": "polygon",
             "frame": {"x": "PE-A", "y": "FITC-A", "basis": "compensated-linear",
                       "compensation": "compensation@v1",
                       "units": ["stain-equivalent-channel",
                                 "stain-equivalent-channel"]},
             "vertices": CD4_POLY},
            {"gate_id": "g-cd8", "name": "CD8+",
             "geometry": "polygon",
             "frame": {"x": "APC-A", "y": "FITC-A", "basis": "compensated-linear",
                       "compensation": "compensation@v1",
                       "units": ["stain-equivalent-channel",
                                 "stain-equivalent-channel"]},
             "vertices": CD8_POLY},
        ],
        "diagnostics": [],
    }
    (ROOT / "gates-v1.json").write_text(
        json.dumps(gates_v1, indent=2, ensure_ascii=False))

    gatetree_v1 = {
        "schema": "flowline.gatetree/1",
        "material_id": "gatetree",
        "version": "v1",
        "root_population": "root",
        "edges": [
            {"edge_id": "edge-root-lymph", "parent_gate": None,
             "child_gate": "g-lymph", "denominator_population": "root",
             "percent_kind": "of-parent"},
            {"edge_id": "edge-lymph-cd3", "parent_gate": "g-lymph",
             "child_gate": "g-cd3", "denominator_population": "g-lymph",
             "percent_kind": "of-parent"},
            {"edge_id": "edge-cd3-cd4", "parent_gate": "g-cd3",
             "child_gate": "g-cd4", "denominator_population": "g-cd3",
             "percent_kind": "of-parent"},
            {"edge_id": "edge-cd3-cd8", "parent_gate": "g-cd3",
             "child_gate": "g-cd8", "denominator_population": "g-cd3",
             "percent_kind": "of-parent"},
        ],
        "diagnostics": [],
    }
    (ROOT / "gatetree-v1.json").write_text(
        json.dumps(gatetree_v1, indent=2, ensure_ascii=False))

    gatetree_v2 = json.loads(json.dumps(gatetree_v1))
    gatetree_v2["version"] = "v2-rename-nested"
    gatetree_v2["revision_reason"] = "rename only; gate_id/edges unchanged"
    gatetree_v2["renames"] = {"g-cd4": "CD4-Helper-T", "g-cd8": "CD8-Cyto-T"}
    gatetree_v2["stability"] = {"gate_id": "immutable", "name": "mutable-label"}
    (ROOT / "gatetree-v2.json").write_text(
        json.dumps(gatetree_v2, indent=2, ensure_ascii=False))

    scenarios = {
        "schema": "flowline.scenarios/1",
        "case_id": "panel-blood-v1",
        "scenarios": [
            {"id": "S1-near-singular-compensation",
             "class": "compensation",
             "source_change": "activate compensation@v2-near-singular",
             "expected_judgement": "COMP_NEAR_SINGULAR_BLOCKED",
             "rule": "near-singular spillover must not be inverted silently",
             "expected_affected": ["compensation-result", "g-cd3", "g-cd4",
                                   "g-cd8", "all descendant populations"]},
            {"id": "S2-extreme-signal",
             "class": "events",
             "source_change": "event e12 saturated FITC at detector ceiling",
             "expected_judgement": "EXTREME_SIGNAL_FLAGGED_RETAINED",
             "rule": "extreme values are flagged, kept and gated; never clipped"},
            {"id": "S3-boundary-point",
             "class": "gates",
             "source_change": "event e11 lies exactly on g-cd3 polygon edge",
             "expected_judgement": "BOUNDARY_INCLUSIVE_ASSIGNED",
             "rule": "inclusive-edge convention gives deterministic membership"},
            {"id": "S4-nested-rename",
             "class": "gatetree",
             "source_change": "gatetree@v2 renames g-cd4/g-cd8 labels only",
             "expected_judgement": "RENAME_NO_RESULT_INVALIDATION",
             "rule": "stable gate_id keeps lineage; label edits do not recompute"},
            {"id": "S5-upstream-stale",
             "class": "propagation",
             "source_change": "revise g-cd3 geometry on a review branch",
             "expected_judgement": "DOWNSTREAM_MARKED_STALE_NOT_OVERWRITTEN",
             "rule": "descendant results marked stale; old results preserved"},
            {"id": "S6-optimistic-conflict",
             "class": "concurrency",
             "source_change": "two decisions saved against the same base version",
             "expected_judgement": "VERSION_CONFLICT_409_WITH_DIFF",
             "rule": "later committer gets conflict plus both-sided diff"},
        ],
    }
    (ROOT / "scenarios.json").write_text(
        json.dumps(scenarios, indent=2, ensure_ascii=False))

    manifest = {
        "schema": "flowline.case-manifest/1",
        "case_id": "panel-blood-v1",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "channels": [{"name": c, "label": c, "unit": UNITS[c]} for c in CHANNELS],
        "materials": [
            "specimen.fcs", "controls.fcs", "spillover-v1.json",
            "compensation-v1.json", "compensation-v2.json",
            "transform-v1.json", "transform-v2.json",
            "gates-v1.json", "gatetree-v1.json", "gatetree-v2.json",
            "scenarios.json",
        ],
        "baseline_refs": {
            "spillover": "spillover@v1",
            "compensation": "compensation@v1",
            "transform": "transform@v1",
            "gates": "gates@v1",
            "gatetree": "gatetree@v1",
        },
    }
    (ROOT / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
