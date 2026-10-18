// SyncBoard file manager: this PC on one side, the phone on the other.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);

// ---------- helpers ----------

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

function fmtSize(n) {
  if (n == null) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

function fmtDate(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  const now = new Date();
  const p = (x) => String(x).padStart(2, "0");
  const time = `${p(d.getHours())}:${p(d.getMinutes())}`;
  if (d.toDateString() === now.toDateString()) return `今天 ${time}`;
  const y = new Date(now);
  y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return `昨天 ${time}`;
  return d.getFullYear() === now.getFullYear() ? `${d.getMonth() + 1}月${d.getDate()}日 ${time}` : `${d.getFullYear()}/${p(d.getMonth() + 1)}/${p(d.getDate())}`;
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

const ICONS = {
  pc: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
  phone: '<svg viewBox="0 0 24 24"><rect x="7" y="3" width="10" height="18" rx="2.5"/><path d="M11 18h2"/></svg>',
  back: '<svg viewBox="0 0 24 24"><path d="M15 6l-6 6 6 6"/></svg>',
  fwd: '<svg viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg>',
  up: '<svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6"/></svg>',
  refresh: '<svg viewBox="0 0 24 24"><path d="M20 12a8 8 0 11-2.3-5.7M20 4v5h-5"/></svg>',
  newdir: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/><path d="M12 10.5v5M9.5 13h5"/></svg>',
  eye: '<svg viewBox="0 0 24 24"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>',
  search: '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/></svg>',
  send: '<svg viewBox="0 0 24 24"><path d="M5 12h14m-5-5l5 5-5 5"/></svg>',
  sendBack: '<svg viewBox="0 0 24 24"><path d="M19 12H5m5-5l-5 5 5 5"/></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 00-2-2H6a2 2 0 00-2 2v8a2 2 0 002 2h2"/></svg>',
  cut: '<svg viewBox="0 0 24 24"><circle cx="6" cy="18" r="3"/><circle cx="18" cy="18" r="3"/><path d="M8 16L19 4M16 16L5 4"/></svg>',
  paste: '<svg viewBox="0 0 24 24"><rect x="5" y="4" width="14" height="17" rx="2"/><path d="M9 4h6v3H9z"/></svg>',
  rename: '<svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16z"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>',
  open: '<svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 01-1 1H5a1 1 0 01-1-1V7a1 1 0 011-1h5"/></svg>',
  reveal: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/></svg>',
  link: '<svg viewBox="0 0 24 24"><path d="M10 14a4 4 0 005.7 0l3-3a4 4 0 00-5.7-5.7l-1 1M14 10a4 4 0 00-5.7 0l-3 3a4 4 0 005.7 5.7l1-1"/></svg>',
  explorer: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v2"/><path d="M3 7v10a2 2 0 002 2h6"/><path d="M15 13h6v6M21 13l-7 7"/></svg>',
  star: '<svg viewBox="0 0 24 24"><path d="M12 3.5l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9l-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z"/></svg>',
  x: '<svg viewBox="0 0 12 12"><path d="M2.5 2.5l7 7m0-7l-7 7"/></svg>',
};
const KIND_ICON = {
  dir: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/></svg>',
  drive: '<svg viewBox="0 0 24 24"><rect x="3" y="7" width="18" height="10" rx="2"/><path d="M7 13h.01M11 13h6"/></svg>',
  img: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 9"/></svg>',
  vid: '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M10 9.5v5l4.5-2.5z"/></svg>',
  aud: '<svg viewBox="0 0 24 24"><path d="M9 18V6l11-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/></svg>',
  zip: '<svg viewBox="0 0 24 24"><path d="M6 3h9l4 4v14H6z"/><path d="M11 6h2M11 9h2M11 12h2M11 15h2v3h-2z"/></svg>',
  doc: '<svg viewBox="0 0 24 24"><path d="M6 3h9l4 4v14H6z"/><path d="M9 12h7M9 15.5h7M9 8.5h3"/></svg>',
  apk: '<svg viewBox="0 0 24 24"><rect x="5" y="9" width="14" height="10" rx="2"/><path d="M8 9a4 4 0 018 0M9 5.5L8 4M15 5.5L16 4"/></svg>',
  other: '<svg viewBox="0 0 24 24"><path d="M6 3h9l4 4v14H6z"/><path d="M15 3v4h4"/></svg>',
};
const EXT = {
  img: "jpg jpeg png gif webp bmp heic heif svg ico tif tiff avif",
  vid: "mp4 mkv mov avi webm 3gp m4v flv wmv ts",
  aud: "mp3 flac wav aac m4a ogg opus wma amr",
  zip: "zip rar 7z tar gz bz2 xz tgz",
  doc: "pdf doc docx xls xlsx ppt pptx txt md csv rtf odt epub json xml html log",
  apk: "apk apks xapk",
};
const KIND_BY_EXT = {};
for (const [k, list] of Object.entries(EXT)) for (const e of list.split(" ")) KIND_BY_EXT[e] = k;
const ext = (n) => (n.includes(".") ? n.split(".").pop().toLowerCase() : "");
const kindOf = (e) => (e.drive ? "drive" : e.d ? "dir" : KIND_BY_EXT[ext(e.n)] || "other");

// ---------- state ----------

let state = null; // main app state (peers, theme)
let peer = null; // phone id
let active = "pc";
let clip = null; // { side, paths, mv }
let pcPlaces = null;
let phoneRoots = null;
let queueItems = [];

function connectedPeers() {
  return (state?.peers || []).filter((p) => p.connected);
}

class Pane {
  constructor(side) {
    this.side = side;
    this.el = $(`pane-${side}`);
    this.path = null;
    this.back = [];
    this.fwd = [];
    this.entries = [];
    this.view = [];
    this.sel = new Set();
    this.anchor = null;
    this.sort = { key: "n", dir: 1 };
    this.filter = "";
    this.showHidden = false;
    this.loading = false;
    this.error = null;
    this.editing = false;
    this.seq = 0;
    this.build();
  }

  get isPhone() {
    return this.side === "phone";
  }
  get other() {
    return this.isPhone ? panes.pc : panes.phone;
  }
  sep() {
    return this.isPhone ? "/" : "\\";
  }
  join(name) {
    if (!this.isPhone && this.path === "") return name; // drive list
    const s = this.sep();
    return this.path.endsWith(s) ? this.path + name : this.path + s + name;
  }
  parentOf(p) {
    if (this.isPhone) {
      const vol = this.volumeOf(p);
      if (!vol || p === vol.path) return null;
      return p.slice(0, p.lastIndexOf("/")) || "/";
    }
    if (p === "") return null;
    if (/^[A-Za-z]:\\?$/.test(p)) return "";
    const t = p.replace(/\\$/, "");
    const i = t.lastIndexOf("\\");
    return i <= 2 ? t.slice(0, 3).replace(/\\?$/, "\\") : t.slice(0, i);
  }
  volumeOf(p) {
    return (phoneRoots?.volumes || []).filter((v) => p === v.path || p.startsWith(v.path + "/")).sort((a, b) => b.path.length - a.path.length)[0];
  }

  build() {
    const who = this.isPhone
      ? `<span class="p-who">${ICONS.phone}<span class="who-name" data-r="who">手机</span><span class="dot" data-r="dot"></span></span>`
      : `<span class="p-who">${ICONS.pc}<span class="who-name">此电脑</span></span>`;
    this.el.innerHTML = `
      <div class="p-head">
        <div class="p-title">${who}<span class="grow"></span>
          <label class="search">${ICONS.search}<input data-r="search" placeholder="筛选当前文件夹" spellcheck="false" /></label>
          <button class="tool" data-a="newdir" title="新建文件夹 (Ctrl+Shift+N)">${ICONS.newdir}</button>
          ${this.isPhone ? "" : `<button class="tool" data-a="explorer" title="在资源管理器中打开当前文件夹">${ICONS.explorer}</button>`}
          <button class="tool" data-a="hidden" title="显示隐藏文件">${ICONS.eye}</button>
          <button class="tool" data-a="refresh" title="刷新 (F5)">${ICONS.refresh}</button>
        </div>
        <div class="p-nav">
          <button class="tool" data-a="back" title="后退 (Alt+←)">${ICONS.back}</button>
          <button class="tool" data-a="fwd" title="前进 (Alt+→)">${ICONS.fwd}</button>
          <button class="tool" data-a="up" title="上一级 (Backspace)">${ICONS.up}</button>
          <div class="crumbs" data-r="crumbs"></div>
          <button class="tool" data-a="fav" title="收藏这个文件夹">${ICONS.star}</button>
        </div>
        <div class="places" data-r="places"></div>
      </div>
      <div class="cols">
        <button data-sort="n">名称 <i></i></button><button class="c-date" data-sort="m">修改时间 <i></i></button><button data-sort="s">大小 <i></i></button>
      </div>
      <div class="list-wrap">
        <div class="loading-line" data-r="loading" hidden></div>
        <div class="list" data-r="list" tabindex="0"></div>
        <div class="p-empty" data-r="empty" hidden></div>
      </div>
      <div class="p-foot"><span data-r="count"></span><span class="grow"></span><span data-r="free"></span></div>
      <div class="selbar" data-r="selbar" hidden></div>
      <div class="drop-hint" data-r="drop" hidden></div>`;
    this.r = {};
    this.el.querySelectorAll("[data-r]").forEach((n) => (this.r[n.dataset.r] = n));

    this.el.addEventListener("mousedown", () => setActive(this.side));
    this.el.querySelectorAll("[data-a]").forEach((b) => (b.onclick = () => this.action(b.dataset.a)));
    this.el.querySelectorAll("[data-sort]").forEach((b) => {
      b.onclick = () => {
        const k = b.dataset.sort;
        this.sort = { key: k, dir: this.sort.key === k ? -this.sort.dir : k === "n" ? 1 : -1 };
        this.render();
      };
    });
    // The places row scrolls sideways with the mouse wheel.
    this.r.places.addEventListener("wheel", (e) => {
      if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
        this.r.places.scrollLeft += e.deltaY;
        e.preventDefault();
      }
    }, { passive: false });
    this.r.search.oninput = () => {
      this.filter = this.r.search.value.trim().toLowerCase();
      this.render();
    };
    this.r.search.onkeydown = (e) => {
      if (e.key === "Escape") {
        this.r.search.value = "";
        this.filter = "";
        this.render();
        this.r.list.focus();
      }
      if (e.key === "ArrowDown" || e.key === "Enter") {
        e.preventDefault();
        this.r.list.focus();
        if (!this.sel.size && this.view[0]) this.select(this.view[0].n);
      }
    };
    this.r.crumbs.onclick = (e) => {
      if (e.target === this.r.crumbs || e.target.classList.contains("fill")) this.editPath();
    };

    const list = this.r.list;
    list.onclick = (e) => {
      // The click that ends a box selection.
      if (this.boxed) {
        this.boxed = false;
        return;
      }
      const row = e.target.closest(".row-f");
      if (!row) {
        if (!e.ctrlKey && !e.shiftKey) this.clearSel();
        return;
      }
      this.clickRow(row.dataset.n, e);
    };
    list.ondblclick = (e) => {
      const row = e.target.closest(".row-f");
      if (row) this.open(row.dataset.n);
    };
    list.oncontextmenu = (e) => {
      e.preventDefault();
      const row = e.target.closest(".row-f");
      if (row && !this.sel.has(row.dataset.n)) this.select(row.dataset.n);
      if (!row) this.clearSel();
      showMenu(this, e.clientX, e.clientY);
    };
    list.onscroll = () => hideMenu();

    // Dragging rows to the other pane, onto folders or the breadcrumb: see startDrag.
    // Pressing on a file's icon or name drags it; anywhere else draws a selection box.
    list.onmousedown = (e) => {
      if (e.button !== 0) return;
      const row = e.target.closest(".row-f");
      if (row && e.target.closest(".f-icon, .f-label") && this.path !== "") return startDrag(this, row, e);
      if (e.target === list || row) startBox(this, e);
    };
  }

  byName(n) {
    return this.entries.find((e) => e.n === n);
  }

  selPaths() {
    return [...this.sel].map((n) => this.join(n));
  }

  ready() {
    if (this.isPhone) return !!peer && !!phoneRoots;
    return true;
  }

  async go(path, { push = true } = {}) {
    if (path == null) return;
    if (push && this.path != null && this.path !== path) {
      this.back.push(this.path);
      this.fwd = [];
    }
    this.path = path;
    this.sel.clear();
    this.anchor = null;
    this.r.search.value = "";
    this.filter = "";
    await this.load();
  }

  async load({ keepSel = false } = {}) {
    if (this.path == null) return;
    const seq = ++this.seq;
    this.loading = true;
    this.error = null;
    this.renderHead();
    this.r.loading.hidden = false;
    try {
      let r;
      if (!this.isPhone && this.path === "") {
        r = { entries: (pcPlaces?.drives || []).map((d) => ({ n: d.path, label: d.name, d: true, drive: true })) };
      } else if (this.isPhone) {
        if (!peer) throw new Error("offline");
        r = await invoke("phone_list", { peer, path: this.path });
      } else {
        r = await invoke("local_list", { path: this.path });
      }
      if (seq !== this.seq) return;
      this.entries = r.entries;
      if (!keepSel) this.sel.clear();
      else this.sel = new Set([...this.sel].filter((n) => this.byName(n)));
    } catch (e) {
      if (seq !== this.seq) return;
      this.entries = [];
      this.error = String(e.message || e);
    } finally {
      if (seq === this.seq) {
        this.loading = false;
        this.r.loading.hidden = true;
        this.render();
      }
    }
  }

  // ---- rendering ----

  render() {
    this.renderHead();
    const f = this.filter;
    let v = this.entries.filter((e) => (this.showHidden || !(e.h || e.n.startsWith("."))) && (!f || e.n.toLowerCase().includes(f) || (e.label || "").toLowerCase().includes(f)));
    const { key, dir } = this.sort;
    const coll = new Intl.Collator("zh-CN", { numeric: true, sensitivity: "base" });
    v.sort((a, b) => {
      if (a.d !== b.d) return a.d ? -1 : 1;
      if (key === "n") return dir * coll.compare(a.n, b.n);
      const x = (a[key] || 0) - (b[key] || 0);
      return x ? dir * x : coll.compare(a.n, b.n);
    });
    this.view = v;
    this.el.querySelectorAll("[data-sort]").forEach((b) => {
      b.classList.toggle("on", b.dataset.sort === key);
      b.querySelector("i").textContent = b.dataset.sort === key ? (dir > 0 ? "▲" : "▼") : "";
    });

    const cutSet = clip?.mv && clip.side === this.side ? new Set(clip.paths) : null;
    const html = v
      .map((e) => {
        const k = kindOf(e);
        const thumb = this.isPhone && (k === "img" || k === "vid");
        const size = e.drive ? "" : e.d ? (this.isPhone ? `${e.s} 项` : "") : fmtSize(e.s);
        const cls = ["row-f", this.sel.has(e.n) ? "sel" : "", cutSet?.has(this.join(e.n)) ? "cut" : ""].join(" ");
        return `<div class="${cls}" data-n="${esc(e.n)}">
          <span class="f-name"><span class="f-icon k-${k}"${thumb ? ` data-thumb="${esc(e.n)}" data-m="${e.m}"` : ""}>${KIND_ICON[k]}</span><span class="f-label" title="${esc(e.n)}">${esc(e.label || e.n)}</span></span>
          <span class="f-date">${fmtDate(e.m)}</span><span class="f-size">${size}</span></div>`;
      })
      .join("");
    this.r.list.innerHTML = html;
    if (this.isPhone) this.observeThumbs();

    // Empty / error / offline states.
    let empty = "";
    if (this.isPhone && !peer) {
      const paired = (state?.peers || []).length;
      empty = `<div>${ICONS.phone}<b>手机未连接</b>${paired ? "打开手机上的 SyncBoard，确认手机服务在运行、和电脑在同一网络。" : "还没有配对的手机。在主窗口点「添加手机」。"}</div>`;
    } else if (this.error) {
      empty = `<div>${KIND_ICON.dir}<b>无法打开</b>${esc(this.error)}<br><button class="btn small" data-retry>重试</button></div>`;
    } else if (!this.loading && !v.length) {
      empty = `<div>${KIND_ICON.dir}<b>${f ? "没有匹配的项目" : "空文件夹"}</b>${f ? "" : "拖入文件，或从另一侧发送到这里"}</div>`;
    }
    this.r.empty.innerHTML = empty;
    this.r.empty.hidden = !empty;
    this.r.empty.querySelector("[data-retry]")?.addEventListener("click", () => this.load());
    this.renderFoot();
    this.renderSelbar();
  }

  renderHead() {
    const b = (a) => this.el.querySelector(`[data-a="${a}"]`);
    b("back").disabled = !this.back.length;
    b("fwd").disabled = !this.fwd.length;
    b("up").disabled = this.path == null || this.parentOf(this.path) == null;
    b("newdir").disabled = !this.ready() || this.path === "" || this.path == null;
    b("hidden").classList.toggle("on", this.showHidden);
    b("refresh").disabled = !this.ready();
    if (!this.isPhone) b("explorer").disabled = !this.path;

    if (this.isPhone) {
      const peers = connectedPeers();
      const all = state?.peers || [];
      const name = peers.find((p) => p.id === peer)?.name || all[0]?.name || "手机";
      this.r.who.innerHTML = peers.length > 1
        ? `<select data-r="peerSel">${peers.map((p) => `<option value="${esc(p.id)}"${p.id === peer ? " selected" : ""}>${esc(p.name)}</option>`).join("")}</select>`
        : esc(name);
      this.r.who.querySelector("select")?.addEventListener("change", (e) => switchPeer(e.target.value));
      this.r.dot.className = `dot ${peer ? "on" : ""}`;
    }
    if (!this.editing) this.renderCrumbs();
    this.renderPlaces();
  }

  renderCrumbs() {
    const c = this.r.crumbs;
    if (this.path == null) {
      c.innerHTML = "";
      return;
    }
    const parts = [];
    if (this.isPhone) {
      const vol = this.volumeOf(this.path);
      if (vol) {
        parts.push({ label: vol.name, path: vol.path });
        const rest = this.path.slice(vol.path.length).split("/").filter(Boolean);
        let acc = vol.path;
        for (const r of rest) parts.push({ label: r, path: (acc += "/" + r) });
      } else parts.push({ label: this.path, path: this.path });
    } else {
      parts.push({ label: "此电脑", path: "" });
      if (this.path) {
        const segs = this.path.split("\\").filter(Boolean);
        let acc = "";
        segs.forEach((s, i) => {
          acc = i === 0 ? s + "\\" : acc.replace(/\\?$/, "\\") + s;
          parts.push({ label: s, path: acc });
        });
      }
    }
    c.innerHTML = parts.map((p, i) => `${i ? '<span class="sep">›</span>' : ""}<button data-p="${esc(p.path)}">${esc(p.label)}</button>`).join("") + '<span class="fill"></span>';
    c.querySelectorAll("button").forEach((btn) => {
      btn.onclick = () => this.go(btn.dataset.p);
    });
    c.scrollLeft = c.scrollWidth;
  }

  editPath() {
    if (this.path == null || (this.isPhone && !peer)) return;
    this.editing = true;
    const c = this.r.crumbs;
    c.innerHTML = `<input value="${esc(this.path)}" spellcheck="false" />`;
    const input = c.querySelector("input");
    input.focus();
    input.select();
    const done = (go) => {
      if (!this.editing) return;
      this.editing = false;
      const v = input.value.trim();
      if (go && v && v !== this.path) this.go(this.isPhone ? v.replace(/\/+$/, "") || "/" : v);
      else this.renderCrumbs();
    };
    input.onkeydown = (e) => {
      if (e.key === "Enter") done(true);
      if (e.key === "Escape") done(false);
      e.stopPropagation();
    };
    input.onblur = () => done(false);
  }

  places() {
    if (this.isPhone) {
      if (!phoneRoots) return [];
      const h = phoneRoots.home;
      const list = [
        { name: "内部存储", path: h },
        { name: "相机", path: `${h}/DCIM/Camera` },
        { name: "截图", path: `${h}/Pictures/Screenshots` },
        { name: "下载", path: `${h}/Download` },
        { name: "SyncBoard 接收", path: `${h}/Download/SyncBoard` },
        { name: "文档", path: `${h}/Documents` },
        { name: "图片", path: `${h}/Pictures` },
        { name: "微信", path: `${h}/Pictures/WeiXin` },
      ];
      for (const v of phoneRoots.volumes.slice(1)) list.push({ name: v.name, path: v.path });
      return list;
    }
    if (!pcPlaces) return [];
    return [...pcPlaces.places, ...pcPlaces.drives.map((d) => ({ name: d.name, path: d.path }))];
  }

  // ---- favorites (per side, and per phone), kept in this window's local storage ----

  favKey() {
    return this.isPhone ? (peer ? `fm.fav.phone.${peer}` : null) : "fm.fav.pc";
  }
  favs() {
    const k = this.favKey();
    if (!k) return [];
    try {
      return JSON.parse(localStorage.getItem(k) || "[]");
    } catch {
      return [];
    }
  }
  saveFavs(list) {
    const k = this.favKey();
    if (!k) return;
    try {
      localStorage.setItem(k, JSON.stringify(list));
    } catch {}
    this.renderHead();
  }
  isFav(path) {
    return this.favs().some((f) => f.path === path);
  }
  baseName(path) {
    return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
  }
  addFav(path, name = this.baseName(path)) {
    if (!path || this.isFav(path)) return;
    this.saveFavs([...this.favs(), { name, path }]);
    toast(`已收藏「${name}」`);
  }
  removeFav(path) {
    this.saveFavs(this.favs().filter((f) => f.path !== path));
  }
  async renameFav(path) {
    const f = this.favs().find((x) => x.path === path);
    if (!f) return;
    const name = await dialog({ title: "重命名收藏", value: f.name, ok: "保存" });
    if (name) this.saveFavs(this.favs().map((x) => (x.path === path ? { ...x, name } : x)));
  }

  // Order of the places row (favorites and built-ins mixed) and hidden built-ins.
  prefKey(what) {
    return this.isPhone ? (peer ? `fm.${what}.phone.${peer}` : null) : `fm.${what}.pc`;
  }
  pref(what) {
    const k = this.prefKey(what);
    try {
      return (k && JSON.parse(localStorage.getItem(k) || "[]")) || [];
    } catch {
      return [];
    }
  }
  setPref(what, v) {
    const k = this.prefKey(what);
    try {
      if (k) v == null ? localStorage.removeItem(k) : localStorage.setItem(k, JSON.stringify(v));
    } catch {}
    this.renderPlaces();
  }

  /** Favorites, then built-ins, in the user's order; hidden built-ins left out. */
  chips() {
    const hidden = new Set(this.pref("hidden"));
    const all = [...this.favs().map((f) => ({ ...f, fav: true })), ...this.places().filter((p) => !hidden.has(p.path))];
    const seen = new Set();
    const uniq = all.filter((p) => !seen.has(p.path) && seen.add(p.path));
    const order = this.pref("order");
    const rank = (p) => {
      const i = order.indexOf(p.path);
      return i < 0 ? order.length + uniq.indexOf(p) : i;
    };
    return uniq.sort((a, b) => rank(a) - rank(b));
  }

  renderPlaces() {
    const chips = this.chips();
    this.r.places.innerHTML = chips
      .map((p) => `<button class="place${p.fav ? " fav" : ""}${p.path === this.path ? " on" : ""}" data-p="${esc(p.path)}"${p.fav ? ` data-fav="1"` : ""} title="${esc(p.path)}">${p.fav ? ICONS.star : ""}${esc(p.name)}</button>`)
      .join("");
    this.r.places.querySelectorAll("button").forEach((b) => {
      // Press and drag sideways to reorder; a plain click opens the place.
      b.onmousedown = (e) => {
        if (e.button === 0) dragChip(this, b, e);
      };
      b.onclick = () => {
        if (b._dragged) return (b._dragged = false);
        this.go(b.dataset.p);
      };
      b.oncontextmenu = (e) => {
        e.preventDefault();
        e.stopPropagation();
        const path = b.dataset.p;
        const reset = ["refresh", "恢复默认位置和顺序", () => {
          this.setPref("order", null);
          this.setPref("hidden", null);
        }];
        showItems(b.dataset.fav
          ? [["open", "打开", () => this.go(path)], ["rename", "重命名收藏", () => this.renameFav(path)], "-",
             ["star", "取消收藏", () => this.removeFav(path), false, "", true], "-", reset]
          : [["open", "打开", () => this.go(path)], ["x", "隐藏这个位置", () => this.setPref("hidden", [...this.pref("hidden"), path])], "-", reset],
          e.clientX, e.clientY);
      };
    });
    const star = this.el.querySelector('[data-a="fav"]');
    const on = this.path != null && this.path !== "" && this.isFav(this.path);
    star.classList.toggle("on", on);
    star.classList.toggle("starred", on);
    star.disabled = !this.ready() || !this.path;
    star.title = on ? "取消收藏这个文件夹" : "收藏这个文件夹";
  }

  renderFoot() {
    const n = this.view.length;
    const selSize = [...this.sel].map((x) => this.byName(x)).filter((e) => e && !e.d).reduce((a, e) => a + (e.s || 0), 0);
    this.r.count.textContent = this.path == null ? "" : this.sel.size ? `已选 ${this.sel.size} / ${n} 项${selSize ? ` · ${fmtSize(selSize)}` : ""}` : `${n} 项`;
    if (this.isPhone && peer && this.path) {
      const vol = this.volumeOf(this.path);
      this.r.free.textContent = vol?.total ? `可用 ${fmtSize(vol.free)} / ${fmtSize(vol.total)}` : "";
    } else this.r.free.textContent = "";
  }

  renderSelbar() {
    const bar = this.r.selbar;
    if (!this.sel.size || this.path === "") {
      bar.hidden = true;
      return;
    }
    const otherOk = this.other.ready() && this.other.path;
    const to = this.isPhone ? "电脑" : "手机";
    bar.innerHTML = `<span class="count">已选 ${this.sel.size} 项</span>
      <button class="primary" data-s="send" ${otherOk ? "" : "disabled"} title="${otherOk ? `发送到 ${esc(this.other.path)}` : `${to}不可用`}">${this.isPhone ? ICONS.sendBack : ICONS.send}发送到${to}</button>
      <span class="div"></span>
      <button data-s="copy" title="复制 (Ctrl+C)">${ICONS.copy}</button>
      <button data-s="cut" title="剪切 (Ctrl+X)">${ICONS.cut}</button>
      ${this.sel.size === 1 ? `<button data-s="rename" title="重命名 (F2)">${ICONS.rename}</button>` : ""}
      <button data-s="delete" title="删除 (Delete)">${ICONS.trash}</button>
      <button data-s="clear" title="取消选择 (Esc)">${ICONS.x}</button>`;
    bar.hidden = false;
    bar.querySelectorAll("[data-s]").forEach((b) => {
      b.onclick = () => {
        const s = b.dataset.s;
        if (s === "send") sendToOther(this);
        if (s === "copy") setClip(this, false);
        if (s === "cut") setClip(this, true);
        if (s === "rename") rename(this);
        if (s === "delete") remove(this);
        if (s === "clear") this.clearSel();
      };
    });
  }

  // ---- thumbnails (phone) ----

  observeThumbs() {
    this.io?.disconnect();
    this.io = new IntersectionObserver((items) => {
      for (const it of items) {
        if (!it.isIntersecting) continue;
        this.io.unobserve(it.target);
        loadThumb(this, it.target);
      }
    }, { root: this.r.list, rootMargin: "200px" });
    this.r.list.querySelectorAll("[data-thumb]").forEach((n) => {
      const key = this.join(n.dataset.thumb) + "|" + n.dataset.m;
      if (thumbCache.has(key)) n.innerHTML = `<img src="${thumbCache.get(key)}" alt="">`;
      else this.io.observe(n);
    });
  }

  // ---- selection ----

  clickRow(n, e) {
    if (e.shiftKey && this.anchor) {
      const names = this.view.map((x) => x.n);
      const [a, b] = [names.indexOf(this.anchor), names.indexOf(n)].sort((x, y) => x - y);
      if (!e.ctrlKey) this.sel.clear();
      names.slice(a, b + 1).forEach((x) => this.sel.add(x));
    } else if (e.ctrlKey) {
      this.sel.has(n) ? this.sel.delete(n) : this.sel.add(n);
      this.anchor = n;
    } else {
      this.sel = new Set([n]);
      this.anchor = n;
    }
    this.refreshSel();
  }

  select(n) {
    this.sel = new Set([n]);
    this.anchor = n;
    this.refreshSel();
    this.r.list.querySelector(`[data-n="${CSS.escape(n)}"]`)?.scrollIntoView({ block: "nearest" });
  }

  clearSel() {
    if (!this.sel.size) return;
    this.sel.clear();
    this.refreshSel();
  }

  refreshSel() {
    this.r.list.querySelectorAll(".row-f").forEach((r) => r.classList.toggle("sel", this.sel.has(r.dataset.n)));
    this.renderFoot();
    this.renderSelbar();
  }

  move(delta, extend) {
    if (!this.view.length) return;
    const names = this.view.map((x) => x.n);
    const cur = this.anchor && this.sel.has(this.anchor) ? names.indexOf(this.anchor) : -1;
    const i = Math.max(0, Math.min(names.length - 1, cur < 0 ? 0 : cur + delta));
    if (extend) {
      this.sel.add(names[i]);
      this.anchor = names[i];
      this.refreshSel();
      this.r.list.querySelector(`[data-n="${CSS.escape(names[i])}"]`)?.scrollIntoView({ block: "nearest" });
    } else this.select(names[i]);
  }

  // ---- actions ----

  async open(n) {
    const e = this.byName(n);
    if (!e) return;
    if (e.d) return this.go(this.join(n));
    if (this.isPhone) {
      toast(`正在打开 ${n}…`);
      await call("phone_open", { peer, path: this.join(n) }).catch(() => {});
    } else call("local_open", { path: this.join(n) });
  }

  action(a) {
    if (a === "back" && this.back.length) {
      this.fwd.push(this.path);
      this.go(this.back.pop(), { push: false });
    }
    if (a === "fwd" && this.fwd.length) {
      this.back.push(this.path);
      this.go(this.fwd.pop(), { push: false });
    }
    if (a === "up") {
      const p = this.parentOf(this.path);
      if (p != null) {
        const child = this.path;
        this.go(p).then(() => {
          const name = this.isPhone ? child.split("/").pop() : this.path === "" ? child : child.replace(/\\$/, "").split("\\").pop();
          if (this.byName(name)) this.select(name);
        });
      }
    }
    if (a === "refresh") this.load({ keepSel: true });
    if (a === "hidden") {
      this.showHidden = !this.showHidden;
      this.render();
    }
    if (a === "newdir") newFolder(this);
    if (a === "explorer" && this.path) call("local_open", { path: this.path });
    if (a === "fav" && this.path) this.isFav(this.path) ? this.removeFav(this.path) : this.addFav(this.path);
  }
}

const thumbCache = new Map();
let thumbBusy = 0;
const thumbWait = [];
function loadThumb(pane, node) {
  if (thumbBusy >= 4) return thumbWait.push([pane, node]);
  thumbBusy++;
  const path = pane.join(node.dataset.thumb);
  const key = path + "|" + node.dataset.m;
  invoke("phone_thumb", { peer, path, px: 96 })
    .then((url) => {
      thumbCache.set(key, url);
      if (thumbCache.size > 400) thumbCache.delete(thumbCache.keys().next().value);
      if (node.isConnected) node.innerHTML = `<img src="${url}" alt="">`;
    })
    .catch(() => {})
    .finally(() => {
      thumbBusy--;
      const next = thumbWait.shift();
      if (next) loadThumb(...next);
    });
}

const panes = { pc: new Pane("pc"), phone: new Pane("phone") };

// Drag and drop with mouse events: WebView2 turns HTML5 drag and drop off while the window
// accepts files from Explorer (which needs their paths).
let drag = null;

function startDrag(pane, row, e) {
  const x0 = e.clientX;
  const y0 = e.clientY;
  const move = (ev) => {
    if (!drag) {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 6) return;
      if (!pane.sel.has(row.dataset.n)) pane.select(row.dataset.n);
      const ghost = document.createElement("div");
      ghost.className = "drag-ghost";
      ghost.textContent = pane.sel.size > 1 ? `${pane.sel.size} 项` : row.dataset.n;
      document.body.appendChild(ghost);
      drag = { pane, paths: pane.selPaths(), names: new Set(pane.sel), ghost, target: null };
      document.body.classList.add("dragging");
    }
    drag.ghost.style.left = `${ev.clientX + 14}px`;
    drag.ghost.style.top = `${ev.clientY + 10}px`;
    const t = dropTarget(ev);
    if (drag.target?.el !== t?.el) {
      drag.target?.el?.classList.remove("drop-target");
      t?.el?.classList.add("drop-target");
    }
    drag.target = t;
    const verb = !t ? "" : t.pane.side !== pane.side ? (t.pane.isPhone ? "发送到手机" : "保存到电脑") : ev.ctrlKey ? "复制到" : "移动到";
    drag.ghost.dataset.verb = t ? `${verb} ${t.label}` : "";
  };
  const up = (ev) => {
    document.removeEventListener("mousemove", move);
    document.removeEventListener("mouseup", up);
    if (!drag) return;
    const d = drag;
    drag = null;
    d.ghost.remove();
    d.target?.el?.classList.remove("drop-target");
    document.body.classList.remove("dragging");
    const t = d.target;
    if (!t) return;
    const same = t.pane.side === pane.side;
    paste({ side: pane.side, paths: d.paths, mv: same && !ev.ctrlKey }, t.pane, t.dest);
  };
  document.addEventListener("mousemove", move);
  document.addEventListener("mouseup", up);
}

