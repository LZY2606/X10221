#!/usr/bin/env python3
"""Generate the offline domain case pack:
case/fc-boundary-chamber/  (FCS3.1 files + versioned artifacts + manifest)

Pure standard library. Run from repository root:
    python3 case/generate_case.py
"""
import json
import math
import os
import random
import struct

ROOT = os.path.dirname(os.path.abspath(__file__))
CASE_DIR = os.path.join(ROOT, "fc-boundary-chamber")
STAGES_DIR = os.path.join(CASE_DIR, "stages")

random.seed(20260923)

# Six channels: 2 scatter + 3 fluorescence (spillover basis) + 1 viability
CH = [
    ("FSC-A", "a.u.", "forward scatter area"),
    ("SSC-A", "a.u.", "side scatter area"),
    ("FITC-A", "MIFU", "CD3-FITC fluorescence"),
    ("PE-A", "MIFU", "CD4-PE fluorescence"),
    ("APC-A", "MIFU", "CD8-APC fluorescence"),
    ("Viability", "MIFU", "live/dead discriminator"),
]
FLUO = ["FITC-A", "PE-A", "APC-A"]
CH_INDEX = {name: i for i, (name, _, _) in enumerate(CH)}

# Near-singular spillover: FITC and PE detector columns almost collinear.
# Uncompensated y = S @ x  with x the "true" fluorophore amounts.
SPILL_NEAR = {
    "matrix": [
        [1.000000, 0.310000, 0.010000],
        [0.300010, 1.000000, 0.050000],
        [0.010000, 0.060000, 1.000000],
    ],
}
SINGULAR_STAGE = {
    "matrix": [
        [1.0, 0.5, 0.0],
        [2.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ],
}

CIRCUIT = {
    "single_stain": {
        "FITC-A": "control/CD3-FITC.fcs",
        "PE-A": "control/CD4-PE.fcs",
        "APC-A": "control/CD8-APC.fcs",
    },
    "beads": "single-stain compensation beads, same lot AB-2026-09",
}

def lu_reciprocal(m):
    n = len(m)
    w = [row[:] for row in m]
    pivots = []
    for col in range(n):
        pivot = max(range(col, n), key=lambda r: abs(w[r][col]))
        best = abs(w[pivot][col])
        if best < 1e-15:
            return 0.0
        w[pivot], w[col] = w[col], w[pivot]
        pivots.append(w[col][col])
        for r in range(col + 1, n):
            f = w[r][col] / w[col][col]
            for j in range(col, n):
                w[r][j] -= f * w[col][j]
    mn = min(abs(p) for p in pivots)
    mx = max(abs(p) for p in pivots)
    return mn / mx


def invert(m):
    n = len(m)
    aug = [m[i][:] + [1.0 if i == j else 0.0 for j in range(n)] for i in range(n)]
    for col in range(n):
        pivot = max(range(col, n), key=lambda r: abs(aug[r][col]))
        if abs(aug[pivot][col]) < 1e-15:
            raise ValueError("singular")
        aug[pivot], aug[col] = aug[col], aug[pivot]
        pv = aug[col][col]
        aug[col] = [v / pv for v in aug[col]]
        for r in range(n):
            if r != col:
                f = aug[r][col]
                aug[r] = [aug[r][j] - f * aug[col][j] for j in range(2 * n)]
    return [row[n:] for row in aug]


def mat_vec(m, x):
    return [sum(m[i][k] * x[k] for k in range(len(x))) for i in range(len(m))]


def asinh(x, cof):
    if x is None or (isinstance(x, float) and math.isnan(x)):
        return None
    return math.asinh(x / cof)


print("near-singular reciprocal condition:", lu_reciprocal(SPILL_NEAR["matrix"]))
