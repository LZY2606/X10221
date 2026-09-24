#!/usr/bin/env python3
"""生成离线案例包「流式边界室：补偿矩阵、门控系谱与细胞群不确定性」。

确定性生成（seed 固定），输出 casepack/boundary_chamber.json。
所有领域量携带单位与坐标基准；原始表示保留在 raw 层。
"""
import json, math, random

rng = random.Random(7)

CHANNELS = ["FSC-A", "SSC-A", "FITC-A", "PE-A", "APC-A"]
FLUOR = ["FITC-A", "PE-A", "APC-A"]
RAW_UNIT = "ADC channel (18-bit, 0-262143)"
RAW_BASIS = "linear acquisition scale"
COFACTOR = 150.0

# 观测 = S @ 真实；补偿矩阵即 S（求逆后还原真实信号）
S = [[1.00, 0.18, 0.03],
     [0.05, 1.00, 0.22],
     [0.02, 0.07, 1.00]]

# 近奇异版本：第 3 行 ≈ 第 1 行 + 第 2 行（精确相加为 [1.05, 1.18, 0.25]）
S_SINGULAR = [[1.00, 0.18, 0.03],
              [0.05, 1.00, 0.22],
              [1.05, 1.18, 0.2500001]]

def spill(true):
    return [sum(S[i][j] * true[j] for j in range(3)) for i in range(3)]

def gauss(mu, sigma):
    return rng.gauss(mu, sigma)

events = []
eid = 0

def add_event(fsc, ssc, true, diagnostics=None, raw_override=None, nd=0):
    global eid
    eid += 1
    obs = spill(true)
    raw = {"FSC-A": round(fsc, nd), "SSC-A": round(ssc, nd)}
    for i, ch in enumerate(FLUOR):
        raw[ch] = round(obs[i], nd)
    if raw_override:
        raw.update(raw_override)
    ev = {"id": "ev-%03d" % eid, "raw": raw}
    if diagnostics:
        ev["diagnostics"] = diagnostics
    events.append(ev)
    return ev

# --- 淋巴细胞群：36 个，其中 24 个 CD3+，其内 12 个 CD4+ ---
for k in range(36):
    fsc = min(max(gauss(60000, 11000), 30000), 100000)
    ssc = min(max(gauss(30000, 8000), 10000), 52000)
    if k < 24:  # CD3+
        fitc = gauss(40000, 5000)
        if k < 12:  # CD4+
            pe, apc = gauss(25000, 3000), gauss(8000, 1200)
        else:
            pe, apc = gauss(300, 70), gauss(200, 50)
    else:       # CD3-
        fitc, pe, apc = gauss(300, 80), gauss(280, 70), gauss(220, 60)
    add_event(fsc, ssc, [fitc, pe, apc])

# --- 碎片：8 个，落在淋巴细胞门之外 ---
for _ in range(8):
    add_event(gauss(8000, 2200), gauss(4000, 1300),
              [gauss(200, 60), gauss(180, 50), gauss(150, 40)])

# --- 极端信号：饱和 / 超量程 / 不可转换（原始表示保留） ---
add_event(60000, 30000, [40000, 300, 200],
          diagnostics=["saturated:FITC-A"], raw_override={"FITC-A": 262143})
add_event(61000, 31000, [40000, 300, 200],
          diagnostics=["out_of_range:PE-A"], raw_override={"PE-A": 300000})
add_event(59000, 29000, [40000, 300, 200],
          diagnostics=["unconvertible:APC-A"], raw_override={"APC-A": "OVERFLOW"})

# --- 边界点 B1：恰好落在淋巴细胞门第 0 条边中点 (65000, 9000) ---
add_event(65000, 9000, [300, 280, 220], diagnostics=["boundary:gate:lymphocytes"], nd=3)

# --- 边界点 B2：补偿+变换后恰好落在 CD3 门边 x=4.5 上 ---
comp_fitc = 150.0 * math.sinh(4.5)
comp_pe   = 150.0 * math.sinh(2.0)
comp_apc  = 150.0 * math.sinh(1.0)
add_event(60000, 20000, [comp_fitc, comp_pe, comp_apc],
          diagnostics=["boundary:gate:cd3_pos"], nd=3)

LYMPH_GATE = [[25000, 8000], [105000, 10000], [115000, 55000], [55000, 65000], [20000, 35000]]
LYMPH_GATE_V2 = [[26000, 8000], [105000, 10000], [115000, 55000], [55000, 65000], [20000, 35000]]
CD3_GATE = [[4.5, 0.0], [7.5, 0.0], [7.5, 7.0], [4.5, 7.0]]
CD4_GATE = [[3.5, 1.0], [7.0, 1.0], [7.0, 5.5], [3.5, 5.5]]
CD4_GATE_V2 = [[3.6, 1.0], [7.0, 1.0], [7.0, 5.6], [3.5, 5.6]]

