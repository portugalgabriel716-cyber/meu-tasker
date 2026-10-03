"use strict";
const $ = (s) => document.querySelector(s);
const $$ = (s) => Array.from(document.querySelectorAll(s));

// A chave chega no fragmento (#k=...) pelo redirecionamento do link de abertura.
let K = new URLSearchParams(location.hash.slice(1)).get("k") || "";
try {
  if (K) sessionStorage.setItem("k", K); else K = sessionStorage.getItem("k") || "";
} catch (e) { /* sem storage: segue com o que veio no fragmento */ }

let estado = null, ajustes = null, logSeq = 0, logLinhas = [], ocupado = false;
let offset = 0;            // relógio do servidor - relógio daqui
let horasHist = 24, histCache = {}, forcarLogin = false;

// ------------------------------------------------------------------ utilidades
function esc(s) {
  return String(s == null ? "" : s).replace(/[&<>"]/g,
    (m) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[m]));
}
function hhmm(ts) {
  const d = new Date(ts * 1000);
  return String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0");
}
function dia(ts) {
  const d = new Date(ts * 1000), hoje = new Date();
  if (d.toDateString() === hoje.toDateString()) return hhmm(ts);
  return String(d.getDate()).padStart(2, "0") + "/" + String(d.getMonth() + 1).padStart(2, "0") + " " + hhmm(ts);
}
function dur(s) {
  s = Math.max(0, Math.round(s));
  if (s < 60) return s + " s";
  if (s < 3600) return Math.floor(s / 60) + " min" + (s % 60 && s < 600 ? " " + (s % 60) + " s" : "");
  return Math.floor(s / 3600) + " h " + Math.floor((s % 3600) / 60) + " min";
}
function gb(v, casas) { return v == null ? "—" : Number(v).toFixed(casas == null ? 2 : casas).replace(".", ","); }
function gbCurto(v) { return v == null ? "—" : String(Math.round(Number(v) * 100) / 100).replace(".", ","); }
function agora() { return Date.now() / 1000 + offset; }

function toast(t) {
  const el = $("#toast");
  el.textContent = t; el.style.display = "block";
  clearTimeout(el._t); el._t = setTimeout(() => (el.style.display = "none"), 6500);
}

async function api(path, body) {
  const opt = { headers: { "X-Tasker-Key": K } };
  if (body !== undefined) {
    opt.method = "POST";
    opt.headers["Content-Type"] = "application/json";
    opt.body = JSON.stringify(body);
  }
  const r = await fetch(path, opt);
  if (r.status === 403) { $("#semchave").hidden = false; throw new Error("sem acesso"); }
  if (!r.ok) throw new Error(await r.text());
  return r.json();
}

async function acao(nome, extra) {
  if (ocupado) return null;
  ocupado = true;
  try {
    const r = await api("/api/acao", Object.assign({ acao: nome }, extra || {}));
    if (r.msg || r.erro) toast(r.msg || r.erro);
    return r;
  } catch (e) {
    toast("Falhou: " + e.message);
    return null;
  } finally {
    ocupado = false;
    puxaEstado();
  }
}

// ---------------------------------------------------------------------- abas
function abre(aba) {
  for (const t of $$(".tab")) t.setAttribute("aria-selected", String(t.dataset.tab === aba));
  for (const s of ["painel", "historico", "log-sec", "ajustes"]) $("#" + s).hidden = s !== aba;
  try { localStorage.setItem("aba", aba); } catch (e) {}
  if (aba === "ajustes") puxaAjustes();
  if (aba === "historico") puxaHist(horasHist, $("#chart2"));
  if (aba === "log-sec") { puxaLog(); }
}
for (const t of $$(".tab")) t.addEventListener("click", () => abre(t.dataset.tab));

// --------------------------------------------------------------------- estado
function pill(m) {
  if (!m.ligado) return ["bad", "desligado"];
  if (m.precisa_login) return ["bad", "sem sessão"];
  if (m.pausado) return ["warn", "pausado"];
  if (m.fase === "confirmando refill") return ["warn", "confirmando refill"];
  if (m.erro) return ["warn", "instável"];
  return ["ok", "vigiando"];
}

