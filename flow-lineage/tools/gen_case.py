#!/usr/bin/env python3
"""Deterministic generator for the offline case pack
'flow-boundary-chamber'. Produces real FCS3.1 binary files plus
domain JSON (spillover metadata, matrix candidates, transforms,
gate polygons with parent/child relations, and a replay script).

All quantities carry units / coordinate basis. Original
representations are preserved; only the derived layer normalizes.
"""
import json
import math
import os
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
CASE = os.path.join(ROOT, "cases", "flow-boundary-chamber")
RAW = os.path.join(CASE, "raw")
os.makedirs(RAW, exist_ok=True)

CHANNELS = [
    {"name": "FSC-A", "label": "FSC-A", "unit": "arb.linear", "basis": "linear"},
    {"name": "SSC-A", "label": "SSC-A", "unit": "arb.linear", "basis": "linear"},
    {"name": "FL1-A", "label": "FITC-A (FL1)", "fluor": "FITC", "unit": "rfu", "basis": "raw"},
    {"name": "FL2-A", "label": "PE-A (FL2)", "fluor": "PE", "unit": "rfu", "basis": "raw"},
    {"name": "FL3-A", "label": "PerCP-A (FL3)", "fluor": "PerCP", "unit": "rfu", "basis": "raw"},
]
# Raw fluorescence columns handled by the spillover matrix
FLUORS = ["FL1-A", "FL2-A", "FL3-A"]

# Baseline spillover matrix S (row = measured detector, col = emitting fluor)
S_BASE = [
    [1.000, 0.060, 0.010],
    [0.050, 1.000, 0.020],
    [0.008, 0.040, 1.000],
]
# Exactly singular variant: rows 1 and 3 identical => determinant 0
S_SINGULAR = [
    [1.000, 0.060, 0.010],
    [0.050, 1.000, 0.020],
    [1.000, 0.060, 0.010],
]
# Near-singular variant: third row almost equals the first (rcond << 1e-6)
S_NEARSING = [
    [1.000, 0.060, 0.010],
    [0.050, 1.000, 0.020],
    [1.0000001, 0.0600001, 0.0100001],
]

T = 262144.0  # FCS $PnR max range (pulse area scale, rfu)
M = 4.5
A_FITC = 1.0
A_PE = 1.0
A_PERCP = 1.0


def asinh_v(y, t=T, m=M, a=1.0):
    # asinh cofactor transform, dimensionless display scale [0, ~m]
    return (math.asinh(y / 150.0) + math.log(2.0 ** m * 150.0 / t)) / math.log(10.0) + a


def inv_asinh_v(v, t=T, m=M, a=1.0):
    z = (v - a) * math.log(10.0) - math.log(2.0 ** m * 150.0 / t)
    return 150.0 * math.sinh(z)


def threshold(percent, a=1.0):
    return inv_asinh_v(percent * M, t=T, m=M, a=a)


V_CD3 = asinh_v(threshold(0.42, A_FITC))  # boundary value (display units)
V_PE = asinh_v(threshold(0.55, A_PE))
V_PERCP = asinh_v(threshold(0.60, A_PERCP))

# --- boundary display values (dimensionless, asinh scale) ---
B_CD3 = 0.42 * M
B_PE = 0.55 * M
B_PERCP = 0.60 * M
Y_CD3_EDGE = inv_asinh_v(B_CD3, a=A_FITC)
Y_PE_EDGE = inv_asinh_v(B_PE, a=A_PE)
Y_PERCP_EDGE = inv_asinh_v(B_PERCP, a=A_PERCP)


def comp_back(x_fl, s=S_BASE):
    # Given desired compensated fluorescence vector x, return raw y = S x
    return [sum(s[i][j] * x_fl[j] for j in range(3)) for i in range(3)]


def ev(eid, fsc, ssc, x_fl, note="", flags=None):
    y = comp_back(x_fl)
    return {
        "id": eid, "FSC-A": fsc, "SSC-A": ssc,
        "FL1-A": y[0], "FL2-A": y[1], "FL3-A": y[2],
        "note": note, "flags": flags or [],
    }


EVENTS = [
    ev("E01", 52000, 31000, [8200, 900, 600], "CD3+ CD4+ helper", ["cd3", "cd4"]),
    ev("E02", 55000, 29000, [7600, 7800, 500], "CD3+ CD8+ cytotoxic", ["cd3", "cd8"]),
    ev("E03", 48000, 33000, [6100, 4200, 3000], "CD3+ double positive", ["cd3", "cd4", "cd8"]),
    ev("E04", 51000, 27000, [900, 500, 400], "lymphocyte triple negative", []),
    ev("E05", 96000, 71000, [900, 500, 400], "debris / monocyte outside G1", ["outside-g1"]),
    ev("E06", 53000, 30500, [Y_CD3_EDGE, 600, 600], "exact CD3 boundary (included)", ["boundary-cd3", "cd3"]),
    ev("E07", 50500, 32000, [6800, Y_PE_EDGE, 300], "exact PE boundary (included)", ["boundary-pe", "cd3", "cd4"]),
    ev("E08", 52000, 28000, [6800, 4200, Y_PERCP_EDGE], "exact PerCP boundary (included)", ["boundary-percp", "cd3", "cd8"]),
    ev("E09", 25000, 15000, [7000, 7000, 7000], "small-FSC lymphocyte", ["cd3", "cd4", "cd8"]),
    ev("E10", 90000, 30000, [7000, 4000, 4000], "high-FSC excluded by G1", ["outside-g1", "cd3"]),
    ev("E11", 50000, 30000, [5.0e9, 600, 600], "saturating extreme FITC signal", ["extreme", "cd3"]),
    ev("E12", 50000, 30000, [600, 5.0e9, 600], "saturating extreme PE signal", ["extreme", "cd3", "cd4"]),
    ev("E13", 50000, 30000, [600, 600, -1.0e6], "negative overshoot after electronics", ["extreme"]),
    {"id": "E14", "FSC-A": 50000, "SSC-A": 30000,
     "FL1-A": "N/A (instrument: OUT_OF_RANGE)", "FL2-A": 600, "FL3-A": 600,
     "note": "unconvertible token must stay in place with diagnostic",
     "flags": ["unconvertible"]},
    {"id": "E15", "FSC-A": 50000, "SSC-A": 30000,
     "FL1-A": "NaN", "FL2-A": 600, "FL3-A": 600,
     "note": "IEEE NaN raw token retained", "flags": ["unconvertible"]},
    ev("E16", 45000, 62000, [600, 600, 600], "high-SSC granulocyte", []),
]

