#!/usr/bin/env python3
"""生成“流式边界室”离线案例包：真实 FCS 3.1 二进制 + 分版本领域 JSON。

固定案例覆盖：近奇异补偿矩阵、极端信号（超量程原始值保留）、边界点（精确落在门边上）、
门嵌套改名、上游过期传播、同版本并发修改所需的两份冲突决定。
运行：python3 scripts/gen_casepack.py
"""
import json, os, struct, hashlib

ROOT = os.path.join(os.path.dirname(__file__), "..", "cases", "flow-boundary-chamber")

def write_fcs(path, params, events, extra=None):
    """params: [(name, stain, range, gain_or_None, bits)]; events: list[tuple[float]]"""
    delim = b"|"
    kv = []
    def add(k, v):
        kv.append((k.encode(), str(v).encode()))
    add("$BEGINANALYSIS", 0); add("$ENDANALYSIS", 0)
    add("$DATATYPE", "F"); add("$MODE", "L"); add("$BYTEORD", "1,2,3,4")
    add("$PAR", len(params)); add("$TOT", len(events))
    add("$CYT", "FlowBoundaryChamberSim"); add("$EXP", "pairwise-gsb")
    for i, (name, stain, rng, gain, bits) in enumerate(params, 1):
        add("$P%dN" % i, name)
        if stain: add("$P%dS" % i, stain)
        add("$P%dR" % i, int(rng)); add("$P%dB" % i, bits); add("$P%dE" % i, "0,0")
        if gain is not None: add("$P%dG" % i, repr(gain))
    data = b"".join(struct.pack("<f", v) for row in events for v in row)
    def serialize():
        return delim + delim.join(delim.decode().join((k.decode(), v.decode())).encode()
                                   for k, v in kv)
    tb = 258
    text = serialize()
    # 定点插入 $BEGINDATA/$ENDDATA（先插在 $DATATYPE 之前）
    for _ in range(5):
        db = tb + len(text)
        de = db + len(data) - 1
        marker = delim + b"$DATATYPE|"
        injected = delim + b"$BEGINDATA|" + str(db).encode() + delim + b"$ENDDATA|" + str(de).encode()
        new_text = serialize().replace(marker, injected + marker, 1)
        if new_text == text:
            break
        text = new_text
    text = text + b" "
    te = tb + len(text) - 1
    db = te + 1
    de = db + len(data) - 1
    # 长度若因尾部空格变化则再注入一次最终值
    marker = delim + b"$DATATYPE|"
    injected = delim + b"$BEGINDATA|" + str(db).encode() + delim + b"$ENDDATA|" + str(de).encode()
    text = serialize().replace(marker, injected + marker, 1) + b" "
    te = tb + len(text) - 1
    db = te + 1
    de = db + len(data) - 1
    injected = delim + b"$BEGINDATA|" + str(db).encode() + delim + b"$ENDDATA|" + str(de).encode()
    text = serialize().replace(marker, injected + marker, 1) + b" "
    te = tb + len(text) - 1
    db = te + 1
    de = db + len(data) - 1
    injected = delim + b"$BEGINDATA|" + str(db).encode() + delim + b"$ENDDATA|" + str(de).encode()
    text = serialize().replace(marker, injected + marker, 1) + b" "
    out = bytearray(b" " * tb)
    out[0:6] = b"FCS3.1"
    out[10:18] = str(tb).rjust(8).encode()
    out[18:26] = str(te).rjust(8).encode()
    out[26:34] = str(db).rjust(8).encode()
    out[34:42] = str(de).rjust(8).encode()
    out += text + data
    with open(path, "wb") as f:
        f.write(out)
    return hashlib.sha256(bytes(out)).hexdigest()