function desenhaEstado() {
  if (!estado) return;
  const m = estado.monitor, sis = estado.sistema;
  offset = (m.agora || Date.now() / 1000) - Date.now() / 1000;
  const [cls, txt] = pill(m);
  const p = $("#pill"); p.className = "pill " + cls; p.textContent = txt;

  // faixas
  $("#login-box").hidden = !(m.precisa_login || forcarLogin);
  $("#login-motivo").textContent = m.precisa_login ||
    "Entre de novo pra trocar a sessão atual por uma nova.";
  const nav = estado.login_navegador;
  $("#nav-estado").textContent = nav && nav.estado !== "parado" ? "— " + nav.msg : "";
  $("#b-nav").disabled = !sis.navegador || (nav && (nav.estado === "abrindo" || nav.estado === "aguardando"));
  if (!sis.navegador) $("#b-nav").title = "Não achei Chrome, Edge nem Chromium instalado.";
  if (nav && nav.estado === "ok" && forcarLogin) forcarLogin = false;
  $("#pausa-box").hidden = !m.pausado;
  $("#pausa-txt").textContent = m.pausa_ate ? "Pausado até " + hhmm(m.pausa_ate) : "Pausado até você retomar";
  $("#off-box").hidden = m.ligado;
  $("#b-retomar").hidden = !m.pausado;
  $("#b-pausa").hidden = m.pausado;
  $("#b-checar").disabled = !m.ligado || m.pausado;

  // medidor
  const v = m.volume, arc = $("#g-arc"), C = 2 * Math.PI * 82;
  if (v && v.total) {
    const frac = Math.max(0, Math.min(1, v.left / v.total));
    arc.setAttribute("stroke-dasharray", (frac * C).toFixed(1) + " " + C.toFixed(1));
    const lim = m.limite || 0;
    arc.setAttribute("stroke", v.left <= lim ? "var(--bad)" : v.left <= lim + 0.2 * v.total ? "var(--warn)" : "var(--ok)");
    const ang = Math.min(1, lim / v.total) * 360;
    $("#g-lim").setAttribute("transform", "rotate(" + ang.toFixed(1) + " 100 100)");
    $("#g-big").textContent = gb(v.left);
    $("#g-unit").textContent = "de " + gbCurto(v.total) + " GB";
    let sub = "refill abaixo de " + gbCurto(lim) + " GB";
    if (v.plan_left != null) sub += " · plano: " + gbCurto(v.plan_left) + (v.plan_total != null ? " de " + gbCurto(v.plan_total) : "") + " GB";
    $("#g-sub").textContent = sub;
  } else {
    arc.setAttribute("stroke-dasharray", "0 " + C);
    $("#g-big").textContent = "—";
    $("#g-sub").textContent = m.ligado ? "esperando a primeira leitura" : "monitor desligado";
  }

  // números
  $("#s-ritmo").textContent = m.ritmo_gb_h == null ? "—" : gb(m.ritmo_gb_h, 2) + " GB/h";
  $("#s-hoje").textContent = m.refills_hoje == null ? "—" : m.refills_hoje;
  $("#s-hoje-d").textContent = m.falhas_seguidas ? m.falhas_seguidas + " falha(s) seguida(s)" : "";
  const u = m.ultimo_refill;
  if (u) {
    $("#s-ult").textContent = hhmm(u.t);
    $("#s-ult-d").textContent = (u.latencia ? "entrou em " + Math.round(u.latencia) + " s · " : "") + (u.why || "");
  } else if (!$("#s-ult").dataset.hist) { $("#s-ult").textContent = "—"; }
  if (m.token_expira) {
    const f = m.token_expira - agora();
    $("#s-token").textContent = f > 0 ? "ativa" : "renovando";
    $("#s-token-d").textContent = f > 0 ? "token renova em " + dur(Math.max(0, f - 300)) : "";
  } else {
    $("#s-token").textContent = m.precisa_login ? "sem sessão" : "—";
    $("#s-token-d").textContent = "";
  }
  $("#s-cred").textContent = m.credenciais ? "ligado" : "desligado";
  $("#s-cred-d").textContent = m.credenciais ? "número e senha guardados" : "sem senha guardada";
  $("#cred-estado").textContent = m.credenciais ? "Há número e senha guardados." : "Nada guardado.";
  let fase = "";
  if (m.pendente) fase = "Refill pedido às " + hhmm(m.pendente.at) + " — confirmando pelo volume…";
  else if (m.erro) fase = "Última leitura falhou: " + m.erro;
  else if (m.ultima_checagem) fase = "Última checagem às " + hhmm(m.ultima_checagem) + ".";
  $("#s-fase").textContent = fase;
  tick();

  // sistema
  $("#sw-auto").checked = !!sis.autostart;
  $("#b-monitor").textContent = m.ligado ? "Desligar monitor" : "Ligar monitor";
  $("#sis-info").textContent = "Versão " + sis.versao + " · dados em " + sis.pasta + ".";
  $("#log-pasta").textContent = sis.pasta + (sis.windows ? "\\" : "/") + "tasker.log";

  document.title = (txt === "vigiando" ? "✅ " : cls === "bad" ? "⛔ " : "⏸ ") +
    (v ? gb(v.left) + " GB · " : "") + "Tasker";
}

