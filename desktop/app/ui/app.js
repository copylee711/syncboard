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
  // Phones running the SyncBoard app; file-manager apps (TwinFile) only browse this PC.
  const phones = online.filter((p) => !p.app);
  const appOnly = online.length > 0 && phones.length === 0;
  const dot = $("statusDot");
  if (online.length) {
    dot.className = "dot on";
    $("statusLabel").textContent = online.length === 1 ? `已连接 · ${online[0].addr || ""}` : `已连接 ${online.length} 台设备`;
    $("statusTitle").textContent = online.length === 1 ? online[0].name : online.map((p) => p.name).join("、");
    $("statusSub").textContent = appOnly
      ? "TwinFile 可以浏览这台电脑的文件。剪贴板同步、发送文件、文件管理和手机控制需要手机上安装 SyncBoard App"
      : s.paused ? "同步已暂停，点右上角恢复" : "复制的文字和图片会自动出现在另一端";
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
  $("sendBtn").disabled = phones.length === 0;
  $("filesBtn").disabled = appOnly;
  // The panel controls the phone picked in the device list (the first online one by default).
  renderPhone(panelTarget(), appOnly);

  const peers = $("peers");
  if (!s.peers.length) {
    peers.innerHTML = `<div class="empty">没有设备</div>`;
  } else {
    peers.innerHTML = s.peers
      .map(
        (p) => `<div class="item${p.connected && online.length > 1 ? " pickable" : ""}${p.id === phoneStatus?.id && online.length > 1 ? " picked" : ""}" data-pick="${esc(p.id)}">
          <div class="meta">
            <div class="name">${esc(p.name)}${p.alias ? `<span class="model">${esc(p.model)}</span>` : ""}${p.app === "twinfile" ? `<span class="model">TwinFile</span>` : ""}</div>
            <div class="sub"><span class="dot ${p.connected ? "on" : ""}"></span>${p.connected && p.app ? "在线 · 只能浏览电脑文件，其他功能需要 SyncBoard App" : p.connected ? "在线" : p.keyMismatch ? "配对信息不一致，请取消配对后重新扫码" : "离线"}${p.addr && !(p.keyMismatch && !p.connected) ? " · " + esc(p.addr) : ""}</div>
          </div>
          ${p.connected && p.app ? `<button class="btn small" disabled>发送文件</button>` : ""}
          ${p.connected && !p.app ? `<button class="btn small" data-send="${esc(p.id)}">发送文件</button>
          <button class="icon-btn" title="停止手机服务" data-stop="${esc(p.id)}" data-name="${esc(p.name)}"><svg viewBox="0 0 24 24" style="width:15px;height:15px"><path d="M12 3v8M6.3 6.8a8 8 0 1011.4 0"/></svg></button>` : ""}
          <button class="icon-btn" title="备注" data-alias="${esc(p.id)}"><svg viewBox="0 0 24 24" style="width:15px;height:15px"><path d="M4 20h4L19 9l-4-4L4 16v4zM13.5 6.5l4 4"/></svg></button>
          <button class="icon-btn" title="取消配对" data-unpair="${esc(p.id)}" data-name="${esc(p.name)}"><svg viewBox="0 0 12 12"><path d="M2.5 2.5l7 7m0-7l-7 7"/></svg></button>
        </div>`
      )
      .join("");
  }

  renderTransfers();

  document.querySelectorAll("[data-setting]").forEach((el) => (el.checked = !!s.settings[el.dataset.setting]));
  document.querySelectorAll(".notif-sub").forEach((el) => (el.hidden = !s.settings.phone_notifications));
  $("autostart").checked = s.autostart;
  $("sendto").checked = s.sendto;
  $("downloadDir").textContent = s.downloadDir;
  if (!$("updateBadge").disabled) {
    $("updateBadge").hidden = !available;
    $("updateBadge").textContent = available ? `有新版本 v${available}` : "";
  }
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
// Block bodies: an on* handler that returns false cancels the click (and form submits).
$("pairSheet").onclick = (e) => {
  if (e.target.id === "pairSheet") closePair();
};
$("acceptBtn").onclick = () => answer(true);
$("rejectBtn").onclick = () => answer(false);
$("sendBtn").onclick = () => call("pick_and_send", { peer: null });
$("folderBtn2").onclick = () => call("open_downloads");
$("downloadPick").onclick = () => call("pick_download_dir");
$("filesBtn").onclick = () => call("open_files");
$("testNotify").onclick = () => call("test_notification").then(() => toast("已发送，右下角应该能看到")).catch(() => {});
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
    $("updateBadge").disabled = true;
    $("updateBadge").textContent = "正在下载…";
    $("updateBar").firstElementChild.style.width = "0%";
    $("updateBar").hidden = false;
    await call("install_update", { version: available });
  } catch {
    // call() already showed the error
  } finally {
    $("updateBar").hidden = true;
    $("updateBadge").disabled = false;
    render();
    btn.disabled = false;
    btn.textContent = available ? `更新到 v${available}` : "检查更新";
    btn.classList.toggle("ink", !!available);
  }
};
// Shown when the startup check found a newer release: one click to update.
$("updateBadge").onclick = () => {
  if (!available || $("updateBtn").disabled) return;
  if (!confirm(`下载并安装 v${available}？完成后 SyncBoard 会自动重启。`)) return;
  $("updateBtn").scrollIntoView({ behavior: "smooth", block: "center" });
  $("updateBtn").click();
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
  const al = e.target.closest("[data-alias]");
  if (al) return openAlias(state.peers.find((p) => p.id === al.dataset.alias));
  const un = e.target.closest("[data-unpair]");
  if (un && confirm(`取消与「${un.dataset.name}」的配对？之后需要重新扫码。`)) {
    await call("unpair", { id: un.dataset.unpair });
    refresh();
  }
};
$("transfers").onclick = (e) => {
  if (e.target.closest("[data-open]")) call("open_downloads");
};

