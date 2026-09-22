"use strict";
const S = {
  project: null,
  branch: "main",
  view: null,
  events: [],
  atSeq: null,
  xChannel: null,
  yChannel: null,
  channels: [],
  showPrePost: false,
  prePoints: null,
  postPoints: null,
  headSnap: 0,
};

const $ = (id) => document.getElementById(id);
const api = async (path, opts = {}) => {
  const res = await fetch(path, {
    headers: opts.body ? { "Content-Type": "application/json" } : {},
    ...opts,
  });
  const ct = res.headers.get("content-type") || "";
  const data = ct.includes("json") ? await res.json() : await res.text();
  if (!res.ok) throw Object.assign(new Error(data.error || res.statusText), { status: res.status, data });
  return data;
};
const toast = (msg, isErr = false) => {
  const t = $("toast");
  t.textContent = msg;
  t.className = "toast" + (isErr ? " err" : "");
  setTimeout(() => t.classList.add("hidden"), 3200);
};
const pct = (n) => (Number.isFinite(n) ? n.toFixed(2) + "%" : "—");
const esc = (s) => String(s ?? "").replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));

async function boot() {
  const h = await api("/api/health");
  $("alg-tag").textContent = h.algorithm + " · schema v" + h.schema_version;
  await refreshProjects();
  bindUi();
}

async function refreshProjects() {
  const rows = await api("/api/projects");
  const sel = $("project-select");
  sel.innerHTML = "";
  for (const r of rows) {
    const o = document.createElement("option");
    o.value = r[0];
    o.textContent = `${r[1]} (${r[0]}) @${r[2]}`;
    sel.appendChild(o);
  }
  if (rows.length) {
    S.project = sel.value;
    await loadProject();
  }
}

async function loadProject() {
  S.atSeq = null;
  const detail = await api(`/api/projects/${S.project}`);
  S.branches = detail.branches;
  if (!detail.branches.includes(S.branch)) S.branch = "main";
  renderBranchTabs(detail.branches);
  await refreshView();
  await refreshEvents();
}

function renderBranchTabs(branches) {
  const tabs = $("branch-tabs");
  tabs.innerHTML = "";
  for (const b of branches) {
    const t = document.createElement("span");
    t.className = "tab" + (b === S.branch ? " active" : "");
    t.textContent = b;
    t.dataset.branch = b;
    t.onclick = async () => {
      S.branch = b;
      S.atSeq = null;
      await loadProject();
    };
    tabs.appendChild(t);
  }
  const cmp = $("cmp-branch");
  cmp.innerHTML = "";
  for (const b of branches) {
    const o = document.createElement("option");
    o.value = b;
    o.textContent = b;
    cmp.appendChild(o);
  }
}

async function refreshView() {
  const q = new URLSearchParams({ branch: S.branch });
  if (S.atSeq != null) q.set("at_seq", S.atSeq);
  S.view = await api(`/api/projects/${S.project}/view?` + q);
  S.headSnap = S.view.head_seq;
  $("head-badge").textContent = `分支 ${S.branch} · 显示到 #${S.view.head_seq}` +
    (S.atSeq != null && S.atSeq < liveHead() ? "（历史视图）" : "");
  const sourceId = S.view.sample_source;
  if (sourceId) {
    const fcs = await api(`/api/projects/${S.project}/fcs/${sourceId}`);
    S.channels = fcs.channels.map((c) => c.name);
    S.fcs = fcs;
  } else {
    S.channels = [];
    S.fcs = null;
  }
  renderAxes();
  renderTree();
  renderComp();
  await renderScatter();
}

function liveHead() {
  return S.events.length ? Math.max(...S.events.map((e) => e.seq)) : 0;
}

async function refreshEvents() {
  S.events = await api(`/api/projects/${S.project}/events`);
  const ol = $("event-list");
  ol.innerHTML = "";
  S.events
    .filter((e) => e.branch === S.branch || S.branches.includes(e.branch))
    .forEach((e) => {
      const li = document.createElement("li");
      if (S.atSeq === e.seq || (S.atSeq == null && e.seq === S.view.head_seq && e.branch === S.branch))
        li.classList.add("at");
      li.innerHTML = `<span class="kind">#${e.seq} ${e.kind}</span>
        <span class="src">[${e.branch}] ${esc(e.direct_source)} · ${esc(e.event_id)}</span>`;
      li.onclick = () => inspectEvent(e.seq);
      ol.appendChild(li);
    });
}