function tick() {
  if (!estado) return;
  const m = estado.monitor;
  if (!m.ligado) { $("#s-prox").textContent = "—"; $("#s-prox-d").textContent = "monitor desligado"; return; }
  if (m.pausado) { $("#s-prox").textContent = "pausado"; $("#s-prox-d").textContent = ""; return; }
  if (m.fase === "checando") { $("#s-prox").textContent = "agora"; return; }
  if (!m.proxima_checagem) { $("#s-prox").textContent = "—"; $("#s-prox-d").textContent = ""; return; }
  const f = m.proxima_checagem - agora();
  $("#s-prox").textContent = f <= 0.5 ? "agora" : "em " + dur(f);
  $("#s-prox-d").textContent = m.motivo_intervalo || "";
}

async function puxaEstado() {
  try {
    estado = await api("/api/estado");
    desenhaEstado();
  } catch (e) {
    const p = $("#pill"); p.className = "pill bad"; p.textContent = "sem contato";
    document.title = "⛔ Tasker";
  }
}

// --------------------------------------------------------------------- gráfico
function desenhaGrafico(box, h, horas) {
  const fim = agora(), ini = fim - horas * 3600;
  const pts = h.volume.filter((p) => p[0] >= ini);
  if (pts.length < 2) {
    box.innerHTML = '<div class="empty">ainda sem amostras suficientes — uma por checagem</div>';
    return;
  }
  const W = box.clientWidth || 800, H = 170, P = { t: 10, r: 10, b: 22, l: 36 };
  const ymax = Math.max(1, Math.ceil(Math.max.apply(null, pts.map((p) => p[1])) * 2) / 2);
  const X = (t) => P.l + ((t - ini) / (fim - ini)) * (W - P.l - P.r);
  const Y = (g) => P.t + (1 - g / ymax) * (H - P.t - P.b);
  const brecha = Math.max(600, horas * 3600 / 300);
  let d = "", ant = null;
  for (const [t, g] of pts) {   // buraco (monitor parado) não vira rampa inventada
    d += (ant === null || t - ant > brecha ? " M" : " L") + X(t).toFixed(1) + " " + Y(g).toFixed(1);
    ant = t;
  }
  const css = getComputedStyle(document.documentElement);
  const cor = (n) => css.getPropertyValue(n).trim();
  let svg = '<svg viewBox="0 0 ' + W + " " + H + '" preserveAspectRatio="none" role="img" aria-label="Volume de refill no período">';
  for (let g = 0; g <= ymax + 1e-9; g += 0.5) {
    svg += '<line x1="' + P.l + '" x2="' + (W - P.r) + '" y1="' + Y(g) + '" y2="' + Y(g) + '" stroke="' + cor("--line") + '"/>' +
      '<text x="' + (P.l - 6) + '" y="' + (Y(g) + 4) + '" text-anchor="end" font-size="11" fill="' + cor("--muted") + '">' + String(g).replace(".", ",") + "</text>";
  }
  const lim = estado && estado.monitor.limite;
  if (lim && lim < ymax) {
    svg += '<line x1="' + P.l + '" x2="' + (W - P.r) + '" y1="' + Y(lim) + '" y2="' + Y(lim) +
      '" stroke="' + cor("--bad") + '" stroke-dasharray="4 4" opacity=".6"/>';
  }
  for (let k = 0; k <= 4; k++) {
    const t = ini + (k * (fim - ini)) / 4;
    const anc = k === 0 ? "start" : k === 4 ? "end" : "middle";
    svg += '<text x="' + X(t) + '" y="' + (H - 6) + '" text-anchor="' + anc + '" font-size="11" fill="' + cor("--muted") + '">' +
      (k === 4 ? "agora" : horas > 24 ? dia(t) : hhmm(t)) + "</text>";
  }
  svg += '<path d="' + d.trim() + '" fill="none" stroke="' + cor("--accent") +
    '" stroke-width="2" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>';
  if (h.refills.length < 300) {
    for (const r of h.refills) {
      if (r.t < ini) continue;
      const y = r.ok ? (r.after != null ? r.after : ymax) : (r.before != null ? r.before : 0);
      svg += '<circle cx="' + X(r.t) + '" cy="' + Y(Math.min(y, ymax)) + '" r="' + (horas > 24 ? 2.5 : 3.5) + '" fill="' +
        cor(r.ok ? "--ok" : "--bad") + '" stroke="' + cor("--card") + '" stroke-width="1.5"/>';
    }
  }
  svg += '<line class="cross" y1="' + P.t + '" y2="' + (H - P.b) + '" stroke="' + cor("--muted") + '" style="display:none"/></svg>';
  box.innerHTML = svg + '<div class="tip"></div>';
  const el = box.querySelector("svg"), tip = box.querySelector(".tip"), cross = box.querySelector(".cross");
  el.addEventListener("mousemove", (ev) => {
    const b = el.getBoundingClientRect();
    const t = ini + ((ev.clientX - b.left) / b.width) * (fim - ini);
    const p = pts.reduce((a, c) => (Math.abs(c[0] - t) < Math.abs(a[0] - t) ? c : a));
    cross.style.display = ""; cross.setAttribute("x1", X(p[0])); cross.setAttribute("x2", X(p[0]));
    tip.style.display = "block";
    tip.textContent = dia(p[0]) + " · " + gb(p[1]) + " GB";
    tip.style.left = Math.min((X(p[0]) / W) * b.width + 10, b.width - tip.offsetWidth - 4) + "px";
    tip.style.top = Math.max(0, (Y(p[1]) / H) * b.height - 32) + "px";
  });
  el.addEventListener("mouseleave", () => { tip.style.display = "none"; cross.style.display = "none"; });
}

