#!/usr/bin/env python3
"""Generate the offline domain case pack 'flow-boundary-chamber'.

Outputs genuine FCS 3.1 binary files (big-endian float32 event matrix) plus
separately versioned domain artifacts as JSON. No generic spreadsheets.
"""
import json
import math
import os
import random
import struct

ROOT = os.path.join(os.path.dirname(__file__), "..", "cases", "flow-boundary-chamber")
INIT = os.path.join(ROOT, "initial")

CHANNELS = [
    ("ch_fsc", "FSC-A", 262144.0),
    ("ch_ssc", "SSC-A", 262144.0),
    ("ch_fitc", "FITC-A", 262144.0),
    ("ch_pe", "PE-A", 262144.0),
    ("ch_apc", "APC-A", 262144.0),
    ("ch_time", "Time", 1000.0),
]

# Deterministic synthetic acquisition.
random.seed(20260924)


def make_events():
    ev = []
    eid = 0

    def add(fsc, ssc, fitc, pe, apc, t, tag):
        nonlocal eid
        ev.append({
            "event_id": f"ev_{eid:03d}",
            "tag": tag,
            "values": {
                "ch_fsc": float(fsc), "ch_ssc": float(ssc),
                "ch_fitc": float(fitc), "ch_pe": float(pe),
                "ch_apc": float(apc), "ch_time": float(t),
            },
        })
        eid += 1

    for _ in range(10):  # debris cluster
        add(random.uniform(40, 120), random.uniform(30, 90),
            random.uniform(40, 200), random.uniform(40, 200),
            random.uniform(40, 200), random.uniform(0, 60), "debris")
    for i in range(30):  # lymphocyte cluster
        fsc = random.uniform(205, 335)
        ssc = random.uniform(145, 275)
        cd3 = i < 14
        fitc = random.uniform(8000, 20000) if cd3 else random.uniform(60, 400)
        pe = random.uniform(6000, 18000) if cd3 and i < 6 else random.uniform(60, 500)
        apc = random.uniform(60, 600)
        add(fsc, ssc, fitc, pe, apc, random.uniform(60, 400),
            "cd3+" if cd3 else "cd3-")
    # Boundary points: exact vertex / exact edge membership.
    add(200.0, 140.0, 9000.0, 300.0, 200.0, 410.0, "boundary_g0_vertex")
    add(340.0, 210.0, 300.0, 300.0, 200.0, 420.0, "boundary_g0_edge")
    add(210.0, 160.0, 12000.0, 300.0, 200.0, 430.0, "boundary_g1_vertex")
    add(260.0, 200.0, 15000.0, 12000.0, 300.0, 440.0, "boundary_g2_edge")
    # Extreme signals.
    add(280.0, 220.0, 262143.0, 400.0, 300.0, 450.0, "saturated_fitc")
    add(250.0, 190.0, 60.0, 16000.0, 300.0, 460.0, "compensated_negative")
    add(230.0, 170.0, 0.0, 0.0, 1.0, 470.0, "zero_signal")
    return ev


def write_fcs(path, events, spill):
    n = len(events)
    npar = len(CHANNELS)
    text_pairs = [
        ("$BEGINANALYSIS", "0"), ("$ENDANALYSIS", "0"),
        ("$BEGINSTEXT", "0"), ("$ENDSTEXT", "0"),
        ("$DATATYPE", "F"), ("$MODE", "L"), ("$NEXTDATA", "0"),
        ("$PAR", str(npar)), ("$TOT", str(n)),
        ("$BYTEORD", "4,3,2,1"),
        ("$FIL", os.path.basename(path)),
        ("$SPILLOVER", spill),
    ]
    for idx, (cid, name, rng) in enumerate(CHANNELS, start=1):
        text_pairs += [
            (f"$P{idx}N", name), (f"$P{idx}B", "32"),
            (f"$P{idx}R", str(int(rng))), (f"$P{idx}E", "0,0"),
            (f"$P{idx}N_LONG", cid),
        ]
    sep = b"\x0c"
    text = sep + sep.join(f"{k}{sep.decode()}{v}".encode() for k, v in text_pairs) + sep
    while len(text) % 4 != 0:
        text = text[:-1] + b" " + sep
    data_offset = 58 + len(text)
    payload = b"".join(
        struct.pack(">%df" % npar, *[e["values"][c[0]] for c in CHANNELS])
        for e in events
    )
    end_data = data_offset + len(payload) - 1
    fields = (58, 58 + len(text) - 1, data_offset, end_data, 0, 0)
    header = b"FCS3.1    " + b"".join(b"%-8d" % v for v in fields)
    assert len(header) == 58, len(header)
    with open(path, "wb") as fh:
        fh.write(header + text + payload)


