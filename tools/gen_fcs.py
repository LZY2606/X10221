#!/usr/bin/env python3
"""生成 cases/flow-boundary-chamber/events.fcs —— 一个最小但合法的 FCS 3.0 文件。

数据刻意包含：
- 正常淋巴细胞群（FSC-A/SSC-A 散点）
- 两个恰好落在 P1 门边界上的事件（边界点情形）
- 一个超出 $PnR 检测范围的极端信号（FL1-A = 300000）
- 一个负值极端信号（FL2-A = -100）
- FL3-A 通道（单位未知，导入后不可转换，保留原值并附诊断）
"""
import struct, os

PARAMS = [
    ("FSC-A", "262144", "0,0", "scatter"),
    ("SSC-A", "262144", "0,0", "scatter"),
    ("FL1-A", "262144", "0,0", "FITC fluorescence"),
    ("FL2-A", "262144", "0,0", "PE fluorescence"),
    ("FL3-A", "262144", "0,0", "unknown"),
]

def cluster(cx, cy, n, sx, sy, fl1, fl2, fl3, seed):
    out = []
    x = seed
    def rnd():
        nonlocal x
        x = (1103515245 * x + 12345) % (2**31)
        return (x / 2**31) - 0.5
    for _ in range(n):
        out.append([cx + rnd()*2*sx, cy + rnd()*2*sy,
                    fl1 + rnd()*2*fl1*0.15, fl2 + rnd()*2*fl2*0.15,
                    fl3 + rnd()*2*fl3*0.2])
    return out

events = []
# 淋巴细胞主群（P1 内）
events += cluster(50000, 25000, 40, 9000, 6000, 8000, 3000, 500, 7)
# 碎片/大细胞群（P1 外）
events += cluster(120000, 90000, 8, 15000, 10000, 2000, 1500, 300, 13)
events += cluster(5000, 3000, 6, 2000, 1500, 1000, 800, 200, 29)
# 边界点：恰好落在 P1 多边形 (20000,5000)-(90000,5000)-(90000,60000)-(20000,60000) 的边上
events.append([20000.0, 30000.0, 5000.0, 2500.0, 400.0])   # 左边界 x=20000
events.append([55000.0, 60000.0, 6000.0, 2800.0, 450.0])   # 上边界 y=60000
# 极端信号：超出检测范围 / 负值（保留原始表示，派生层附诊断）
events.append([50000.0, 25000.0, 300000.0, 3000.0, 500.0]) # FL1-A 超量程
events.append([50000.0, 25000.0, 7000.0, -100.0, 500.0])   # FL2-A 负值

n = len(events)
p = len(PARAMS)

text_pairs = {
    "$BEGINANALYSIS": "0", "$ENDANALYSIS": "0",
    "$BYTEORD": "1,2,3,4", "$DATATYPE": "F", "$MODE": "L",
    "$NEXTDATA": "0", "$PAR": str(p), "$TOT": str(n),
    "CYT": "FlowChamber Boundary Bench", "$CYT": "FlowChamber Boundary Bench",
    "$TIMESTEP": "0.01", "CASE": "flow-boundary-chamber",
}
for i, (name, rng, amp, _unit) in enumerate(PARAMS, 1):
    text_pairs[f"$P{i}N"] = name
    text_pairs[f"$P{i}R"] = rng
    text_pairs[f"$P{i}E"] = amp
    text_pairs[f"$P{i}B"] = "32"

DELIM = "|"
body = "".join(f"{DELIM}{k}{DELIM}{v}" for k, v in text_pairs.items())
body = DELIM + body[len(DELIM):] + DELIM

HEADER_LEN = 58
text_start = HEADER_LEN
text_end = text_start + len(body) - 1
data_start = text_end + 1
data_end = data_start + n * p * 4 - 1

header = b"FCS3.0    "
header += f"{text_start:8d}{text_end:8d}{data_start:8d}{data_end:8d}{0:8d}{0:8d}".encode()
assert len(header) == HEADER_LEN, len(header)

data = b"".join(struct.pack("<f", v) for ev in events for v in ev)

out = os.path.join(os.path.dirname(__file__), "..", "cases", "flow-boundary-chamber", "events.fcs")
with open(out, "wb") as f:
    f.write(header + body.encode() + data)
print(f"wrote {out}: {n} events x {p} params, {len(header)+len(body)+len(data)} bytes")