/**
 * Box selection, as in Explorer: rows the box touches get selected (added to the current
 * selection with Ctrl). The list scrolls when the pointer nears its top or bottom edge.
 */
function startBox(pane, e) {
  const list = pane.r.list;
  const lr = () => list.getBoundingClientRect();
  const at = (cx, cy) => ({ x: cx - lr().left, y: cy - lr().top + list.scrollTop });
  const start = at(e.clientX, e.clientY);
  const base = e.ctrlKey ? new Set(pane.sel) : new Set();
  let cur = { x: e.clientX, y: e.clientY };
  let box = null;
  let raf = 0;
  const update = () => {
    const p = at(cur.x, cur.y);
    const x0 = Math.min(start.x, p.x), x1 = Math.max(start.x, p.x);
    const y0 = Math.min(start.y, p.y), y1 = Math.max(start.y, p.y);
    Object.assign(box.style, { left: `${x0}px`, top: `${y0}px`, width: `${x1 - x0}px`, height: `${y1 - y0}px` });
    const sel = new Set(base);
    for (const r of list.querySelectorAll(".row-f")) {
      const top = r.offsetTop, bottom = top + r.offsetHeight;
      const left = r.offsetLeft, right = left + r.offsetWidth;
      if (bottom > y0 && top < y1 && right > x0 && left < x1) {
        if (e.ctrlKey && base.has(r.dataset.n)) sel.delete(r.dataset.n);
        else sel.add(r.dataset.n);
      }
    }
    pane.sel = sel;
    pane.refreshSel();
  };
  const scroll = () => {
    const r = lr();
    const edge = 28;
    const dy = cur.y < r.top + edge ? -Math.ceil((r.top + edge - cur.y) / 3) : cur.y > r.bottom - edge ? Math.ceil((cur.y - r.bottom + edge) / 3) : 0;
    if (dy) {
      list.scrollTop += dy;
      update();
    }
    raf = requestAnimationFrame(scroll);
  };
  const move = (ev) => {
    cur = { x: ev.clientX, y: ev.clientY };
    if (!box) {
      if (Math.hypot(ev.clientX - e.clientX, ev.clientY - e.clientY) < 5) return;
      box = document.createElement("div");
      box.className = "marquee";
      list.appendChild(box);
      raf = requestAnimationFrame(scroll);
    }
    update();
  };
  const up = () => {
    document.removeEventListener("mousemove", move);
    document.removeEventListener("mouseup", up);
    cancelAnimationFrame(raf);
    if (!box) return;
    box.remove();
    pane.boxed = true;
    pane.anchor = [...pane.sel].pop() || null;
  };
  document.addEventListener("mousemove", move);
  document.addEventListener("mouseup", up);
}