def main():
    os.makedirs(INIT, exist_ok=True)
    events = make_events()

    channels = [
        {"channel_id": c, "short_name": n, "range": r, "unit": "a.u.",
         "basis": "raw detector signal, linear scale" if c != "ch_time"
         else "milliseconds since acquisition start"}
        for c, n, r in CHANNELS
    ]
    spill = "3,FITC-A,PE-A,APC-A,1.00000,0.12000,0.00000,0.08000,1.00000,0.04000,0.00000,0.06000,1.00000"
    write_fcs(os.path.join(INIT, "sample.fcs"), events, spill)

    # Single-stain controls (small genuine FCS files).
    for cid, fname, idx in (("ch_fitc", "cd3_fitc_control.fcs", 2),
                            ("ch_pe", "pe_control.fcs", 3),
                            ("ch_apc", "apc_control.fcs", 4)):
        ctrl = []
        for k in range(8):
            vals = {c[0]: (50.0 if k == 0 else float(100 + 10 * k)) for c in CHANNELS}
            vals[cid] = float(4000 + k * 900)
            ctrl.append({"event_id": f"ctl_{cid}_{k}", "tag": "single_stain",
                         "values": vals})
        write_fcs(os.path.join(INIT, fname), ctrl, spill)

    with open(os.path.join(INIT, "raw_events.json"), "w") as fh:
        json.dump({"artifact_id": "art_events", "version": "v1",
                   "source_file": "sample.fcs",
                   "representation": "float32 big-endian FCS 3.1 $DATATYPE=F",
                   "channels": channels, "events": events}, fh, indent=2)

    spill_meta = {
        "artifact_id": "art_spill_meta", "version": "v1",
        "acquisition": {"date": "2026-09-24", "operator": "flow-core",
                        "unit_basis": "a.u. detector voltage scale"},
        "controls": [
            {"control_id": "ctl_fitc", "file": "cd3_fitc_control.fcs",
             "channel_id": "ch_fitc", "fluorochrome": "FITC", "voltage_v": 350},
            {"control_id": "ctl_pe", "file": "pe_control.fcs",
             "channel_id": "ch_pe", "fluorochrome": "PE", "voltage_v": 380},
            {"control_id": "ctl_apc", "file": "apc_control.fcs",
             "channel_id": "ch_apc", "fluorochrome": "APC", "voltage_v": 420},
        ],
        "spillover_matrix_ref": "art_spill/v1",
    }
    json.dump(spill_meta, open(os.path.join(INIT, "spillover_metadata.json"), "w"), indent=2)

    matrix = {
        "artifact_id": "art_spill", "version": "v1",
        "channels": ["ch_fitc", "ch_pe", "ch_apc"],
        "unit_basis": "dimensionless fraction of detector signal",
        "rows": [
            [1.00, 0.12, 0.00],
            [0.08, 1.00, 0.04],
            [0.00, 0.06, 1.00],
        ],
    }
    json.dump(matrix, open(os.path.join(INIT, "compensation_matrix.json"), "w"), indent=2)

    transform = {
        "artifact_id": "art_transform", "version": "v1",
        "transforms": [
            {"channel_id": c, "family": "asinh", "cofactor": 150.0,
             "cofactor_unit": "a.u.", "output_unit": "asinh(a.u.)"}
            for c in ("ch_fitc", "ch_pe", "ch_apc")
        ],
        "linear_channels": [
            {"channel_id": "ch_fsc", "unit": "a.u."},
            {"channel_id": "ch_ssc", "unit": "a.u."},
            {"channel_id": "ch_time", "unit": "ms"}],
    }
    json.dump(transform, open(os.path.join(INIT, "transform_params.json"), "w"), indent=2)

    gates = {
        "artifact_id": "art_gates", "version": "v1",
        "coord_basis": "raw linear detector units [a.u.]",
        "gates": [
            {"gate_id": "gate_lymph", "name": "Lymphocytes",
             "x_channel": "ch_fsc", "y_channel": "ch_ssc",
             "polygon": [[200, 140], [340, 140], [340, 280], [200, 280]]},
            {"gate_id": "gate_cd3", "name": "CD3+",
             "x_channel": "ch_fitc", "y_channel": "ch_ssc",
             "polygon": [[2000, 140], [22000, 140], [22000, 280], [2000, 280]]},
            {"gate_id": "gate_cd4", "name": "CD4-like",
             "x_channel": "ch_fitc", "y_channel": "ch_pe",
             "polygon": [[8000, 5000], [20000, 5000], [20000, 20000], [8000, 20000]]},
        ],
    }
    json.dump(gates, open(os.path.join(INIT, "gates.json"), "w"), indent=2)

    lineage = {
        "artifact_id": "art_lineage", "version": "v1",
        "edges": [
            {"child_gate": "gate_cd3", "parent_gate": "gate_lymph"},
            {"child_gate": "gate_cd4", "parent_gate": "gate_cd3"},
        ],
    }
    json.dump(lineage, open(os.path.join(INIT, "lineage.json"), "w"), indent=2)
    print("wrote", len(events), "events")


if __name__ == "__main__":
    main()
