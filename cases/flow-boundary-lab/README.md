# 流式边界室：补偿矩阵、门控系谱与细胞群不确定性

本案例包是“流式平台导入 FCS 事件、单染对照和门控草案，逐步复核从原始信号到最终细胞群的每次变换”的完整离线叙事。

## 初始版本（基线）
六类材料分开版本化，导入时保留原始表示，派生层才规范化：
- `raw_channels.csv` — 原始通道值（仪器单位，坐标基准见 `# basis` 头），含极端信号 E009（饱和 262144）与不可转换值 E010（N/A，留在原位置并附诊断）
- `spillover.json` — 单染对照得到的 spillover 元数据
- `comp_matrix.json` — 补偿矩阵 v1（良态）
- `transform_params.json` — 变换参数（log10，坐标基准 decades=4.5）
- `gates.json` — 门多边形（lymph ⊃ live ⊃ cd3pos），E005 精确落在 live 门下边上
- `hierarchy.json` — 父子关系

## 剧本（scenarios.json 驱动重放）
1. 参数修订：transform_params 升版（cofactor 150→180）
2. 局部源改动：comp_matrix v2 为近奇异矩阵 —— 不能被默默求逆，生成候选等待裁定
3. 冲突人工决定：边界点 E005 在 main 判 in、在 review-b 判 out，两个分支都保留
4. 门嵌套改名：cd3pos → cd3pos_t（hierarchy v2，保留改名映射）
5. 上游过期传播：live 门修订后，下游 cd3pos 群体标记过期而不覆盖
6. 同版本并发修改：draft-review 两个窗口基于同一版本保存，后提交者看到冲突与双方差异

复核路径：审阅页显示通道散点、门树、补偿前后差异与每个群体的分母来源，可将两个草案并排（/compare）。每个显示值都带 data-stable-id。导出、删库、再导入后身份、顺序与来源关系保持。
