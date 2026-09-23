#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""离线案例包生成器：生成真实 FCS 3.1 二进制文件与领域元数据。

领域材料（绝不拿通用表格替代）：
  cases/fcs/sample.fcs      24 个事件（含极端信号/饱和、边界点、溢出负值）
  cases/fcs/ctrl_fitc.fcs   FITC 单染对照
  cases/fcs/ctrl_pe.fcs     PE 单染对照
  cases/spillover_good.json 良态 spillover 元数据（S0）
  cases/spillover_singular.json  完全奇异矩阵（两列相同）
  cases/spillover_nearsing.json  近奇异矩阵（两列近似共线，多候选）
  cases/transform_v1.json / transform_v2.json  变换参数修订
  cases/gates_v1.json / gates_v2.json          门多边形（v2 局部收窄 g_cd3）
  cases/names_v2.json                           门嵌套改名（仅 live→存活细胞）
  cases/manifest.json                            重放步骤（机器可读事件序列蓝图）

所有领域量都携带单位或坐标基准（线性脉冲计数 linear_counts，或 asinh 坐标）。
"""
import json
import os
import struct
import math

HERE = os.path.dirname(os.path.abspath(__file__))
CASE = os.path.join(HERE, "..", "cases")
FCS = os.path.join(CASE, "fcs")
os.makedirs(FCS, exist_ok=True)

CHANNELS = [
    # name, short, long
    ("FSC-A", "FSC", "Forward Scatter"),
    ("SSC-A", "SSC", "Side Scatter"),
    ("FITC-A", "FITC", "Fluorescein"),
    ("PE-A", "PE", "Phycoerythrin"),
    ("APC-A", "APC", "Allophycocyanin"),
]
N = len(CHANNELS)

# 良态 spillover（行=检测器，列=荧光通道；前两列为散射不参与补偿）
S0 = [
    [1.00, 0.02, 0.01],  # FITC detector
    [0.10, 1.00, 0.03],  # PE detector
    [0.01, 0.05, 1.00],  # APC detector
]
S_NEAR = [
    [1.00, 0.96, 0.01],
    [0.97, 1.00, 0.03],
    [0.01, 0.05, 1.00],
]
S_SING = [
    [1.0, 1.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
]


def write_fcs(path, events, spillover, spill_name):
    n_events = len(events)
    txt_pairs = [
        ("BEGINANALYSIS", "0"),
        ("ENDANALYSIS", "0"),
        ("BEGINSTEXT", "0"),
        ("ENDSTEXT", "0"),
        ("BYTEORD", "1,2,3,4"),
        ("DATATYPE", "F"),
        ("$NEXTDATA", "0"),
        ("$SYS", "FlowLine Case Generator"),
        ("CREATOR", "gen_cases.py"),
        ("$TOT", str(n_events)),
        ("$PAR", str(N)),
        ("$MODE", "L"),
    ]
    for i, (name, short, longn) in enumerate(CHANNELS, start=1):
        txt_pairs += [
            ("$P%dN" % i, name),
            ("$P%dS" % i, longn),
            ("$P%dE" % i, "0,0" if i <= 2 else "0,0"),
            ("$P%dR" % i, "262144"),
            ("$P%dG" % i, "1"),
            ("$P%dU" % i, "linear_counts"),
            ("$P%dV" % i, "0.0"),
            ("$P%dT" % i, "262144.0"),
        ]
    # $SPILLOVER: n, names, then n rows
    fluo = [CHANNELS[i][0] for i in (2, 3, 4)]
    lines = [str(len(fluo)), ",".join(fluo)]
    for row in spillover:
        lines.append(",".join("%.6f" % v for v in row))
    txt_pairs.append(("$SPILLOVER", "\\n".join(lines)))
    txt_pairs.append(("SPILL_META_ID", spill_name))

    text = "/"
    for k, v in txt_pairs:
        text += k + "/" + v + "/"
    tb = text.encode("utf-8")
    # header offsets: text begins at 256, data follows, ANALYSIS 0
    text_begin = 256
    text_end = text_begin + len(tb) - 1
    data_begin = text_end + 1
    raw = b"".join(struct.pack("<" + "f" * N, *e) for e in events)
    data_end = data_begin + len(raw) - 1

    header = "FCS3.1    "
    header += "%-8d%-8d" % (text_begin, text_end)
    header += "%-8d%-8d" % (data_begin, data_end)
    header += "%-8d%-8d" % (0, 0)
    header = header.ljust(256, " ")
    assert len(header) == 256, len(header)
    with open(path, "wb") as fh:
        fh.write(header.encode("ascii"))
        fh.write(tb)
        fh.write(raw)


# ---------------------------------------------------------------------------
# 事件生成（确定性，无随机）
# 散射：活细胞簇 (FSC~120k,SSC~70k)，碎片簇 (FSC~30k,SSC~15k)
# g_live 多边形 v1（linear_counts 坐标），边界事件 e01 严格落在边上
# ---------------------------------------------------------------------------
live_poly = [[60000, 30000], [190000, 30000], [190000, 140000], [60000, 140000]]
# 下边 y=30000, x∈[60000,190000] 是边界线
events = []

# e00 活细胞 FITC 亮
events.append((120000, 70000, 60000 + int(round(0.02*12000)), 12000, 2000))
# e01 边界点：严格落在 g_live 下边 y=30000
events.append((100000, 30000, 20000, 4000, 1500))
# e02..e09 活细胞 FITC 亮（共 10 个 FITC+ 候选）
fitc_extra = [
    (130000, 75000, 64000, 9000, 1800),
    (125000, 68000, 58000, 7000, 2200),
    (115000, 90000, 70000, 15000, 3000),
    (140000, 60000, 55000, 8000, 1200),
    (135000, 95000, 68000, 20000, 5000),
    (150000, 80000, 62000, 11000, 2600),
    (110000, 72000, 57000, 6000, 1000),
    (160000, 100000, 72000, 18000, 4200),
]
events.extend(fitc_extra)
# e10 极端饱和事件：所有通道 262143（PE 亦高，会落 CD3 外，保留诊断）
events.append((262143, 262143, 262143, 262143, 262143))
# e11..e16 活细胞但 FITC 阴性（6 个），其中一个 PE 亮
events += [
    (128000, 66000, 3000, 40000, 2000),
    (133000, 85000, 5000, 5000, 60000),   # APC+
    (145000, 70000, 2500, 6000, 2000),
    (118000, 62000, 4200, 7000, 3000),
    (155000, 90000, 3500, 55000, 4000),  # PE 亮
    (122000, 78000, 2800, 9000, 2500),
]
# e17..e22 碎片 6 个
events += [
    (30000, 15000, 2000, 1000, 500),
    (25000, 12000, 1500, 900, 400),
    (40000, 20000, 3000, 2000, 800),
    (20000, 10000, 1000, 500, 300),
    (35000, 18000, 2500, 1500, 700),
    (28000, 14000, 1800, 1100, 600),
]
# e23 补偿后负信号检验：未染色但 FITC 通道轻微正/PE 低，补偿后 FITC 应为负
events.append((140000, 82000, 800, 120000, 1500))
assert len(events) == 24, len(events)


def simple_control(fluor_idx, n_pos=5, n_neg=3):
    out = []
    for _ in range(n_pos):
        row = [130000, 70000, 2000, 2000, 2000]
        row[fluor_idx] = 80000
        out.append(tuple(row))
    for _ in range(n_neg):
        out.append((125000, 68000, 1800, 1800, 1800))
    return out


write_fcs(os.path.join(FCS, "sample.fcs"), events, S0, "spill_good_v1")
write_fcs(os.path.join(FCS, "ctrl_fitc.fcs"), simple_control(2), S0, "spill_good_v1")
write_fcs(os.path.join(FCS, "ctrl_pe.fcs"), simple_control(3), S0, "spill_good_v1")


def asinh_v1(x):
    return math.asinh(x / 150.0)


# g_cd3 在 g_live 内；坐标基准 asinh(cofactor=150,linear_counts)
# 覆盖 FITC 亮 9 个（e00,e02..e09），排除 PE/APC 亮与 e23；饱和事件 SSC asinh 高被排除
cd3_poly_asinh = [
    [asinh_v1(50000), 4.5],
    [asinh_v1(80000), 4.5],
    [asinh_v1(80000), 7.2],
    [asinh_v1(50000), 7.2],
]
# g_cd4 在 g_cd3 内（演示嵌套）；x=FITC asinh, y=APC asinh
cd4_poly_asinh = [
    [3.0, asinh_v1(40000)],
    [6.0, asinh_v1(60000)],
    [6.0, asinh_v1(90000)],
    [3.0, asinh_v1(90000)],
]

# g_cd3 v2：局部收窄（FITC asinh 下界提高到 53000）——只影响 g_cd3 子树
cd3_poly_v2_asinh = [
    [asinh_v1(53000), 4.5],
    [asinh_v1(80000), 4.5],
    [asinh_v1(80000), 7.2],
    [asinh_v1(53000), 7.2],
]


def dump(name, obj):
    with open(os.path.join(CASE, name), "w", encoding="utf-8") as fh:
        json.dump(obj, fh, ensure_ascii=False, indent=2, sort_keys=False)
        fh.write("\n")


spill_good = {
    "schema": "flowline.spillover_meta/v1",
    "id": "spill_good_v1",
    "units_note": "矩阵无量纲（检测器线性脉冲计数线性映射），源信号单位 linear_counts",
    "fluorescence_channels": ["FITC-A", "PE-A", "APC-A"],
    "matrix": S0,
    "controls": [
        {"channel": "FITC-A", "fcs": "fcs/ctrl_fitc.fcs"},
        {"channel": "PE-A", "fcs": "fcs/ctrl_pe.fcs"},
    ],
    "tolerance": {"near_singular_condition": 50.0},
}
spill_sing = dict(spill_good, id="spill_singular_v1", matrix=S_SING,
                  controls=[], note="完全奇异：FITC/PE 两列相同，无法唯一求逆")
spill_near = dict(spill_good, id="spill_near_v1", matrix=S_NEAR,
                  controls=[{"channel": "FITC-A", "fcs": "fcs/ctrl_fitc.fcs"}],
                  note="近奇异：FITC/PE 近似共线；保留全部合法候选，禁止静默求逆")

dump("spillover_good.json", spill_good)
dump("spillover_singular.json", spill_sing)
dump("spillover_nearsing.json", spill_near)

transform_v1 = {
    "schema": "flowline.transform/v1", "id": "tf_v1",
    "params": {
        "FSC-A": {"method": "linear", "unit": "linear_counts"},
        "SSC-A": {"method": "linear", "unit": "linear_counts"},
        "FITC-A": {"method": "asinh", "cofactor": 150.0, "unit_in": "linear_counts",
                   "coordinate": "asinh(cofactor=150,linear_counts)"},
        "PE-A": {"method": "asinh", "cofactor": 150.0, "unit_in": "linear_counts",
                 "coordinate": "asinh(cofactor=150,linear_counts)"},
        "APC-A": {"method": "asinh", "cofactor": 150.0, "unit_in": "linear_counts",
                  "coordinate": "asinh(cofactor=150,linear_counts)"},
    },
}
transform_v2 = json.loads(json.dumps(transform_v1))
transform_v2["id"] = "tf_v2"
transform_v2["note"] = "参数修订：cofactor 150→300（只改显示坐标，门点按其坐标基准重放，不静默覆盖结论）"
for ch in ("FITC-A", "PE-A", "APC-A"):
    transform_v2["params"][ch]["cofactor"] = 300.0
    transform_v2["params"][ch]["coordinate"] = "asinh(cofactor=300,linear_counts)"
dump("transform_v1.json", transform_v1)
dump("transform_v2.json", transform_v2)

gates_v1 = {
    "schema": "flowline.gate_set/v1", "id": "gates_v1",
    "polygons": [
        {"gate_id": "g_live", "name": "Live", "parent": None,
         "x": "FSC-A", "y": "SSC-A", "coordinate_basis": "linear_counts",
         "vertices": live_poly, "boundary_convention": "inclusive"},
        {"gate_id": "g_cd3", "name": "CD3+", "parent": "g_live",
         "x": "FITC-A", "y": "SSC-A", "coordinate_basis": "asinh(cofactor=150,linear_counts)",
         "vertices": cd3_poly_asinh, "boundary_convention": "inclusive"},
        {"gate_id": "g_cd4", "name": "CD4+", "parent": "g_cd3",
         "x": "FITC-A", "y": "APC-A", "coordinate_basis": "asinh(cofactor=150,linear_counts)",
         "vertices": cd4_poly_asinh, "boundary_convention": "inclusive"},
    ],
}
gates_v2 = json.loads(json.dumps(gates_v1))
gates_v2["id"] = "gates_v2"
gates_v2["note"] = "源数据的局部改动：仅 g_cd3 多边形收窄；g_live/g_cd4 点不变"
gates_v2["polygons"][1]["vertices"] = cd3_poly_v2_asinh
dump("gates_v1.json", gates_v1)
dump("gates_v2.json", gates_v2)

names_v2 = {
    "schema": "flowline.gate_names/v1", "id": "names_v2",
    "note": "门嵌套改名：gate_id 保持稳定，仅显示名变化；不改变成员与父关系",
    "names": {"g_live": "存活细胞", "g_cd3": "CD3 阳性", "g_cd4": "CD4 阳性"},
}
dump("names_v2.json", names_v2)

manifest = {
    "schema": "flowline.case_manifest/v1",
    "id": "flow_boundary_chamber_v1",
    "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
    "narrative": [
        "导入 FCS 事件、单染对照与门控草案；六类来源分开版本化并逐项复核",
        "尝试完全奇异补偿矩阵：拒绝静默求逆，记录被否定结果",
        "尝试近奇异补偿矩阵：保留全部合法候选，人工裁定其一",
        "以良态矩阵补偿：检查极端/饱和事件、边界点与补偿后负值（保留诊断）",
        "修订变换参数（cofactor 150→300）：结论按门点坐标基准重放，不覆盖历史",
        "局部改动 g_cd3 多边形：仅 g_cd3 子树结果标记过期，g_live 不受影响",
        "门嵌套改名：ID/父子关系与成员稳定，仅名称版本更新",
        "草案 A/B 同版本并发保存：后提交者得到版本冲突与双方差异",
        "导出→删除本地库→重新导入：身份、顺序与来源链保持；时间旅行可回到旧事件",
    ],
    "materials": {
        "sample_fcs": "fcs/sample.fcs",
        "ctrl_fitc_fcs": "fcs/ctrl_fitc.fcs",
        "ctrl_pe_fcs": "fcs/ctrl_pe.fcs",
        "spillover_good": "spillover_good.json",
        "spillover_singular": "spillover_singular.json",
        "spillover_nearsing": "spillover_nearsing.json",
        "transform_v1": "transform_v1.json",
        "transform_v2": "transform_v2.json",
        "gates_v1": "gates_v1.json",
        "gates_v2": "gates_v2.json",
        "names_v2": "names_v2.json",
    },
    "expected": {
        "n_events": 24,
        "live_inclusive": 18, "live_exclusive": 17,
        "cd3_v1_inclusive": 9, "cd3_v2_inclusive_min": 5, "cd3_v2_inclusive_max": 8,
        "singular_matrix": "rejected_non_invertible",
        "near_singular_min_candidates": 2,
        "extreme_event_index": 10,
        "boundary_event_index": 1,
        "negative_after_comp_event_index": 23,
    },
}
dump("manifest.json", manifest)
print("cases generated at", os.path.normpath(CASE))
