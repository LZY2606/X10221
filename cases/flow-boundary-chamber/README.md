# 流式边界室：补偿矩阵、门控系谱与细胞群不确定性

这是 Flowline 自带的**领域专用离线案例包**，不是通用表格样例。

## 材料清单（来源与解释分开、各自版本化）

| 稳定身份 | 材料 | 单位 / 坐标基准 |
| --- | --- | --- |
| `DS.fbc-events.v1` | `v1/events.fcs`（FCS 3.1，ASCII/DATA，24 事件 × 6 通道） | ADC count，$PnR=262144 |
| `SM.fbc.v1` / `SM.fbc.v2` | 单染对照与 spillover 元数据 | dimensionless_fraction_of_primary |
| `SP.fbc.v1` / `SP.fbc.v2` | spillover 矩阵（v2 为近奇异：PE/APC 两列几乎共线） | dimensionless |
| `TR.fbc.v1` / `TR.fbc.v2` | asinh 变换 cofactor | ADC count |
| `GP.fbc.v1` / `GP.fbc.v2` | 门多边形（顶点坐标与 basis 显式标注） | linear:ADC_count 或 asinh:cofactor_ADC |
| `LN.fbc.v1` | 父子关系（门控系谱，独立于门形状版本化） | — |

原始词法表示保留在 FCS DATA 段中：`SAT`（饱和）、`OVF`（溢出）、`BAD`（无法解析）
三个 token 对应事件 E018/E019/E020；这些事件留在原位置并附诊断，任何门都不允许删除它们。

## 固定边界情形（重放必须实际经过）

1. 矩阵奇异：`SP.fbc.v2` 的 rcond≈5e-7 < 1e-3，直接逆 `INV.direct` 不可采纳；
   系统保留 `INV.ridge-small`(λ=1e-3) 与 `INV.ridge-large`(λ=1e-1) 两个合法候选。
2. 极端信号：E017 FITC=262144（= $PnR，at_range）。
3. 边界点：E013/E014/E015/E016/E021 落在 G_LYM 边上，按半开规则唯一归属。
4. 门嵌套改名：G_TCELL 仅改 label，gate_id 与父子关系不动，不触发失效。
5. 上游过期传播：修订 G_LYM 后，LIVE/TCELL/CD4/CD8 标记 stale，旧结果保留不覆盖。
6. 同版本并发修改：基于同一 head_seq 的第二个决定返回 409 并记录双方差异。

## 重放叙事

见 `v2/scenario.json`。审阅入口是本地审阅页（散点/门树/补偿前后差异/分母来源），
接受、否定、参数调整都生成新的只追加事件；回到旧事件恢复当时钉住的版本与结论。