def gate_payload(series_label, axes, polygon, axes_basis):
    return {"label": series_label, "axes": axes, "polygon": polygon,
            "axes_basis": axes_basis,
            "unit": RAW_UNIT if axes_basis == "raw" else "arcsinh intensity",
            "basis": RAW_BASIS if axes_basis == "raw" else "arcsinh(x/cofactor)"}

pack = {
    "pack": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
    "pack_id": "boundary-chamber",
    "pack_version": 1,
    "panel": {"channels": CHANNELS, "fluorescence": FLUOR,
              "unit": RAW_UNIT, "basis": RAW_BASIS},
    "artifacts": {
        "raw_events": {
            "series": "raw:events", "kind": "RawChannels", "label": "原始通道值",
            "unit": RAW_UNIT, "basis": RAW_BASIS,
            "events": events},
        "spillover": {
            "series": "meta:spillover", "kind": "SpilloverMeta", "label": "spillover 元数据",
            "unit": "ratio (detector/fluorophore)", "basis": "median of single-stain controls",
            "controls": [
                {"fluorophore": "FITC", "detector": "FITC-A",
                 "samples": [round(gauss(40000, 800), 1) for _ in range(5)]},
                {"fluorophore": "PE", "detector": "PE-A",
                 "samples": [round(gauss(30000, 600), 1) for _ in range(5)]},
                {"fluorophore": "APC", "detector": "APC-A",
                 "samples": [round(gauss(20000, 500), 1) for _ in range(5)]}],
            "coefficients": S},
        "matrix": {
            "series": "matrix:comp", "kind": "CompensationMatrix", "label": "补偿矩阵",
            "unit": "ratio", "basis": "observed = matrix @ true",
            "channels": FLUOR, "matrix": S},
        "transform": {
            "series": "xform:arcsinh", "kind": "TransformParams", "label": "变换参数",
            "unit": "arcsinh intensity", "basis": "arcsinh(x/cofactor)",
            "kind_name": "arcsinh", "cofactor": COFACTOR},
        "gates": [
            {"series": "gate:lymphocytes", "kind": "GatePolygon",
             **gate_payload("Lymphocytes", ["FSC-A", "SSC-A"], LYMPH_GATE, "raw")},
            {"series": "gate:cd3_pos", "kind": "GatePolygon",
             **gate_payload("CD3+", ["FITC-A", "PE-A"], CD3_GATE, "transformed")},
            {"series": "gate:cd4_pos", "kind": "GatePolygon",
             **gate_payload("CD4+", ["PE-A", "APC-A"], CD4_GATE, "transformed")}],
        "hierarchy": {
            "series": "tree:hierarchy", "kind": "GateHierarchy", "label": "父子关系",
            "unit": "n/a", "basis": "gate series references",
            "nodes": [{"gate": "gate:lymphocytes", "parent": None},
                      {"gate": "gate:cd3_pos", "parent": "gate:lymphocytes"},
                      {"gate": "gate:cd4_pos", "parent": "gate:cd3_pos"}]},
    },
    "replay": [
        {"id": "revise_transform", "kind": "revise_transform",
         "series": "xform:arcsinh", "cofactor": 100.0,
         "note": "参数修订：cofactor 150 -> 100，仅使依赖变换的群体过期"},
        {"id": "revise_gate_local", "kind": "revise_gate",
         "series": "gate:cd4_pos", "label": "CD4+", "polygon": CD4_GATE_V2,
         "note": "只影响局部依赖的源数据改动：仅 cd4 群体过期"},
        {"id": "rename_gate", "kind": "rename_gate",
         "series": "gate:cd4_pos", "label": "CD4+ T cells",
         "note": "门嵌套改名：稳定身份不变，不触发失效"},
        {"id": "revise_gate_upstream", "kind": "revise_gate",
         "series": "gate:lymphocytes", "label": "Lymphocytes", "polygon": LYMPH_GATE_V2,
         "note": "上游过期传播：lymphocytes 及全部下游标记过期而不覆盖"},
        {"id": "import_near_singular_matrix", "kind": "import_matrix",
         "series": "matrix:comp", "label": "补偿矩阵 v2（近奇异）",
         "channels": FLUOR, "matrix": S_SINGULAR,
         "note": "矩阵奇异：系统拒绝默默求逆，给出合法候选"},
        {"id": "conflicting_rulings", "kind": "conflicting_rulings",
         "note": "互相冲突的人工决定：main 接受包含边界点，review-b 否定"},
        {"id": "concurrent_save", "kind": "concurrent_save",
         "series": "gate:cd3_pos",
         "note": "同版本并发修改：后提交者看到版本冲突及双方差异"}],
}

with open("casepack/boundary_chamber.json", "w", encoding="utf-8") as f:
    json.dump(pack, f, ensure_ascii=False, indent=1)
print("events:", len(events))
