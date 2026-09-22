# 流式边界室：补偿矩阵、门控系谱与细胞群不确定性

离线、确定性的流式细胞术复核案例。所有材料以“原始表示”入库，派生层才做规范化；
原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形与父子关系分别是独立版本化材料。

## 坐标基准与单位（所有领域量都带基准）

- `FSC-A` / `SSC-A`：线性 ADC 计数（a.u.），范围 0..262144，门控基准为线性原始值。
- `FITC-A`（CD4）/ `PE-A`（CD8）：原始荧光计数（a.u.）。
- spillover/补偿矩阵：无量纲分数，表示“源通道信号泄漏到目标通道的比例”。
- 变换：`asinh(x/cofactor)`，cofactor 单位与原始荧光计数相同；变换后坐标无量纲，门坐标基于补偿后变换空间。
- 门多边形使用半开规则（left/bottom 边含于多边形，top/right 边不含），恰好落在边上的事件成为
  边界候选（boundary candidates），保留全部合法裁定，不静默丢弃。

## 固定事件（17 个，含边界与极端信号）

- EV010：FITC/PE 检测器溢出（overflow），原始值留在原位并附诊断；变换值置 null 但不删除事件。
- EV017：可转换的极端亮信号（FITC 单阳）。
- EV004：恰好落在 Lymphocytes 门斜边上；EV010：FSC 恰好落在右边界；EV012：补偿后 PE 恰为 0，落在 CD4 门下边界。

## 失效传播（staleness-propagation）

- 近奇异补偿矩阵不能被默默求逆（规则 `COMP-NEARSING`）：保留 identity 与两级 Tikhonov 候选。
- 门的百分比分母必须钉住父群版本（规则 `GATE-DENOM-PIN`）：每个百分比记录 parent generation id。
- 修订上游门后下游结果标记过期而不覆盖（规则 `STALE-NO-OVERWRITE`）：旧 generation 继续可读。

## 情形索引

1. 参数修订：`revisions/transform.v2.json`（FITC cofactor 1500→1200，EV017 掉出 CD4）。
2. 只影响局部依赖的源数据改动：`revisions/fcs-events.v2.json`（仅 EV006 FITC 改变）。
3. 门嵌套改名：`revisions/hierarchy.v2-rename.json`（仅显示名，节点/门稳定 id 不变）。
4. 上游过期传播：`revisions/gates/lymphocytes.v2.json`（斜边移动，EV004 被纳入）。
5. 近奇异/奇异补偿：`revisions/compensation.v2-near-singular.json`、`compensation.v3-singular.json`。
6. 同版本并发修改：两个窗口以同一 parent 版本保存，后提交者得到 409 与双方差异。