async function inspectEvent(seq) {
  const env = await api(`/api/projects/${S.project}/events/${seq}`);
  showModal(`事件 #${seq} · ${env.kind}`,
    `<pre style="white-space:pre-wrap;font-size:11px">${esc(JSON.stringify(env.payload, null, 2))}</pre>
     <div class="denom">稳定身份：${esc(env.event_id)} · 分支 ${esc(env.branch)}</div>`,
    null);
}

function renderAxes() {
  const defs = ["FSC-A", "SSC-A", "FITC-A", "PE-A", "APC-A"];
  const fill = (sel, def) => {
    sel.innerHTML = "";
    for (const c of S.channels) {
      const o = document.createElement("option");
      o.value = c; o.textContent = c; sel.appendChild(o);
    }
    if (S.channels.includes(def)) sel.value = def;
  };
  fill($("x-axis"), "FSC-A");
  fill($("y-axis"), "SSC-A");
  S.xChannel = $("x-axis").value;
  S.yChannel = $("y-axis").value;
}

async function renderScatter() {
  if (!S.fcs) return;
  const sid = S.view.sample_source;
  const q = new URLSearchParams({
    branch: S.branch, source_id: sid, x: S.xChannel, y: S.yChannel,
  });
  if (S.atSeq != null) q.set("at_seq", S.atSeq);
  const sc = await api(`/api/projects/${S.project}/scatter?` + q);
  drawScatter(sc, false);
}

function drawScatter(sc, rawMode) {
  const cv = $("scatter");
  const ctx = cv.getContext("2d");
  ctx.clearRect(0, 0, cv.width, cv.height);
  const W = cv.width, H = cv.height, pad = 38;
  const pts = sc.points;
  const xs = pts.filter(p => isFinite(p.x)).map(p => p.x);
  const ys = pts.filter(p => isFinite(p.y)).map(p => p.y);
  const xmin = Math.min(...xs), xmax = Math.max(...xs);
  const ymin = Math.min(...ys), ymax = Math.max(...ys);
  const X = (x) => pad + ((x - xmin) / (xmax - xmin || 1)) * (W - 2 * pad);
  const Y = (y) => H - pad - ((y - ymin) / (ymax - ymin || 1)) * (H - 2 * pad);

  // 轴
  ctx.strokeStyle = "#8fa0b4"; ctx.beginPath();
  ctx.moveTo(pad, pad); ctx.lineTo(pad, H - pad); ctx.lineTo(W - pad, H - pad); ctx.stroke();
  ctx.fillStyle = "#43536a"; ctx.font = "11px sans-serif";
  ctx.fillText(`${sc.x_channel} (${sc.x_basis})`, W / 2 - 60, H - 12);
  ctx.save(); ctx.translate(13, H / 2 + 40); ctx.rotate(-Math.PI / 2);
  ctx.fillText(`${sc.y_channel} (${sc.y_basis})`, 0, 0); ctx.restore();

  // 门多边形
  const gate = Object.values(S.view.gates).find(g => g.x_channel === sc.x_channel && g.y_channel === sc.y_channel);
  if (gate) {
    ctx.strokeStyle = "#d07a12"; ctx.lineWidth = 1.5; ctx.beginPath();
    gate.polygon.forEach((p, i) => {
      const gx = X(p.x), gy = Y(p.y);
      i ? ctx.lineTo(gx, gy) : ctx.moveTo(gx, gy);
    });
    ctx.closePath(); ctx.stroke();
    ctx.fillStyle = "#d07a12";
    ctx.fillText(`${gate.name} r${gate.revision}`, X(gate.polygon[0].x), Y(gate.polygon[0].y) - 6);
  }

  // 门内成员高亮
  const members = new Set();
  for (const p of Object.values(S.view.populations)) {
    if (p.gate_id === (gate && gate.gate_id)) p.member_indices.forEach(i => members.add(i));
  }

  for (const p of pts) {
    if (!isFinite(p.x) || !isFinite(p.y)) {
      ctx.fillStyle = "#b3261e"; ctx.fillRect(pad, 4 + p.event_index % 14, 6, 6);
      continue;
    }
    ctx.beginPath();
    ctx.arc(X(p.x), Y(p.y), members.has(p.event_index) ? 3.6 : 2.4, 0, 2 * Math.PI);
    if (!p.x_ok || !p.y_ok) ctx.fillStyle = "#b3261e";
    else if (members.has(p.event_index)) ctx.fillStyle = "#1c7a3d";
    else ctx.fillStyle = "rgba(58,96,160,.55)";
    ctx.fill();
  }
  $("scatter-basis").textContent =
    `来源 ${sc.source_id} · ${pts.length} 事件 · 无法转换保留原位: ${sc.untransformable} · 红=极端/NaN · 绿=当前门内`;
  $("scatter-legend").innerHTML =
    `<span><span class="dot" style="background:#3a60a0"></span>门外</span>
     <span><span class="dot" style="background:#1c7a3d"></span>门内（成员）</span>
     <span><span class="dot" style="background:#b3261e"></span>NaN/负/超量程（未丢弃）</span>
     <span><span class="dot" style="background:#d07a12"></span>门多边形（含边）</span>`;
}

