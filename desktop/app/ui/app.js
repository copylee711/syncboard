const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

let state = null;
let qrTimer = null;
let currentReq = null;
let available = null; // newer release version found by "检查更新"

const PHONE_ICON = '<svg viewBox="0 0 24 24"><rect x="7" y="3" width="10" height="18" rx="2.5"/><path d="M11 18h2"/></svg>';

function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function fmtSize(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

function toast(text, kind = "") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = text;
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), kind === "error" ? 5000 : 2600);
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    toast(String(e), "error");
    throw e;
  }
}

function applyTheme(theme) {
  const t = theme || "system";
  document.documentElement.dataset.theme = t;
  document.querySelectorAll("[data-theme-opt]").forEach((b) => b.classList.toggle("on", b.dataset.themeOpt === t));
}

function render() {
  const s = state;
  applyTheme(s.settings.theme);
  $("syncToggle").checked = !s.paused;
  // A pause switch, not a status: nothing to pause until a phone is paired.
  $("syncPill").hidden = !s.peers.length;
  $("syncLabel").textContent = s.paused ? "同步已暂停" : "同步已开启";

  const online = s.peers.filter((p) => p.connected);
  const dot = $("statusDot");
  if (online.length) {
    dot.className = "dot on";
    $("statusLabel").textContent = online.length === 1 ? `已连接 · ${online[0].addr || ""}` : `已连接 ${online.length} 台设备`;
    $("statusTitle").textContent = online.length === 1 ? online[0].name : online.map((p) => p.name).join("、");
    $("statusSub").textContent = s.paused ? "同步已暂停，点右上角恢复" : "复制的文字和图片会自动出现在另一端";
  } else if (s.peers.length) {
    dot.className = "dot wait";
    $("statusLabel").textContent = "未连接";
    $("statusTitle").textContent = "等待手机连接";
    $("statusSub").textContent = "确认手机和电脑在同一网络，且手机端服务正在运行";
  } else {
    dot.className = "dot";
    $("statusLabel").textContent = "未配对";
    $("statusTitle").textContent = "还没有配对的手机";
    $("statusSub").textContent = "点「添加手机」，用手机上的 SyncBoard 扫码";
  }
  $("sendBtn").disabled = online.length === 0;

  const peers = $("peers");
  if (!s.peers.length) {
    peers.innerHTML = `<div class="empty">没有设备</div>`;
  } else {
    peers.innerHTML = s.peers
      .map(
        (p) => `<div class="item">
          <div class="meta">
            <div class="name">${esc(p.name)}</div>
            <div class="sub"><span class="dot ${p.connected ? "on" : ""}"></span>${p.connected ? "在线" : p.keyMismatch ? "配对信息不一致，请取消配对后重新扫码" : "离线"}${p.addr && !(p.keyMismatch && !p.connected) ? " · " + esc(p.addr) : ""}</div>
          </div>
          ${p.connected ? `<button class="btn small" data-send="${esc(p.id)}">发送文件</button>
          <button class="icon-btn" title="停止手机服务" data-stop="${esc(p.id)}" data-name="${esc(p.name)}"><svg viewBox="0 0 24 24" style="width:15px;height:15px"><path d="M12 3v8M6.3 6.8a8 8 0 1011.4 0"/></svg></button>` : ""}
          <button class="icon-btn" title="取消配对" data-unpair="${esc(p.id)}" data-name="${esc(p.name)}"><svg viewBox="0 0 12 12"><path d="M2.5 2.5l7 7m0-7l-7 7"/></svg></button>
        </div>`
      )
      .join("");
  }

  renderTransfers();

  document.querySelectorAll("[data-setting]").forEach((el) => (el.checked = !!s.settings[el.dataset.setting]));
  $("autostart").checked = s.autostart;
  $("sendto").checked = s.sendto;
  $("downloadDir").textContent = s.downloadDir;
  $("versionInfo").textContent = available ? `v${s.version} · 有新版本 v${available}` : `v${s.version}`;
  $("adbPath").textContent = s.adb ? `adb：${s.adb}` : "未找到 adb";
  $("ips").textContent = s.ips.join(" 或 ") || "（未检测到局域网地址）";

  if (s.pairRequests.length && !currentReq) showPairRequest(s.pairRequests[0]);
}

