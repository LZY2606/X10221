#!/usr/bin/env python3
"""Generate the offline case pack "flow-boundary-chamber".

Produces real FCS 3.0 binary files (unstained, two single-stain controls,
one sample containing extreme / boundary / NaN events) plus separately
versioned domain artefacts: spillover metadata, compensation matrices
(healthy + near-singular), transform parameters and two gating drafts.
"""
import json, math, os, random, struct

ROOT = os.path.join(os.path.dirname(__file__), "..", "cases", "flow-boundary-chamber")
RANGE = 262144.0
COFACTOR = 150.0
# spillover: rows = detectors (FITC-A, PE-A), cols = fluorochromes (FITC, PE)
SPILL = [[1.0, 0.15], [0.08, 1.0]]

CHANNELS = ["FSC-A", "SSC-A", "FITC-A", "PE-A"]

def asinh(x):
    return math.asinh(x)

def craft_raw(fitc_true, pe_true):
    """observed = S @ true  (inverse of the compensation step)."""
    f = fitc_true + SPILL[0][1] * pe_true
    p = SPILL[1][0] * fitc_true + pe_true
    return (f, p)

def from_transformed(tx, ty):
    """desired compensated+transformed point -> raw detector values."""
    f_true = math.sinh(tx) * COFACTOR
    p_true = math.sinh(ty) * COFACTOR
    return craft_raw(f_true, p_true)

def write_fcs(path, events, src_name):
    n = len(events)
    text_body = (
        "/$BEGINANALYSIS/0/$ENDANALYSIS/0/$BEGINSTEXT/0/$ENDSTEXT/0"
        "/$BYTEORD/1,2,3,4/$DATATYPE/F/$MODE/L/$NEXTDATA/0"
        "/$CYT/BoundaryCyte-500/$SRC/{src}"
        "/$PAR/{np}/$TOT/{tot}".format(src=src_name, np=len(CHANNELS), tot=n)
    )
    for i, ch in enumerate(CHANNELS, start=1):
        text_body += "/$P{n}B/32/$P{n}E/0,0/$P{n}N/{ch}/$P{n}S/{ch}/$P{n}R/{r}/$P{n}G/1".format(
            n=i, ch=ch, r=int(RANGE))
    # iterate because $BEGINDATA/$ENDDATA digit count affects text length
    bdata, edata = 0, 0
    for _ in range(4):
        text = text_body + "/$BEGINDATA/{}/$ENDDATA/{}/".format(bdata, edata)
        bdata = 58 + len(text)
        edata = bdata + n * len(CHANNELS) * 4 - 1
    text = text_body + "/$BEGINDATA/{}/$ENDDATA/{}/".format(bdata, edata)
    header = "FCS3.0    " + "".join("{:>8}".format(v) for v in
             (58, 58 + len(text) - 1, bdata, edata, 0, 0))
    assert len(header) == 58
    with open(path, "wb") as fh:
        fh.write(header.encode("ascii"))
        fh.write(text.encode("ascii"))
        for ev in events:
            fh.write(struct.pack("<4f", *ev))
    return n

def gauss_events(rng, n, fsc, ssc, fitc_true, pe_true, spread=0.06):
    out = []
    for _ in range(n):
        fs = min(max(rng.gauss(fsc, 8000), 1000), RANGE - 1)
        ss = min(max(rng.gauss(ssc, 6000), 1000), RANGE - 1)
        ft = max(fitc_true * rng.gauss(1.0, spread), 1.0)
        pt = max(pe_true * rng.gauss(1.0, spread), 1.0)
        fo, po = craft_raw(ft, pt)
        out.append((fs, ss, fo, po))
    return out

