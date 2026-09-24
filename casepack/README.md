# 流式边界室：补偿矩阵、门控系谱与细胞群不确定性

离线案例包，覆盖六类边界情形：矩阵奇异、极端信号、边界点、门嵌套改名、
上游过期传播、同版本并发修改。所有量携带单位或坐标基准；原始表示保留在
`events_raw.csv` 的原始 token 中，规范化只发生在派生层。

## 叙事顺序（不拆成无关按钮）
1. 导入 FCS 事件、单染对照（spillover）、门控草案 → 基线各自独立版本化。
2. 一次参数修订：变换 cofactor 150 → 500（`transforms.json` v2）。
3. 一次局部源数据改动：补偿矩阵换成近奇异版本 → 判定 blocked，仅补偿依赖失效。
4. 一次互相冲突的人工决定：草案 A/B 对边界事件 e7 裁定相反，同版本并发保存 → 冲突。
5. 上游门嵌套改名（`gates_v2_rename.json`）→ 下游群体标记 stale，不覆盖。

## 文件
- `events_raw.csv` — FCS 事件原始通道值（含极端值、边界点、不可转换 token）
- `spillover.json` — 单染对照 spillover 元数据
- `comp_matrix_ok.json` / `comp_matrix_singular.json` — 良态 / 近奇异补偿矩阵
- `transforms.json` — 变换参数（v1/v2）
- `gates_v1.json` / `gates_v2_rename.json` — 门多边形与父子关系（改名版）
- `draft_a.json` / `draft_b.json` — 两个并排复核草案
- `replay.json` — 重放脚本与机器可断言语句
