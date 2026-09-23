#!/usr/bin/env python3
"""Generate the offline flow-cytometry boundary-case pack (FCS 3.1 + JSON).

Run:  python3 tools/make_case_pack.py
Out:  case_pack/  (committed to the repo, fully offline)

The pack deliberately contains:
  - extreme signals (saturation at the ADC ceiling, negative-ish raw)
  - an unconvertible value (NaN in PE channel, kept in place with a diagnostic)
  - boundary points lying exactly on gate edges
  - single-stain controls from which the spillover matrix is derived
"""
import json
import math
import os
import struct

ROOT = os.path.join(os.path.dirname(__file__), "..", "case_pack")

# --- fixed instrument / channel basis -------------------------------------
# $PnN short name, detector, fluorophore, unit, coordinate basis.
CHANNELS = [
    ("FSC-A", "FSC-A detector", None, "ADC_counts", "forward_scatter_linear"),
    ("SSC-A", "SSC-A detector", None, "ADC_counts", "side_scatter_linear"),
    ("FITC-A", "525/40-A", "FITC", "ADC_counts", "fluorescence_linear"),
    ("PE-A", "575/25-A", "PE", "ADC_counts", "fluorescence_linear"),
    ("APC-A", "660/20-A", "APC", "ADC_counts", "fluorescence_linear"),
    ("FSC-H", "FSC-H detector", None, "ADC_counts", "forward_scatter_pulse_height"),
    ("SSC-H", "SSC-H detector", None, "ADC_counts", "side_scatter_pulse_height"),
    ("FITC-H", "525/40-H", "FITC", "ADC_counts", "fluorescence_pulse_height"),
    ("Time", "time_counter", None, "ms", "acquisition_time"),
]
ADC_MAX = 262143.0  # 18-bit ADC ceiling; saturation uses this value

# Base spillover matrix S (rows = measured, cols = true fluorophore).
# S is applied as measured = S @ true.  Round to 2 decimals so that the
# single-stain controls re-derive exactly these coefficients.
S = {
    "FITC-A": {"FITC-A": 1.00, "PE-A": 0.06, "APC-A": 0.00},
    "PE-A":   {"FITC-A": 0.15, "PE-A": 1.00, "APC-A": 0.04},
    "APC-A":  {"FITC-A": 0.00, "PE-A": 0.08, "APC-A": 1.00},
}
FLUORS = ["FITC-A", "PE-A", "APC-A"]


def comp_to_measured(comp):
    """compensated (true) fluorescence dict -> measured (raw) fluorescence."""
    out = {}
    for r in FLUORS:
        v = sum(S[r][c] * comp.get(c, 0.0) for c in FLUORS)
        out[r] = v
    return out


# --- event population ------------------------------------------------------
# id, group, compensated fluorescence (FITC, PE, APC), fsc, ssc, notes
# Groups: cd4, cd8, dneg (CD3-like fluorescent dim), mono, debris, edge,
#         saturated, nan_value
def _flu(fitc, pe, apc):
    return {"FITC-A": fitc, "PE-A": pe, "APC-A": apc}