async function puxaHist(horas, box) {
  try {
    const h = await api("/api/historico?horas=" + horas);
    histCache[horas] = h;
    desenhaGrafico(box, h, horas);
    const r = h.resumo;
    const txt = r.refills_periodo + " refill" + (r.refills_periodo === 1 ? "" : "s") +
      (r.falhas_periodo ? " · " + r.falhas_periodo + " falha" + (r.falhas_periodo > 1 ? "s" : "") : "") +
      (r.latencia_media ? " · entram em ~" + Math.round(r.latencia_media) + " s" : "");
    if (box.id === "chart1") {
      $("#c-resumo").textContent = txt;
      const ok = h.refills.filter((x) => x.ok);
      if (ok.length && !(estado && estado.monitor.ultimo_refill)) {
        const u = ok[ok.length - 1];
        $("#s-ult").textContent = hhmm(u.t); $("#s-ult").dataset.hist = "1";
        $("#s-ult-d").textContent = (u.latencia ? "entrou em " + Math.round(u.latencia) + " s · " : "") + (u.why || "");
      }
    } else {
      $("#h-resumo").textContent = txt;
      $("#h-lista").innerHTML = h.refills.slice().reverse().slice(0, 150).map((r) => {
        const g = r.before != null && r.after != null ? gb(r.before) + " → " + gb(r.after) + " GB"
          : r.before != null ? "com " + gb(r.before) + " GB" : "";
        return '<li><span class="t">' + dia(r.t) + "</span>" +
          (r.ok ? '<span class="okc" title="deu certo">✓</span>' : '<span class="badc" title="falhou">✗</span>') +
          "<span>" + esc(r.ok ? (g || "refill") + (r.why ? " — " + r.why : "") + (r.latencia ? " · " + Math.round(r.latencia) + " s" : "")
            : "não entrou" + (g ? " (" + g + ")" : "")) + "</span></li>";
      }).join("") || '<li class="muted">nenhum refill no período</li>';
    }
  } catch (e) { /* tenta de novo no próximo ciclo */ }
}
for (const b of $$("#range button")) {
  b.addEventListener("click", () => {
    horasHist = Number(b.dataset.h);
    for (const o of $$("#range button")) o.setAttribute("aria-pressed", String(o === b));
    puxaHist(horasHist, $("#chart2"));
  });
}
addEventListener("resize", () => {
  if (histCache[24] && !$("#painel").hidden) desenhaGrafico($("#chart1"), histCache[24], 24);
  if (histCache[horasHist] && !$("#historico").hidden) desenhaGrafico($("#chart2"), histCache[horasHist], horasHist);
});