/** Reorders a places chip by dragging it sideways; the new order is remembered. */
function dragChip(pane, chip, e) {
  const row = pane.r.places;
  const x0 = e.clientX;
  let moving = false;
  const others = () => [...row.querySelectorAll(".place")].filter((c) => c !== chip);
  const move = (ev) => {
    const dx = ev.clientX - x0;
    if (!moving) {
      if (Math.abs(dx) < 6) return;
      moving = true;
      chip.classList.add("lifting");
      document.body.classList.add("dragging");
    }
    chip.style.transform = `translateX(${dx}px)`;
    // Show where it would land.
    const at = others().find((c) => {
      const r = c.getBoundingClientRect();
      return ev.clientX < r.left + r.width / 2;
    });
    others().forEach((c) => c.classList.toggle("insert-before", c === at));
    row.classList.toggle("insert-end", !at);
  };
  const up = (ev) => {
    document.removeEventListener("mousemove", move);
    document.removeEventListener("mouseup", up);
    if (!moving) return;
    chip._dragged = true;
    document.body.classList.remove("dragging");
    row.classList.remove("insert-end");
    const at = others().find((c) => {
      const r = c.getBoundingClientRect();
      return ev.clientX < r.left + r.width / 2;
    });
    const order = others().map((c) => c.dataset.p);
    order.splice(at ? order.indexOf(at.dataset.p) : order.length, 0, chip.dataset.p);
    pane.setPref("order", order);
  };
  document.addEventListener("mousemove", move);
  document.addEventListener("mouseup", up);
}