EVENTS = [
    # id, kind, FSC, SSC, compensated (FITC,PE,APC), notes
    ("E01", "lymph_cd4",  82000, 31000, _flu(12000,  900,  700), "CD4 bright"),
    ("E02", "lymph_cd4",  79000, 28000, _flu(15000, 1100,  500), "CD4 bright"),
    ("E03", "lymph_cd4",  86000, 33000, _flu(10500, 1300,  900), "CD4 dim-positive"),
    ("E04", "lymph_cd4",  74000, 36000, _flu(18000,  800,  650), "CD4 bright, high SSC edge"),
    ("E05", "lymph_cd8",  83000, 26000, _flu(  900,14000, 1200), "CD8 bright"),
    ("E06", "lymph_cd8",  88000, 29000, _flu( 1200,11000,  900), "CD8 positive"),
    ("E07", "lymph_cd8",  71000, 30000, _flu(  700,16500, 1500), "CD8 bright"),
    ("E08", "lymph_dneg", 80000, 32000, _flu(  800,  900,  600), "double negative"),
    ("E09", "lymph_dneg", 90000, 27000, _flu(  600,  700,  800), "double negative"),
    ("E10", "lymph_dneg", 77000, 24000, _flu(  950, 1050,  550), "double negative"),
    ("E11", "lymph_cd4",  90000, 40000, _flu( 9800, 1000,  700), "lymph gate corner (90000,40000)"),
    ("E12", "lymph_cd8",  70000, 20000, _flu( 1100,12500,  800), "lymph gate corner (70000,20000)"),
    ("E13", "monocyte",  145000, 95000, _flu( 2000, 9000, 1800), "monocyte, PE bright"),
    ("E14", "monocyte",  160000,110000, _flu( 1500, 7500, 2200), "monocyte"),
    ("E15", "monocyte",  130000, 88000, _flu( 2400,10500, 1500), "monocyte, PE bright"),
    ("E16", "debris",     18000,  9000, _flu(  200,  150,  100), "debris low FSC"),
    ("E17", "debris",     32000, 14000, _flu(  300,  250,  180), "debris"),
    ("E18", "debris",     12000,  6000, _flu(  150,  100,   90), "debris"),
    ("E19", "debris",     45000, 19000, _flu(  400,  350,  220), "debris just under cell gate"),
    # Boundary points: exactly on cell polygon edges
    ("E20", "boundary",   60000, 20000, _flu(  900,  800,  500), "ON cell-gate bottom edge (60000,20000)"),
    ("E21", "boundary",  100000, 20000, _flu( 1000,  900,  600), "ON cell-gate bottom edge (100000,20000)"),
    # Extreme signals
    ("E22", "saturated",  84000, 30000, None, "FITC saturated at ADC ceiling"),
    ("E23", "negative_raw", 81000,29000,_flu( -500, 800,  600), "negative FITC after compensation (autofluo)"),
    # Unconvertible value
    ("E24", "nan_value",  78000, 31000, "NAN_PE", "PE value unconvertible (NaN); kept with diagnostic"),
]

# Cell gate polygon (FSC-A, SSC-A): rectangle-ish quadrilateral.
# Bottom edge y=20000 from x=60000..100000 holds the two boundary events.
CELL_POLY = [(50000, 20000), (105000, 20000), (190000, 60000),
             (190000, 140000), (50000, 140000)]
# Lymphocyte polygon inside the cell gate.
LYMPH_POLY = [(70000, 20000), (90000, 20000), (90000, 40000), (70000, 40000)]


def build_matrix():
    """Return (ids, meta, rows) with rows in CHANNELS column order."""
    rows = []
    meta = []
    for idx, (eid, kind, fsc, ssc, flu, note) in enumerate(EVENTS):
        if flu is None:  # saturated FITC
            measured = {"FITC-A": ADC_MAX, "PE-A": 1000.0, "APC-A": 700.0}
        elif flu == "NAN_PE":
            measured = comp_to_measured(_flu(900, 0.0, 600))
            measured["PE-A"] = float("nan")
        else:
            measured = comp_to_measured(flu)
        t = 10.0 * (idx + 1)
        row = [
            float(fsc),
            float(ssc),
            float(measured["FITC-A"]),
            float(measured["PE-A"]),
            float(measured["APC-A"]),
            float(fsc) * 0.92,
            float(ssc) * 0.95,
            float(measured["FITC-A"]) * 0.9,
            t,
        ]
        rows.append(row)
        meta.append({"event_id": eid, "kind": kind, "note": note})
    return meta, rows