function renderTransfers() {
  const list = state.transfers;
  $("transfersSection").hidden = list.length === 0;
  $("transfers").innerHTML = list
    .map((t) => {
      const dir = t.incoming ? "收到" : "发送";
      let sub, bar = "";
      if (t.type === "fileProgress") {
        const pct = t.total ? Math.floor((t.done / t.total) * 100) : 0;
        sub = `${dir}中 · ${fmtSize(t.done)} / ${fmtSize(t.total)}`;
        bar = `<div class="bar"><i style="width:${pct}%"></i></div>`;
      } else if (t.type === "fileDone") {
        sub = t.incoming ? "已收到 · 点击打开文件夹" : "已发送";
      } else {
        sub = `${dir}失败 · ${esc(t.reason)}`;
      }
      return `<div class="item" ${t.type === "fileDone" && t.incoming ? 'data-open="1" style="cursor:pointer"' : ""}>
        <div class="meta"><div class="name">${esc(t.name)}</div><div class="sub">${sub}</div>${bar}</div>
      </div>`;
    })
    .join("");
}

async function refresh() {
  state = await invoke("get_state");
  render();
}

// ---------- adding a phone ----------

let tab = "install";

function setStatus(stage, text) {
  const el = $("setupStatus");
  el.hidden = !text;
  el.className = `setup-status ${stage || ""}`;
  $("setupText").textContent = text || "";
}

async function showTab(name) {
  tab = name;
  document.querySelectorAll(".stepper li").forEach((li) => li.classList.toggle("on", li.dataset.step === (name === "install" ? "1" : "2")));
  document.querySelectorAll("[data-pane]").forEach((p) => (p.hidden = p.dataset.pane !== name));
  clearInterval(qrTimer);
  setStatus("", "");
  await invoke("setup_cancel");
  if (name === "wifi") {
    $("wifiQr").innerHTML = "";
    const load = async () => {
      const r = await call("setup_wifi");
      $("wifiQr").innerHTML = r.svg;
      setStatus("", "等待手机扫码…");
    };
    await load();
    // The QR's pairing token lasts 30 minutes; replacing it abandons a scan in progress.
    // Not while a phone is mid-pairing: a new QR restarts the flow.
    qrTimer = setInterval(() => tab === "wifi" && !$("setupStatus").classList.contains("progress") && load(), 25 * 60 * 1000);
  } else if (name === "usb") {
    await call("setup_usb");
  } else if (name === "install") {
    if (!$("apkQr").innerHTML) {
      const r = await call("apk_qr");
      $("apkQr").innerHTML = r.svg;
      $("apkName").textContent = $("apkName2").textContent = r.apk;
    }
  } else if (name === "hotspot") {
    $("hotspotQr").innerHTML = "";
    try {
      const h = await call("hotspot_start");
      $("hotspotQr").innerHTML = h.svg;
      $("hotspotName").textContent = h.ssid;
    } catch {
      showTab("wifi");
    }
  }
}

function openPair() {
  $("pairSheet").hidden = false;
  // One linear path: install the app, then scan the wireless debugging code.
  showTab("install");
}

function closePair() {
  if ($("pairSheet").hidden) return;
  $("pairSheet").hidden = true;
  clearInterval(qrTimer);
  invoke("setup_cancel");
  invoke("hotspot_stop");
}

document.querySelectorAll("[data-go]").forEach((b) => (b.onclick = () => showTab(b.dataset.go)));
$("toConnect").onclick = () => showTab("wifi");

listen("setup", (e) => {
  const { stage, text } = e.payload;
  setStatus(stage, text);
  if (stage === "done") setTimeout(closePair, 1500);
});

function showPairRequest(r) {
  currentReq = r;
  $("codeTitle").textContent = `「${r.name}」请求配对`;
  $("codeDigits").textContent = r.code;
  $("codeSheet").hidden = false;
}

async function answer(accept) {
  if (!currentReq) return;
  const req = currentReq.req;
  currentReq = null;
  $("codeSheet").hidden = true;
  await call("answer_pair", { req, accept });
  refresh();
}

// ---------- events ----------