function renderTree() {
  const v = S.view;
  const roots = Object.values(v.gates).filter(g => !g.parent);
  const ul = $("gate-tree");
  ul.innerHTML = "";
  const node = (g) => {
    const li = document.createElement("li");
    const pop = v.populations[g.gate_id];
    const statusCls = pop && pop.status !== "fresh" ? "stale" : "fresh";
    li.innerHTML = `<span class="gnode">${esc(g.name)} <span class="rev">[${g.gate_id} r${g.revision}]</span>
      <span class="${statusCls}">${pop ? pop.status : "无群结果"}</span></span>`;
    const kids = Object.values(v.gates).filter(k => k.parent === g.gate_id);
    if (kids.length) {
      const sub = document.createElement("ul");
      kids.forEach(k => sub.appendChild(node(k)));
      li.appendChild(sub);
    }
    return li;
  };
  roots.forEach(g => ul.appendChild(node(g)));

  const tb = $("pop-rows");
  tb.innerHTML = "";
  for (const p of Object.values(v.populations)) {
    const tr = document.createElement("tr");
    const denom = p.denominator.parent_gate
      ? `${p.denominator.parent_gate} → ${p.denominator.parent_pop_result_id || "?"} (n=${p.denominator.parent_count})`
      : `根源 ${p.denominator.root_source_id} (n=${p.root_count})`;
    tr.innerHTML =
      `<td class="id">${esc(p.pop_result_id)}<br><span class="denom">${esc(p.gate_name)}</span></td>
       <td>${p.count}</td><td>${pct(p.percent_of_parent)}</td><td>${pct(p.percent_of_root)}</td>
       <td class="denom">${esc(denom)}<br>${esc(p.denominator.parent_revision_sig.slice(0, 70))}</td>
       <td class="${p.status === "fresh" ? "fresh" : "stale"}">${p.status}${
         p.stale_reason ? "<br><span class='denom'>" + esc(p.stale_reason) + "</span>" : ""
       }</td>`;
    tb.appendChild(tr);
  }
}

function renderComp() {
  const c = S.view.compensation;
  const box = $("comp-body");
  if (!c) { box.innerHTML = "<span class='denom'>尚无补偿矩阵</span>"; return; }
  const verdict = c.verdict;
  const stateCls = c.blocked ? "blocked" : "fresh";
  const matrixTable = (m) =>
    `<table class="matrix"><tbody>${m.map(r => `<tr>${r.map(x => `<td>${x.toExponential(2)}</td>`).join("")}</tr>`).join("")}</tbody></table>`;
  let html = `<div>spillover <b>${esc(c.spillover_id)}</b> r${c.revision} ·
    det=${verdict.determinant.toExponential(2)} · cond≈${
      verdict.condition_number ? verdict.condition_number.toExponential(2) : "∞"
    } <span class="${stateCls}">${c.blocked ? "阻断" : "可补偿"}</span></div>
    <div class="denom">${esc(verdict.reason)}</div>
    <details><summary>spillover 矩阵</summary>${matrixTable(
      (S.view.spillovers[c.spillover_id] || {}).matrix || [])}</details>`;
  if (c.blocked && verdict.candidates.length) {
    html += "<div class='denom'>无法唯一决定：保留全部合法候选，接受/否定只生成事件，不改原数据。</div>";
    for (const cand of verdict.candidates) {
      html += `<div class="cand">
        <b>${esc(cand.label)}</b> <span class="denom">${esc(cand.candidate_id)}</span>
        <div class="denom">${esc(cand.justification)}</div>
        <details><summary>逆矩阵</summary>${matrixTable(cand.inverse)}</details>
        <button data-accept="${esc(cand.candidate_id)}">接受此候选</button>
        <button data-reject-cand="${esc(cand.candidate_id)}">否定矩阵</button>
      </div>`;
    }
  } else if (!c.blocked) {
    html += `<div>生效逆矩阵：${c.active_decision
      ? "候选 " + esc(c.active_decision.candidate_id || "auto") + "（" + esc(c.active_decision.decision_id) + "）"
      : "良态自动逆"}</div>
      <button id="btn-revert-decision">撤销当前裁定（恢复前一分支）</button>`;
  } else {
    html += `<div class="blocked">${esc(c.blocked_reason || "")}</div>`;
  }
  const all = c.all_decisions || [];
  if (all.length) {
    html += "<div class='denom'>裁定史（含未采纳）：" + all.map(d =>
      `#${d.at_seq} ${d.outcome} ${esc(d.candidate_id || "")}${d.reverted ? "（已撤销）" : ""}`).join(" ｜ ") + "</div>";
  }
  box.innerHTML = html;
  box.querySelectorAll("[data-accept]").forEach(b => b.onclick = () =>
    decide("accepted_candidate", b.dataset.accept, "审阅页接受候选"));
  box.querySelectorAll("[data-reject-cand]").forEach(b => b.onclick = () =>
    decide("rejected", null, "审阅页否定矩阵"));
  const rv = $("btn-revert-decision");
  if (rv) rv.onclick = async () => {
    const d = c.active_decision;
    await postJson("/revert", { reverted_event_id: d.event_id, note: "审阅页撤销" });
  };
}