P_MAIN = [
    ("FSC-A", "forward scatter", 262144, 1.0, 32),
    ("SSC-A", "side scatter", 262144, 1.0, 32),
    ("FL1-A", "FITC", 262144, 1.0, 32),
    ("FL2-A", "PE", 262144, None, 32),  # 故意缺失 $P4G：原始元数据保留“未知增益”
]
# 行5 精确落在 P1 门 (100,200)-(900,800) 的下边界 y=200 上；行7 FL1 极端超量程
MAIN = [
    (500.0, 400.0, 1200.0, 900.0),
    (480.0, 420.0, 2600.0, 400.0),
    (520.0, 380.0, 5200.0, 600.0),
    (300.0, 300.0, 1800.0, 500.0),
    (850.0, 700.0, 700.0, 1500.0),
    (400.0, 200.0, 3000.0, 350.0),   # 边界点（P1 下边缘，恰好在边上）
    (460.0, 410.0, 9000.0, 420.0),
    (600.0, 500.0, 400000.0, 300.0),# 极端信号：400000 > $P3R=262144，原始值保留
]
P_CTRL = P_MAIN
CTRL_FL1 = [(500.0, 400.0, 8000.0, 960.0) for _ in range(20)]   # FL1 单染，FL2 拖尾约 0.12
CTRL_FL2 = [(500.0, 400.0, 160.0, 8000.0) for _ in range(20)]   # FL2 单染，FL1 拖尾约 0.02

os.makedirs(os.path.join(ROOT, "fcs"), exist_ok=True)
h1 = write_fcs(os.path.join(ROOT, "fcs", "sample-main.fcs"), P_MAIN, MAIN)
h2 = write_fcs(os.path.join(ROOT, "fcs", "control-fl1.fcs"), P_CTRL, CTRL_FL1)
h3 = write_fcs(os.path.join(ROOT, "fcs", "control-fl2.fcs"), P_CTRL, CTRL_FL2)
print("fcs sha:", h1[:12], h2[:12], h3[:12])

def dump(rel, obj):
    path = os.path.join(ROOT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=2)
        f.write("\n")

# ---- spillover 元数据（从单染对照估计；与补偿矩阵分开版本化） ----
dump("spillover/spillover-v1.json", {
  "spillover_id": "spillover:main",
  "version": 1,
  "channels": ["FL1-A", "FL2-A"],
  "coefficients": {"FL1-A": {"FL2-A": 0.12}, "FL2-A": {"FL1-A": 0.02}},
  "unit": "ratio",
  "basis": "单染对照阳性峰中位荧光比值：control-fl1.fcs、control-fl2.fcs（每孔 20 事件，单位 counts）",
  "controls": [
    {"file": "fcs/control-fl1.fcs", "stain": "FITC", "positive_median_FL1": 8000, "positive_median_FL2": 960},
    {"file": "fcs/control-fl2.fcs", "stain": "PE", "positive_median_FL1": 160, "positive_median_FL2": 8000}
  ],
  "notes": ["FL2-A 通道缺少 $P4G，增益未知，按原始 counts 比较，未做增益归一化"]
})

# ---- 补偿矩阵 v1（良态） ----
dump("compensation/matrix-v1.json", {
  "matrix_id": "matrix:comp-main",
  "version": 1,
  "channels": ["FL1-A", "FL2-A"],
  "rows": [[1.0, 0.12], [0.02, 1.0]],
  "unit": "ratio",
  "basis": "spillover:main@v1：行=接收通道，列=发射通道；y=M^{-1}x",
  "source": "spillover:main@v1"
})

# ---- 近奇异矩阵（分支2：不得默默求逆） ----
t = 0.9999999999999
dump("compensation/matrix-nearsingular.json", {
  "matrix_id": "matrix:comp-main",
  "version": 2,
  "channels": ["FL1-A", "FL2-A"],
  "rows": [[1.0, t], [t, 1.0]],
  "unit": "ratio",
  "basis": "模拟两通道补偿系数被错误地设为几乎完全相关（仪器串扰标定失败）",
  "source": "manual edit during review",
  "expect": {
    "rule": "comp.near_singular.no_silent_inverse",
    "status": "near_singular",
    "cond_threshold": 1e12,
    "candidate_ids": ["damped-pseudoinverse", "use-previous-version", "reject-compensation"]
  }
})