def write_fcs(path, rows, source_identifier, extra_keywords=None):
    """Write a minimal FCS3.1 file, float32 big-endian ($DATATYPE F, $BYTEORD 4,3,2,1)."""
    n_events = len(rows)
    n_par = len(CHANNELS)
    data = b"".join(struct.pack(">%df" % n_par, *r) for r in rows)
    begin_data = 0  # patched after building text segment
    keywords = [
        ("BEGINANALYSIS", "0"),
        ("ENDANALYSIS", "0"),
        ("BEGINSTEXT", "0"),
        ("ENDSTEXT", "0"),
        ("$BEGINANALYSIS", "0"),
        ("$ENDANALYSIS", "0"),
        ("$BEGINSTEXT", "0"),
        ("$ENDSTEXT", "0"),
        ("$MODE", "L"),
        ("$DATATYPE", "F"),
        ("$BYTEORD", "4,3,2,1"),
        ("$NEXTDATA", "0"),
        ("$TOT", str(n_events)),
        ("$PAR", str(n_par)),
        ("$FIL", os.path.basename(path)),
        ("$SRC", source_identifier),
        ("$CYT", "FlowLine Simulated Acquisition Unit v1"),
        ("$DATE", "2026-09-24"),
        ("$BTIM", "10:00:00"),
        ("$ETIM", "10:05:00"),
        ("$TIMESTEP", "0.01"),
        ("FLOWLINE/CASE", "flowline-boundary-v1"),
    ]
    for i, (short, detector, fluor, unit, basis) in enumerate(CHANNELS, start=1):
        keywords.append(("$P%dN" % i, short))
        keywords.append(("$P%dS" % i, detector))
        keywords.append(("$P%dR" % i, str(int(ADC_MAX) + 1)))
        keywords.append(("$P%dB" % i, "32"))
        keywords.append(("$P%dE" % i, "0,0"))
        keywords.append(("$P%dG" % i, "1"))
        keywords.append(("FLOWLINE/$P%dUNIT" % i, unit))
        keywords.append(("FLOWLINE/$P%dBASIS" % i, basis))
        if fluor:
            keywords.append(("$P%dFLUOR" % i, fluor))
    if extra_keywords:
        keywords.extend(extra_keywords)

    def render_text(begin_data, end_data):
        kvs = [("$BEGINDATA", str(begin_data)), ("$ENDDATA", str(end_data))] + keywords
        body = "|".join("%s|%s" % (k, v) for k, v in kvs)
        return ("|" + body + "|").encode("ascii")

    # header layout: 58 bytes reserved for TEXT 10..57
    text = render_text(256 + len(b"") , 0)
    begin_text = 256
    # iterate because text length is independent of the digit values (fixed width
    # layout is not required by FCS3.1; offsets in TEXT are authoritative).
    begin_data = begin_text + len(text)
    end_data = begin_data + len(data) - 1
    text = render_text(begin_data, end_data)
    begin_data = begin_text + len(text)
    end_data = begin_data + len(data) - 1
    text = render_text(begin_data, end_data)
    end_text = begin_text + len(text) - 1
    header = ("FCS3.1    "
              "%8d%8d%8d%8d%8d%8d" % (begin_text, end_text,
                                      begin_data, end_data, 0, 0)).encode("ascii")
    header = header.ljust(begin_text, b" ")
    with open(path, "wb") as fh:
        fh.write(header)
        fh.write(text)
        fh.write(data)


# --- single-stain controls --------------------------------------------------
# Bright and dim populations of 4 events each; cross-channel signal follows S
# so the estimator re-derives the committed coefficients exactly.
def control_rows(primary_idx, brightness):
    out = []
    for scale in brightness:
        comp = {f: 0.0 for f in FLUORS}
        comp[FLUORS[primary_idx]] = float(scale)
        m = comp_to_measured(comp)
        out.append([
            80000.0, 30000.0,
            m["FITC-A"], m["PE-A"], m["APC-A"],
            73600.0, 28500.0, m["FITC-A"] * 0.9, 100.0,
        ])
    return out


def estimate_spillover():
    """Median cross/primary ratio per bright single-stain event.

    Deterministically re-derives S (coeffs rounded to 2 decimals) from the
    committed controls.  This mirrors the platform estimation rule.
    """
    est = {r: {c: 0.0 for c in FLUORS} for r in FLUORS}
    for ci, cname in enumerate(FLUORS):
        for r in control_rows(ci, [10000.0]):
            primary = r[2 + ci]
            for ri, rname in enumerate(FLUORS):
                est[rname][cname] = r[2 + ri] / primary
    for r in FLUORS:
        for c in FLUORS:
            est[r][c] = round(est[r][c], 2)
    return est


def dump_json(path, obj):
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(obj, fh, ensure_ascii=False, indent=2, sort_keys=False)
        fh.write("\n")