// ------------------------------------------------------------------------- log
function classe(l) {
  if (/falh|erro|não consegui|recusad|caiu|NÃO/i.test(l)) return "e";
  if (/confirmado|renovado|login por senha ok|aceito|sessão nova/i.test(l)) return "s";
  if (/aviso|pausa|cooldown|espera/i.test(l)) return "w";
  return "";
}
function desenhaLog() {
  const f = $("#log-filtro").value.trim().toLowerCase();
  const el = $("#log");
  const linhas = f ? logLinhas.filter((l) => l.toLowerCase().includes(f)) : logLinhas;
  el.innerHTML = linhas.map((l) => { const c = classe(l); return c ? '<span class="' + c + '">' + esc(l) + "</span>" : esc(l); }).join("\n");
  if ($("#log-seguir").checked) el.scrollTop = el.scrollHeight;
}
async function puxaLog() {
  try {
    const r = await api("/api/log?desde=" + logSeq);
    if (r.reset) logLinhas = [];
    if (r.linhas.length || r.reset) {
      logLinhas = logLinhas.concat(r.linhas).slice(-3000);
      desenhaLog();
    }
    logSeq = r.seq;
  } catch (e) {}
}
$("#log-filtro").addEventListener("input", desenhaLog);

// --------------------------------------------------------------------- ajustes
function desenhaAjustes() {
  const a = ajustes;
  $("#avisos").innerHTML = a.avisos.length
    ? '<div class="card">' + a.avisos.map((x) => '<div class="notice">' + esc(x) + "</div>").join("") + "</div>" : "";
  let h = "";
  for (const g of a.grupos) {
    h += '<div class="card"><h2>' + esc(g) + "</h2>";
    for (const c of a.campos.filter((x) => x.group === g)) {
      const trava = c.origem === "ambiente";
      h += '<div class="field" id="f-' + c.key + '"><div class="lbl"><b>' + esc(c.label) + "</b><small>" + esc(c.help) + "</small>" +
        (trava ? '<div class="lock">Definido pela variável ' + esc(c.env) + " — o ambiente tem a última palavra.</div>" : "") +
        '<div class="err" id="e-' + c.key + '" hidden></div></div><div class="ctl">';
      if (c.type === "bool") {
        h += '<input type="checkbox" class="switch" id="c-' + c.key + '"' + (c.valor ? " checked" : "") + (trava ? " disabled" : "") + ' aria-label="' + esc(c.label) + '">';
      } else if (c.type === "str") {
        h += '<input type="text" class="wide" id="c-' + c.key + '" value="' + esc(c.valor || "") + '"' + (trava ? " disabled" : "") + ' spellcheck="false" aria-label="' + esc(c.label) + '">';
      } else {
        h += '<input type="number" id="c-' + c.key + '" value="' + esc(c.valor) + '" step="' + (c.type === "float" ? "0.05" : "1") +
          '" min="' + esc(c.min) + '" max="' + esc(c.max) + '"' + (trava ? " disabled" : "") + ' aria-label="' + esc(c.label) + '"><span class="un">' + esc(c.unit_label) + "</span>";
      }
      h += "</div></div>";
    }
    h += "</div>";
  }
  $("#cfg").innerHTML = h;
}
async function puxaAjustes() {
  try { ajustes = await api("/api/ajustes"); desenhaAjustes(); } catch (e) {}
}
async function salvar() {
  if (!ajustes || ocupado) return;
  ocupado = true;
  const valores = {};
  for (const c of ajustes.campos) {
    if (c.origem === "ambiente") continue;
    const el = $("#c-" + c.key);
    valores[c.key] = c.type === "bool" ? el.checked : el.value;
  }
  try {
    for (const c of ajustes.campos) { $("#e-" + c.key).hidden = true; $("#f-" + c.key).classList.remove("bad"); }
    const r = await api("/api/ajustes", { valores });
    let primeiro = null;
    for (const k in r.erros || {}) {
      const e = $("#e-" + k); e.textContent = r.erros[k]; e.hidden = false;
      $("#f-" + k).classList.add("bad");
      if (!primeiro) primeiro = $("#f-" + k);
    }
    if (primeiro) primeiro.scrollIntoView({ block: "center", behavior: "smooth" });
    toast(r.msg);
    if (r.ok) puxaAjustes();   // com erro, mantém o que a pessoa digitou na tela
  } catch (e) { toast("Falhou: " + e.message); }
  ocupado = false;
  puxaEstado();
}