# ---- 变换参数 v1（asinh，仅荧光通道） ----
dump("transforms/asinh-v1.json", {
  "transform_id": "transform:asinh-main",
  "version": 1,
  "method": "asinh",
  "channels": ["FL1-A", "FL2-A"],
  "cofactor": 150.0,
  "unit": "fluorescence",
  "basis": "y = asinh(x / 150)，x 单位 counts（补偿后）；FSC-A/SSC-A 不变换"
})

# ---- 参数修订：cofactor 150 -> 200（影响全部依赖变换的下游） ----
dump("revisions/asinh-v2.json", {
  "transform_id": "transform:asinh-main",
  "version": 2,
  "method": "asinh",
  "channels": ["FL1-A", "FL2-A"],
  "cofactor": 200.0,
  "unit": "fluorescence",
  "basis": "y = asinh(x / 200)，审阅者将 cofactor 从 150 改为 200（参数修订）",
  "revision_kind": "parameter"
})

# ---- 门多边形 v1（坐标基准：FSC/SSC 为 counts，FL 为 asinh(x/cofactor)） ----
GATE_BASIS = "scatter=counts(raw passthrough); fluorescence=asinh(compensated counts/150)"
dump("gates/gate-p1-v1.json", {
  "gate_id": "gate:p1-lymph", "version": 1, "name": "P1 Lymphocytes",
  "x_channel": "FSC-A", "y_channel": "SSC-A",
  "vertices": [[100.0,200.0],[900.0,200.0],[900.0,800.0],[100.0,800.0]],
  "basis": GATE_BASIS,
  "notes": ["sample-main.fcs 行5 (400,200) 精确落在下边缘，是 boundary 测试点"]
})
dump("gates/gate-p2-v1.json", {
  "gate_id": "gate:p2-live", "version": 1, "name": "P2 Live",
  "x_channel": "FL2-A", "y_channel": "FL1-A",
  "vertices": [[0.0,2.5],[3.0,2.5],[3.0,5.0],[0.0,5.0]],
  "basis": GATE_BASIS
})
dump("gates/gate-p3-v1.json", {
  "gate_id": "gate:p3-fl1pos", "version": 1, "name": "P3 FL1+",
  "x_channel": "FL2-A", "y_channel": "FL1-A",
  "vertices": [[0.0,3.6],[2.0,3.6],[2.0,5.0],[0.0,5.0]],
  "basis": GATE_BASIS
})

# ---- 父子关系（树）v1：与门多边形分开版本化 ----
dump("gates/tree-v1.json", {
  "tree_id": "tree:main", "version": 1, "file_id": "file:fcs:sample-main",
  "root": "gate:root",
  "nodes": [
    {"gate_id": "gate:root", "gate_version": 1, "name": "All Events"},
    {"gate_id": "gate:p1-lymph", "gate_version": 1, "name": "P1 Lymphocytes"},
    {"gate_id": "gate:p2-live", "gate_version": 1, "name": "P2 Live"},
    {"gate_id": "gate:p3-fl1pos", "gate_version": 1, "name": "P3 FL1+"}
  ],
  "edges": [
    ["gate:root", "gate:p1-lymph"],
    ["gate:p1-lymph", "gate:p2-live"],
    ["gate:p2-live", "gate:p3-fl1pos"]
  ]
})

