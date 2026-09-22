// 流式边界室复核界面：两个草案并排；所有操作仅追加事件，不改写原始数据。
const state = {
  branches: [],
  a: { branch: "main", seq: null, view: null, head: 0, target: "A" },
  b: { branch: "main", seq: null, view: null, head: 0, target: "B" },
};

async function api(path, opts) {
  const res = await fetch(path, opts);
  const text = await res.text();
  let body;
  try { body = JSON.parse(text); } catch { body = { raw: text }; }
  if (!res.ok) throw Object.assign(new Error(body.error || res.statusText), { status: res.status, body });
  return body;
}
const formBody = (obj) =>
  new URLSearchParams(Object.entries(obj).filter(([, v]) => v !== undefined && v !== null));

function toast(msg, isErr) {
  const t = document.getElementById("toast");
  t.textContent = msg;
  t.style.display = "block";
  t.className = "toast" + (isErr ? " err" : "");
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (t.style.display = "none"), 9000);
}

async function refresh() {
  const bs = await api("/api/branches");
  state.branches = bs.branches;
  for (const side of ["a", "b"]) {
    const s = state[side];
    if (!state.branches.find((b) => b.branch === s.branch)) {
      s.branch = "main";
      s.seq = null;
    }
    const ev = await api(`/api/events?branch=${encodeURIComponent(s.branch)}`);
    s.head = ev.head_seq;
    if (s.seq === null || s.seq > s.head) s.seq = s.head;
    s.view = await api(`/api/view?branch=${encodeURIComponent(s.branch)}&seq=${s.seq}`);
  }
  renderPanes();
  renderDiff();
}

function setBranch(side, branch) {
  state[side].branch = branch;
  state[side].seq = null;
  refresh().catch((e) => toast(e.message, true));
}
function setSeq(side, seq) {
  state[side].seq = Number(seq);
  refresh().catch((e) => toast(e.message, true));
}

