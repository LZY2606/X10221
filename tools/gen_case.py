#!/usr/bin/env python3
"""确定性生成离线案例包：flow-boundary-chamber-v1。
所有派生坐标直接算出并写入 JSON，FCS 文本段按最小 FCS3.1 风格字节保留。"""
import hashlib, json, math, os, struct

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "src", "case")

CHANNELS = ["FSC-A", "SSC-A", "FITC-A", "PE-A", "PerCP-A", "APC-A", "APC-Cy7-A"]
FLUOR = CHANNELS[2:]
COFACTOR = {"FITC-A": 150.0, "PE-A": 150.0, "PerCP-A": 100.0,
            "APC-A": 100.0, "APC-Cy7-A": 120.0}

# spillover S（6x6，荧光通道），良态
S = [
    [1.000, 0.020, 0.000, 0.000, 0.000],
    [0.040, 1.000, 0.010, 0.000, 0.000],
    [0.000, 0.050, 1.000, 0.120, 0.000],
    [0.000, 0.000, 0.080, 1.000, 0.030],
    [0.000, 0.000, 0.000, 0.060, 1.000],
]

# 补偿目标值（线性脉冲面积尺度）；FSC/SSC 不参与补偿
# 列: FSC, SSC, FITC, PE, PerCP, APC, APC-Cy7, 备注
TARGETS = [
    ("e01", 90000, 30000, 18000, 120,   90,    60,   40),
    ("e02", 88000, 42000, 16000, 80,    14000, 50,   30),
    ("e03", 95000, 35000, 90,    14000, 100,   70,   45),
    ("e04", 92000, 33000, 110,   12000, 90,    55,   35),
    ("e05", 91000, 28000, 17000, 110,   15000, 80,   50),
    ("e06", 93000, 31000, 16000, 95,    100,   16000, 40),
    ("e07", 89000, 29000, 15500, 105,   90,    13000, 60),
    ("e08", 94000, 34000, 17000, 90,    110,   15000, 55),
    ("e09", 90000, 25000, 95,    100,   85,    65,    12000),
    # 非淋巴：单核、碎片
    ("e10", 120000, 90000, 150, 130, 110, 80, 60),
    ("e11", 20000, 8000,   60,  50,  40,  30, 25),
    # 极端：过补偿负信号；超量程；不可转换 SAT（保留原位）
    ("e12", 90500, 32000, -900, 100, 110, 70, 50),
    ("e13", 90700, 30500, 300000, 90, 100, 60, 40),
    ("e14", 91200, 31500, 90, "SAT", 95, 75, 45),
]

def solve_raw(comp, s):
    """raw = S^-1 comp，高斯消元（良态）。"""
    n = len(s)
    m = [list(s[i]) + [float(comp[i])] for i in range(n)]
    for k in range(n):
        piv = max(range(k, n), key=lambda r: abs(m[r][k]))
        m[k], m[piv] = m[piv], m[k]
        for i in range(k + 1, n):
            f = m[i][k] / m[k][k]
            for j in range(k, n + 1):
                m[i][j] -= f * m[k][j]
    x = [0.0] * n
    for i in range(n - 1, -1, -1):
        x[i] = (m[i][n] - sum(m[i][j] * x[j] for j in range(i + 1, n))) / m[i][i]
    return x

def asinh10(x, c):
    return math.asinh(x / c) / math.log(10.0)

def r2(x):
    return round(x + 1e-9, 2)


def transform_v1():
    out = []
    for ch in CHANNELS:
        if ch in COFACTOR:
            out.append({"channel": ch, "method": "asinh", "cofactor": COFACTOR[ch],
                        "offset": 0.0, "output_unit": "asinh:log10-scale"})
        else:
            out.append({"channel": ch, "method": "linear", "cofactor": 1.0,
                        "offset": 0.0, "output_unit": "linear:pulse-area"})
    return {"material_id": "mat-transform", "version": 1, "transforms": out}

def rect_poly(x0, y0, x1, y1):
    return [[round(x0, 4), round(y0, 4)], [round(x1, 4), round(y0, 4)],
            [round(x1, 4), round(y1, 4)], [round(x0, 4), round(y1, 4)]]