listen("state", (e) => {
  state = e.payload;
  render();
});

listen("updateProgress", (e) => {
  const { done, total } = e.payload;
  const mb = (n) => (n / 1048576).toFixed(1);
  if (total > 0) {
    $("updateBar").firstElementChild.style.width = `${Math.min(100, (done * 100) / total)}%`;
    $("updateBtn").textContent = $("updateBadge").textContent = `正在下载 ${Math.floor((done * 100) / total)}%`;
  } else {
    $("updateBtn").textContent = $("updateBadge").textContent = `正在下载 ${mb(done)} MB`;
  }
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

// Look for a newer release quietly at startup; errors (offline) are ignored.
setTimeout(() => {
  invoke("check_update").then((r) => {
    if (r.newer) {
      available = r.latest;
      if (state) render();
    }
  }).catch(() => {});
}, 3000);

// ---------- phone panel (status and controls) ----------

let phoneStatus = null; // { id, ... } of the phone the panel shows
let panelPeer = null;
function panelTarget() {
  // File-manager apps (TwinFile) have no phone controls.
  const online = (state?.peers || []).filter((p) => p.connected && !p.app);
  return online.find((p) => p.id === panelPeer) || online[0];
}
$("peers").addEventListener("click", (e) => {
  const it = e.target.closest("[data-pick]");
  if (!it || e.target.closest("button") || !it.classList.contains("pickable")) return;
  panelPeer = it.dataset.pick;
  render();
});
let statusTimer = null;
let ringing = false;

function renderPhone(p, appOnly = false) {
  $("phonePanel").hidden = !p && !appOnly;
  if (!p) {
    phoneStatus = null;
    if (appOnly) {
      // Show what a SyncBoard phone would offer, greyed out.
      $("phoneStats").innerHTML = `<span>手机上安装并启动 SyncBoard App 后，可以在这里查看电量、找手机、控制播放和锁屏</span>`;
      $("phonePanel").querySelectorAll(".ctl").forEach((b) => (b.disabled = true));
    }
    return;
  }
  if (phoneStatus?.id !== p.id) {
    phoneStatus = { id: p.id };
    refreshPhoneStatus();
  }
  const st = phoneStatus;
  const parts = [];
  if (st.battery != null) {
    const cls = st.charging || st.full ? "charging" : st.battery <= 15 ? "low" : "";
    parts.push(`<span><span class="battery ${cls}"><i style="width:${st.battery}%"></i></span>${st.battery}%${st.full ? " · 已充满" : st.charging ? " · 充电中" : ""}</span>`);
  }
  if (st.storageTotal) parts.push(`<span>存储可用 ${fmtSize(st.storageFree)} / ${fmtSize(st.storageTotal)}</span>`);
  if (st.model) parts.push(`<span>${esc(st.model)} · Android ${esc(st.android)}</span>`);
  if (st.unsupported) parts.push(`<span>更新手机 App 后可在这里查看电量、控制手机</span>`);
  $("phoneStats").innerHTML = parts.join("");
  $("phonePanel").querySelectorAll(".ctl").forEach((b) => (b.disabled = !!st.unsupported));
  $("ringBtn").classList.toggle("on", ringing);
  $("ringBtn").querySelector("span").textContent = ringing ? "停止响铃" : "找手机";
}

async function refreshPhoneStatus() {
  const id = phoneStatus?.id;
  if (!id) return;
  try {
    const r = await invoke("phone_fs", { peer: id, op: "status", args: {} });
    if (phoneStatus?.id === id) phoneStatus = { id, ...r };
  } catch (e) {
    // An older phone app doesn't answer.
    if (phoneStatus?.id === id) phoneStatus = { id, unsupported: /更新/.test(String(e)) };
  }
  if (state) renderPhone(panelTarget());
}

function phoneCmd(op, args) {
  if (!phoneStatus?.id) return Promise.reject();
  return call("phone_fs", { peer: phoneStatus.id, op, args });
}

$("phonePanel").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-ctl]");
  if (!b) return;
  const a = b.dataset.ctl;
  if (a === "ring") {
    const stop = ringing;
    ringing = !ringing;
    renderPhone(panelTarget());
    await phoneCmd("ring", { stop }).catch(() => (ringing = false));
    if (ringing) {
      toast("手机正在响铃，找到后可在手机上按音量键或在这里停止");
      clearTimeout(b._t);
      b._t = setTimeout(() => {
        ringing = false;
        renderPhone(panelTarget());
      }, 60000);
    }
    return;
  }
  if (a === "link") {
    const last = (await invoke("clip_history").catch(() => [])).find((h) => /^https?:\/\//.test(h.text.trim()));
    $("linkInput").value = last ? last.text.trim() : "";
    $("linkSheet").hidden = false;
    setTimeout(() => $("linkInput").select());
    return;
  }
  phoneCmd("media", { action: a }).catch(() => {});
});