const win = window.__TAURI__.window.getCurrentWindow();
$("minBtn").onclick = () => win.minimize();
$("closeBtn").onclick = () => win.close();
$("addBtn").onclick = openPair;
$("githubBtn").onclick = $("releasesLink").onclick = () => call("open_releases");
$("closePair").onclick = closePair;
$("pairSheet").onclick = (e) => e.target.id === "pairSheet" && closePair();
$("acceptBtn").onclick = () => answer(true);
$("rejectBtn").onclick = () => answer(false);
$("sendBtn").onclick = () => call("pick_and_send", { peer: null });
$("folderBtn").onclick = $("folderBtn2").onclick = () => call("open_downloads");
$("syncToggle").onchange = (e) => call("set_paused", { paused: !e.target.checked, minutes: null });
$("autostart").onchange = (e) => call("set_autostart", { on: e.target.checked }).catch(refresh);
$("sendto").onchange = (e) =>
  call("set_sendto", { on: e.target.checked })
    .then(() => e.target.checked && toast("已添加：在文件上右键 → 发送到 → SyncBoard 手机"))
    .catch(refresh);
$("updateBtn").onclick = async () => {
  const btn = $("updateBtn");
  btn.disabled = true;
  try {
    if (!available) {
      btn.textContent = "正在检查…";
      const r = await call("check_update");
      if (!r.newer) return toast(`已是最新版本（v${r.current}）`);
      available = r.latest;
      render();
      if (!confirm(`发现新版本 v${r.latest}（当前 v${r.current}）。现在下载并重启 SyncBoard？`)) return;
    }
    btn.textContent = "正在下载…";
    await call("install_update", { version: available });
  } catch {
    // call() already showed the error
  } finally {
    btn.disabled = false;
    btn.textContent = available ? `更新到 v${available}` : "检查更新";
    btn.classList.toggle("ink", !!available);
  }
};
$("adbBtn").onclick = async () => {
  $("adbBtn").disabled = true;
  try {
    toast(await call("adb_start"));
  } finally {
    $("adbBtn").disabled = false;
  }
};

document.querySelectorAll("[data-theme-opt]").forEach((b) => {
  b.onclick = () => {
    applyTheme(b.dataset.themeOpt);
    call("set_settings", { patch: { theme: b.dataset.themeOpt } });
  };
});

document.querySelectorAll("[data-setting]").forEach((el) => {
  el.onchange = () => call("set_settings", { patch: { [el.dataset.setting]: el.checked } });
});

$("peers").onclick = async (e) => {
  const send = e.target.closest("[data-send]");
  if (send) return call("pick_and_send", { peer: send.dataset.send });
  const stop = e.target.closest("[data-stop]");
  if (stop && confirm(`停止「${stop.dataset.name}」上的 SyncBoard 服务？之后需要在手机上或用数据线重新启动。`)) {
    return call("stop_phone_service", { id: stop.dataset.stop });
  }
  const un = e.target.closest("[data-unpair]");
  if (un && confirm(`取消与「${un.dataset.name}」的配对？之后需要重新扫码。`)) {
    await call("unpair", { id: un.dataset.unpair });
    refresh();
  }
};
$("transfers").onclick = (e) => e.target.closest("[data-open]") && call("open_downloads");

listen("state", (e) => {
  state = e.payload;
  render();
});

listen("toast", (e) => toast(e.payload.text, e.payload.kind));

listen("engine", (e) => {
  const ev = e.payload;
  switch (ev.type) {
    case "paired":
      setStatus("done", `已与「${ev.name}」配对`);
      setTimeout(closePair, 1500);
      refresh();
      break;
    case "pairFailed":
      toast(ev.reason, "error");
      break;
    case "pairRequest":
      refresh();
      break;
    case "keyMismatch":
      toast(`「${ev.name}」的配对信息不一致，请重新配对`, "error");
      refresh();
      break;
    case "peerConnected":
    case "peerDisconnected":
      refresh();
      break;
    case "fileProgress":
    case "fileDone":
    case "fileFailed": {
      state.transfers = [ev, ...state.transfers.filter((t) => t.id !== ev.id)].slice(0, 20);
      renderTransfers();
      if (ev.type === "fileDone" && ev.incoming) toast(`收到文件：${ev.name}`);
      if (ev.type === "fileFailed") toast(`${ev.name}：${ev.reason}`, "error");
      break;
    }
  }
});

// Drag & drop files onto the window to send them.
listen("tauri://drag-enter", () => state?.peers.some((p) => p.connected) && ($("drop").hidden = false));
listen("tauri://drag-leave", () => ($("drop").hidden = true));
listen("tauri://drag-drop", (e) => {
  $("drop").hidden = true;
  const paths = e.payload.paths || [];
  if (paths.length) call("send_paths", { peer: null, paths }).then(() => toast(`正在发送 ${paths.length} 个文件`));
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closePair();
  }
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

refresh();