/** Where a drag would drop: a folder row, a breadcrumb, or (other pane only) its current folder. */
function dropTarget(ev) {
  const el = document.elementFromPoint(ev.clientX, ev.clientY);
  const paneEl = el?.closest(".pane");
  if (!paneEl) return null;
  const pane = panes[paneEl.dataset.side];
  if (!pane.ready() || !pane.path || pane.error) return null;
  const same = pane === drag.pane;
  const crumb = el.closest(".crumbs button") || el.closest(".places .place");
  if (crumb && crumb.dataset.p) {
    if (same && crumb.dataset.p === pane.path) return null;
    return { pane, dest: crumb.dataset.p, el: crumb, label: crumb.textContent };
  }
  const row = el.closest(".row-f");
  const entry = row && pane.byName(row.dataset.n);
  if (entry?.d && !(same && drag.names.has(entry.n))) return { pane, dest: pane.join(entry.n), el: row, label: entry.n };
  if (same) return null;
  return { pane, dest: pane.path, el: pane.el.querySelector(".list"), label: "当前文件夹" };
}

function setActive(side) {
  active = side;
  for (const p of Object.values(panes)) p.el.classList.toggle("active", p.side === side);
}

// ---------- operations ----------

function dialog({ title, text = "", value = null, ok = "确定", danger = false, select = null }) {
  return new Promise((resolve) => {
    $("dialogTitle").textContent = title;
    $("dialogText").textContent = text;
    $("dialogText").hidden = !text;
    const input = $("dialogInput");
    input.hidden = value == null;
    input.value = value ?? "";
    $("dialogOk").textContent = ok;
    $("dialogOk").classList.toggle("danger", danger);
    $("dialogOk").classList.toggle("ink", !danger);
    $("dialog").hidden = false;
    setTimeout(() => {
      if (value != null) {
        input.focus();
        if (select) input.setSelectionRange(...select);
        else input.select();
      } else $("dialogOk").focus();
    });
    const close = (v) => {
      $("dialog").hidden = true;
      $("dialogForm").onsubmit = $("dialogCancel").onclick = $("dialog").onclick = null;
      resolve(v);
    };
    $("dialogForm").onsubmit = (e) => {
      e.preventDefault();
      close(value != null ? input.value.trim() : true);
    };
    $("dialogCancel").onclick = () => close(null);
    // A block body: returning false from an on* handler would cancel the OK button's submit.
    $("dialog").onclick = (e) => {
      if (e.target.id === "dialog") close(null);
    };
  });
}