$("linkForm").onsubmit = async (e) => {
  e.preventDefault();
  let url = $("linkInput").value.trim();
  if (!url) return;
  if (!/^https?:\/\//.test(url)) url = "https://" + url;
  $("linkSheet").hidden = true;
  await phoneCmd("open", { url }).then(() => toast("已在手机上打开")).catch(() => {});
};
$("linkCancel").onclick = () => ($("linkSheet").hidden = true);
$("linkSheet").onclick = (e) => {
  if (e.target.id === "linkSheet") $("linkSheet").hidden = true;
};

// ---------- clipboard history ----------

let historyAll = false;
function fmtAgo(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.floor(s / 3600)} 小时前`;
  return new Date(ms).toLocaleDateString();
}
async function refreshHistory() {
  const items = await invoke("clip_history").catch(() => []);
  $("historySection").hidden = !items.length;
  const shown = historyAll ? items : items.slice(0, 5);
  $("history").innerHTML =
    shown.map((h, i) => `<div class="h-item" data-i="${i}" title="点击复制"><div class="h-text">${esc(h.text)}</div><div class="h-meta">${h.from === "phone" ? "手机" : "电脑"}<br>${fmtAgo(h.at)}</div></div>`).join("") +
    (items.length > 5 ? `<button class="h-more" id="historyMore">${historyAll ? "收起" : `显示全部 ${items.length} 条`}</button>` : "");
  $("history").querySelectorAll(".h-item").forEach((el) => {
    el.onclick = async () => {
      await call("clip_recopy", { text: shown[+el.dataset.i].text });
      toast("已复制到电脑和手机");
      refreshHistory();
    };
  });
  const more = $("historyMore");
  if (more) more.onclick = () => ((historyAll = !historyAll), refreshHistory());
}
// Keeps transfers still running.
$("transfersClear").onclick = async () => {
  await call("transfers_clear");
  state.transfers = state.transfers.filter((t) => t.type === "fileProgress");
  renderTransfers();
};
async function openApps() {
  const apps = await call("notif_apps");
  $("appsList").innerHTML = apps.length
    ? apps
        .map(
          (a) => `<label class="row"><span>${esc(a.name)}<small>${esc(a.pkg)}</small></span><span class="switch"><input type="checkbox" data-pkg="${esc(a.pkg)}" ${a.muted ? "" : "checked"} /><i></i></span></label>`
        )
        .join("")
    : `<p class="fine">还没有收到过手机通知。手机上开启「手机通知同步」后，发来通知的 App 会出现在这里。</p>`;
  $("appsSheet").hidden = false;
}
$("appsBtn").onclick = () => {
  openApps();
};
$("appsList").onchange = (e) => {
  const pkg = e.target.dataset.pkg;
  if (pkg) call("set_app_muted", { pkg, muted: !e.target.checked });
};
$("appsDone").onclick = () => {
  $("appsSheet").hidden = true;
};
$("appsSheet").onclick = (e) => {
  if (e.target.id === "appsSheet") $("appsSheet").hidden = true;
};

let aliasPeer = null;
function openAlias(p) {
  if (!p) return;
  aliasPeer = p;
  $("aliasInput").value = p.alias || "";
  $("aliasInput").placeholder = p.model;
  $("aliasHint").textContent = `型号：${p.model}。留空则显示型号。`;
  $("aliasSheet").hidden = false;
  setTimeout(() => $("aliasInput").select());
}
$("aliasForm").onsubmit = async (e) => {
  e.preventDefault();
  $("aliasSheet").hidden = true;
  if (aliasPeer) await call("set_alias", { id: aliasPeer.id, alias: $("aliasInput").value });
};
$("aliasCancel").onclick = () => {
  $("aliasSheet").hidden = true;
};
$("aliasSheet").onclick = (e) => {
  if (e.target.id === "aliasSheet") $("aliasSheet").hidden = true;
};
$("historyClear").onclick = async () => {
  await call("clip_history_clear");
  refreshHistory();
};

// Only while this window exists (it is destroyed in light mode): no background polling.
function tick() {
  if (document.visibilityState !== "visible") return;
  refreshHistory();
}
setInterval(tick, 4000);
statusTimer = setInterval(() => document.visibilityState === "visible" && refreshPhoneStatus(), 30000);
window.addEventListener("focus", () => {
  refreshHistory();
  refreshPhoneStatus();
});
refreshHistory();