function esc(v) {
  return String(v ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
}
function num(v, d = 2) {
  return typeof v === "number" && isFinite(v) ? v.toFixed(d) : (v ?? "—");
}

function renderPanes() {
  document.getElementById("caseId").textContent =
    (state.a.view ? `案例 ${state.a.view.branch === "main" ? "" : ""}` : "");
  for (const side of ["A", "B"]) {
    const s = state[side.toLowerCase()];
    document.getElementById("pane" + side).innerHTML = paneHtml(side, s);
  }
  for (const side of ["A", "B"]) drawScatter(side);
}

function paneHtml(side, s) {
  const v = s.view;
  if (!v) return "<div class='pane-body'>载入中…</div>";
  const branchOpts = state.branches
    .map((b) => `<option ${b.branch === s.branch ? "selected" : ""}>${esc(b.branch)}</option>`)
    .join("");
  return `
  <div class="pane-head">
    <b>草案 ${side}</b>
    <select onchange="setBranch('${side.toLowerCase()}', this.value)">${branchOpts}</select>
    <span class="muted">回看 seq</span>
    <input type="number" min="0" max="${s.head}" value="${s.seq}" style="width:70px"
      onchange="setSeq('${side.toLowerCase()}', this.value)" />
    <span class="muted">head=${s.head}</span>
  </div>
  <div class="pane-body">
    ${headsHtml(v)}
    ${compHtml(v)}
    ${candidatesHtml(side, v)}
    <div class="card"><h3>门散点（当前门：<span id="gatePick${side}"></span>）</h3>
      <select id="gateSelect${side}" onchange="drawScatter('${side}')"></select>
      <canvas id="scatter${side}" width="440" height="260"></canvas>
      <div class="muted" id="scatterNote${side}"></div>
    </div>
    ${treeHtml(v)}
    ${popsHtml(v)}
    ${judgmentsHtml(v)}
    ${eventsHtml(side, s)}
  </div>`;
}

function headsHtml(v) {
  const h = v.heads;
  const rows = [
    ["数据集(原始通道值)", h.dataset], ["spillover 元数据", h.spillover_meta],
    ["补偿矩阵", h.compensation], ["变换参数", h.transform], ["父子关系", h.relations],
  ].map(([k, x]) => `<div class="k">${k}</div><div><code>${esc(x)}</code></div>`).join("");
  const gates = Object.entries(h.gates || {})
    .map(([g, ver]) => `<span class="tag">${esc(g)}@${esc(ver)}</span>`).join(" ");
  return `<div class="card"><h3>分开版本化的材料（seq=${v.requested_seq}）</h3>
    <div class="kv">${rows}<div class="k">门头部</div><div>${gates}</div></div></div>`;
}

function compHtml(v) {
  const rows = v.compensation_compare || [];
  if (!rows.length) return "";
  if (rows[0] && rows[0].invertible === false) {
    return `<div class="card"><h3>补偿前后差异</h3>
      <span class="tag blocked">矩阵不可求逆</span>
      <div class="muted">${esc(rows[0].reason)}</div></div>`;
  }
  return `<div class="card"><h3>补偿前后差异（头部矩阵 <code>${esc(rows[0] && rows[0].compensation_version)}</code>）</h3>
  <table><thead><tr><th>事件</th><th>通道</th><th>原始 ADC-count</th><th>补偿后</th><th>Δ</th></tr></thead><tbody>
  ${rows.flatMap((r) => Object.entries(r.channels || {}).map(([ch, c]) =>
    `<tr><td>${esc(r.event_id)}</td><td>${esc(ch)}</td>
      <td>${num(c.raw, 1)}</td><td>${num(c.compensated, 1)}</td><td>${num(c.delta, 1)}</td></tr>`)).join("")}
  </tbody></table></div>`;
}

function candidatesHtml(side, v) {
  const sets = v.candidate_sets || [];
  if (!sets.length) return "";
  return `<div class="card"><h3>多候选与裁定（无法唯一决定时全部保留）</h3>` +
    sets.map((s) => {
      const decided = s.accepted !== null && s.accepted !== undefined;
      const acc = decided ? (Array.isArray(s.accepted) ? s.accepted[0] : s.accepted) : null;
      return `<div class="cand">
        <div><b>${esc(s.kind)}</b> <code>${esc(s.candidate_set_id)}</code>
          <span class="tag ${decided ? "ok" : "ambiguous"}">${decided ? "已裁定 " + esc(acc ?? "整体否定") : "待裁定"}</span></div>
        <div class="muted">对象: <code>${esc(s.subject)}</code></div>
        <ul>${s.candidates.map((c) => `<li><code>${esc(c.candidate_id)}</code> — ${esc(c.label)}
            <div class="muted">${esc(c.detail)}</div></li>`).join("")}</ul>
        ${s.rationale ? `<div class="muted">依据: ${esc(s.rationale)}（事件 <code>${esc(s.decided_by_event)}</code>）</div>` : ""}
        ${!decided ? candidateButtons(side, s) : ""}
      </div>`;
    }).join("") + `</div>`;
}

function candidateButtons(side, s) {
  if (s.kind === "boundary-membership") {
    const gate = s.subject.split("|")[0];
    return `<div>${s.candidates.map((c, i) =>
      `<button onclick="decideBoundary('${side}', '${gate}', '${i === 0 ? "include" : "exclude"}')">${i === 0 ? "接受计入" : "接受排除"}</button>`).join(" ")}
      <button class="danger" onclick="decideBoundary('${side}', '${gate}', 'reject-all')">整体否定</button></div>`;
  }
  return ""; // 近奇异补偿由顶部场景按钮裁定（reject / regularize 互斥）
}

function treeHtml(v) {
  const nodes = v.gate_tree || [];
  const byParent = {};
  for (const n of nodes) (byParent[n.parent_gate_id || ""] ||= []).push(n);
  const renderNode = (n, depth) => {
    const pop = n.live_pop || n.latest_pop;
    const status = popStatusTag(pop);
    return `<div style="margin-left:${depth * 14}px">
      <span>▸ <b>${esc(n.name)}</b> <code>${esc(n.gate_id)}</code>
        <span class="muted">@${esc(n.gate_version_id)}</span> ${status}</span>
      <div class="muted" style="margin-left:14px">空间: ${esc(n.space.basis)}
        （x=${esc(n.space.x_channel)} ${esc(n.space.x_unit)}, y=${esc(n.space.y_channel)} ${esc(n.space.y_unit)}）
        · 边界策略=${esc(n.boundary_policy)}</div>
      ${(byParent[n.gate_id] || []).map((c) => renderNode(c, depth + 1)).join("")}
    </div>`;
  };
  const roots = byParent[""] || [];
  return `<div class="card"><h3>门树（父子谱系）</h3>${roots.map((r) => renderNode(r, 0)).join("")}</div>`;
}

function popStatusTag(pop) {
  if (!pop) return `<span class="tag blocked">无派生</span>`;
  const st = pop.status.status;
  const cls = pop.stale ? "stale" : st;
  const label = pop.stale ? "过期保留" : st === "ok" ? "有效" : st === "blocked" ? "阻塞" : "歧义";
  return `<span class="tag ${cls}">${label}</span>`;
}

function popsHtml(v) {
  const pops = v.pops || [];
  return `<div class="card"><h3>群体派生：分母钉版与过期传播</h3>
  <table><thead><tr><th>群体稳定身份</th><th>门版本</th><th>分子</th><th>分母(父群版本)</th><th>%</th><th>状态</th><th>失效原因</th><th>seq</th></tr></thead><tbody>
  ${pops.map((p) => `<tr>
    <td><code>${esc(p.pop_id.slice(0, 18))}…</code><div class="muted">${esc(p.gate_name)}</div></td>
    <td><code>${esc(p.gate_version_id)}</code>${p.parent_gate_version_id ? `<div class="muted">父@${esc(p.parent_gate_version_id)}</div>` : "<div class='muted'>根门</div>"}</td>
    <td>${p.numerator ?? "—"}<div class="muted">分母来自: ${p.parent_pop_id ? "父群 " + esc(p.parent_pop_id.slice(0, 14)) + "…" : "事件总数"}</div></td>
    <td>${p.denominator ?? "—"}</td>
    <td>${num(p.percent)}</td>
    <td>${popStatusTag(p)}${p.contested ? ' <span class="tag ambiguous">contested</span>' : ""}</td>
    <td class="muted">${(p.stale_reasons || []).map(esc).join("<br>")}</td>
    <td>${p.created_seq}</td>
  </tr>`).join("")}
  </tbody></table></div>`;
}

function judgmentsHtml(v) {
  const js = v.judgments || [];
  if (!js.length) return "";
  return `<details class="card"><summary><h3 style="display:inline">机器可读判定依据（${js.length} 条）</h3></summary>
  <table><thead><tr><th>规则</th><th>级别</th><th>对象</th><th>判断</th></tr></thead><tbody>
  ${js.map((j) => `<tr><td><code>${esc(j.rule_id)}</code></td>
    <td><span class="tag ${j.severity}">${esc(j.severity)}</span></td>
    <td><code>${esc(j.subject)}</code></td><td>${esc(j.message)}</td></tr>`).join("")}
  </tbody></table></details>`;
}

function eventsHtml(side, s) {
  // 用 view 中没有的事件列表，需要单独渲染（由 refresh 时附带，见下面填充）。
  return `<div class="card"><h3>事件时间线（点击回到当时视图，不覆盖历史）</h3>
    <div id="events${side}"><span class="muted">载入事件…</span></div></div>`;
}

// ---- 散点：当前门坐标空间中的事件；无法转换的值不绘制该点但保留在事件清单 ----
function drawScatter(side) {
  const s = state[side.toLowerCase()];
  const v = s.view;
  const sel = document.getElementById("gateSelect" + side);
  if (!sel || !v) return;
  if (!sel.options.length) {
    for (const n of v.gate_tree || []) {
      const o = document.createElement("option");
      o.value = n.gate_id;
      o.textContent = `${n.name} (${n.space.basis})`;
      sel.appendChild(o);
    }
  }
  const gateId = sel.value;
  document.getElementById("gatePick" + side).textContent = gateId;
  const node = (v.gate_tree || []).find((n) => n.gate_id === gateId);
  const cv = document.getElementById("scatter" + side);
  const ctx = cv.getContext("2d");
  ctx.clearRect(0, 0, cv.width, cv.height);
  if (!node) return;

  // 用该门最新派生（含过期标记）拿到成员事件；坐标从服务端 gate_tree.space 无法直接给出，
  // 因此坐标从 pops 的诊断无法获得——改用 /api/view 中 gate_tree 附加的点（见 ensure points）。
  const points = node.points || [];
  const note = document.getElementById("scatterNote" + side);
  const dropped = points.filter((p) => p.x === null || p.y === null);
  note.innerHTML = `坐标基准 ${esc(node.space.basis)}，x=${esc(node.space.x_channel)}(${esc(node.space.x_unit)})，` +
    `y=${esc(node.space.y_channel)}(${esc(node.space.y_unit)})。` +
    (dropped.length ? `<br><span class="blocked">${dropped.length} 个事件含无法转换值，保留原位置不绘制：` +
      dropped.map((d) => esc(d.event_id)).join(", ") + "</span>" : "");

  const xs = points.map((p) => p.x).filter((x) => x !== null);
  const ys = points.map((p) => p.y).filter((y) => y !== null);
  if (!xs.length) {
    ctx.fillStyle = "#93a0bd";
    ctx.fillText("无可绘制数值（原始表示已保留）", 10, 20);
    drawPolygon(ctx, node, null, null);
    return;
  }
  const xmin = Math.min(...xs, ...(node.polygon || []).map((p) => p.x));
  const xmax = Math.max(...xs, ...(node.polygon || []).map((p) => p.x));
  const ymin = Math.min(...ys, ...(node.polygon || []).map((p) => p.y));
  const ymax = Math.max(...ys, ...(node.polygon || []).map((p) => p.y));
  const pad = 28;
  const sx = (x) => pad + ((x - xmin) / ((xmax - xmin) || 1)) * (cv.width - 2 * pad);
  const sy = (y) => cv.height - pad - ((y - ymin) / ((ymax - ymin) || 1)) * (cv.height - 2 * pad);
  drawPolygon(ctx, node, sx, sy);

  const memberSet = new Set((node.live_pop || node.latest_pop || {}).numerator_event_ids || []);
  const ambSet = new Set((node.live_pop || node.latest_pop || {}).ambiguous_event_ids || []);
  for (const p of points) {
    if (p.x === null || p.y === null) continue;
    ctx.beginPath();
    ctx.arc(sx(p.x), sy(p.y), 4, 0, Math.PI * 2);
    if (ambSet.has(p.event_id)) ctx.fillStyle = "#c084fc";
    else if (memberSet.has(p.event_id)) ctx.fillStyle = "#4ade80";
    else ctx.fillStyle = "#5b6b8c";
    ctx.fill();
    ctx.fillStyle = "#93a0bd";
    ctx.font = "9px sans-serif";
    ctx.fillText(p.event_id, sx(p.x) + 5, sy(p.y) - 4);
    if ((p.diagnostics || []).length) {
      ctx.fillStyle = "#fbbf24";
      ctx.fillText("⚠", sx(p.x) - 4, sy(p.y) - 12);
    }
  }
}

function drawPolygon(ctx, node, sx, sy) {
  const poly = node.polygon || [];
  if (!poly.length || !sx) return;
  ctx.beginPath();
  poly.forEach((p, i) => (i === 0 ? ctx.moveTo(sx(p.x), sy(p.y)) : ctx.lineTo(sx(p.x), sy(p.y))));
  ctx.closePath();
  ctx.strokeStyle = "#6ea8fe";
  ctx.setLineDash([5, 4]);
  ctx.stroke();
  ctx.setLineDash([]);
}

// ---- 操作 ----
async function loadEventLists() {
  for (const side of ["A", "B"]) {
    const s = state[side.toLowerCase()];
    const ev = await api(`/api/events?branch=${encodeURIComponent(s.branch)}`);
    const box = document.getElementById("events" + side);
    if (box) {
      box.innerHTML = ev.events.map((e) => {
        const cls = e.retracted ? "blocked" : (e.seq === s.seq ? "ok" : "");
        return `<div class="ev ${cls}" onclick="setSeq('${side.toLowerCase()}', ${e.seq})">
          #${e.seq} <code>${esc(e.event_id.slice(0, 18))}…</code>
          <span class="tag">${esc(e.kind)}</span>
          ${e.retracted ? '<span class="tag blocked">已撤销</span>' : ""}
          <span class="muted">${esc(e.at)}</span></div>`;
      }).join("");
    }
  }
}

async function createBranch() {
  const name = document.getElementById("newBranch").value.trim();
  if (!name) return toast("请输入新分支名", true);
  const from = state.a.branch;
  try {
    await api("/api/branches", { method: "POST", body: formBody({ branch: name, from, note: "并排复核草案" }) });
    state.b.branch = name;
    state.b.seq = null;
    toast(`已从 ${from} 分叉出 ${name}，并在右窗打开`);
    await refresh();
  } catch (e) { toast(e.message, true); }
}

function currentSideForAction() {
  // 场景作用于左窗分支，带它的 head_seq 作为乐观锁基线。
  return state.a;
}

async function act(path) {
  const s = currentSideForAction();
  try {
    const r = await api("/api/" + path, {
      method: "POST",
      body: formBody({ branch: s.branch, expected_seq: s.seq === s.head ? s.head : s.seq, rationale: "复核操作" }),
    });
    toast(`已在 ${s.branch} 追加事件 seq=${r.seq}`);
    state.a.seq = null;
    await refresh();
  } catch (e) { handleConflict(e, s); }
}

async function decideBoundary(side, gateId, choice) {
  const s = state[side.toLowerCase()];
  try {
    await api("/api/action/decide-boundary", {
      method: "POST",
      body: formBody({ branch: s.branch, expected_seq: s.seq, gate_id: gateId, accept: choice,
        rationale: choice === "include" ? "边界点计入（闭区间）" : choice === "exclude" ? "边界点排除（开区间）" : "整体否定，保留歧义" }),
    });
    toast("裁定已作为新事件追加");
    s.seq = null;
    await refresh();
  } catch (e) { handleConflict(e, s); }
}

async function singularDecision(choice) {
  const s = state.a;
  try {
    const r = await api("/api/action/singular-decision", {
      method: "POST",
      body: formBody({ branch: s.branch, expected_seq: s.seq === s.head ? s.head : s.seq, choice }),
    });
    toast(`近奇异补偿裁定=${choice}，seq=${r.seq}`);
    state.a.seq = null;
    await refresh();
  } catch (e) { handleConflict(e, s); }
}

function handleConflict(e, s) {
  if (e.status === 409 && e.body && e.body.conflict) {
    const c = e.body.conflict;
    toast(`版本冲突（同版本并发修改）：\n分支 ${c.branch} 基线 seq=${c.expected_seq}，` +
      `当前 head=${c.actual_head}\n对方新事件: ${(c.newer_event_ids || []).map((x) => x.slice(0, 16)).join(", ")}\n` +
      `请刷新并在并排窗口查看双方差异后再提交。`, true);
  } else {
    toast(e.message, true);
  }
}

function renderDiff() {
  // 并排差异：在两个窗格头部下方插入轻量摘要；完整差异见接口 /api/diff。
  const a = state.a.view, b = state.b.view;
  if (!a || !b) return;
  const summary = document.createElement("div");
  summary.className = "card";
  const headsDiff = ["dataset", "spillover_meta", "compensation", "transform", "relations"]
    .filter((k) => JSON.stringify(a.heads[k]) !== JSON.stringify(b.heads[k]))
    .map((k) => `<li>${esc(k)}: <code>${esc(a.heads[k])}</code> → <code>${esc(b.heads[k])}</code></li>`)
    .join("");
  const popA = new Map((a.pops || []).map((p) => [p.pop_id, p]));
  const popB = new Map((b.pops || []).map((p) => [p.pop_id, p]));
  const onlyA = [...popA.keys()].filter((k) => !popB.has(k)).length;
  const onlyB = [...popB.keys()].filter((k) => !popA.has(k)).length;
  summary.innerHTML = `<h3>并排差异（每个值带稳定路径身份）</h3>
    <ul class="diff">${headsDiff || "<li>材料头部一致</li>"}</ul>
    <div class="muted">群体仅 A: ${onlyA}；仅 B: ${onlyB}（完整叶子级差异见 <code>/api/diff</code>）</div>`;
  // 插到 B 窗格顶部（pane-body 的第一张卡片）。
  const body = document.querySelector("#paneB .pane-body");
  if (body && !document.getElementById("diffCard")) {
    summary.id = "diffCard";
    body.insertBefore(summary, body.firstChild);
  } else if (body) {
    body.replaceChild(summary, document.getElementById("diffCard"));
  }
}

async function exportArchive() {
  const body = await api("/api/export");
  const blob = new Blob([JSON.stringify(body, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "flow-boundary-chamber-archive.json";
  a.click();
  toast("已导出归档（身份、顺序、父子来源、原始字节）");
}

async function importArchive(ev) {
  const file = ev.target.files[0];
  if (!file) return;
  const text = await file.text();
  let parsed;
  try { parsed = JSON.parse(text); } catch (e) { return toast("归档不是合法 JSON", true); }
  const raw = parsed.raw_source_b64;
  if (!raw) return toast("归档缺少 raw_source_b64", true);
  try {
    await api("/api/reset-import", {
      method: "POST",
      body: formBody({ archive_json: JSON.stringify(parsed), raw_source_b64: raw }),
    });
    toast("已删除本地数据库并重新导入：身份/顺序/来源关系恢复");
    state.a.seq = null;
    state.b.seq = null;
    await refresh();
  } catch (e) { toast(e.message, true); }
}

async function init() {
  try {
    const info = await api("/api/case");
    document.getElementById("caseId").textContent =
      `${info.case_id} · ${info.seeded ? "已播种基线" : "空库"}`;
    await refresh();
    await loadEventLists();
  } catch (e) {
    toast("初始化失败: " + e.message, true);
  }
}
init();
