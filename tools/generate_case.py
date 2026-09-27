#!/usr/bin/env python3
"""Generate the offline domain case pack: real FCS 3.1 binaries + JSON layers.

Layout under cases/flow-boundary-chamber/:
  materials/0001/*.fcs|json  - initial separately versioned source layers
  materials/0002/*          - parameter revision
  materials/0003/*          - local-only source change
  singular/                 - near-singular spillover branch material
  broken/                   - malformed FCS (atomic rollback case)
  scenario.json             - machine readable replay script + expectations

All domain quantities carry units/coordinate frames. Raw representations are
kept verbatim; normalisation only happens in derived compute layers.
"""
import json
import math
import os
import struct

ROOT = os.path.join(os.path.dirname(__file__), "..", "cases", "flow-boundary-chamber")

CHANNELS = ["FSC-A", "SSC-A", "FITC-A", "PE-A"]
UNITS = {
    "FSC-A": "linear-arbitrary-scale",
    "SSC-A": "linear-arbitrary-scale",
    "FITC-A": "channel-intensity",
    "PE-A": "channel-intensity",
}

# Spillover matrix S: raw = S * compensated (fluorescence block, FITC/PE).
S = [
    [1.00, 0.10],
    [0.25, 1.00],
]
# Compensation matrix C = S^-1, verified explicitly and stored separately.
# det = 0.975
S_INV = [
    [1.0 / 0.975, -0.10 / 0.975],
    [-0.25 / 0.975, 1.0 / 0.975],
]

# Near-singular spillover: FITC and PE almost indistinguishable.
S_NEAR = [
    [1.0, 0.10000],
    [1.0, 0.10001],
]


def compensate(raw_fitc, raw_pe, s=S):
    c_f = (raw_fitc - s[0][1] * raw_pe) / (s[0][0] - s[0][1] * s[1][0])
    c_p = (raw_pe - s[1][0] * c_f) / s[1][1]
    return c_f, c_p


def asinh(x, cofactor):
    return math.log((x / cofactor) + math.sqrt((x / cofactor) ** 2 + 1.0))


# (id, fsc, ssc, raw_fitc, raw_pe, note)
RAW_EVENTS = [
    ("e01", 120000, 30000, 1000, 300, "unstained-negative"),
    ("e02", 130000, 42000, 1150, 420, "unstained-negative"),
    ("e03", 145000, 60000, 50500, 12700, "FITC-single-positive"),
    ("e04", 138000, 55000, 40500, 10200, "FITC-single-positive"),
    ("e05", 150000, 70000, 5100, 40300, "PE-single-positive"),
    ("e06", 142000, 64000, 4200, 31400, "PE-single-positive"),
    ("e07", 160000, 75000, 50600, 40200, "double-positive"),
    ("e08", 155000, 72000, 44600, 34300, "double-positive"),
    ("e09", 12000, 9000, 800, 260, "debris-low-scatter"),
    ("e10", 170000, 90000, 262143, 262143, "extreme-saturated-signal"),
    ("e11", 105000, 28000, 200, 60, "negative-after-compensation"),
    ("e12", 132000, 46000, 25600, 6475, "polygon-boundary-point"),
]
