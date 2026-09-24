# 流式边界室：补偿矩阵、门控系谱与细胞群不确定性

离线案例包，随仓库自带，不依赖外部数据。叙事主线：流式平台导入 FCS 事件、
单染对照和门控草案，审阅者逐步复核从原始信号到最终细胞群的每次变换。

## 基线（六类材料分开版本化）

- `raw-channels`：10 个 FCS 事件的原始通道值（FSC-A / SSC-A / FITC / PE，单位 RFU，
  坐标基准 acquisition）。内含三种边界材料：
  - `e06`：FSC-A 变换后恰好落在 Lymph 门左边界上（边界点）；
  - `e07`：FITC = 300000 RFU，超出检测器线性上限 262144（极端信号，原值保留不截断）；
  - `e08`：FITC 原始表示为 `SATURATED`，无法转换为数值（留在原位置并附诊断）；
  - `e09`：PE = -50，无法取对数（留在原位置并附诊断）。
- `spillover-meta`：FITC/PE 单染对照系数与坐标基准。
- `compensation-matrix`：2×2 spillover 矩阵（v1 良态）。
- `transform-params`：四通道 log10(v/150) 变换，输出基准 transformed。
- `gate-polygon`：G1=Lymph(FSC-A×SSC-A)，G2=FITC+(FITC×PE)，坐标基准 transformed。
- `gate-hierarchy`：root → G1 → G2。

## 修订序列（每次只改一个直接来源）

1. `gate-polygon` v2：上游门 Lymph 左边界右移 → 下游群体标记过期而不覆盖。
2. `gate-hierarchy` v2：门嵌套改名 FITC+ → FITC-pos，gate_id 不变，身份稳定。
3. `spillover-meta` v2：PE 单染系数 0.02 → 0.021，只波及荧光补偿依赖（局部失效）。
4. `compensation-matrix` v2：近奇异矩阵（行列式 ≈ 1e-12），派生层拒绝默默求逆，
   保留 blocked 候选与未补偿回退候选供裁定。

## 冲突决定

重放末尾在同一 base_head 上提交两次裁定：第一次被采纳，第二次收到版本冲突，
双方差异写入 conflict 事件。被采纳与未采纳的结果都保留在档案中。

## 机器可读取事件

每次导入、派生、失效标记、裁定、撤销、冲突都生成事件，载荷含输入片段、
判定状态、受影响对象与规则引用（`near-singular-compensation-matrix`、
`denominator-pinned-to-parent-version`、`stale-not-overwrite`）。