def main():
    rng = random.Random(20260919)
    os.makedirs(os.path.join(ROOT, "fcs"), exist_ok=True)
    for d in ("spillover", "compensation", "transforms", "gating", "replay"):
        os.makedirs(os.path.join(ROOT, d), exist_ok=True)

    counts = {}
    counts["beads_unstained"] = write_fcs(
        os.path.join(ROOT, "fcs", "beads_unstained.fcs"),
        gauss_events(rng, 200, 80000, 30000, 500, 400), "beads_unstained")
    counts["ctrl_fitc"] = write_fcs(
        os.path.join(ROOT, "fcs", "ctrl_fitc.fcs"),
        gauss_events(rng, 200, 82000, 31000, 50000, 20), "ctrl_fitc")
    counts["ctrl_pe"] = write_fcs(
        os.path.join(ROOT, "fcs", "ctrl_pe.fcs"),
        gauss_events(rng, 200, 81000, 29000, 30, 40000), "ctrl_pe")

    sample = []
    sample += gauss_events(rng, 100, 85000, 32000, 800, 700)      # negative
    sample += gauss_events(rng, 80, 86000, 33000, 30000, 60)      # FITC+
    sample += gauss_events(rng, 60, 84000, 31000, 40, 25000)      # PE+
    sample += gauss_events(rng, 40, 85000, 32000, 30000, 25000)   # double positive
    # extreme signals: detector saturation at range max
    for _ in range(8):
        sample.append((rng.gauss(85000, 5000), rng.gauss(32000, 4000), RANGE, RANGE))
    # boundary points exactly on gate g2 / g3 polygon edges (transformed space)
    boundary_targets = [(4.0, 2.0), (8.0, 3.0), (6.0, 0.0), (4.0, 0.0),
                        (5.0, 8.0), (4.0, 6.0), (6.0, 4.5), (8.0, 4.5),
                        (4.0, 4.5), (7.0, 8.0), (8.0, 0.0), (4.0, 8.0)]
    for tx, ty in boundary_targets:
        fo, po = from_transformed(tx, ty)
        sample.append((rng.gauss(85000, 5000), rng.gauss(32000, 4000), fo, po))
    # unconvertible values: NaN must stay in place with a diagnostic
    for _ in range(2):
        sample.append((rng.gauss(85000, 5000), rng.gauss(32000, 4000),
                       float("nan"), 900.0))
    counts["sample_a"] = write_fcs(os.path.join(ROOT, "fcs", "sample_a.fcs"),
                                   sample, "sample_a")

    spill_v1 = {
        "id": "spillover-meta", "version": 1, "unit": "ratio",
        "basis": "median_single_stain_minus_unstained",
        "fluorochromes": ["FITC", "PE"], "detectors": ["FITC-A", "PE-A"],
        "entries": [
            {"fluorochrome": "FITC", "detector": "FITC-A", "coefficient": 1.0,
             "control_ref": "ctrl_fitc"},
            {"fluorochrome": "FITC", "detector": "PE-A", "coefficient": 0.08,
             "control_ref": "ctrl_fitc"},
            {"fluorochrome": "PE", "detector": "FITC-A", "coefficient": 0.15,
             "control_ref": "ctrl_pe"},
            {"fluorochrome": "PE", "detector": "PE-A", "coefficient": 1.0,
             "control_ref": "ctrl_pe"},
        ],
    }
    json.dump(spill_v1, open(os.path.join(ROOT, "spillover", "spillover_v1.json"), "w"), indent=2)

    matrix_v1 = {
        "id": "comp-matrix", "version": 1, "unit": "ratio",
        "fluorochromes": ["FITC", "PE"], "detectors": ["FITC-A", "PE-A"],
        "matrix": SPILL, "derived_from": "spillover-meta@1",
    }
    json.dump(matrix_v1, open(os.path.join(ROOT, "compensation", "matrix_v1.json"), "w"), indent=2)

    matrix_v2 = {
        "id": "comp-matrix", "version": 2, "unit": "ratio",
        "fluorochromes": ["FITC", "PE"], "detectors": ["FITC-A", "PE-A"],
        "matrix": [[1.0, 0.999999], [0.999999, 1.0]],
        "derived_from": "manual-upload",
        "note": "near-singular: determinant ~ 2e-6, must NOT be silently inverted",
    }
    json.dump(matrix_v2, open(os.path.join(ROOT, "compensation", "matrix_v2_near_singular.json"), "w"), indent=2)

    transforms_v1 = {
        "id": "transform-params", "version": 1, "kind": "asinh",
        "basis_template": "asinh(cofactor={cofactor})",
        "channels": [
            {"channel": "FSC-A", "kind": "linear", "unit": "AU"},
            {"channel": "SSC-A", "kind": "linear", "unit": "AU"},
            {"channel": "FITC-A", "kind": "asinh", "cofactor": COFACTOR, "unit": "AU"},
            {"channel": "PE-A", "kind": "asinh", "cofactor": COFACTOR, "unit": "AU"},
        ],
    }
    json.dump(transforms_v1, open(os.path.join(ROOT, "transforms", "transforms_v1.json"), "w"), indent=2)

    g2_alpha = [[4.0, 0.0], [8.0, 0.0], [8.0, 8.0], [4.0, 8.0]]
    g3_alpha = [[4.0, 4.5], [8.0, 4.5], [8.0, 8.0], [4.0, 8.0]]
    draft_alpha = {
        "draft": "alpha", "version": 1,
        "sample_ref": "sample_a",
        "gates": [
            {"id": "g1-lymph", "version": 1, "name": "Lymphocytes",
             "parent": None, "x_channel": "FSC-A", "y_channel": "SSC-A",
             "basis": "raw_detector", "unit": "AU",
             "polygon": [[60000.0, 15000.0], [120000.0, 15000.0],
                         [120000.0, 60000.0], [60000.0, 60000.0]]},
            {"id": "g2-fitc-pos", "version": 1, "name": "FITC+",
             "parent": "g1-lymph@1", "x_channel": "FITC-A", "y_channel": "PE-A",
             "basis": "asinh(cofactor=150)", "unit": "AU",
             "polygon": g2_alpha},
            {"id": "g3-dp", "version": 1, "name": "DP",
             "parent": "g2-fitc-pos@1", "x_channel": "FITC-A", "y_channel": "PE-A",
             "basis": "asinh(cofactor=150)", "unit": "AU",
             "polygon": g3_alpha},
        ],
    }
    json.dump(draft_alpha, open(os.path.join(ROOT, "gating", "draft_alpha_v1.json"), "w"), indent=2)

    draft_beta = {
        "draft": "beta", "version": 1,
        "sample_ref": "sample_a",
        "gates": [
            {"id": "g1-lymph", "version": 1, "name": "Lymphocytes",
             "parent": None, "x_channel": "FSC-A", "y_channel": "SSC-A",
             "basis": "raw_detector", "unit": "AU",
             "polygon": [[60000.0, 15000.0], [120000.0, 15000.0],
                         [120000.0, 60000.0], [60000.0, 60000.0]]},
            {"id": "g2-fitc-pos", "version": 1, "name": "FITC+ (wide)",
             "parent": "g1-lymph@1", "x_channel": "FITC-A", "y_channel": "PE-A",
             "basis": "asinh(cofactor=150)", "unit": "AU",
             "polygon": [[3.5, 0.0], [8.5, 0.0], [8.5, 8.0], [3.5, 8.0]]},
            {"id": "g3-dp", "version": 1, "name": "DP",
             "parent": "g2-fitc-pos@1", "x_channel": "FITC-A", "y_channel": "PE-A",
             "basis": "asinh(cofactor=150)", "unit": "AU",
             "polygon": [[3.5, 4.2], [8.5, 4.2], [8.5, 8.0], [3.5, 8.0]]},
        ],
    }
    json.dump(draft_beta, open(os.path.join(ROOT, "gating", "draft_beta_v1.json"), "w"), indent=2)

    replay = {
        "id": "replay-flow-boundary-chamber",
        "narrative": "import -> near-singular matrix -> adjudicate -> transform revision "
                     "-> gate rename -> local source change -> conflicting decisions",
        "steps": [
            {"op": "apply_matrix", "matrix": "compensation/matrix_v2_near_singular.json",
             "actor": "reviewer-a",
             "expect": "rejected_near_singular"},
            {"op": "adjudicate", "candidate_kind": "regularized_inverse",
             "pick": "lambda=0.01", "decision": "accept", "actor": "reviewer-a"},
            {"op": "adjudicate", "candidate_kind": "regularized_inverse",
             "pick": "lambda=0.0001", "decision": "reject", "actor": "reviewer-b"},
            {"op": "revise_transform", "channel": "FITC-A", "cofactor": 100.0,
             "actor": "reviewer-a",
             "expect": "stale:g2-fitc-pos,g3-dp"},
            {"op": "rename_gate", "gate_id": "g3-dp", "draft": "alpha",
             "new_name": "DP (FITC+PE+)", "actor": "reviewer-b"},
            {"op": "amend_source", "source": "sample_a",
             "patch": [{"event_index": 0, "channel": "FITC-A", "raw_value": "50000"}],
             "actor": "reviewer-b",
             "expect": "local_invalidation"},
            {"op": "concurrent_adjudicate", "actor_a": "reviewer-a",
             "actor_b": "reviewer-b", "expect": "conflict"},
        ],
    }
    json.dump(replay, open(os.path.join(ROOT, "replay", "replay.json"), "w"), indent=2)

    manifest = {
        "case_id": "flow-boundary-chamber",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "fcs": {k: {"path": "fcs/%s.fcs" % k, "events": v} for k, v in counts.items()},
        "spillover": ["spillover/spillover_v1.json"],
        "compensation": ["compensation/matrix_v1.json",
                         "compensation/matrix_v2_near_singular.json"],
        "transforms": ["transforms/transforms_v1.json"],
        "gating": ["gating/draft_alpha_v1.json", "gating/draft_beta_v1.json"],
        "replay": "replay/replay.json",
        "scenarios": ["singular_matrix", "extreme_signals", "boundary_points",
                      "gate_nesting_rename", "upstream_stale_propagation",
                      "concurrent_same_version_edit"],
    }
    json.dump(manifest, open(os.path.join(ROOT, "manifest.json"), "w"), indent=2)
    print("case pack written:", counts)

if __name__ == "__main__":
    main()