// --------------------------------------------------------------------- botões
function fechaMenu() { $("#m-pausa").hidden = true; $("#b-pausa").setAttribute("aria-expanded", "false"); }
$("#b-pausa").addEventListener("click", (ev) => {
  ev.stopPropagation();
  const m = $("#m-pausa"); m.hidden = !m.hidden;
  $("#b-pausa").setAttribute("aria-expanded", String(!m.hidden));
});
document.addEventListener("click", fechaMenu);
for (const b of $$("#m-pausa button")) {
  b.addEventListener("click", () => {
    fechaMenu();
    const min = Number(b.dataset.min);
    if (!min && !confirm("Pausar sem prazo? O Tasker só volta quando você clicar em Retomar.")) return;
    acao("pausar", { minutos: min });
  });
}
$("#b-checar").addEventListener("click", () => acao("checar"));
$("#b-retomar").addEventListener("click", () => acao("retomar"));
$("#b-retomar2").addEventListener("click", () => acao("retomar"));
$("#b-ligar").addEventListener("click", () => acao("ligar"));
$("#b-monitor").addEventListener("click", () => acao(estado && estado.monitor.ligado ? "desligar" : "ligar"));
$("#b-teste").addEventListener("click", () => acao("testar_aviso"));
$("#b-atalho").addEventListener("click", () => acao("atalho"));
$("#sw-auto").addEventListener("change", (ev) => acao("autostart", { ligado: ev.target.checked }));
$("#b-sair").addEventListener("click", async () => {
  if (!confirm("Encerrar o Tasker? Ninguém vai vigiar o volume até você abrir de novo.")) return;
  await acao("sair");
  document.body.innerHTML = '<main><div class="card"><h2>Tasker encerrado</h2><p>Pode fechar esta janela. Pra voltar, use o atalho.</p></div></main>';
});
$("#b-cred-off").addEventListener("click", () => { if (confirm("Apagar número e senha guardados?")) acao("credenciais_apagar"); });
$("#b-relogin").addEventListener("click", () => { forcarLogin = true; abre("painel"); desenhaEstado(); $("#l-num").focus(); });
$("#b-nav").addEventListener("click", () => {
  const n = estado && estado.login_navegador;
  if (n && (n.estado === "abrindo" || n.estado === "aguardando")) acao("login_cancelar");
  else acao("login_navegador");
});
$("#b-entrar").addEventListener("click", async () => {
  const r = await acao("login_senha", { msisdn: $("#l-num").value, senha: $("#l-senha").value,
                                         lembrar: $("#l-lembrar").checked });
  if (r && r.ok) { $("#l-senha").value = ""; forcarLogin = false; puxaEstado(); }
});
$("#l-senha").addEventListener("keydown", (ev) => { if (ev.key === "Enter") $("#b-entrar").click(); });
$("#b-salvar").addEventListener("click", salvar);
$("#b-padroes").addEventListener("click", async () => {
  if (!confirm("Voltar todos os ajustes pro padrão?")) return;
  await acao("padroes"); puxaAjustes();
});

// ------------------------------------------------------------------------ ciclo
if (!K) $("#semchave").hidden = false;
let abaSalva = "painel";
try { abaSalva = localStorage.getItem("aba") || "painel"; } catch (e) {}
abre(["painel", "historico", "log-sec", "ajustes"].includes(abaSalva) ? abaSalva : "painel");
puxaEstado();
puxaHist(24, $("#chart1"));
setInterval(puxaEstado, 2000);
setInterval(tick, 1000);
setInterval(() => { if (!$("#log-sec").hidden) puxaLog(); }, 2000);
setInterval(() => {
  if (!$("#painel").hidden) puxaHist(24, $("#chart1"));
  if (!$("#historico").hidden) puxaHist(horasHist, $("#chart2"));
}, 30000);