def build_case():
    # 原始事件：荧光通道保留“传感器原始读数”，由 raw=S^-1 comp 得到
    events = []
    compensated = {}
    for row in TARGETS:
        eid, fsc, ssc = row[0], row[1], row[2]
        flu = row[3:8]
        numeric = [float(v) for v in flu if isinstance(v, (int, float))]
        sat_cols = [i for i, v in enumerate(flu) if not isinstance(v, (int, float))]
        # SAT 列先占位，其余列用缩减系统不现实；改为：SAT 保留字符串，
        # 数值列 raw 直接使用目标值（该事件不进入补偿统计，诊断标注）。
        raw = {}
        comp = {}
        if sat_cols:
            for i, ch in enumerate(FLUOR):
                if i in sat_cols:
                    raw[ch] = flu[i]
                    comp[ch] = None
                else:
                    raw[ch] = float(flu[i])
                    comp[ch] = float(flu[i])
        else:
            sol = solve_raw(numeric, S)
            for i, ch in enumerate(FLUOR):
                raw[ch] = r2(sol[i])
            # 用取整后的 raw 重新补偿，作为后续门控使用的真实补偿值
            rawnums = [raw[ch] for ch in FLUOR]
            for i, ch in enumerate(FLUOR):
                comp[ch] = round(sum(S[i][j] * rawnums[j] for j in range(len(FLUOR))), 3)
        raw["FSC-A"] = float(fsc)
        raw["SSC-A"] = float(ssc)
        comp["FSC-A"] = float(fsc)
        comp["SSC-A"] = float(ssc)
        channels = []
        for ch in CHANNELS:
            v = raw[ch]
            if isinstance(v, str):
                channels.append([ch, {"raw": v, "value": None,
                    "unit": "linear:pulse-area",
                    "diagnostic": "unconvertible value kept in place: SAT (saturated token), not dropped for plotting"}])
            else:
                channels.append([ch, {"raw": ("%.2f" % v if ch in FLUOR else str(int(v))),
                    "value": v, "unit": "linear:pulse-area", "diagnostic": None}])
        events.append({"event_id": eid, "channels": channels})
        compensated[eid] = comp
    return events, compensated


def gate_set(comp):
    def tval(eid, ch):
        c = COFACTOR[ch]
        return asinh10(comp[eid][ch], c)

    # gate-lymph 使用 FSC/SSC 线性坐标；e09(90000,25000) 落在底边（边界点）
    lymph_v1 = {
        "gate_id": "gate-lymph", "version": 1, "name": "Lymphocytes",
        "x_channel": "FSC-A", "y_channel": "SSC-A",
        "coordinate_basis": "transformed:linear:pulse-area@mat-transform/v1",
        "polygon": rect_poly(70000, 25000, 110000, 60000),
        "boundary_convention": "raycast-include",
    }
    # v2：收缩多边形（SSC 上缘 60000 -> 40000），e02(SSC 42000) 被移出，
    # 用于“上游修订 -> 下游标记过期而不覆盖”。
    lymph_v2 = dict(lymph_v1, version=2,
                    polygon=rect_poly(70000, 25000, 110000, 40000))

    def bounds(eids, xch, ych, mx=0.18, my=0.18):
        xs = [tval(e, xch) for e in eids]
        ys = [tval(e, ych) for e in eids]
        return min(xs) - mx, min(ys) - my, max(xs) + mx, max(ys) + my

    # CD3(FITC) vs CD19(PE)
    t_ids = ["e01", "e02", "e05", "e06", "e07", "e08"]
    b_ids = ["e03", "e04"]
    tx0, ty0, tx1, ty1 = bounds(t_ids, "FITC-A", "PE-A")
    bx0, by0, bx1, by1 = bounds(b_ids, "FITC-A", "PE-A")
    gate_t_v1 = {
        "gate_id": "gate-t", "version": 1, "name": "T cells (CD3+)",
        "x_channel": "FITC-A", "y_channel": "PE-A",
        "coordinate_basis": "transformed:asinh:log10-scale@mat-transform/v1",
        "polygon": rect_poly(tx0, ty0, tx1, ty1),
        "boundary_convention": "raycast-include",
    }
    gate_b_v1 = {
        "gate_id": "gate-b", "version": 1, "name": "B cells (CD19+)",
        "x_channel": "FITC-A", "y_channel": "PE-A",
        "coordinate_basis": "transformed:asinh:log10-scale@mat-transform/v1",
        "polygon": rect_poly(bx0, by0, bx1, by1),
        "boundary_convention": "raycast-include",
    }
    # CD4(PerCP) 门：父 gate-t
    cd4_ids = ["e02", "e05"]
    x0, y0, x1, y1 = bounds(cd4_ids, "PerCP-A", "APC-A")
    gate_cd4_v1 = {
        "gate_id": "gate-cd4", "version": 1, "name": "CD4+ T",
        "x_channel": "PerCP-A", "y_channel": "APC-A",
        "coordinate_basis": "transformed:asinh:log10-scale@mat-transform/v1",
        "polygon": rect_poly(x0, y0, x1, y1),
        "boundary_convention": "raycast-include",
    }
    # CD8(APC) 门：父 gate-t
    cd8_ids = ["e06", "e07", "e08"]
    x0, y0, x1, y1 = bounds(cd8_ids, "APC-A", "PerCP-A")
    gate_cd8_v1 = {
        "gate_id": "gate-cd8", "version": 1, "name": "CD8+ T",
        "x_channel": "APC-A", "y_channel": "PerCP-A",
        "coordinate_basis": "transformed:asinh:log10-scale@mat-transform/v1",
        "polygon": rect_poly(x0, y0, x1, y1),
        "boundary_convention": "raycast-include",
    }
    # T 门 v2：仅改名（嵌套改名），多边形与坐标完全不变
    gate_t_v2 = dict(gate_t_v1, version=2, name="T-lineage CD3+ (renamed)")
    return {
        "v1": [lymph_v1, gate_t_v1, gate_b_v1, gate_cd4_v1, gate_cd8_v1],
        "staged": {"gate-t": gate_t_v2, "gate-lymph": lymph_v2},
    }