const BAD_NAME = /[\\/:*?"<>|]/;

async function newFolder(pane) {
  if (!pane.ready() || !pane.path) return;
  const name = await dialog({ title: "新建文件夹", value: "新建文件夹", ok: "新建" });
  if (!name) return;
  if (BAD_NAME.test(name)) return toast("名称不能包含 \\ / : * ? \" < > |", "error");
  const path = pane.join(name);
  try {
    if (pane.isPhone) await call("phone_fs", { peer, op: "mkdir", args: { path } });
    else await call("local_mkdir", { path });
  } catch {
    return;
  }
  await pane.load();
  pane.select(name);
}

async function rename(pane) {
  if (pane.sel.size !== 1 || pane.path === "") return;
  const old = [...pane.sel][0];
  const e = pane.byName(old);
  const dot = !e.d && old.lastIndexOf(".") > 0 ? old.lastIndexOf(".") : old.length;
  const name = await dialog({ title: "重命名", value: old, ok: "重命名", select: [0, dot] });
  if (!name || name === old) return;
  if (BAD_NAME.test(name)) return toast("名称不能包含 \\ / : * ? \" < > |", "error");
  try {
    if (pane.isPhone) await call("phone_fs", { peer, op: "rename", args: { path: pane.join(old), to: pane.join(name) } });
    else await call("local_rename", { path: pane.join(old), to: pane.join(name) });
  } catch {
    return;
  }
  await pane.load();
  pane.select(name);
}

async function remove(pane) {
  if (!pane.sel.size || pane.path === "") return;
  const n = pane.sel.size;
  const names = [...pane.sel];
  const what = n === 1 ? `「${names[0]}」` : `「${names[0]}」等 ${n} 项`;
  const ok = pane.isPhone
    ? await dialog({ title: "永久删除？", text: `${what}会从手机上永久删除，无法恢复。`, ok: "删除", danger: true })
    : await dialog({ title: "移到回收站？", text: `${what}会被移到回收站，需要时可以从回收站还原。`, ok: "移到回收站", danger: true });
  if (!ok) return;
  const paths = pane.selPaths();
  try {
    if (pane.isPhone) await call("phone_fs", { peer, op: "delete", args: { paths } });
    else await call("local_delete", { paths });
  } catch {}
  pane.load();
}

function setClip(pane, mv) {
  if (!pane.sel.size || pane.path === "") return;
  clip = { side: pane.side, paths: pane.selPaths(), mv };
  toast(`已${mv ? "剪切" : "复制"} ${clip.paths.length} 项，到目标文件夹按 Ctrl+V`);
  Object.values(panes).forEach((p) => p.render());
}

/** Puts `src` ({side, paths, mv}) into `pane`'s folder `dest`: within one side, or as a transfer. */
async function paste(src, pane, dest = pane.path) {
  if (!src || !pane.ready() || !dest) return;
  if (src.side === pane.side) {
    if (src.paths.some((p) => dest === p || dest.startsWith(p + (pane.isPhone ? "/" : "\\")))) return toast("不能放进它自己里面", "error");
    try {
      if (pane.isPhone) await call("phone_fs", { peer, op: src.mv ? "move" : "copy", args: { paths: src.paths, dir: dest } });
      else await call("local_paste", { paths: src.paths, dir: dest, mv: src.mv });
    } catch {}
    if (src.mv && clip === src) clip = null;
    Object.values(panes).forEach((p) => p.side === pane.side && p.load({ keepSel: true }));
    return;
  }
  if (!peer) return toast("手机未连接", "error");
  await call("queue_add", { job: { dir: pane.isPhone ? "up" : "down", peer, paths: src.paths, dest, mv: src.mv } });
  if (src.mv && clip === src) clip = null;
  openQueue(false);
}

function sendToOther(pane) {
  if (!pane.sel.size) return;
  const other = pane.other;
  if (!other.ready() || !other.path) return toast(pane.isPhone ? "先在左侧打开一个电脑文件夹" : "手机未连接", "error");
  paste({ side: pane.side, paths: pane.selPaths(), mv: false }, other, other.path);
  pane.clearSel();
}

// ---------- context menu ----------

function hideMenu() {
  $("menu").hidden = true;
}

function showMenu(pane, x, y) {
  setActive(pane.side);
  const n = pane.sel.size;
  const single = n === 1 ? pane.byName([...pane.sel][0]) : null;
  const atDrives = pane.path === "";
  const items = [];
  const to = pane.isPhone ? "电脑" : "手机";
  if (n) {
    items.push(["open", single?.d ? "打开文件夹" : "打开", () => pane.open(single.n), !single, "Enter"]);
    if (!atDrives) {
      items.push([pane.isPhone ? "sendBack" : "send", `发送到${to}`, () => sendToOther(pane), !(pane.other.ready() && pane.other.path)]);
      items.push("-");
      items.push(["copy", "复制", () => setClip(pane, false), false, "Ctrl+C"]);
      items.push(["cut", "剪切", () => setClip(pane, true), false, "Ctrl+X"]);
    }
  }
  if (!atDrives) items.push(["paste", clip ? `粘贴 ${clip.paths.length} 项${clip.side !== pane.side ? `（从${clip.side === "pc" ? "电脑" : "手机"}）` : ""}` : "粘贴", () => paste(clip, pane), !clip || !pane.ready(), "Ctrl+V"]);
  if (n && !atDrives) {
    items.push("-");
    if (single?.d) {
      const p = pane.join(single.n);
      items.push(pane.isFav(p) ? ["star", "取消收藏", () => pane.removeFav(p)] : ["star", "添加到收藏", () => pane.addFav(p)]);
    }
    items.push(["rename", "重命名", () => rename(pane), n !== 1, "F2"]);
    items.push(["link", "复制路径", () => navigator.clipboard.writeText(pane.selPaths().join("\n")).then(() => toast("已复制路径"))]);
    if (!pane.isPhone) items.push(["reveal", "在资源管理器中显示", () => call("local_reveal", { path: pane.selPaths()[0] })]);
    items.push(["trash", "删除", () => remove(pane), false, "Delete", true]);
  }
  if (!n && !atDrives) {
    items.push("-");
    if (pane.path) items.push(pane.isFav(pane.path) ? ["star", "取消收藏当前文件夹", () => pane.removeFav(pane.path)] : ["star", "收藏当前文件夹", () => pane.addFav(pane.path)]);
    items.push(["newdir", "新建文件夹", () => newFolder(pane), !pane.ready(), "Ctrl+Shift+N"]);
    if (!pane.isPhone && pane.path) items.push(["reveal", "在资源管理器中打开", () => call("local_open", { path: pane.path })]);
    items.push(["refresh", "刷新", () => pane.load({ keepSel: true }), false, "F5"]);
  }
  showItems(items, x, y);
}

function showItems(items, x, y) {
  const m = $("menu");
  m.innerHTML = items
    .map((it, i) => (it === "-" ? "<hr>" : `<button data-i="${i}" class="${it[5] ? "danger" : ""}" ${it[3] ? "disabled" : ""}>${ICONS[it[0]] || ""}${esc(it[1])}${it[4] ? `<kbd>${it[4]}</kbd>` : ""}</button>`))
    .join("");
  m.querySelectorAll("button").forEach((b) => (b.onclick = () => {
    hideMenu();
    items[b.dataset.i][2]();
  }));
  m.hidden = false;
  const r = m.getBoundingClientRect();
  m.style.left = `${Math.min(x, innerWidth - r.width - 6)}px`;
  m.style.top = `${Math.min(y, innerHeight - r.height - 6)}px`;
}

document.addEventListener("mousedown", (e) => !e.target.closest("#menu") && hideMenu());
window.addEventListener("blur", hideMenu);

// ---------- keyboard ----------

document.addEventListener("keydown", (e) => {
  if (!$("dialog").hidden) {
    if (e.key === "Escape") $("dialogCancel").click();
    return;
  }
  if (e.target.tagName === "INPUT" || e.target.tagName === "SELECT") return;
  const p = panes[active];
  const k = e.key;
  if (k === "Escape") {
    hideMenu();
    p.clearSel();
  } else if (k === "Tab" && $("panes").dataset.layout === "both") {
    e.preventDefault();
    setActive(p.other.side);
    p.other.r.list.focus();
  } else if (k === "Enter" && p.sel.size === 1) p.open([...p.sel][0]);
  else if (k === "Backspace" || (e.altKey && k === "ArrowUp")) p.action("up");
  else if (e.altKey && k === "ArrowLeft") p.action("back");
  else if (e.altKey && k === "ArrowRight") p.action("fwd");
  else if (k === "ArrowDown" || k === "ArrowUp") {
    e.preventDefault();
    p.move(k === "ArrowDown" ? 1 : -1, e.shiftKey);
  } else if (k === "Delete") remove(p);
  else if (k === "F2") rename(p);
  else if (k === "F5") p.load({ keepSel: true });
  else if (e.ctrlKey && e.shiftKey && (k === "N" || k === "n")) newFolder(p);
  else if (e.ctrlKey && (k === "a" || k === "A")) {
    e.preventDefault();
    p.sel = new Set(p.view.map((x) => x.n));
    p.refreshSel();
  } else if (e.ctrlKey && (k === "c" || k === "C")) setClip(p, false);
  else if (e.ctrlKey && (k === "x" || k === "X")) setClip(p, true);
  else if (e.ctrlKey && (k === "v" || k === "V")) paste(clip, p);
  else if (e.ctrlKey && (k === "f" || k === "F")) {
    e.preventDefault();
    p.r.search.focus();
  } else return;
});
document.addEventListener("contextmenu", (e) => {
  if (!e.target.closest(".list") && e.target.tagName !== "INPUT") e.preventDefault();
});

// ---------- transfer queue ----------

let speed = { t: 0, bytes: 0, rate: 0 };

function openQueue(expand) {
  $("queue").hidden = false;
  if (expand != null) {
    $("queueList").hidden = !expand;
    $("queue").classList.toggle("open", expand);
  }
}

function renderQueue(items) {
  const prev = queueItems;
  queueItems = items;
  // Refresh panes showing a folder that just received something.
  for (const it of items) {
    const was = prev.find((x) => x.qid === it.qid);
    if (it.state === "done" && was && was.state !== "done") {
      const target = it.dir === "up" ? panes.phone : panes.pc;
      if (target.path === it.dest) target.load({ keepSel: true });
      // A move removed the source.
      if (it.mv) (it.dir === "up" ? panes.pc : panes.phone).load({ keepSel: true });
    }
  }
  if (!items.length) {
    $("queue").hidden = true;
    return;
  }
  $("queue").hidden = false;
  const running = items.filter((i) => i.state === "running" || i.state === "queued");
  const failed = items.filter((i) => i.state === "failed").length;
  const total = items.reduce((a, i) => a + (i.state === "cancelled" ? 0 : i.total), 0);
  const done = items.reduce((a, i) => a + (i.state === "cancelled" ? 0 : i.state === "done" ? i.total : i.done), 0);
  const now = performance.now();
  if (now - speed.t > 900) {
    const dt = (now - speed.t) / 1000;
    speed.rate = speed.t && done >= speed.bytes ? (done - speed.bytes) / dt : 0;
    speed = { t: now, bytes: done, rate: speed.rate };
  }
  $("queueSpin").hidden = !running.length;
  $("queueText").textContent = running.length
    ? `正在传输 · 剩余 ${running.length} 项`
    : failed
      ? `传输完成，${failed} 项失败`
      : "传输完成";
  $("queueSpeed").textContent = running.length
    ? `${fmtSize(done)} / ${fmtSize(total)}${speed.rate > 0 ? ` · ${fmtSize(speed.rate)}/s` : ""}`
    : `${items.length} 项`;
  $("queueProgress").style.width = `${total ? Math.min(100, (done * 100) / total) : running.length ? 0 : 100}%`;

  const arrow = (d) => (d === "up" ? ICONS.send : ICONS.sendBack);
  const label = { queued: "等待中", running: "", done: "完成", failed: "失败", cancelled: "已取消" };
  $("queueList").innerHTML =
    items
      .slice()
      .reverse()
      .map((i) => {
        const pct = i.total ? Math.min(100, (i.done * 100) / i.total) : 0;
        const where = `${i.dir === "up" ? "到手机" : "到电脑"} ${i.dest}`;
        const files = i.files > 1 ? ` · ${i.filesDone}/${i.files} 个文件` : "";
        const sub = i.state === "failed" ? `<div class="q-sub err">${esc(i.error || "失败")}</div>` : `<div class="q-sub">${esc(where)}${files}${i.total ? ` · ${fmtSize(i.state === "done" ? i.total : i.done)} / ${fmtSize(i.total)}` : ""}</div>`;
        const bar = i.state === "running" ? `<div class="bar"><i style="width:${pct}%"></i></div>` : "";
        const btn = i.state === "running" || i.state === "queued"
          ? `<button class="icon-btn" data-cancel="${i.qid}" title="取消">${ICONS.x}</button>`
          : `<button class="icon-btn" data-clear="${i.qid}" title="移除">${ICONS.x}</button>`;
        return `<div class="q-item"><span class="q-dir">${arrow(i.dir)}</span><div style="min-width:0"><div class="q-name">${esc(i.name)}</div>${sub}${bar}</div>
          <span class="q-state ${i.state}">${i.state === "running" ? `${Math.floor(pct)}%` : label[i.state]}</span>${btn}</div>`;
      })
      .join("") + `<div class="queue-actions"><button class="btn small" id="queueClear">清除已完成</button></div>`;
  $("queueList").querySelectorAll("[data-cancel]").forEach((b) => (b.onclick = () => invoke("queue_cancel", { qid: +b.dataset.cancel })));
  $("queueList").querySelectorAll("[data-clear]").forEach((b) => (b.onclick = () => invoke("queue_clear", { qid: +b.dataset.clear })));
  $("queueClear").onclick = () => invoke("queue_clear", { qid: null });
}

$("queueBar").onclick = () => openQueue($("queueList").hidden);

// ---------- phone / peers ----------

async function switchPeer(id) {
  peer = id;
  phoneRoots = null;
  thumbCache.clear();
  if (!peer) {
    panes.phone.path = null;
    panes.phone.entries = [];
    panes.phone.render();
    return;
  }
  try {
    phoneRoots = await invoke("phone_fs", { peer, op: "roots", args: {} });
  } catch (e) {
    phoneRoots = null;
    panes.phone.error = String(e);
    panes.phone.entries = [];
    panes.phone.path = null;
    panes.phone.render();
    return;
  }
  const p = panes.phone;
  const keep = p.path && p.volumeOf(p.path) ? p.path : null;
  p.back = [];
  p.fwd = [];
  await p.go(keep || phoneRoots.home, { push: false });
}

async function refreshState() {
  state = await invoke("get_state");
  document.documentElement.dataset.theme = state.settings.theme || "system";
  const online = connectedPeers();
  if (!online.some((p) => p.id === peer)) await switchPeer(online[0]?.id || null);
  else panes.phone.renderHead();
}

listen("engine", (e) => {
  const t = e.payload.type;
  if (t === "peerConnected" || t === "peerDisconnected" || t === "paired") refreshState();
});
listen("state", (e) => {
  state = e.payload;
  document.documentElement.dataset.theme = state.settings.theme || "system";
});
listen("queue", (e) => renderQueue(e.payload));

// Files dropped from Explorer: onto whichever pane is under the cursor.
listen("tauri://drag-enter", () => {});
listen("tauri://drag-over", (e) => {
  const pane = paneAt(e.payload.position);
  for (const p of Object.values(panes)) p.r.drop.hidden = p !== pane || !pane.ready() || !pane.path;
  if (pane) pane.r.drop.textContent = pane.isPhone ? `松开发送到手机：${pane.path}` : `松开复制到：${pane.path}`;
});
listen("tauri://drag-leave", () => Object.values(panes).forEach((p) => (p.r.drop.hidden = true)));
listen("tauri://drag-drop", (e) => {
  Object.values(panes).forEach((p) => (p.r.drop.hidden = true));
  const pane = paneAt(e.payload.position);
  const paths = e.payload.paths || [];
  if (!pane || !paths.length || !pane.ready() || !pane.path) return;
  paste({ side: "pc", paths, mv: false }, pane, pane.path);
});
function paneAt(pos) {
  if (!pos) return null;
  const x = pos.x / devicePixelRatio;
  const y = pos.y / devicePixelRatio;
  return Object.values(panes).find((p) => {
    const r = p.el.getBoundingClientRect();
    return r.width && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }) || null;
}

// ---------- window ----------

$("minBtn").onclick = () => win.minimize();
$("maxBtn").onclick = () => win.toggleMaximize();
$("closeBtn").onclick = () => win.close();
document.querySelector(".titlebar").addEventListener("dblclick", (e) => {
  if (e.target.hasAttribute("data-tauri-drag-region")) win.toggleMaximize();
});

function setLayout(l) {
  $("panes").dataset.layout = l;
  document.querySelectorAll("[data-layout]").forEach((b) => b.classList.toggle("on", b.dataset.layout === l));
  try { localStorage.setItem("fm.layout", l); } catch {}
  if (l !== "both") setActive(l);
  Object.values(panes).forEach((p) => p.renderSelbar());
}
document.querySelectorAll("#layout [data-layout]").forEach((b) => (b.onclick = () => setLayout(b.dataset.layout)));

(async function init() {
  let layout = "both";
  try { layout = localStorage.getItem("fm.layout") || "both"; } catch {}
  setLayout(layout);
  setActive(layout === "phone" ? "phone" : "pc");
  pcPlaces = await invoke("local_places");
  const downloads = pcPlaces.places.find((p) => p.name === "下载")?.path || pcPlaces.home;
  panes.pc.go(downloads, { push: false });
  panes.phone.render();
  await refreshState();
  renderQueue(await invoke("queue_state"));
})();