async function decide(outcome, candidateId, rationale) {
  const body = { branch: S.branch, expected_head: S.headSnap, spillover_id: S.view.compensation.spillover_id,
    outcome, candidate_id: candidateId, rationale };
  try {
    await postJson("/decisions", body);
    toast("裁定已作为新事件追加");
    await loadProject();
  } catch (e) {
    await handleConflict(e, "decide compensation", JSON.stringify(body));
  }
}

async function postJson(suffix, body) {
  return api(`/api/projects/${S.project}${suffix}`, {
    method: "POST",
    body: JSON.stringify({ ...body, branch: body.branch || S.branch }),
  });
}

async function handleConflict(e, action, draft) {
  if (e.status !== 409) { toast(e.message, true); return; }
  const diff = [
    `本地基准头=${(e.data && e.data.conflict) ? e.data.conflict.expected_head : S.headSnap}`,
    "服务端头已前进（另一窗口先保存）",
    "门几何/参数存在差异，需要人工选择合并或新开分支",
  ];
  const c = e.data?.conflict || { expected_head: S.headSnap, actual_head: S.view.head_seq };
  await api(`/api/projects/${S.project}/conflicts`, {
    method: "POST",
    body: JSON.stringify({
      branch: S.branch, expected_head: c.expected_head, attempted_action: action,
      left_draft: "另一窗口先提交版本", right_draft: draft, diff,
    }),
  });
  toast(`版本冲突已留档（${action}），请刷新后选择合并或分叉`, true);
  await loadProject();
}

function bindUi() {
  $("project-select").onchange = async (e) => { S.project = e.target.value; S.branch = "main"; await loadProject(); };
  $("btn-new").onclick = async () => {
    const title = prompt("新项目标题", "流式复核草稿");
    if (!title) return;
    const r = await api("/api/projects", { method: "POST", body: JSON.stringify({ title }) });
    S.project = r.project_id;
    await refreshProjects();
  };
  $("btn-load-case").onclick = async () => {
    let pid = S.project;
    if (!pid) {
      const r = await api("/api/projects", { method: "POST", body: JSON.stringify({ title: "流式边界室" }) });
      pid = r.project_id;
      S.project = pid;
    }
    try {
      const r = await api(`/api/projects/${pid}/load-case`, { method: "POST", body: "{}" });
      toast(`案例重放完成：${r.steps.length} 步，项目 ${r.project_id}`);
      S.branch = "main";
      await refreshProjects();
    } catch (e) {
      toast("案例重放失败：" + e.message, true);
    }
  };
  $("x-axis").onchange = async () => { S.xChannel = $("x-axis").value; await renderScatter(); };
  $("y-axis").onchange = async () => { S.yChannel = $("y-axis").value; await renderScatter(); };
  $("btn-comp-prepost").onclick = () => toast("当前散点即为补偿+变换后；补偿前 raw 值见每点 x_raw/y_raw 与 FCS 详情");
  $("btn-fork").onclick = async () => {
    const name = prompt("新分支名（对当前材料重新解释）", "draft-" + Math.floor(Math.random() * 1000));
    if (!name) return;
    try {
      await postJson("/forks", { from_branch: S.branch, new_branch: name, display_name: name, fork_at_head: S.headSnap });
      S.branch = name;
      await loadProject();
      toast(`已分叉到 ${name}，旧分支与裁定保留`);
    } catch (e) { toast(e.message, true); }
  };
  $("btn-compare").onclick = compareDrafts;
  $("btn-close-cmp").onclick = () => $("compare-panel").classList.add("hidden");
  $("btn-export").onclick = exportBundle;
  $("bundle-file").onchange = importBundle;
  $("fcs-file").onchange = uploadFcs;

  // 点击历史条目后：提供“回到该版本”按钮（modal 里渲染）
  window.addEventListener("click", (ev) => {
    if (ev.target.id === "modal-cancel") $("modal").classList.add("hidden");
  });
}