def hierarchy_v1():
    return {"material_id": "mat-hierarchy", "version": 1, "parent_of": [
        ["gate-lymph", None],
        ["gate-t", "gate-lymph"],
        ["gate-b", "gate-lymph"],
        ["gate-cd4", "gate-t"],
        ["gate-cd8", "gate-t"],
    ]}


NEAR_SINGULAR = [
    [1.000, 0.020, 0.000, 0.000, 0.000],
    [0.040, 1.000, 0.000, 0.000, 0.000],
    [0.000, 0.000, 1.000, 0.99999, 0.000],
    [0.000, 0.000, 0.99999, 1.000, 0.000],
    [0.000, 0.000, 0.000, 0.000, 1.000],
]

def transform_v2():
    t = transform_v1()
    t = json.loads(json.dumps(t))
    t["version"] = 2
    for tr in t["transforms"]:
        if tr["channel"] == "FITC-A":
            tr["cofactor"] = 300.0
    return t

def compensation_v1():
    return {"material_id": "mat-compensation", "version": 1,
            "labels": FLUOR,
            "rows": [[round(v, 5) for v in row] for row in S],
            "unit": "dimensionless:spillover-fraction"}

def compensation_near_singular():
    return {"material_id": "mat-compensation", "version": 2,
            "labels": FLUOR, "rows": NEAR_SINGULAR,
            "unit": "dimensionless:spillover-fraction",
            "note": "APC/PerCP 通道近共线单染产生的近奇异 spillover（条件数>=1e5），系统不得静默求逆"}

def raw_events_v2(events_v1):
    # 只改一个直接来源：e02 的 PerCP 原始读数 15242.78 -> 24000.00，
    # 影响范围仅 CD4 群（e02 的 PerCP 被推高，仍在 CD4 门内）与依赖它的百分比分母链路。
    ev = json.loads(json.dumps(events_v1))
    for e in ev:
        if e["event_id"] == "e02":
            for ch in e["channels"]:
                if ch[0] == "PerCP-A":
                    ch[1] = {"raw": "24000.00", "value": 24000.0,
                             "unit": "linear:pulse-area",
                             "diagnostic": "source patch: raw detector value revised (v2); original retained in mat-raw-events/v1"}
    return {"material_id": "mat-raw-events", "version": 2,
            "channels": CHANNELS, "events": ev}

def fcs_text_blob(events):
    lines = [
        "FCS3.1 $BEGINANALYSIS 0 $ENDANALYSIS 0",
        "$PAR 7 $TOT %d" % len(events),
        "$P1N FSC-A $P1R 262144 $P1E 0,0 $P1G 1",
        "$P2N SSC-A $P2R 262144 $P2E 0,0 $P2G 1",
        "$P3N FITC-A $P3R 262144 $P3E 0,0 $P3G 1",
        "$P4N PE-A $P4R 262144 $P4E 0,0 $P4G 1",
        "$P5N PerCP-A $P5R 262144 $P5E 0,0 $P5G 1",
        "$P6N APC-A $P6R 262144 $P6E 0,0 $P6G 1",
        "$P7N APC-Cy7-A $P7R 262144 $P7E 0,0 $P7G 1",
        "FLOWBOUNDARY SPILLOVERBASIS single-stain-controls-v1",
        "FLOWBOUNDARY SATTOKEN SAT",
    ]
    for e in events:
        vals = ",".join(ch[1]["raw"] for ch in e["channels"])
        lines.append("EVENT %s %s" % (e["event_id"], vals))
    return "\n".join(lines) + "\n"

