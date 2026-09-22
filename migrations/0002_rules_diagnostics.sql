-- v2: machine-readable adjudication rule catalog and per-material diagnostics.
-- Existing records from v1 are preserved by the migration test.
CREATE TABLE rules (
  code        TEXT PRIMARY KEY,
  title       TEXT NOT NULL,
  rationale   TEXT NOT NULL,
  basis_ref   TEXT NOT NULL
);

INSERT INTO rules (code, title, rationale, basis_ref) VALUES
 ('COMP-NEARSING',
  '近奇异补偿矩阵不能被默默求逆',
  'spillover 矩阵条件数过高时，数值逆会把噪声放大为虚假细胞事件；必须保留为多候选补偿并显式裁定。',
  'cases/flow-boundary-chamber/materials/spillover-metadata.json#near-singular-policy'),
 ('GATE-DENOM-PIN',
  '门的百分比分母必须钉住父群版本',
  '每个群体百分比引用一个不可变的父群生成（parent generation id），上游修订后历史百分比仍指向当时的父群。',
  'cases/flow-boundary-chamber/materials/hierarchy.json#denominator-pinning'),
 ('STALE-NO-OVERWRITE',
  '修订上游门后下游结果标记过期而不覆盖',
  '上游门修订只使受影响的下游生成失效；旧生成与旧结论继续可读，新生成经传播后才成为当前结论。',
  'cases/flow-boundary-chamber/README.md#staleness-propagation');

CREATE TABLE material_diagnostics (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  material_id    TEXT NOT NULL,
  version        INTEGER NOT NULL,
  code           TEXT NOT NULL,
  severity       TEXT NOT NULL,
  message        TEXT NOT NULL,
  event_id       INTEGER,
  created_at     TEXT NOT NULL
);

ALTER TABLE branches ADD COLUMN note TEXT NOT NULL DEFAULT '';