# ---- 门嵌套改名：只改树名称，门多边形版本不动，不应触发任何失效 ----
dump("revisions/tree-v2-rename.json", {
  "tree_id": "tree:main", "version": 2, "file_id": "file:fcs:sample-main",
  "root": "gate:root",
  "nodes": [
    {"gate_id": "gate:root", "gate_version": 1, "name": "All Events"},
    {"gate_id": "gate:p1-lymph", "gate_version": 1, "name": "P1 Lymphocytes"},
    {"gate_id": "gate:p2-live", "gate_version": 1, "name": "P2 Live Cells"},
    {"gate_id": "gate:p3-fl1pos", "gate_version": 1, "name": "P3 FL1 Bright"}
  ],
  "edges": [
    ["gate:root", "gate:p1-lymph"],
    ["gate:p1-lymph", "gate:p2-live"],
    ["gate:p2-live", "gate:p3-fl1pos"]
  ],
  "revision_kind": "rename-only",
  "expect": {"invalidates": [], "rule": "tree.rename_does_not_invalidate_derived"}
})

# ---- 局部源数据改动：只修订叶门 P3 多边形（仅 P3 自己的成员/统计失效） ----
dump("revisions/gate-p3-v2.json", {
  "gate_id": "gate:p3-fl1pos", "version": 2, "name": "P3 FL1+",
  "x_channel": "FL2-A", "y_channel": "FL1-A",
  "vertices": [[0.0,3.9],[2.5,3.9],[2.5,5.0],[0.0,5.0]],
  "basis": GATE_BASIS,
  "revision_kind": "local-source-change",
  "expect": {
    "invalidated_popstats_prefix": ["popstat:gate:p3-fl1pos"],
    "untouched_prefix": ["popstat:gate:p1-lymph", "popstat:gate:p2-live"],
    "rule": "invalidation.scoped_by_dependency_graph"
  }
})

# ---- 上游门修订：P1 收缩 → P1/P2/P3 整支过期（树的根方向传播） ----
dump("revisions/gate-p1-v2.json", {
  "gate_id": "gate:p1-lymph", "version": 2, "name": "P1 Lymphocytes",
  "x_channel": "FSC-A", "y_channel": "SSC-A",
  "vertices": [[350.0,350.0],[700.0,350.0],[700.0,600.0],[350.0,600.0]],
  "basis": GATE_BASIS,
  "revision_kind": "upstream-gate-change",
  "expect": {
    "invalidated_popstats_prefix": ["popstat:gate:p1-lymph", "popstat:gate:p2-live", "popstat:gate:p3-fl1pos"],
    "rule": "upstream.gate.revision.marks.downstream.stale.without.overwrite"
  }
})

# ---- 同版本并发修改：两份基于同一 head 的不同决定（后提交者必须看到冲突与双方差异） ----
dump("revisions/conflict-a.json", {
  "branch_id": "branch:main",
  "subject_id": "derived:gate:gate:p1-lymph@v1",
  "decision": "include-boundary",
  "label": "审阅者甲：边界点纳入 P1",
  "revision_kind": "concurrent-edit"
})
dump("revisions/conflict-b.json", {
  "branch_id": "branch:main",
  "subject_id": "derived:gate:gate:p1-lymph@v1",
  "decision": "exclude-boundary",
  "label": "审阅者乙：边界点排除出 P1",
  "revision_kind": "concurrent-edit"
})