def write_fcs(path, channels, rows, cyt_label, extra_tokens=None):
    """Write a minimal FCS3.1 file: 256-byte header, TEXT keywords,
    $DATATYPE D (binary float64 big endian), one event per row.
    Non-numeric tokens are written as IEEE NaN and reported in
    extra_tokens for sidecar preservation of the original string."""
    n = len(channels)
    nev = len(rows)
    tokens = [
        ("$BEGINANALYSIS", "0"), ("$ENDANALYSIS", "0"),
        ("$BEGINSTEXT", "0"), ("$ENDSTEXT", "0"),
        ("$BYTEORD", "4,3,2,1"), ("$DATATYPE", "D"),
        ("$MODE", "L"), ("$NEXTDATA", "0"),
        ("$TOT", str(nev)), ("$PAR", str(n)),
        ("$CYT", cyt_label), ("$CYTSN", "BC-001"),
        ("$DATE", "2026-09-27"), ("$BTIM", "09:00:00"),
        ("$ETIM", "09:05:00"),
    ]
    for i, ch in enumerate(channels, start=1):
        tokens.append((f"$P{i}N", ch["name"]))
        tokens.append((f"$P{i}S", ch["label"]))
        tokens.append((f"$P{i}B", "64"))
        tokens.append((f"$P{i}E", "0,0"))
        tokens.append((f"$P{i}R", str(int(T))))
        unit = ch.get("unit", "")
        tokens.append((f"$P{i}U", unit))
    body = b"|" + b"".join(
        (k.encode() + b"|" + v.encode() + b"|") for k, v in tokens
    )
    text_start = 256
    text_end = text_start + len(body) - 1
    data_start = text_end + 1
    payload = b""
    token_report = extra_tokens if extra_tokens is not None else None
    for r in rows:
        for ch in channels:
            val = r[ch["name"]]
            if isinstance(val, str):
                if token_report is not None:
                    token_report.append((r["id"], ch["name"], val))
                payload += struct.pack(">d", float("nan"))
            else:
                payload += struct.pack(">d", float(val))
    data_end = data_start + len(payload) - 1
    header = "FCS3.1    "
    header += f"{text_start:>8d}{text_end:>8d}"
    header += f"{data_start:>8d}{data_end:>8d}"
    header += f"{0:>8d}{0:>8d}"
    header += " " * (256 - len(header))
    assert len(header) == 256
    with open(path, "wb") as f:
        f.write(header.encode())
        f.write(body)
        f.write(payload)


def control_rows(fluor_idx, pos_values):
    rows = []
    for k, amp in enumerate(pos_values):
        x = [0.0, 0.0, 0.0]
        x[fluor_idx] = amp
        e = ev(f"C{fluor_idx}-{k:02d}", 50000, 30000, x,
               f"single-stain control {FLUORS[fluor_idx]}", ["single-stain"])
        rows.append(e)
    return rows


main_tokens = []
write_fcs(os.path.join(RAW, "events.fcs"), CHANNELS, EVENTS,
          "BoundaryLab-BC", main_tokens)

control_files = []
for idx, fname in enumerate(["control_fitc.fcs", "control_pe.fcs", "control_percp.fcs"]):
    toks = []
    write_fcs(os.path.join(RAW, fname), CHANNELS,
              control_rows(idx, [1000.0, 5000.0, 12000.0, 25000.0]),
              "BoundaryLab-BC", toks)
    control_files.append({
        "file": f"raw/{fname}",
        "fluorophore": CHANNELS[2 + idx]["fluor"],
        "detector": FLUORS[idx],
        "events": 4,
        "unconvertible": [
            {"event_id": e, "channel": c, "raw_token": t} for e, c, t in toks
        ],
    })

with open(os.path.join(RAW, "events.unconvertible.json"), "w") as f:
    json.dump({
        "source_file": "raw/events.fcs",
        "coordinate_basis": "raw",
        "unconvertible": [
            {"event_id": e, "channel": c, "raw_token": t,
             "diagnostic": "value cannot be normalized; original token retained at source position"}
            for e, c, t in main_tokens
        ],
    }, f, indent=2, ensure_ascii=False)
print("FCS written:", len(EVENTS), "main events;", main_tokens)