def main():
    os.makedirs(os.path.join(ROOT, "controls"), exist_ok=True)
    meta, rows = build_matrix()
    write_fcs(os.path.join(ROOT, "events.fcs"), rows,
              "acquisition/run-2026-09-24-tube-01")
    dump_json(os.path.join(ROOT, "events.meta.json"), {
        "schema": "flowline.event-meta/v1",
        "events": meta,
    })

    controls = []
    for ci, cname in enumerate(FLUORS):
        fn = "single_stain_%s.fcs" % cname.replace("-", "").lower()
        crows = control_rows(ci, [12000.0, 9000.0, 6000.0, 3000.0])
        write_fcs(os.path.join(ROOT, "controls", fn), crows,
                  "single-stain-control/%s" % cname,
                  extra_keywords=[("$P%dFLUOR_CONTROL" % (ci + 3), cname)])
        controls.append({
            "file": "controls/" + fn,
            "fluorophore": cname,
            "detector_channel": cname,
            "brightness_scales": [12000, 9000, 6000, 3000],
            "unit": "ADC_counts",
        })

    spill = {
        "schema": "flowline.spillover/v1",
        "id": "spill-base",
        "derivation": {
            "method": "median_ratio_single_stain",
            "controls": [c["file"] for c in controls],
            "rounding": "round_half_up_2dp",
        },
        "row_basis": "measured_channel",
        "column_basis": "true_fluorophore",
        "matrix_order": FLUORS,
        "matrix": [[S[r][c] for c in FLUORS] for r in FLUORS],
        "unit": "fraction_of_primary_signal",
        "controls": controls,
    }
    dump_json(os.path.join(ROOT, "spillover.json"), spill)
    assert estimate_spillover() == {r: S[r] for r in FLUORS}, "control mismatch"

    # Baseline compensation proposal = inverse(S).  Near-singular and exactly
    # singular challenger matrices are declared for the replay narrative but
    # NOT applied automatically.
    inv = invert3(S)
    dump_json(os.path.join(ROOT, "compensation.json"), {
        "schema": "flowline.compensation/v1",
        "id": "comp-base",
        "candidates": [
            {
                "id": "comp-base-inverse",
                "source_spillover": "spill-base",
                "matrix_order": FLUORS,
                "matrix": [[round(inv[r][c], 6) for c in range(3)]
                           for r in range(3)],
                "unit": "dimensionless_compensation_gain",
                "note": "exact inverse of spill-base at import",
            },
            {
                "id": "comp-near-singular",
                "source_spillover": "spill-base",
                "matrix_order": FLUORS,
                "matrix": [
                    [1.0, 0.70, 0.0],
                    [0.70, 1.0, 0.70],
                    [0.0, 0.70, 1.0],
                ],
                "unit": "dimensionless_compensation_gain",
                "matrix_kind": "spillover_proposal",
                "note": "strong 70% spillover chain; det=0.02, condition number ~198; must not be silently inverted",
                "expect": "blocked_near_singular",
            },
            {
                "id": "comp-singular",
                "source_spillover": "spill-base",
                "matrix_order": FLUORS,
                "matrix": [
                    [1.0, 0.70710678, 0.0],
                    [0.70710678, 1.0, 0.70710678],
                    [0.0, 0.70710678, 1.0],
                ],
                "unit": "dimensionless_compensation_gain",
                "matrix_kind": "spillover_proposal",
                "note": "1/sqrt(2) chain tridiagonal; det == 0 (columns linearly dependent)",
                "expect": "blocked_singular",
            },
        ],
        "adopted": None,
    })

    dump_json(os.path.join(ROOT, "transforms.json"), {
        "schema": "flowline.transforms/v1",
        "id": "transforms-v1",
        "parameters": [
            {"channel": "FSC-A", "family": "linear", "scale": 1.0,
             "unit": "ADC_counts", "coordinate_basis": "raw"},
            {"channel": "SSC-A", "family": "linear", "scale": 1.0,
             "unit": "ADC_counts", "coordinate_basis": "raw"},
            {"channel": "FITC-A", "family": "asinh", "cofactor": 150.0,
             "unit": "asinh(ADC_counts/150)", "coordinate_basis": "compensated"},
            {"channel": "PE-A", "family": "asinh", "cofactor": 150.0,
             "unit": "asinh(ADC_counts/150)", "coordinate_basis": "compensated"},
            {"channel": "APC-A", "family": "asinh", "cofactor": 150.0,
             "unit": "asinh(ADC_counts/150)", "coordinate_basis": "compensated"},
            {"channel": "Time", "family": "linear", "scale": 1.0,
             "unit": "ms", "coordinate_basis": "raw"},
        ],
    })

    dump_json(os.path.join(ROOT, "gates.json"), {
        "schema": "flowline.gates/v1",
        "id": "gates-v1",
        "gates": [
            {
                "id": "gate-cells",
                "label": "Cells",
                "geometry": {"type": "polygon",
                             "coordinates": [list(p) for p in CELL_POLY]},
                "axes": [{"channel": "FSC-A", "space": "raw", "unit": "ADC_counts"},
                         {"channel": "SSC-A", "space": "raw", "unit": "ADC_counts"}],
                "parent": None,
            },
            {
                "id": "gate-lymphocytes",
                "label": "Lymphocytes",
                "geometry": {"type": "polygon",
                             "coordinates": [list(p) for p in LYMPH_POLY]},
                "axes": [{"channel": "FSC-A", "space": "raw", "unit": "ADC_counts"},
                         {"channel": "SSC-A", "space": "raw", "unit": "ADC_counts"}],
                "parent": "gate-cells",
            },
            {
                "id": "gate-cd4",
                "label": "CD4+",
                "geometry": {"type": "rectangle",
                             "x_min": 2.5, "x_max": 6.0,
                             "y_min": -1.0, "y_max": 2.0},
                "axes": [{"channel": "FITC-A", "space": "asinh_compensated",
                          "unit": "asinh(ADC_counts/150)"},
                         {"channel": "PE-A", "space": "asinh_compensated",
                          "unit": "asinh(ADC_counts/150)"}],
                "parent": "gate-lymphocytes",
            },
            {
                "id": "gate-cd8",
                "label": "CD8+",
                "geometry": {"type": "rectangle",
                             "x_min": -1.0, "x_max": 2.0,
                             "y_min": 2.5, "y_max": 6.0},
                "axes": [{"channel": "FITC-A", "space": "asinh_compensated",
                          "unit": "asinh(ADC_counts/150)"},
                         {"channel": "PE-A", "space": "asinh_compensated",
                          "unit": "asinh(ADC_counts/150)"}],
                "parent": "gate-lymphocytes",
            },
        ],
    })

    dump_json(os.path.join(ROOT, "manifest.json"), {
        "schema": "flowline.case-pack/v1",
        "case_id": "flow-boundary-chamber-v1",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "created": "2026-09-24",
        "files": {
            "events": "events.fcs",
            "event_meta": "events.meta.json",
            "spillover": "spillover.json",
            "compensation": "compensation.json",
            "transforms": "transforms.json",
            "gates": "gates.json",
        },
        "adc_max": ADC_MAX,
        "narrative_stations": [
            "baseline_separately_versioned",
            "near_singular_compensation_blocked",
            "extreme_signals_and_boundary_points",
            "gate_nested_rename",
            "localized_source_change",
            "upstream_stale_propagation",
            "concurrent_same_version_conflict",
        ],
    })
    print("case pack written to", os.path.normpath(ROOT))


def invert3(s):
    a = [[s[r][c] for c in FLUORS] for r in FLUORS]
    n = 3
    aug = [a[i] + [1.0 if i == j else 0.0 for j in range(n)] for i in range(n)]
    for col in range(n):
        piv = max(range(col, n), key=lambda r: abs(aug[r][col]))
        aug[col], aug[piv] = aug[piv], aug[col]
        p = aug[col][col]
        aug[col] = [v / p for v in aug[col]]
        for r in range(n):
            if r != col:
                f = aug[r][col]
                aug[r] = [aug[r][k] - f * aug[col][k] for k in range(2 * n)]
    return [row[n:] for row in aug]


if __name__ == "__main__":
    main()