async function compareDrafts() {
  const right = $("cmp-branch").value;
  if (right === S.branch) { toast("请选择另一个分支做并排", true); return; }
  $("cmp-l").textContent = "左：" + S.branch;
  $("cmp-r").textContent = "右：" + right;
  const q = new URLSearchParams({ left_branch: S.branch, right_branch: right });
  const d = await api(`/api/projects/${S.project}/diff?` + q);
  const tb = $("diff-rows");
  tb.innerHTML = "";
  for (const r of d.rows) {
    const tr = document.createElement("tr");
    if (r.differs) tr.className = "diff";
    tr.innerHTML = `<td>${esc(r.identity)}</td><td>${esc(r.label)}</td>
      <td>${esc(r.left)}</td><td>${esc(r.right)}</td>`;
    tb.appendChild(tr);
  }
  $("compare-panel").classList.remove("hidden");
}

async function exportBundle() {
  const b = await api("/api/export");
  const blob = new Blob([JSON.stringify(b, null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "flowlineage-bundle.json";
  a.click();
  toast("档案已导出（事件 + 原始字节 + 顺序）");
}

async function importBundle(ev) {
  const file = ev.target.files[0];
  if (!file) return;
  const text = await file.text();
  const r = await api("/api/import-bundle", { method: "POST", body: text });
  toast(`导入校验通过：${r.imported_projects} 个项目（身份/顺序/来源保持）。可删除本地 db 后用同文件恢复。`);
  await refreshProjects();
}

async function uploadFcs(ev) {
  const file = ev.target.files[0];
  if (!file || !S.project) return;
  const sourceId = prompt("来源稳定身份 source_id（如 raw-sample-2）", "raw-upload-1");
  if (!sourceId) return;
  const role = confirm("确定=样本(sample)，取消=单染(single_stain)") ? "sample" : "single_stain";
  const fd = new FormData();
  fd.append("file", file);
  const q = new URLSearchParams({
    branch: S.branch, expected_head: S.headSnap, source_id: sourceId, role, revision: 1,
  });
  const res = await fetch(`/api/projects/${S.project}/import-fcs?` + q, { method: "POST", body: fd });
  if (!res.ok) {
    const j = await res.json().catch(() => ({}));
    await handleConflict({ status: res.status, message: j.error, data: j }, "import-fcs", file.name);
    return;
  }
  toast("FCS 已导入：原始表示保留，异常值留在原位并附诊断");
  await loadProject();
}

// ---- 时间旅行：历史事件 modal 中提供回读按钮 ----
const _inspect = inspectEvent;
inspectEvent = async function (seq) {
  const env = await api(`/api/projects/${S.project}/events/${seq}`);
  showModal(`事件 #${seq} · ${env.kind}`,
    `<pre style="white-space:pre-wrap;font-size:11px">${esc(JSON.stringify(env.payload, null, 2))}</pre>
     <div class="denom">稳定身份：${esc(env.event_id)} · 分支 ${esc(env.branch)}</div>`,
    async () => {
      S.atSeq = seq;
      await refreshView();
      await refreshEvents();
      $("modal").classList.add("hidden");
      toast(`已回到 #${seq}：显示当时依赖版本与结论，不覆盖任何历史`);
    },
    "回到此版本");
};

function showModal(title, bodyHtml, onOk, okText = "确定") {
  $("modal-title").textContent = title;
  $("modal-body").innerHTML = bodyHtml;
  $("modal").classList.remove("hidden");
  $("modal-cancel").onclick = () => $("modal").classList.add("hidden");
  $("modal-ok").textContent = okText;
  $("modal-ok").onclick = async () => {
    if (onOk) await onOk();
    $("modal").classList.add("hidden");
  };
}

boot();