manifest = {
  "case_id": "flow-boundary-chamber",
  "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
  "version": 1,
  "domain": "flow-cytometry",
  "narrative": [
    "流式平台导入 FCS 事件、单染对照与门控草案；逐项核对原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形与父子关系（分开版本化）。",
    "触发边界组合：近奇异矩阵、极端信号、边界点、门嵌套改名、上游过期传播、同版本并发修改，留下输入片段、判定状态与受影响对象。",
    "第二分支检验近奇异矩阵不能被默默求逆、百分比分母钉住父群版本、上游修订后下游标记过期而不覆盖，并保留全部合法候选。",
    "审阅页显示通道散点、门树、补偿前后差异与每个群体的分母来源，两个草案可并排；任何裁定生成新事件，回到旧事件恢复当时结论。"
  ],
  "files": {
    "main_events": "fcs/sample-main.fcs",
    "single_stain_controls": ["fcs/control-fl1.fcs", "fcs/control-fl2.fcs"],
    "spillover": "spillover/spillover-v1.json",
    "matrix_v1": "compensation/matrix-v1.json",
    "matrix_near_singular": "compensation/matrix-nearsingular.json",
    "transform_v1": "transforms/asinh-v1.json",
    "gates": ["gates/gate-p1-v1.json", "gates/gate-p2-v1.json", "gates/gate-p3-v1.json"],
    "tree_v1": "gates/tree-v1.json",
    "revisions": [
      "revisions/asinh-v2.json",
      "revisions/tree-v2-rename.json",
      "revisions/gate-p3-v2.json",
      "revisions/gate-p1-v2.json",
      "revisions/conflict-a.json",
      "revisions/conflict-b.json"
    ]
  },
  "baseline_versions": {
    "spillover:main": 1, "matrix:comp-main": 1, "transform:asinh-main": 1,
    "gate:p1-lymph": 1, "gate:p2-live": 1, "gate:p3-fl1pos": 1, "tree:main": 1
  },
  "judgments": [
    {"rule": "comp.near_singular.no_silent_inverse",
     "when": "matrix:comp-main@v2",
     "expect_status": "near_singular",
     "expect_candidates": ["damped-pseudoinverse", "use-previous-version", "reject-compensation"],
     "assertion": "逆矩阵字段不得出现在 ready 结果中；必须由人工选择候选"},
    {"rule": "gate.boundary.ambiguous",
     "when": "gate:p1-lymph@v1 / sample-main 行5",
     "expect_candidates": ["include-boundary", "exclude-boundary"],
     "assertion": "精确落在门边上的点保留 include/exclude 两个合法候选"},
    {"rule": "popstat.denominator.pinned_to_parent_version",
     "when": "每次群体统计",
     "assertion": "denom_source 显式给出父群 stat id+version；父群修订后旧统计分母不变"},
    {"rule": "upstream.gate.revision.marks.downstream.stale.without.overwrite",
     "when": "gate:p1-lymph@v2",
     "assertion": "P1/P2/P3 旧版本统计标记 stale，行不删除、数字不覆盖"},
    {"rule": "tree.rename_does_not_invalidate_derived",
     "when": "tree:main@v2",
     "assertion": "仅名称变化时 affected 列表为空"},
    {"rule": "concurrency.same_branch_conflict",
     "when": "同一 head 连续提交 conflict-a/conflict-b",
     "assertion": "后提交得到 409 与双方差异事件 conflict.detected"},
    {"rule": "raw.extreme_value.preserved",
     "when": "sample-main 行7 FL1-A=400000 > $P3R=262144",
     "assertion": "原始字节/数值保留在 events_raw，诊断 event.exceeds_range 挂在该事件上"}
  ],
  "steps": [
    {"id": "import", "label": "初始导入（基线，全部 v1）"},
    {"id": "matrix_singular", "label": "登记近奇异矩阵并尝试补偿", "material": "compensation/matrix-nearsingular.json"},
    {"id": "branch_strict", "label": "新分支拒绝补偿；主分支回退上一版矩阵"},
    {"id": "boundary", "label": "复核 P1 边界点，两个审阅者各持 include/exclude"},
    {"id": "rename", "label": "门嵌套改名（P2/P3），确认无失效"},
    {"id": "local_change", "label": "修订叶门 P3 多边形，确认仅局部失效"},
    {"id": "upstream_stale", "label": "修订上游门 P1，确认整支 stale 而不覆盖"},
    {"id": "parameter_revision", "label": "修订 cofactor 150→200，确认下游按依赖失效"},
    {"id": "conflict", "label": "同版本并发修改，后提交者看到冲突与双方差异"}
  ]
}
dump("manifest.json", manifest)
print("casepack written at", os.path.abspath(ROOT))