def assemble():
    events, comp = build_case()
    raw_v1 = {"material_id": "mat-raw-events", "version": 1,
              "channels": CHANNELS, "events": events}
    gates = gate_set(comp)
    meta = {
        "material_id": "mat-spillover-meta", "version": 1,
        "detectors": FLUOR,
        "fluorochromes": ["FITC", "PE", "PerCP", "APC", "APC-Cy7"],
        "coordinate_basis": "linear:pulse-area",
        "single_stain_controls": [
            {"fluorochrome": "FITC", "control_event_ids": ["ssc-fitc-01", "ssc-fitc-02"],
             "note": "FITC 单染微球，门 P1 中位数"},
            {"fluorochrome": "PE", "control_event_ids": ["ssc-pe-01", "ssc-pe-02"],
             "note": "PE 单染细胞"},
            {"fluorochrome": "PerCP", "control_event_ids": ["ssc-percp-01", "ssc-percp-02"],
             "note": "PerCP 单染细胞"},
            {"fluorochrome": "APC", "control_event_ids": ["ssc-apc-01", "ssc-apc-02"],
             "note": "APC 单染细胞"},
            {"fluorochrome": "APC-Cy7", "control_event_ids": ["ssc-apccy7-01", "ssc-apccy7-02"],
             "note": "APC-Cy7 单染细胞"},
        ],
    }
    case = {
        "case_id": "flow-boundary-chamber-v1",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "domain": "flow-cytometry",
        "materials": {
            "raw_events": [raw_v1, raw_events_v2(events)],
            "spillover_meta": [meta],
            "compensation": [compensation_v1(), compensation_near_singular()],
            "transform": [transform_v1(), transform_v2()],
            "gates": gates["v1"] + [gates["staged"]["gate-t"], gates["staged"]["gate-lymph"]],
            "hierarchy": [hierarchy_v1()],
        },
        "baseline_refs": {
            "raw_events": "mat-raw-events/v1",
            "spillover_meta": "mat-spillover-meta/v1",
            "compensation": "mat-compensation/v1",
            "transform": "mat-transform/v1",
            "gates": {
                "gate-lymph": "gate-lymph/v1",
                "gate-t": "gate-t/v1",
                "gate-b": "gate-b/v1",
                "gate-cd4": "gate-cd4/v1",
                "gate-cd8": "gate-cd8/v1",
            },
            "hierarchy": "mat-hierarchy/v1",
        },
        "scenarios": [
            {"id": "import-rollback", "kind": "import-failure-rollback",
             "description": "导入缺少必需材料的包必须整体回滚，不留半份记录"},
            {"id": "singular-compensation", "kind": "near-singular-matrix",
             "description": "近奇异补偿矩阵不能被默默求逆；保留全部合法候选等待人工裁定"},
            {"id": "transform-revision", "kind": "parameter-revision",
             "description": "修订变换参数（FITC cofactor 150->300），按依赖图限定失效范围"},
            {"id": "local-source-patch", "kind": "source-patch-local",
             "description": "只改一个直接来源 e02/PerCP 原始读数，失效范围仅限其下游"},
            {"id": "gate-rename", "kind": "gate-nested-rename",
             "description": "门嵌套改名：几何不变、谱系引用保持，旧版仍可读"},
            {"id": "upstream-gate-revision", "kind": "upstream-stale-propagation",
             "description": "修订上游 lymph 门后，下游结果标记过期而不覆盖"},
            {"id": "concurrent-modification", "kind": "same-version-concurrent",
             "description": "同版本并发修改：后提交者看到版本冲突及双方差异"},
        ],
        "expected_baseline": {
            "root_count": 14,
            "gate_members": {
                "gate-lymph": 12, "gate-t": 6, "gate-b": 2,
                "gate-cd4": 2, "gate-cd8": 3,
            },
        },
    }
    blob = fcs_text_blob(events)
    return case, blob


def main():
    case, blob = assemble()
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "flow_case.json"), "w", encoding="utf-8") as f:
        json.dump(case, f, ensure_ascii=False, indent=2, sort_keys=False)
        f.write("\n")
    blob_bytes = blob.encode("utf-8")
    with open(os.path.join(OUT, "fcs_text.txt"), "wb") as f:
        f.write(blob_bytes)
    manifest = {
        "case_id": case["case_id"],
        "files": [
            {"path": "flow_case.json",
             "sha256": hashlib.sha256(open(os.path.join(OUT, "flow_case.json"), "rb").read()).hexdigest(),
             "role": "versioned-materials-and-scenarios"},
            {"path": "fcs_text.txt",
             "sha256": hashlib.sha256(blob_bytes).hexdigest(),
             "role": "raw-fcs-text-segment-preserved"},
        ],
    }
    with open(os.path.join(OUT, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)
        f.write("\n")
    # 自检：打印各群成员，供开发期核对
    import importlib.util
    print("wrote case with", len(case["materials"]["raw_events"][0]["events"]), "events")
    print(blob[:200])

if __name__ == "__main__":
    main()
