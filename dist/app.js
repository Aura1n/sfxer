/* sfxer — state, virtual list, sidebar, detail panel, shortcuts. */
(() => {
  const T = window.__TAURI__;
  const api = window.API;
  const F = window.FMT;
  const { el, debounce, icon, iconEl } = window.DOMX;

  const ROW_H = 34;
  const PAGE = 400;

  // The two stretchable columns keep small minima so the fixed metadata columns
  // never push the ⋯ cell past the pane edge on a narrow window.
  const COLS = [
    { key: "play", w: "24px", fixed: true },
    { key: "name", label: "名称", w: "minmax(100px, 1.3fr)", sort: "name" },
    { key: "dur", label: "时长", w: "58px", sort: "duration", align: "right" },
    { key: "sr", label: "采样率", w: "52px", sort: "sample_rate", align: "right" },
    { key: "bits", label: "位深", w: "40px", sort: null, align: "right" },
    { key: "ch", label: "声道", w: "44px", sort: "channels", align: "right" },
    { key: "fmt", label: "格式", w: "52px", sort: "ext" },
    { key: "size", label: "大小", w: "66px", sort: "size", align: "right" },
    { key: "peak", label: "峰值", w: "52px", sort: "peak_db", align: "right" },
    { key: "tags", label: "标签", w: "minmax(64px, 1fr)" },
    { key: "act", w: "26px", fixed: true },
  ];

  const SCOPES = [
    { id: "all", label: "全部音频", icon: "library" },
    { id: "untagged", label: "未打标签", icon: "tagOff" },
    { id: "notes", label: "有备注", icon: "note" },
    { id: "recent", label: "最近导入", icon: "history" },
    { id: "clipped", label: "峰值溢出", icon: "warning", countKey: "clipped" },
    { id: "undecodable", label: "不可解码", icon: "broken", countKey: "undecodable" },
  ];

  const SORT_FIELDS = [
    ["name", "名称"],
    ["duration", "时长"],
    ["sample_rate", "采样率"],
    ["size", "大小"],
    ["peak_db", "峰值"],
    ["added", "加入时间"],
    ["modified", "文件修改时间"],
    ["plays", "播放次数"],
    ["path", "完整路径"],
  ];

  // Every filter control writes into this shape, and a saved view is one of
  // these stored as JSON — so applying a view is handing the object back.
  function newFilter() {
    return {
      text: "",
      tagIds: [],
      scope: "all",
      rootId: null,
      formats: [],
      durations: [],
      sampleRates: [],
      channels: [],
      sort: "name",
      dir: "asc",
      offset: 0,
      limit: PAGE,
    };
  }

  const sortLabel = (k) => (SORT_FIELDS.find(([v]) => v === k) || ["", "名称"])[1];

  const S = {
    roots: [],
    tags: [],
    tagById: new Map(),
    stats: {},
    rows: [],
    total: 0,
    sel: new Set(),
    cursor: -1,
    anchor: -1,
    detail: null,
    loadedAll: false,
    scanning: false,
    colsOff: JSON.parse(localStorage.getItem("cols") || "[]"),
    filter: newFilter(),
    triage: { on: false, i: 0 },
  };

  const $ = (id) => document.getElementById(id);
  const dom = {
    app: $("app"),
    body: document.querySelector(".body"),
    search: $("search"),
    rowsScroll: $("rows-scroll"),
    rowsVirtual: $("rows-virtual"),
    listEmpty: $("list-empty"),
    colsHead: $("cols-head"),
    roots: $("roots"),
    scopes: $("scopes"),
    tags: $("tags"),
    tagFilter: $("tag-filter"),
    newTag: $("new-tag"),
    detail: $("detail"),
    detailBody: $("detail-body"),
    detailEmpty: $("detail-empty"),
    statusCount: $("status-count"),
    statusSel: $("status-sel"),
    statusMsg: $("status-msg"),
    progress: $("progress"),
    progressBar: $("progress-bar"),
    progressText: $("progress-text"),
    filterBadge: $("filter-badge"),
    dlgSettings: $("dlg-settings"),
    settingsBody: $("settings-body"),
    settingsNav: $("settings-nav"),
    settingsHead: $("dlg-settings-head"),
    btnSettingsClose: $("btn-settings-close"),
    dlgConfirm: $("dlg-confirm"),
    confirmText: $("confirm-text"),
  };

  const visibleCols = () => COLS.filter((c) => !S.colsOff.includes(c.key) || c.fixed);
  const gridTemplate = () => visibleCols().map((c) => c.w).join(" ");

  function applyTemplate() {
    const tpl = gridTemplate();
    dom.colsHead.style.gridTemplateColumns = tpl;
    dom.rowsVirtual.style.setProperty("--tpl", tpl);
    document.documentElement.style.setProperty("--tpl", tpl);
  }

  /* ------------------------------------------------------------------ header */
  function renderHead() {
    dom.colsHead.textContent = "";
    applyTemplate();
    for (const c of visibleCols()) {
      if (c.fixed) {
        // The trailing ⋯ cell doubles as the "restore hidden columns" button,
        // so the header keeps exactly one child per grid column.
        if (c.key === "act" && S.colsOff.length) dom.colsHead.appendChild(restoreBtn());
        else dom.colsHead.appendChild(el("span"));
        continue;
      }
      const b = el("button", S.filter.sort === c.sort ? "sorted" : "");
      b.type = "button";
      b.appendChild(el("span", null, c.label));
      if (c.sort) {
        const arrow = el("span", "arrow");
        arrow.innerHTML = icon(S.filter.sort === c.sort && S.filter.dir === "desc" ? "down" : "up");
        b.appendChild(arrow);
      }
      b.title = c.sort ? `按${c.label}排序` : c.label;
      if (c.sort) {
        b.addEventListener("click", () => {
          if (S.filter.sort === c.sort) S.filter.dir = S.filter.dir === "asc" ? "desc" : "asc";
          else {
            S.filter.sort = c.sort;
            S.filter.dir = c.key === "name" || c.key === "fmt" ? "asc" : "desc";
          }
          renderHead();
          reload();
        });
      }
      b.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        if (c.key === "name") return;
        S.colsOff.push(c.key);
        localStorage.setItem("cols", JSON.stringify(S.colsOff));
        renderHead();
        renderRows();
      });
      dom.colsHead.appendChild(b);
    }
  }

  function restoreBtn() {
    const restore = el("button", "col-restore");
    restore.type = "button";
    restore.innerHTML = icon("add");
    restore.title = "恢复隐藏的列";
    restore.addEventListener("click", () => {
      S.colsOff = [];
      localStorage.setItem("cols", "[]");
      renderHead();
      renderRows();
    });
    return restore;
  }

  /* -------------------------------------------------------------------- rows */
  let rafPending = false;

  function scheduleRows() {
    if (rafPending) return;
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      renderRows();
    });
  }

  function rowMarkup(r) {
    const cells = [];
    for (const c of visibleCols()) {
      switch (c.key) {
        case "play": {
          const b = el("span", "play" + (r.decodeable ? "" : " no-decode"));
          b.innerHTML = r.decodeable ? icon("playFill") : icon("broken");
          b.title = r.decodeable ? "试听 (空格)" : "此格式无法解码播放";
          cells.push(b);
          break;
        }
        case "name": {
          const w = el("span", "c-name");
          w.appendChild(el("span", "name" + (r.alias ? " alias" : ""), r.alias || r.name));
          if (r.hasNotes) {
            const m = el("span", "note-mark");
            m.innerHTML = icon("noteFill");
            m.title = "有备注";
            w.appendChild(m);
          }
          // The column is too narrow for two names, so the real filename lives in
          // the tooltip and the detail caption; the weight alone tells them apart.
          w.title = r.alias ? `${r.name}\n${r.path}\nF2 改显示名` : `${r.path}\nF2 改显示名`;
          cells.push(w);
          break;
        }
        case "dur":
          cells.push(el("span", "num", F.timeShort(r.durationMs)));
          break;
        case "sr":
          cells.push(el("span", "num", F.khz(r.sampleRate)));
          break;
        case "bits":
          cells.push(el("span", "num", r.bitDepth || "—"));
          break;
        case "ch":
          cells.push(el("span", "num", r.channels || "—"));
          break;
        case "fmt":
          cells.push(el("span", "ext", r.ext ? r.ext.toUpperCase().replace(".", "") : "—"));
          break;
        case "size":
          cells.push(el("span", "num", F.bytes(r.sizeBytes).replace(/ /g, "\u2009")));
          break;
        case "peak":
          cells.push(el("span", "num", r.peakDb === null || r.peakDb === undefined ? "—" : F.db(r.peakDb)));
          break;
        case "tags": {
          const t = el("span", "tags-cell");
          const ids = r.tagIds || [];
          ids.slice(0, 3).forEach((id) => {
            const tag = S.tagById.get(id);
            if (tag) t.appendChild(window.TagUI.chip(tag, {}));
          });
          if (ids.length > 3) t.appendChild(el("span", "chip more", `+${ids.length - 3}`));
          cells.push(t);
          break;
        }
        case "act": {
          const b = el("span", "row-act");
          b.innerHTML = icon("dots");
          b.title = "更多操作";
          cells.push(b);
          break;
        }
      }
    }
    return cells;
  }

  function renderRows() {
    flushRename();
    const scrollTop = dom.rowsScroll.scrollTop;
    const viewH = dom.rowsScroll.clientHeight;
    const totalH = S.rows.length * ROW_H;
    dom.rowsVirtual.style.height = `${totalH}px`;

    const start = Math.max(0, Math.floor(scrollTop / ROW_H) - 8);
    const end = Math.min(S.rows.length, Math.ceil((scrollTop + viewH) / ROW_H) + 8);

    const frag = document.createDocumentFragment();
    for (let i = start; i < end; i++) {
      const r = S.rows[i];
      const row = el("div", "row");
      row.style.gridTemplateColumns = gridTemplate();
      row.style.transform = `translateY(${i * ROW_H}px)`;
      row.dataset.id = String(r.id);
      row.dataset.index = String(i);
      if (S.sel.has(r.id)) row.classList.add("sel");
      if (i === S.cursor) row.classList.add("cursor");
      for (const c of rowMarkup(r)) row.appendChild(c);
      frag.appendChild(row);
    }
    dom.rowsVirtual.textContent = "";
    dom.rowsVirtual.appendChild(frag);

    if (!S.rows.length) {
      dom.listEmpty.hidden = false;
      dom.listEmpty.textContent = S.filter.text || S.filter.tagIds.length || S.filter.scope !== "all"
        ? "没有符合条件的条目"
        : S.roots.length
          ? "此范围内还没有音频文件"
          : "还没有添加文件夹";
    } else {
      dom.listEmpty.hidden = true;
    }
    maybeLoadMore(scrollTop + viewH, totalH);
    renderStatus();
  }

  function maybeLoadMore(viewBottom, totalH) {
    if (S.loadedAll || S.busy) return;
    if (totalH - viewBottom > 1200) return;
    if (S.rows.length >= S.total) {
      S.loadedAll = true;
      return;
    }
    S.filter.offset = S.rows.length;
    S.filter.limit = PAGE;
    fetchPage(true);
  }

  async function fetchPage(append) {
    S.busy = true;
    try {
      const res = await api.query(S.filter);
      if (append) S.rows = S.rows.concat(res.items);
      else {
        S.rows = res.items;
        S.loadedAll = false;
      }
      S.total = res.total;
      if (S.rows.length >= res.total) S.loadedAll = true;
    } catch (e) {
      console.error(e);
      dom.listEmpty.hidden = false;
      dom.listEmpty.textContent = `查询失败：${e}`;
    } finally {
      S.busy = false;
      renderRows();
    }
  }

  const reload = debounce(() => {
    // The triage queue is exactly "this filter, untagged", so any query change
    // ends the session — counted as one refresh, not one per item tagged.
    if (S.triage.on) {
      S.triage = { on: false, i: 0, total: 0 };
      refreshTags();
      refreshCounts();
      showDetail();
    }
    S.filter.offset = 0;
    S.filter.limit = PAGE;
    S.rows = [];
    S.loadedAll = false;
    dom.rowsScroll.scrollTop = 0;
    fetchPage(false);
    renderStatus();
  }, 90);

  /* --------------------------------------------------------------- selection */
  function selectedRows() {
    return S.rows.filter((r) => S.sel.has(r.id));
  }

  function setSelection(ids, cursor) {
    S.sel = new Set(ids);
    S.cursor = cursor === undefined ? S.cursor : cursor;
    renderRows();
    showDetail();
  }

  function selectRow(index, ev) {
    const r = S.rows[index];
    if (!r) return;
    if (ev && (ev.ctrlKey || ev.metaKey)) {
      if (S.sel.has(r.id)) S.sel.delete(r.id);
      else S.sel.add(r.id);
      S.anchor = index;
      S.cursor = index;
    } else if (ev && ev.shiftKey && S.anchor >= 0) {
      const [a, b] = [Math.min(S.anchor, index), Math.max(S.anchor, index)];
      if (!ev.ctrlKey && !ev.metaKey) S.sel.clear();
      for (let i = a; i <= b; i++) S.sel.add(S.rows[i].id);
      S.cursor = index;
    } else {
      S.sel = new Set([r.id]);
      S.anchor = index;
      S.cursor = index;
    }
    renderRows();
    showDetail();
  }

  function ensureCursorVisible(index) {
    const top = index * ROW_H;
    const h = dom.rowsScroll.clientHeight;
    if (top < dom.rowsScroll.scrollTop) dom.rowsScroll.scrollTop = top;
    else if (top + ROW_H > dom.rowsScroll.scrollTop + h) dom.rowsScroll.scrollTop = top + ROW_H - h;
  }

  // Renaming happens on the row itself: a dialog would take the eye off the list.
  function startRowRename(index) {
    const r = S.rows[index];
    if (!r) return;
    ensureCursorVisible(index);
    renderRows();
    const cell = dom.rowsVirtual.querySelector(`.row[data-index="${index}"] .c-name`);
    if (!cell) return;
    openRename(cell, [...cell.childNodes], r.alias || r.name, r.name, async (v) => {
      const next = v && v !== r.name ? v : "";
      if (next === (r.alias || "")) return;
      const alias = await applyAlias(r, v);
      flash(alias ? `显示名改为「${alias}」` : "已恢复文件名");
    });
  }

  /* ----------------------------------------------------------------- sources */
  // Where the audio came from is bookkeeping, not navigation: rows are a status
  // strip, so a left click no longer re-scopes the list and the scan time lives
  // in the tooltip instead of an extra line per source.
  function renderRoots() {
    dom.roots.textContent = "";
    if (!S.roots.length) {
      const d = el("div", "side-item");
      d.style.color = "var(--text-3)";
      d.appendChild(el("span", "name", "尚未添加文件夹"));
      dom.roots.appendChild(d);
      return;
    }
    for (const r of S.roots) {
      const on = S.filter.scope === "root" && S.filter.rootId === r.id;
      const item = el("div", "side-item" + (on ? " on" : ""));
      item.appendChild(iconEl("folder"));
      const name = el("span", "name", r.label);
      item.appendChild(name);
      const count = el("span", "count" + (r.lastScanAt ? "" : " warn"), r.lastScanAt ? F.num(r.count) : "未扫描");
      item.appendChild(count);

      const resc = el("span", "act");
      resc.innerHTML = icon("sync");
      resc.title = "重新扫描该来源";
      resc.addEventListener("click", (e) => {
        e.stopPropagation();
        api.scan(r.id, true);
      });
      item.appendChild(resc);

      const rm = el("span", "act");
      rm.innerHTML = icon("remove");
      rm.title = "从库中移除（不删除文件）";
      rm.addEventListener("click", (e) => {
        e.stopPropagation();
        removeRoot(r);
      });
      item.appendChild(rm);

      item.title = `${r.path}\n${F.num(r.count)} 项 · ${r.lastScanAt ? "扫描于 " + F.ago(r.lastScanAt) : "尚未扫描"}\n右键可只看这个来源`;
      item.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        sourceMenu(item, r);
      });
      dom.roots.appendChild(item);
    }
  }

  function sourceMenu(anchor, r) {
    const box = el("div", "flyout");
    box.style.position = "fixed";
    const rect = anchor.getBoundingClientRect();
    box.style.top = `${Math.max(4, rect.top - 132)}px`;
    box.style.left = `${Math.min(rect.left, window.innerWidth - 260)}px`;
    const close = () => box.remove();

    box.appendChild(el("div", "grp", r.label));

    const only = menuItem("只看这个来源", "filter");
    only.addEventListener("click", () => {
      close();
      S.filter.scope = "root";
      S.filter.rootId = r.id;
      renderScopes();
      renderRoots();
      reload();
    });
    box.appendChild(only);

    const scan = menuItem("重新扫描", "sync");
    scan.addEventListener("click", () => {
      close();
      api.scan(r.id, true);
    });
    box.appendChild(scan);

    const rm = menuItem("从库中移除", "remove", "danger");
    rm.addEventListener("click", () => {
      close();
      removeRoot(r);
    });
    box.appendChild(rm);

    document.body.appendChild(box);
    const onDoc = (e) => {
      if (!box.contains(e.target)) {
        box.remove();
        document.removeEventListener("pointerdown", onDoc, true);
      }
    };
    setTimeout(() => document.addEventListener("pointerdown", onDoc, true), 0);
  }

  function removeRoot(r) {
    confirm(`从库中移除「${r.label}」及其 ${F.num(r.count)} 个条目的标签与备注？磁盘上的文件不会被删除。`).then((ok) => {
      if (ok) api.removeRoot(r.id).then(refreshAll);
    });
  }

  function renderScopes() {
    dom.scopes.textContent = "";
    const counts = S.stats || {};
    for (const sc of SCOPES) {
      if (sc.countKey && !counts[sc.countKey]) continue;
      const on = S.filter.scope === sc.id && S.filter.rootId === null;
      const item = el("div", "side-item" + (on ? " on" : ""));
      item.appendChild(iconEl(sc.icon));
      item.appendChild(el("span", "name", sc.label));
      const c = sc.countKey ? counts[sc.countKey] : counts[sc.id === "all" ? "trackCount" : sc.id];
      // The door to triage belongs to the filter, not to the selection, so it
      // stands on the scope row whenever that scope is the one in view.
      if (on && sc.id === "untagged" && c) {
        const go = el("button", "tri-go");
        go.type = "button";
        go.textContent = "整理";
        go.title = "逐条打标签 (T)";
        go.addEventListener("click", (e) => {
          e.stopPropagation();
          startTriage();
        });
        item.appendChild(go);
      }
      if (c !== undefined && c !== null) item.appendChild(el("span", "count", F.num(c)));
      item.addEventListener("click", () => {
        S.filter.scope = sc.id;
        S.filter.rootId = null;
        renderScopes();
        renderRoots();
        reload();
        // Nothing selected: the panel still has to learn which scope it is in,
        // because that is what decides whether triage can be offered.
        if (!S.sel.size) showDetail();
      });
      dom.scopes.appendChild(item);
    }
  }

  function renderTags() {
    const q = dom.tagFilter.value.trim().toLowerCase();
    dom.tags.textContent = "";
    const list = S.tags.filter((t) => !q || t.name.toLowerCase().includes(q));
    if (!list.length) {
      const d = el("div", "side-item");
      d.style.color = "var(--text-3)";
      d.appendChild(el("span", "name", q ? "没有匹配标签" : "还没有标签"));
      dom.tags.appendChild(d);
      return;
    }
    const sel = new Set(S.filter.tagIds);
    for (const t of list) {
      const item = el("div", "side-item" + (sel.has(t.id) ? " on" : ""));
      item.appendChild(window.TagUI.colorDot(t.color));
      item.appendChild(el("span", "name", t.name));
      item.appendChild(el("span", "count", F.num(t.useCount)));

      const edit = el("span", "act");
      edit.innerHTML = icon("dots");
      edit.title = "重命名 / 颜色 / 删除";
      edit.addEventListener("click", (e) => {
        e.stopPropagation();
        tagMenu(edit, t);
      });
      item.appendChild(edit);

      // One tag at a time: picking a tag shows its files, picking it again
      // drops the filter. Stacking tags read as a filter you could not see.
      item.addEventListener("click", () => {
        S.filter.tagIds = sel.has(t.id) ? [] : [t.id];
        renderTags();
        reload();
      });
      dom.tags.appendChild(item);
    }
  }

  function menuItem(text, iconName, cls) {
    const b = el("button", "mi" + (cls ? " " + cls : ""));
    b.type = "button";
    if (iconName) b.appendChild(iconEl(iconName));
    b.appendChild(el("span", null, text));
    return b;
  }

  function tagMenu(anchor, tag) {
    const box = el("div", "flyout");
    box.style.position = "fixed";
    const r = anchor.getBoundingClientRect();
    box.style.top = `${r.bottom + 4}px`;
    box.style.left = `${Math.min(r.left, window.innerWidth - 260)}px`;

    const swatchRow = el("div", "rowform");
    swatchRow.appendChild(el("span", "hint", "颜色"));
    const colors = ["", "#e81123", "#ff8c00", "#f2c811", "#10893e", "#0099bc", "#0067c0", "#694cc0", "#b4009e", "#8a8a8a"];
    for (const c of colors) {
      const b = el("button", "check");
      b.type = "button";
      b.style.background = c || "var(--bg-input)";
      b.style.borderRadius = "50%";
      b.style.width = "16px";
      b.style.height = "16px";
      if (tag.color === c) b.classList.add("on");
      b.addEventListener("click", () => api.updateTag(tag.id, null, c).then(refreshTags).then(close));
      swatchRow.appendChild(b);
    }
    box.appendChild(swatchRow);

    const rename = menuItem("重命名…", "edit");
    rename.addEventListener("click", async () => {
      close();
      const name = await promptText("重命名标签", tag.name);
      if (name && name !== tag.name) {
        await api.updateTag(tag.id, name, null);
        await refreshTags();
        reload();
      }
    });
    box.appendChild(rename);

    const del = menuItem(`删除标签（${F.num(tag.useCount)} 个条目）`, "remove", "danger");
    del.addEventListener("click", () => {
      close();
      confirm(`删除标签「${tag.name}」？它将从 ${F.num(tag.useCount)} 个条目上移除。`).then(async (ok) => {
        if (ok) {
          await api.deleteTag(tag.id);
          S.filter.tagIds = S.filter.tagIds.filter((x) => x !== tag.id);
          await refreshAll();
        }
      });
    });
    box.appendChild(del);

    const close = () => box.remove();
    document.body.appendChild(box);
    const onDoc = (e) => {
      if (!box.contains(e.target) && e.target !== anchor) {
        box.remove();
        document.removeEventListener("pointerdown", onDoc, true);
      }
    };
    setTimeout(() => document.addEventListener("pointerdown", onDoc, true), 0);
  }

  /* ----------------------------------------------------------------- detail */
  let detailGen = 0;

  async function showDetail() {
    flushRename();
    paintDetailAlias = null;
    const gen = ++detailGen;
    const ids = [...S.sel];
    if (ids.length === 0) {
      dom.detailBody.hidden = true;
      dom.detailEmpty.hidden = false;
      dom.detailEmpty.textContent = "";
      // The untagged filter is the only place triage can begin, so the door
      // stands in the empty panel rather than in the sidebar.
      if (!S.triage.on && S.filter.scope === "untagged" && (S.stats.untagged || 0) > 0) {
        const b = el("button", "btn accent tri-start");
        b.textContent = "开始整理 (T)";
        b.addEventListener("click", startTriage);
        dom.detailEmpty.appendChild(b);
      } else dom.detailEmpty.textContent = "未选择条目";
      S.detail = null;
      return;
    }
    dom.detailEmpty.hidden = true;
    dom.detailBody.hidden = false;
    dom.detailBody.textContent = "";

    // Selection changes never tear the bar down: it holds the track it was last
    // given, which is what a transport pinned under the list is for. Only a
    // single row re-arms it.
    if (ids.length > 1) return renderBulk(ids);

    // Arm playback from the row itself; the awaits below only fill in the panel.
    const armed = S.rows.find((r) => r.id === ids[0]);
    if (armed) window.PlayerView.prepare(armed);

    const d = await api.detail(ids[0]);
    // A newer selection may have resolved first; painting this one afterwards
    // would leave a stale single panel under the current view.
    if (gen !== detailGen) return;
    S.detail = d;
    if (S.triage.on) {
      const idx = S.rows.findIndex((r) => r.id === d.id);
      if (idx >= 0) S.triage.i = idx;
    }
    // The bar loads its own envelope, so the panel paints as soon as the detail
    // does instead of waiting on a second round trip.
    if (S.triage.on) renderTriage(d);
    else renderSingle(d);
  }

  // A click has to name the row it wants played: the detail panel arrives a
  // couple of round trips later and the player must not lag behind it.
  function playRow(r, one) {
    if (!r || !r.decodeable) return;
    if (one) window.PlayerView.setMode("one");
    window.PlayerView.prepare(r);
    window.PlayerView.play(r.id);
  }

  async function loadMoreForQueue() {
    if (S.busy || S.loadedAll) return;
    S.filter.offset = S.rows.length;
    S.filter.limit = PAGE;
    await fetchPage(true);
  }

  // The list is the queue, but only 列表循环 walks it on its own: a track that
  // ends in 只播一次 stays ended. Pressed buttons still wrap — a button that
  // does nothing reads as a broken control.
  async function stepRow(delta, auto) {
    if (!S.rows.length) return;
    const cur = window.PlayerView.currentTrack();
    // An automatic step needs something to have just finished; a pressed button
    // with nothing loaded starts the queue at the end it was asked for.
    if (!cur && auto) return;
    if (auto && window.PlayerView.loopMode() === "off") return window.PlayerView.stop();
    const wrap = !auto || window.PlayerView.loopMode() === "all";
    let i = cur ? S.rows.findIndex((r) => r.id === cur.id) : delta > 0 ? -1 : S.rows.length;
    for (let hops = 0; hops <= S.rows.length + 1; hops++) {
      i += delta;
      if (i === S.rows.length) await loadMoreForQueue();
      if (i >= S.rows.length) {
        if (!wrap) return window.PlayerView.stop();
        i = 0;
      } else if (i < 0) {
        if (!wrap) return window.PlayerView.stop();
        i = S.rows.length - 1;
      }
      const r = S.rows[i];
      // A row the machine cannot decode has nothing to play, so the queue walks
      // past it instead of parking on a silent button.
      if (r && r.decodeable) {
        setSelection([r.id], i);
        ensureCursorVisible(i);
        playRow(r);
        return;
      }
    }
  }

  function section(title, iconName) {
    const s = el("section");
    if (title) {
      const h = el("h3");
      if (iconName) h.appendChild(iconEl(iconName, "h-ico"));
      h.appendChild(el("span", null, title));
      s.appendChild(h);
    }
    dom.detailBody.appendChild(s);
    return s;
  }

  function kv(dl, k, v, cls) {
    const dt = el("dt", null, k);
    const dd = el("dd", cls || null, v);
    dl.append(dt, dd);
  }

  /* --------------------------------------------------------------- renaming */
  // The editor lives inside DOM that other code repaints, so every rebuild
  // path flushes the open one instead of silently discarding the edit.
  let pendingRename = null;
  // Held while the single-item panel is on screen, so a rename started in the
  // list can update its headline without rebuilding the panel.
  let paintDetailAlias = null;

  function flushRename() {
    const p = pendingRename;
    pendingRename = null;
    if (p) p();
  }

  // Display names are library metadata only — the file on disk keeps its name.
  async function applyAlias(row, v) {
    const alias = v && v !== row.name ? v : "";
    await api.setAlias(row.id, alias);
    row.alias = alias || null;
    if (S.detail && S.detail.id === row.id) {
      S.detail.alias = row.alias;
      // Renaming from the list must not re-arm the player, so the open panel
      // repaints its own head instead of being rebuilt from scratch.
      if (paintDetailAlias) paintDetailAlias(row.alias || "");
    }
    renderRows();
    refreshCounts();
    return alias;
  }

  function openRename(host, nodes, current, placeholder, submit) {
    flushRename();
    const input = el("input", "rename");
    input.type = "text";
    input.value = current;
    input.placeholder = placeholder;
    input.spellcheck = false;
    let settled = false;
    const stop = (commit) => {
      if (settled) return;
      settled = true;
      pendingRename = null;
      const v = input.value.trim();
      host.replaceChildren(...nodes);
      if (commit) submit(v);
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        stop(true);
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        stop(false);
      }
    });
    input.addEventListener("blur", () => stop(true));
    host.replaceChildren(input);
    input.focus();
    input.select();
    pendingRename = () => stop(true);
  }

  function renderSingle(d) {
    const head = section();
    const box = el("div", "d-title");
    const nameRow = el("div", "d-namerow");
    const nameEl = el("div", "d-name", d.alias || d.name);
    const pen = el("button", "d-edit");
    pen.type = "button";
    pen.innerHTML = icon("edit");
    pen.title = "改显示名 (F2)";
    // The filename sits at the title's lower right, where it reads as a caption
    // about the file rather than a second heading — and only when it differs.
    const fileEl = el("div", "d-file");
    nameRow.append(nameEl, pen, fileEl);
    box.appendChild(nameRow);
    box.appendChild(el("div", "d-path", d.path));
    head.appendChild(box);

    const paintAlias = (v) => {
      nameEl.textContent = v || d.name;
      nameEl.title = v || d.name;
      fileEl.textContent = v ? d.name : "";
      fileEl.hidden = !v;
    };
    paintAlias(d.alias);
    paintDetailAlias = paintAlias;

    const openAliasEditor = () => {
      const row = S.rows.find((r) => r.id === d.id);
      if (!row) return;
      openRename(nameRow, [nameEl, pen, fileEl], d.alias || "", d.name, async (v) => {
        if (v === (d.alias || "")) return;
        const alias = await applyAlias(row, v);
        paintAlias(alias);
        flash(alias ? `显示名改为「${alias}」` : "已恢复文件名");
      });
    };
    pen.addEventListener("click", openAliasEditor);
    nameEl.addEventListener("dblclick", openAliasEditor);

    const actions = el("div", "d-actions");
    const reveal = el("button", "btn");
    reveal.innerHTML = icon("folderOpen") + "<span>定位文件</span>";
    reveal.addEventListener("click", () => api.reveal(d.id));
    actions.appendChild(reveal);

    const copyPath = el("button", "btn");
    copyPath.innerHTML = icon("copy") + "<span>复制路径</span>";
    copyPath.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(d.path);
        copyPath.querySelector("span").textContent = "已复制";
        setTimeout(() => (copyPath.querySelector("span").textContent = "复制路径"), 1200);
      } catch (_) {}
    });
    actions.appendChild(copyPath);

    const rm = el("button", "btn subtle");
    rm.innerHTML = icon("remove") + "<span>移出库</span>";
    rm.style.color = "var(--danger)";
    rm.addEventListener("click", () =>
      confirm(`把「${d.alias || d.name}」从库中移除？文件保留在磁盘，其标签与备注会被删除。`).then(async (ok) => {
        if (ok) {
          await api.removeTracks([d.id]);
          S.sel.delete(d.id);
          await refreshAll();
        }
      })
    );
    actions.appendChild(rm);
    head.appendChild(actions);
    // Playback lives in the bar under the list, so the panel only explains a file
    // the machine cannot preview at all.
    if (!d.decodeable) {
      head.appendChild(el("div", "bulk-note", "该文件使用本机无法解码的编码（如 WMA），仍可检索、打标与备注。"));
    }

    const metaSec = section("属性", "info");
    const dl = el("dl", "meta-grid");
    kv(dl, "时长", F.ms(d.durationMs));
    kv(dl, "采样率", d.sampleRate ? F.hz(d.sampleRate) : "—");
    kv(dl, "位深", d.bitDepth ? `${d.bitDepth} bit` : "—");
    kv(dl, "声道", d.channels ? F.ch(d.channels) : "—");
    kv(dl, "编码", d.codec || "—");
    kv(dl, "容器", d.container || "—");
    kv(dl, "码率", d.bitrate ? F.bitrate(d.bitrate) : "—");
    kv(dl, "大小", F.bytes(d.sizeBytes));
    kv(dl, "峰值", d.peakDb === null || d.peakDb === undefined ? "—" : `${F.db(d.peakDb)} dBFS`, d.peakDb >= -0.1 ? "warn" : null);
    kv(dl, "平均", d.rmsDb === null || d.rmsDb === undefined ? "—" : `${F.db(d.rmsDb)} dBFS`);
    kv(dl, "文件修改", F.date(d.mtimeMs ? Math.floor(d.mtimeMs / 1000) : d.mtime));
    kv(dl, "加入库", F.date(d.addedAt));
    if (d.playCount) kv(dl, "播放", `${d.playCount} 次 · ${F.ago(d.lastPlayedAt)}`);
    metaSec.appendChild(dl);

    const tagSec = section("标签", "tag");
    const editorHost = el("div", "tag-editor");
    tagSec.appendChild(editorHost);
    const state = {};
    for (const id of d.tagIds || []) state[id] = "on";
    window.TagUI.editor(
      editorHost,
      S.tags,
      state,
      async (tagId, active) => {
        await api.updateTags([d.id], active ? [tagId] : [], active ? [] : [tagId]);
        await afterTagMutation([d.id]);
      },
      async (names) => {
        const ids = await ensureTags(names);
        await api.updateTags([d.id], ids, []);
        await afterTagMutation([d.id]);
      }
    );

    const noteSec = section("备注", "note");
    const ta = el("textarea", "notes");
    ta.value = d.notes || "";
    ta.spellcheck = false;
    const saved = el("div", "bulk-note");
    saved.style.marginTop = "4px";
    noteSec.append(ta, saved);
    const saveNotes = debounce(async () => {
      await api.setNotes([d.id], ta.value, "replace");
      saved.textContent = `已保存 ${new Date().toLocaleTimeString("zh-CN", { hour12: false })}`;
      const row = S.rows.find((r) => r.id === d.id);
      if (row) row.hasNotes = !!ta.value.trim();
      refreshCounts();
      setTimeout(() => (saved.textContent = ""), 1600);
    }, 450);
    ta.addEventListener("input", saveNotes);

    renderStatus();
  }

  /* -------------------------------------------------------------- pane width */
  // The panes are sized by root variables, so a grip only has to move one number
  // and everything hung off it — the grid, the grip's own position, the list
  // columns — follows. Widths are layout state, so they live in localStorage
  // beside the show/hide flags rather than in the library.
  const PANE = {
    sidebar: { v: "--sidebar-w", def: 232, min: 180, max: 440, key: "paneSidebar", grip: "grip-sidebar", off: "no-sidebar" },
    detail: { v: "--detail-w", def: 372, min: 330, max: 660, key: "paneDetail", grip: "grip-detail", off: "no-detail" },
  };
  // Trailing columns are the first thing to fall when a pane is widened, so the
  // list keeps a floor; hiding columns (⋯ / header menu) buys back room.
  const LIST_MIN = 560;

  const gapPane = () =>
    parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--gap-pane")) || 8;

  const paneWidth = (which) =>
    parseInt(getComputedStyle(document.documentElement).getPropertyValue(PANE[which].v), 10) || PANE[which].def;

  // A pane stops at its own ceiling and at whatever leaves the list readable at
  // the current window width; a collapsed neighbour contributes no width at all.
  function setPaneWidth(which, px) {
    const p = PANE[which];
    const otherWhich = which === "sidebar" ? "detail" : "sidebar";
    const other = dom.body.classList.contains(PANE[otherWhich].off) ? 0 : paneWidth(otherWhich);
    const room = dom.body.getBoundingClientRect().width - 2 * gapPane() - LIST_MIN - other;
    // A pane stops at its own floor rather than being crushed to nothing: on a
    // cramped window the list clips its trailing columns instead.
    const hi = Math.max(p.min, Math.min(p.max, room));
    const v = Math.round(Math.max(p.min, Math.min(hi, px)));
    document.documentElement.style.setProperty(p.v, `${v}px`);
    const g = $(p.grip);
    if (g) g.setAttribute("aria-valuenow", String(v));
    return v;
  }

  // Called when the window shrinks or a neighbour appears: keep both panes inside
  // what is left for the list.
  function clampPanes() {
    for (const k of Object.keys(PANE)) setPaneWidth(k, paneWidth(k));
  }

  function bindGrip(grip, which) {
    const p = PANE[which];
    let id = null, startX = 0, startW = 0;
    grip.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      id = e.pointerId;
      grip.setPointerCapture(id);
      startX = e.clientX;
      startW = paneWidth(which);
      document.documentElement.classList.add("resizing");
      grip.classList.add("on");
      e.preventDefault();
    });
    grip.addEventListener("pointermove", (e) => {
      if (id !== e.pointerId) return;
      // The detail pane is anchored to the right edge, so it grows leftwards.
      setPaneWidth(which, startW + (which === "sidebar" ? 1 : -1) * (e.clientX - startX));
    });
    const end = (e) => {
      if (id !== e.pointerId) return;
      id = null;
      document.documentElement.classList.remove("resizing");
      grip.classList.remove("on");
      localStorage.setItem(p.key, String(paneWidth(which)));
    };
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
    grip.addEventListener("dblclick", () => {
      localStorage.removeItem(p.key);
      setPaneWidth(which, p.def);
    });
    grip.addEventListener("keydown", (e) => {
      const dir = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
      if (!dir && e.key !== "Home") return;
      e.preventDefault();
      const next = e.key === "Home" ? p.def : paneWidth(which) + dir * (e.shiftKey ? 64 : 16) * (which === "sidebar" ? 1 : -1);
      setPaneWidth(which, next);
      localStorage.setItem(p.key, String(paneWidth(which)));
    });
  }

  /* ---------------------------------------------------------------- triage */
  // The untagged queue IS the loaded list: tagging an item takes it out of the
  // filter, so the row is spliced locally and the next one slides into the same
  // slot. Sidebar counts and the real query refresh once, when triage ends —
  // not once per item.
  const quickTags = () =>
    [...S.tags].sort((a, b) => (b.useCount || 0) - (a.useCount || 0)).slice(0, 9);

  async function startTriage() {
    if (S.triage.on) return;
    const body = dom.app.querySelector(".body");
    if (body.classList.contains("no-detail")) {
      body.classList.remove("no-detail");
      localStorage.setItem("noDetail", "");
    }
    S.filter.scope = "untagged";
    S.filter.rootId = null;
    S.filter.offset = 0;
    S.filter.limit = Math.max(PAGE, Math.min(2000, (S.stats && S.stats.untagged) || PAGE));
    S.rows = [];
    S.loadedAll = false;
    await fetchPage(false);
    if (!S.rows.length) {
      S.filter.limit = PAGE;
      reload();
      flash("没有未打标签的条目");
      return;
    }
    S.triage = { on: true, i: 0, total: S.rows.length };
    renderScopes();
    selectRow(0, null);
  }

  async function exitTriage() {
    if (!S.triage.on) return;
    S.triage = { on: false, i: 0, total: 0 };
    S.filter.limit = PAGE;
    await Promise.all([refreshTags(), refreshCounts()]);
    reload();
    showDetail();
  }

  const triageCurrent = () => S.rows[S.triage.i] || null;

  // Stepping forward past a row that picked up tags means it has left the
  // untagged set, so it leaves the list too.
  function triageStep(delta) {
    if (!S.rows.length) return exitTriage();
    const r = triageCurrent();
    if (delta > 0 && r && r.tagIds && r.tagIds.length) {
      S.rows.splice(S.triage.i, 1);
      S.total = Math.max(0, S.total - 1);
      if (!S.rows.length) return exitTriage();
      if (S.triage.i >= S.rows.length) S.triage.i = S.rows.length - 1;
      ensureCursorVisible(S.triage.i);
      selectRow(S.triage.i, null);
      return;
    }
    const next = S.triage.i + delta;
    if (next < 0) return;
    if (next >= S.rows.length) return exitTriage();
    S.triage.i = next;
    ensureCursorVisible(next);
    selectRow(next, null);
  }

  // A quick tag is a one-key decision: put it on and move on. Pressing it again
  // while the item already carries it takes it back off and holds position.
  async function triageTag(tagId) {
    const r = triageCurrent();
    if (!r) return;
    const ids = r.tagIds || [];
    if (ids.includes(tagId)) {
      await api.updateTags([r.id], [], [tagId]);
      r.tagIds = ids.filter((x) => x !== tagId);
      showDetail();
      return;
    }
    await api.updateTags([r.id], [tagId], []);
    const t = S.tagById.get(tagId);
    r.tagIds = ids.concat(tagId);
    S.rows.splice(S.triage.i, 1);
    S.total = Math.max(0, S.total - 1);
    if (!S.rows.length) return exitTriage();
    if (S.triage.i >= S.rows.length) S.triage.i = S.rows.length - 1;
    flash(`「${(t && t.name) || ""}」 · 剩余 ${F.num(S.rows.length)}`);
    ensureCursorVisible(S.triage.i);
    selectRow(S.triage.i, null);
  }

  function renderTriage(d) {
    const top = section();
    const bar = el("div", "tri-top");
    bar.appendChild(el("span", "tri-left", `剩余 ${F.num(S.rows.length)}`));
    const quit = el("button", "btn subtle");
    quit.innerHTML = icon("close") + "<span>退出整理</span>";
    quit.addEventListener("click", exitTriage);
    bar.appendChild(quit);
    const prog = el("div", "tri-prog");
    const fill = el("div", "tri-fill");
    const done = S.triage.total - S.rows.length;
    fill.style.width = `${S.triage.total ? Math.round((done / S.triage.total) * 100) : 0}%`;
    prog.appendChild(fill);
    top.append(bar, prog);

    const head = section();
    const nameRow = el("div", "d-namerow");
    nameRow.appendChild(el("div", "d-name", d.alias || d.name));
    if (d.alias) nameRow.appendChild(el("div", "d-file", d.name));
    head.appendChild(nameRow);
    head.appendChild(el("div", "d-path", d.path));

    const tagSec = section("打标签", "tag");
    const qs = quickTags();
    const quick = el("div", "quick-tags");
    qs.forEach((t, idx) => {
      const b = el("button", "qtag" + ((d.tagIds || []).includes(t.id) ? " on" : ""));
      b.type = "button";
      if (t.color) b.appendChild(window.TagUI.colorDot(t.color));
      b.appendChild(el("span", null, t.name));
      b.appendChild(el("kbd", null, String(idx + 1)));
      b.addEventListener("click", () => triageTag(t.id));
      quick.appendChild(b);
    });
    if (qs.length) tagSec.appendChild(quick);
    else tagSec.appendChild(el("div", "bulk-note", "还没有标签，先在左侧新建。"));

    const editorHost = el("div", "tag-editor");
    tagSec.appendChild(editorHost);
    const state = {};
    for (const id of d.tagIds || []) state[id] = "on";
    window.TagUI.editor(
      editorHost,
      S.tags,
      state,
      async (tagId, active) => {
        await api.updateTags([d.id], active ? [tagId] : [], active ? [] : [tagId]);
        const r = triageCurrent();
        if (r) r.tagIds = active ? (r.tagIds || []).concat(tagId) : (r.tagIds || []).filter((x) => x !== tagId);
        // Hand-picking several tags keeps the item on screen; Enter moves on.
        showDetail();
      },
      async (names) => {
        const ids = await ensureTags(names);
        await api.updateTags([d.id], ids, []);
        const r = triageCurrent();
        if (r) r.tagIds = (r.tagIds || []).concat(ids);
        showDetail();
      }
    );

    const navSec = section();
    const nav = el("div", "tri-nav");
    const prev = el("button", "btn subtle");
    prev.innerHTML = icon("up") + "<span>上一条</span>";
    prev.addEventListener("click", () => triageStep(-1));
    const next = el("button", "btn accent");
    next.innerHTML = icon("right") + "<span>下一条</span>";
    next.addEventListener("click", () => triageStep(1));
    nav.append(prev, next);
    navSec.appendChild(nav);
    navSec.appendChild(el("div", "bulk-note", "1-9 打标签并跳过 · Enter 下一条 · 空格试听 · Esc 退出"));

    renderStatus();
  }

  function renderBulk(ids) {
    const head = section();
    head.appendChild(el("div", "d-name", `已选中 ${ids.length} 个条目`));
    const inView = selectedRows();
    const totalDur = inView.reduce((a, r) => a + (r.durationMs || 0), 0);
    const totalSize = inView.reduce((a, r) => a + (r.sizeBytes || 0), 0);
    const sub = el("div", "bulk-note", `合计时长 ${F.time(totalDur)} · ${F.bytes(totalSize)}`);
    head.appendChild(sub);

    const counts = new Map();
    for (const r of inView) for (const t of r.tagIds || []) counts.set(t, (counts.get(t) || 0) + 1);

    const tagSec = section("批量标签", "tagAdd");
    const host = el("div", "tag-editor");
    tagSec.appendChild(host);
    const state = {};
    for (const [tid, c] of counts) state[tid] = c === ids.length ? "on" : "partial";
    window.TagUI.editor(
      host,
      S.tags,
      state,
      async (tagId, active) => {
        await api.updateTags(ids, active ? [tagId] : [], active ? [] : [tagId]);
        await afterTagMutation(ids);
      },
      async (names) => {
        const tids = await ensureTags(names);
        await api.updateTags(ids, tids, []);
        await afterTagMutation(ids);
      }
    );


    const noteSec = section("批量备注", "noteAdd");
    const ta = el("textarea", "notes");
    ta.placeholder = "输入要写入的备注";
    ta.spellcheck = false;
    const row = el("div", "d-actions");
    const rep = el("button", "btn accent", "替换全部");
    const app = el("button", "btn subtle", "追加到末尾");
    const clr = el("button", "btn subtle", "清空备注");
    rep.addEventListener("click", async () => {
      await api.setNotes(ids, ta.value, "replace");
      ta.value = "";
      noteFlash(noteSec, `已写入 ${ids.length} 条`);
      await refreshCounts();
      reload();
    });
    app.addEventListener("click", async () => {
      if (!ta.value.trim()) return;
      await api.setNotes(ids, ta.value, "append");
      ta.value = "";
      noteFlash(noteSec, `已追加到 ${ids.length} 条`);
      await refreshCounts();
    });
    clr.addEventListener("click", async () => {
      await api.setNotes(ids, "", "replace");
      noteFlash(noteSec, `已清空 ${ids.length} 条`);
      await refreshCounts();
    });
    row.append(rep, app, clr);
    noteSec.append(ta, row);

    const rmSec = section("移出库", "remove");
    const rm = el("button", "btn subtle");
    rm.style.color = "var(--danger)";
    rm.innerHTML = icon("remove") + `<span>移除选中的 ${ids.length} 个条目</span>`;
    rm.addEventListener("click", () =>
      confirm(`从库中移除 ${ids.length} 个条目？文件保留在磁盘。`).then(async (ok) => {
        if (ok) {
          await api.removeTracks(ids);
          ids.forEach((i) => S.sel.delete(i));
          await refreshAll();
        }
      })
    );
    rmSec.appendChild(rm);
  }

  function noteFlash(sec, text) {
    const n = el("div", "bulk-note", text);
    n.style.color = "var(--accent)";
    sec.appendChild(n);
    setTimeout(() => n.remove(), 1800);
  }

  async function ensureTags(names) {
    const list = Array.isArray(names) ? names : [names];
    const out = [];
    for (const raw of list) {
      const name = String(raw).trim();
      if (!name) continue;
      let t = S.tags.find((x) => x.name.toLowerCase() === name.toLowerCase());
      if (!t) {
        t = await api.createTag(name, null);
        S.tags.push(t);
      }
      out.push(t.id);
    }
    return out;
  }

  async function afterTagMutation(ids) {
    // One batch query repaints every touched row: a detail call per row turns
    // tagging five hundred items into five hundred round trips.
    const idset = new Set(ids);
    const visible = S.rows.filter((r) => idset.has(r.id)).map((r) => r.id);
    if (visible.length) {
      const flags = await api.rowsFlags(visible);
      for (const r of S.rows) {
        const f = flags[r.id];
        if (!f) continue;
        r.tagIds = f.tagIds;
        r.hasNotes = f.hasNotes;
      }
    }
    await Promise.all([refreshTags(), refreshCounts()]);
    renderRows();
    showDetail();
  }

  /* --------------------------------------------------------------- flyouts */
  function flyoutFor(btn, box, build) {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (!box.hidden) {
        box.hidden = true;
        btn.setAttribute("aria-expanded", "false");
        return;
      }
      box.textContent = "";
      build(box);
      box.hidden = false;
      btn.setAttribute("aria-expanded", "true");
      const onDoc = (ev) => {
        if (!box.contains(ev.target) && ev.target !== btn) {
          box.hidden = true;
          btn.setAttribute("aria-expanded", "false");
          document.removeEventListener("pointerdown", onDoc, true);
        }
      };
      setTimeout(() => document.addEventListener("pointerdown", onDoc, true), 0);
    });
  }

  function activeFilterCount() {
    const f = S.filter;
    let n = 0;
    if (f.formats.length) n++;
    if (f.durations.length) n++;
    if (f.sampleRates.length) n++;
    if (f.channels.length) n++;
    if (f.undecodableOnly) n++;
    return n;
  }

  function buildFilterFlyout(box) {
    const f = S.filter;
    // Called again on every checkbox click, so it must not append to itself.
    box.textContent = "";
    const durCount = new Map((S.stats.durations || []).map((d) => [d.value, d.count]));
    const groups = [
      { title: "格式", key: "formats", items: (S.stats.formats || []).map((x) => ({ v: x.value, label: x.value.toUpperCase().replace(".", ""), count: x.count })) },
      { title: "采样率", key: "sampleRates", items: (S.stats.sampleRates || []).map((x) => ({ v: Number(x.value), label: F.hz(Number(x.value)), count: x.count })) },
      { title: "声道", key: "channels", items: (S.stats.channels || []).map((x) => ({ v: Number(x.value), label: F.ch(Number(x.value)), count: x.count })) },
      {
        title: "时长",
        key: "durations",
        items: [
          { v: "lt1", label: "< 1 秒", count: durCount.get("lt1") },
          { v: "1to3", label: "1–3 秒", count: durCount.get("1to3") },
          { v: "3to10", label: "3–10 秒", count: durCount.get("3to10") },
          { v: "10to60", label: "10–60 秒", count: durCount.get("10to60") },
          { v: "gt60", label: "> 60 秒", count: durCount.get("gt60") },
        ],
      },
    ];

    for (const g of groups) {
      if (!g.items.length) continue;
      box.appendChild(el("div", "grp", g.title));
      for (const it of g.items) {
        const b = el("button", "mi");
        b.type = "button";
        const cb = el("span", "check" + (f[g.key].includes(it.v) ? " on" : ""));
        cb.innerHTML = icon("check");
        b.appendChild(cb);
        b.appendChild(el("span", null, it.label));
        if (it.count !== undefined && it.count !== null) b.appendChild(el("span", "key", F.num(it.count)));
        b.addEventListener("click", () => {
          const arr = f[g.key];
          const i = arr.indexOf(it.v);
          if (i >= 0) arr.splice(i, 1);
          else arr.push(it.v);
          buildFilterFlyout(box);
          updateFilterBadge();
          reload();
        });
        box.appendChild(b);
      }
    }

    box.appendChild(el("div", "sep"));
    const clear = menuItem("清除全部筛选", "clear", "accent");
    clear.addEventListener("click", () => {
      f.formats = [];
      f.durations = [];
      f.sampleRates = [];
      f.channels = [];
      updateFilterBadge();
      buildFilterFlyout(box);
      reload();
    });
    box.appendChild(clear);
  }

  function updateFilterBadge() {
    const n = activeFilterCount();
    dom.filterBadge.hidden = !n;
    dom.filterBadge.textContent = String(n);
  }

  function buildSortFlyout(box) {
    const opts = SORT_FIELDS;
    box.appendChild(el("div", "grp", "排序字段"));
    for (const [k, label] of opts) {
      const on = S.filter.sort === k;
      const b = el("button", "mi" + (on ? " on" : ""));
      b.type = "button";
      const ic = el("span", "ico");
      if (on) ic.innerHTML = icon("check");
      b.appendChild(ic);
      b.appendChild(el("span", null, label));
      if (on) b.appendChild(el("span", "key", S.filter.dir === "asc" ? "升序" : "降序"));
      b.addEventListener("click", () => {
        if (S.filter.sort === k) S.filter.dir = S.filter.dir === "asc" ? "desc" : "asc";
        else {
          S.filter.sort = k;
          S.filter.dir = k === "name" || k === "path" ? "asc" : "desc";
        }
        box.textContent = "";
        box.hidden = true;
        renderHead();
        reload();
      });
      box.appendChild(b);
    }
  }

  /* --------------------------------------------------------------- settings */
  function setGroup(title, iconName) {
    const g = el("div", "set-group");
    const h = el("h3");
    h.appendChild(iconEl(iconName));
    h.appendChild(el("span", null, title));
    g.appendChild(h);
    const card = el("div", "set-card");
    g.appendChild(card);
    dom.settingsBody.appendChild(g);
    return card;
  }

  function setRow(label, desc, control) {
    const row = el("div", "set-row");
    const d = el("div");
    d.appendChild(el("div", "label", label));
    if (desc) d.appendChild(el("div", "desc", desc));
    row.appendChild(d);
    if (control) row.appendChild(control);
    return row;
  }

  const SET_PANES = [
    { id: "look", name: "外观", icon: "palette", build: paneLook },
    { id: "scan", name: "导入与扫描", icon: "folderAdd", build: paneScan },
    { id: "data", name: "数据", icon: "database", build: paneData },
    { id: "update", name: "更新", icon: "sync", build: paneUpdate },
    { id: "about", name: "关于", icon: "info", build: paneAbout },
  ];

  function paneLook() {
    setGroup("外观", "palette").appendChild(
      setSeg(
        "主题",
        [
          ["system", "跟随系统", "panel"],
          ["light", "浅色", "sun"],
          ["dark", "深色", "moon"],
        ],
        S.settings.theme || "system",
        async (v) => {
          await api.settings({ theme: v });
          applyTheme(v);
        }
      )
    );
  }

  function paneScan() {
    const scan = setGroup("导入与扫描", "folderAdd");
    scan.appendChild(
      setSwitch("生成波形峰值", "关闭后导入更快，但不显示波形且无法用峰值筛选", S.settings.genPeaks !== false, async (on) => {
        await api.settings({ genPeaks: on });
      })
    );
    scan.appendChild(
      setSelect("波形精度", "每声道采样的峰值桶数", ["256", "512", "1024", "2048"], String(S.settings.peakBuckets || 512), async (v) => {
        await api.settings({ peakBuckets: parseInt(v, 10) });
      })
    );
    scan.appendChild(
      setSelect(
        "忽略小于",
        "跳过比该字节数更小的文件（占位或损坏文件）",
        [["0", "0 字节"], ["1024", "1 KB"], ["4096", "4 KB"], ["16384", "16 KB"]],
        String(S.settings.minSizeBytes || 0),
        async (v) => {
          await api.settings({ minSizeBytes: parseInt(v, 10) });
        }
      )
    );
    scan.appendChild(
      setSwitch("扫描隐藏文件夹", "包含以 . 或 _ 开头的目录（如版本备份目录、.cache）", !!S.settings.scanHidden, async (on) => {
        await api.settings({ scanHidden: on });
      })
    );
    const rb = el("button", "btn subtle", "开始重建");
    rb.addEventListener("click", async () => {
      dom.dlgSettings.close();
      await api.rebuildPeaks();
    });
    scan.appendChild(setRow("重建全部波形", "修改精度或升级解码器后使用；大库需要数分钟", rb));
  }

  function paneData() {
    const data = setGroup("数据", "database");
    const openBtn = el("button", "btn subtle");
    openBtn.innerHTML = icon("folderOpen") + "<span>打开所在文件夹</span>";
    openBtn.addEventListener("click", () => api.openDbFolder());
    const dbRow = el("div", "set-row");
    const dbd = el("div");
    dbd.appendChild(el("div", "label", "数据库位置"));
    dbd.appendChild(el("code", "path", S.appInfo ? S.appInfo.dbPath : "…"));
    dbRow.append(dbd, openBtn);
    data.appendChild(dbRow);

    const io = el("div", "ctl-row");
    const ex = el("button", "btn subtle");
    ex.innerHTML = icon("export") + "<span>导出</span>";
    ex.addEventListener("click", async () => {
      const path = await api.exportPick();
      if (!path) return;
      const r = await api.exportManifest(path);
      ex.querySelector("span").textContent = `已导出 ${F.num(r.count)} 条`;
      setTimeout(() => (ex.querySelector("span").textContent = "导出"), 2200);
    });
    const imp = el("button", "btn subtle");
    imp.innerHTML = icon("import") + "<span>导入</span>";
    imp.addEventListener("click", async () => {
      const path = await api.importPick();
      if (!path) return;
      const r = await api.importManifest(path);
      imp.querySelector("span").textContent = `已合并 ${F.num(r.updated)} 条`;
      await refreshAll();
      setTimeout(() => (imp.querySelector("span").textContent = "导入"), 2200);
    });
    io.append(ex, imp);
    data.appendChild(setRow("标签与备注备份", "导出为 JSON，换机器或重装后可再导入", io));
  }

  function paneUpdate() {
    const g = setGroup("更新", "sync");
    g.appendChild(setRow("当前版本", "", el("span", "set-val", S.appInfo ? S.appInfo.version : "…")));
    g.appendChild(
      setSwitch("自动检查更新", "启动时匿名查询一次 GitHub Releases，库内容不会离开本机", S.settings.autoUpdate !== false, async (on) => {
        await api.settings({ autoUpdate: on });
      })
    );

    const repo = el("button", "set-link");
    repo.type = "button";
    repo.appendChild(el("span", null, S.appInfo ? S.appInfo.repo : "…"));
    repo.appendChild(iconEl("export"));
    repo.addEventListener("click", () => api.openLink(S.appInfo.repoUrl).catch(() => flash("无法打开浏览器")));
    g.appendChild(setRow("GitHub 仓库", "点击在浏览器中打开项目主页", repo));

    const row = el("div", "set-row");
    const text = el("div");
    text.appendChild(el("div", "label", "检查更新"));
    const note = el("div", "desc", "尚未检查");
    text.appendChild(note);
    const ctl = el("div", "ctl-row");
    const btn = el("button", "btn subtle", "检查更新");
    const go = el("button", "btn accent", "打开下载页");
    go.hidden = true;
    let release = null;
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      go.hidden = true;
      note.textContent = "检查中…";
      release = null;
      try {
        const r = await api.checkUpdate();
        release = r;
        note.textContent = r.newer ? `发现新版本 ${r.version}` : `已是最新版本（${r.version}）`;
        go.hidden = !r.newer;
      } catch (err) {
        note.textContent = `检查失败：${err}`;
      } finally {
        btn.disabled = false;
      }
    });
    go.addEventListener("click", () => release && api.openLink(release.url).catch(() => flash("无法打开浏览器")));
    ctl.append(btn, go);
    row.append(text, ctl);
    g.appendChild(row);
  }

  function paneAbout() {
    const about = setGroup("关于", "info");
    about.appendChild(
      setRow("音频文件", "标签、备注与显示名只写本机数据库，原始文件从不被修改或移动", null)
    );
    about.appendChild(
      setRow(
        "快捷键",
        "空格 播放/暂停 · Alt+←/→ 上一首/下一首 · L 播放方式 · F2 改显示名 · T 整理模式 · Ctrl+I 详情栏 · Ctrl+B 侧栏",
        null
      )
    );
  }

  function buildSettings() {
    dom.settingsBody.textContent = "";
    dom.settingsNav.textContent = "";
    const saved = localStorage.getItem("setPane");
    const active = SET_PANES.find((p) => p.id === saved) || SET_PANES[0];
    for (const p of SET_PANES) {
      const b = el("button", "set-nav-item" + (p.id === active.id ? " on" : ""));
      b.type = "button";
      b.appendChild(iconEl(p.icon));
      b.appendChild(el("span", null, p.name));
      b.addEventListener("click", () => {
        localStorage.setItem("setPane", p.id);
        buildSettings();
      });
      dom.settingsNav.appendChild(b);
    }
    active.build();
  }

  function setSwitch(label, desc, on, onChange) {
    const sw = el("button", "switch" + (on ? " on" : ""));
    sw.type = "button";
    sw.setAttribute("role", "switch");
    sw.setAttribute("aria-checked", String(on));
    sw.addEventListener("click", async () => {
      const next = !sw.classList.contains("on");
      sw.classList.toggle("on", next);
      sw.setAttribute("aria-checked", String(next));
      await onChange(next);
    });
    return setRow(label, desc, sw);
  }

  function setSeg(label, items, current, onChange) {
    const seg = el("div", "seg");
    for (const [v, text, iconName] of items) {
      const b = el("button");
      b.type = "button";
      if (String(current) === String(v)) b.classList.add("on");
      if (iconName) b.appendChild(iconEl(iconName));
      b.appendChild(el("span", null, text));
      b.addEventListener("click", async () => {
        for (const o of seg.children) o.classList.remove("on");
        b.classList.add("on");
        await onChange(v);
      });
      seg.appendChild(b);
    }
    return setRow(label, null, seg);
  }

  function setSelect(label, desc, items, current, onChange) {
    const sel = el("select", "mini");
    for (const it of items) {
      const [v, text] = Array.isArray(it) ? it : [it, it];
      const o = document.createElement("option");
      o.value = String(v);
      o.textContent = text;
      if (String(current) === String(v)) o.selected = true;
      sel.appendChild(o);
    }
    sel.addEventListener("change", () => onChange(sel.value));
    return setRow(label, desc, sel);
  }

  function applyTheme(mode) {
    const root = document.documentElement;
    if (mode === "light" || mode === "dark") root.setAttribute("data-theme", mode);
    else root.removeAttribute("data-theme");
    // The toolbar button offers the theme it would switch *to*.
    const dark = mode === "dark" || ((!mode || mode === "system") && matchMedia("(prefers-color-scheme: dark)").matches);
    const btn = $("btn-theme");
    if (btn) btn.innerHTML = icon(dark ? "sun" : "moon");
  }

  /* ------------------------------------------------------------- dialogs */
  // The UA centres <dialog> with margin:auto; a drag has to freeze that box
  // first, otherwise the panel snaps back to the middle on every pointermove.
  function makeDraggable(dlg, handle) {
    let start = null;
    handle.addEventListener("pointerdown", (e) => {
      if (e.target.closest("button")) return;
      const r = dlg.getBoundingClientRect();
      start = { x: e.clientX, y: e.clientY, left: r.left, top: r.top };
      dlg.style.margin = "0";
      dlg.style.left = `${r.left}px`;
      dlg.style.top = `${r.top}px`;
      handle.setPointerCapture(e.pointerId);
    });
    handle.addEventListener("pointermove", (e) => {
      if (!start) return;
      const maxX = Math.max(0, window.innerWidth - dlg.offsetWidth);
      const maxY = Math.max(0, window.innerHeight - dlg.offsetHeight);
      dlg.style.left = `${Math.min(maxX, Math.max(0, start.left + e.clientX - start.x))}px`;
      dlg.style.top = `${Math.min(maxY, Math.max(0, start.top + e.clientY - start.y))}px`;
    });
    const end = (e) => {
      start = null;
      try {
        handle.releasePointerCapture(e.pointerId);
      } catch (_) {}
    };
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
  }

  let confirmResolve = null;

  function confirm(text) {
    dom.confirmText.textContent = text;
    dom.dlgConfirm.showModal();
    return new Promise((res) => (confirmResolve = res));
  }

  dom.dlgConfirm.addEventListener("close", () => {
    if (confirmResolve) confirmResolve(dom.dlgConfirm.returnValue === "ok");
    confirmResolve = null;
  });

  function promptText(title, value) {
    return new Promise((resolve) => {
      const dlg = el("dialog", "dlg small");
      const head = el("div", "dlg-head");
      head.style.cursor = "default";
      head.appendChild(iconEl("edit"));
      head.appendChild(el("h2", null, title));
      const body = el("div", "dlg-body");
      const input = el("input", "line-edit");
      input.type = "text";
      input.value = value || "";
      body.appendChild(input);
      const foot = el("div", "dlg-foot");
      const cancel = el("button", "btn", "取消");
      cancel.value = "cancel";
      const ok = el("button", "btn accent", "确定");
      ok.value = "ok";
      foot.append(cancel, ok);
      dlg.append(head, body, foot);
      dlg.style.position = "fixed";
      document.body.appendChild(dlg);
      const done = (v) => {
        dlg.close();
        dlg.remove();
        resolve(v);
      };
      cancel.addEventListener("click", () => done(null));
      ok.addEventListener("click", () => done(input.value.trim() || null));
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") done(input.value.trim() || null);
        if (e.key === "Escape") done(null);
      });
      dlg.showModal();
      input.focus();
      input.select();
    });
  }

  /* ------------------------------------------------------------ status bar */
  function renderStatus() {
    const parts = [`${F.num(S.total)} 项`];
    if (S.sel.size) parts.push(`选中 ${S.sel.size}`);
    if (S.filter.text) parts.push(`“${S.filter.text}”`);
    if (S.filter.tagIds.length) {
      const t = S.tags.find((x) => x.id === S.filter.tagIds[0]);
      parts.push(t ? `标签「${t.name}」` : `标签 ${S.filter.tagIds.length} 个`);
    }
    dom.statusCount.textContent = parts.join(" · ");
    dom.statusSel.textContent = S.rows.length && S.rows.length < S.total ? `已加载 ${F.num(S.rows.length)}` : "";
  }

  function setProgress(p) {
    if (!p || !p.active) {
      dom.progress.hidden = true;
      S.scanning = false;
      return;
    }
    S.scanning = true;
    dom.progress.hidden = false;
    const pct = p.total > 0 ? Math.min(100, (p.done / p.total) * 100) : 0;
    dom.progressBar.style.width = `${pct.toFixed(1)}%`;
    dom.progressText.textContent = `${p.phase} ${F.num(p.done)}${p.total ? "/" + F.num(p.total) : ""}`;
  }

  function flash(msg) {
    dom.statusMsg.textContent = msg;
    dom.statusMsg.hidden = !msg;
    clearTimeout(flash.t);
    if (msg) flash.t = setTimeout(() => (dom.statusMsg.hidden = true), 6000);
  }

  /* ---------------------------------------------------------------- refresh */
  async function refreshTags() {
    S.tags = await api.tags();
    S.tagById = new Map(S.tags.map((t) => [t.id, t]));
    renderTags();
  }

  async function refreshCounts() {
    S.stats = await api.stats();
    renderScopes();
    renderRoots();
  }

  async function refreshAll() {
    S.roots = await api.listRoots();
    // Tags load first so the tag rows can name themselves.
    await refreshTags();
    await refreshCounts();
    renderHead();
    await fetchPage(false);
  }

  /* ------------------------------------------------------------- listeners */
  function bind() {
    $("btn-sidebar").addEventListener("click", () => togglePane("sidebar"));
    $("btn-detail").addEventListener("click", () => togglePane("detail"));
    $("btn-theme").addEventListener("click", () => {
      const cur = document.documentElement.getAttribute("data-theme");
      const isDark = cur ? cur === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
      const next = isDark ? "light" : "dark";
      applyTheme(next);
      api.settings({ theme: next });
      S.settings.theme = next;
    });
    $("btn-settings").addEventListener("click", () => {
      buildSettings();
      dom.dlgSettings.showModal();
    });
    dom.btnSettingsClose.addEventListener("click", () => dom.dlgSettings.close());
    makeDraggable(dom.dlgSettings, dom.settingsHead);
    $("btn-add-folder").addEventListener("click", addFolder);
    $("btn-add-folder-2").addEventListener("click", addFolder);
    $("btn-rescan").addEventListener("click", () => api.scan(null, false));

    dom.search.addEventListener("input", debounce(() => {
      S.filter.text = dom.search.value.trim();
      $("btn-search-clear").hidden = !S.filter.text;
      reload();
    }, 130));
    $("btn-search-clear").addEventListener("click", () => {
      dom.search.value = "";
      S.filter.text = "";
      $("btn-search-clear").hidden = true;
      dom.search.focus();
      reload();
    });

    dom.tagFilter.addEventListener("input", renderTags);
    dom.newTag.addEventListener("keydown", async (e) => {
      if (e.key !== "Enter") return;
      const names = e.target.value
        .split(/[,，]/)
        .map((s) => s.trim())
        .filter(Boolean);
      if (!names.length) return;
      e.target.value = "";
      const ids = await ensureTags(names);
      S.filter.tagIds = ids;
      await refreshTags();
      reload();
    });

    flyoutFor($("btn-filter"), $("flyout-filter"), buildFilterFlyout);
    flyoutFor($("btn-sort"), $("flyout-sort"), buildSortFlyout);

    dom.rowsScroll.addEventListener("scroll", scheduleRows);
    window.addEventListener("resize", debounce(() => {
      clampPanes();
      renderRows();
      window.PlayerView.draw();
    }, 120));

    dom.rowsVirtual.addEventListener("pointerdown", (e) => {
      const rowEl = e.target.closest(".row");
      if (!rowEl) return;
      const index = parseInt(rowEl.dataset.index, 10);
      const r = S.rows[index];
      if (e.target.closest(".play")) {
        if (!r.decodeable) return;
        if (!S.sel.has(r.id)) setSelection([r.id], index);
        playRow(r);
        return;
      }
      if (e.target.closest(".row-act")) {
        if (!S.sel.has(r.id)) setSelection([r.id], index);
        rowContextMenu(e.clientX, e.clientY);
        return;
      }
      if (e.button === 1) {
        e.preventDefault();
        return;
      }
      selectRow(index, e);
    });

    dom.rowsVirtual.addEventListener("dblclick", (e) => {
      const rowEl = e.target.closest(".row");
      if (!rowEl) return;
      const r = S.rows[parseInt(rowEl.dataset.index, 10)];
      playRow(r);
    });

    // right click on a row: quick tag menu
    dom.rowsVirtual.addEventListener("contextmenu", (e) => {
      const rowEl = e.target.closest(".row");
      if (!rowEl) return;
      e.preventDefault();
      const index = parseInt(rowEl.dataset.index, 10);
      if (!S.sel.has(S.rows[index].id)) setSelection([S.rows[index].id], index);
      rowContextMenu(e.clientX, e.clientY);
    });

    document.addEventListener("keydown", onKey);

    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
      if (S.settings.theme === "system" || !S.settings.theme) window.PlayerView.draw();
    });
  }

  function togglePane(which) {
    if (which === "sidebar") {
      dom.app.querySelector(".body").classList.toggle("no-sidebar");
      localStorage.setItem("noSidebar", dom.app.querySelector(".body").classList.contains("no-sidebar") ? "1" : "");
    } else {
      dom.app.querySelector(".body").classList.toggle("no-detail");
      localStorage.setItem("noDetail", dom.app.querySelector(".body").classList.contains("no-detail") ? "1" : "");
    }
    scheduleRows();
    clampPanes();
  }

  function rowContextMenu(x, y) {
    const box = el("div", "flyout");
    box.style.position = "fixed";
    box.style.top = `${y}px`;
    box.style.left = `${Math.min(x, window.innerWidth - 240)}px`;
    const ids = [...S.sel];

    const mi = (label, iconName, fn, opts) => {
      const b = menuItem(label, iconName);
      if (opts) {
        if (opts.danger) b.classList.add("danger");
        if (opts.key) b.appendChild(el("span", "key", opts.key));
      }
      b.addEventListener("click", () => {
        box.remove();
        fn();
      });
      box.appendChild(b);
      return b;
    };

    const first = S.rows.find((r) => r.id === ids[0]);

    box.appendChild(el("div", "grp", "播放"));
    if (first && first.decodeable) {
      mi("试听", "playFill", () => playRow(first), { key: "空格" });
      mi("单曲循环本条", "repeatOne", () => playRow(first, true), { key: "L" });
    }
    mi("停止", "stop", () => window.PlayerView.stop());

    box.appendChild(el("div", "sep"));
    box.appendChild(el("div", "grp", "标注"));
    mi(ids.length > 1 ? `添加标签到 ${ids.length} 项…` : "添加标签…", "tagAdd", () => openQuickTag());
    mi("编辑备注", "noteAdd", () => {
      const ta = dom.detailBody.querySelector("textarea.notes");
      if (ta) {
        ta.focus();
        ta.setSelectionRange(ta.value.length, ta.value.length);
      }
    });

    box.appendChild(el("div", "sep"));
    box.appendChild(el("div", "grp", "文件"));
    if (ids.length === 1 && first) mi("复制文件名", "edit", () => navigator.clipboard.writeText(first.alias || first.name));
    mi("复制路径", "copy", () => api.copyPaths(ids));
    if (ids.length === 1) mi("在文件夹中定位", "folderOpen", () => api.reveal(ids[0]));
    mi("移出库", "remove", () => confirm(`从库中移除 ${ids.length} 个条目？文件保留在磁盘。`).then(async (ok) => {
      if (ok) {
        await api.removeTracks(ids);
        ids.forEach((i) => S.sel.delete(i));
        await refreshAll();
      }
    }), { danger: true });

    document.body.appendChild(box);
    const onDoc = (e) => {
      if (!box.contains(e.target)) {
        box.remove();
        document.removeEventListener("pointerdown", onDoc, true);
      }
    };
    setTimeout(() => document.addEventListener("pointerdown", onDoc, true), 0);
  }

  function openQuickTag() {
    const ids = [...S.sel];
    const box = el("div", "suggest quick-tag");
    box.style.position = "fixed";
    box.style.top = "64px";
    box.style.left = "50%";
    box.style.transform = "translateX(-50%)";
    box.style.width = "300px";
    const input = el("input");
    input.placeholder = ids.length > 1 ? `为 ${ids.length} 个条目添加标签` : "输入标签名，回车应用";
    box.appendChild(input);
    const list = el("div", "hits");
    box.appendChild(list);
    const close = () => {
      box.remove();
      document.removeEventListener("pointerdown", onDoc, true);
    };
    const draw = (q) => {
      list.textContent = "";
      const hits = S.tags.filter((t) => t.name.toLowerCase().includes(q.toLowerCase())).slice(0, 8);
      for (const t of hits) {
        const b = window.TagUI.chip(t, { count: t.useCount });
        b.style.cursor = "pointer";
        b.addEventListener("click", async () => {
          await api.updateTags(ids, [t.id], []);
          close();
          await afterTagMutation(ids);
        });
        list.appendChild(b);
      }
      if (!hits.length) list.appendChild(el("div", "hint", "回车创建新标签"));
    };
    input.addEventListener("input", () => draw(input.value.trim()));
    input.addEventListener("keydown", async (e) => {
      if (e.key === "Enter") {
        const names = input.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean);
        if (!names.length) return;
        const tids = await ensureTags(names);
        await api.updateTags(ids, tids, []);
        close();
        await afterTagMutation(ids);
      } else if (e.key === "Escape") close();
    });
    document.body.appendChild(box);
    const onDoc = (e) => {
      if (!box.contains(e.target)) close();
    };
    setTimeout(() => {
      document.addEventListener("pointerdown", onDoc, true);
      input.focus();
      draw("");
    }, 0);
  }

  function onKey(e) {
    const tag = (e.target.tagName || "").toLowerCase();
    const typing = tag === "input" || tag === "textarea" || e.target.isContentEditable;
    const inDialog = [...document.querySelectorAll("dialog")].some((d) => d.open);
    if (inDialog) return;

    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      dom.search.focus();
      dom.search.select();
      return;
    }
    if (e.key === "Escape" && !typing) {
      if (S.triage.on) {
        exitTriage();
        return;
      }
      if (S.filter.text) {
        dom.search.value = "";
        S.filter.text = "";
        $("btn-search-clear").hidden = true;
        reload();
      }
      return;
    }
    if (typing) return;

    // In triage the keys belong to the queue: digits tag, arrows walk, space
    // still auditions the item in place.
    if (S.triage.on && e.key >= "1" && e.key <= "9") {
      e.preventDefault();
      const t = quickTags()[parseInt(e.key, 10) - 1];
      if (t) triageTag(t.id);
      return;
    }
    if (S.triage.on && (e.key === "Enter" || e.key === "ArrowRight")) {
      e.preventDefault();
      triageStep(1);
      return;
    }
    if (S.triage.on && (e.key === "ArrowLeft" || e.key === "Backspace")) {
      e.preventDefault();
      triageStep(-1);
      return;
    }

    switch (e.key) {
      case " ":
      case "Spacebar": {
        e.preventDefault();
        const r = S.rows[S.cursor];
        const armed = window.PlayerView.currentTrack();
        // Space follows the cursor: a row the selection merely moved onto starts
        // here, and only the already armed row pauses or resumes.
        if (!armed || !r || armed.id !== r.id) playRow(r);
        else window.PlayerView.toggle();
        break;
      }
      case "ArrowDown":
      case "ArrowUp": {
        e.preventDefault();
        const d = e.key === "ArrowDown" ? 1 : -1;
        const next = Math.max(0, Math.min(S.rows.length - 1, S.cursor + d));
        if (!S.rows.length) break;
        if (e.shiftKey) {
          const r = S.rows[next];
          if (S.sel.has(r.id)) S.sel.delete(r.id);
          else S.sel.add(r.id);
          S.cursor = next;
          ensureCursorVisible(next);
          renderRows();
          showDetail();
        } else {
          selectRow(next, null);
          ensureCursorVisible(next);
        }
        break;
      }
      case "ArrowLeft":
      case "ArrowRight":
        if (e.altKey) {
          e.preventDefault();
          stepRow(e.key === "ArrowRight" ? 1 : -1, false);
        }
        break;
      case "Home":
        if (S.rows.length) {
          e.preventDefault();
          selectRow(0, null);
          dom.rowsScroll.scrollTop = 0;
        }
        break;
      case "End":
        if (S.rows.length) {
          e.preventDefault();
          selectRow(S.rows.length - 1, null);
          ensureCursorVisible(S.rows.length - 1);
        }
        break;
      case "Enter": {
        const r = S.rows[S.cursor];
        if (r && r.decodeable) {
          e.preventDefault();
          playRow(r);
        }
        break;
      }
      case "F2": {
        if (!S.rows.length) break;
        e.preventDefault();
        startRowRename(S.cursor);
        break;
      }
      case "t":
      case "T":
        if (!e.ctrlKey && !e.metaKey && !S.triage.on) startTriage();
        break;
      case "l":
      case "L":
        if (!e.ctrlKey && !e.metaKey) window.PlayerView.cycleMode();
        break;
      case "Delete": {
        const ids = [...S.sel];
        if (!ids.length) break;
        confirm(`从库中移除 ${ids.length} 个条目？文件保留在磁盘。`).then(async (ok) => {
          if (ok) {
            await api.removeTracks(ids);
            ids.forEach((i) => S.sel.delete(i));
            await refreshAll();
          }
        });
        break;
      }
      default:
        if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
          e.preventDefault();
          setSelection(S.rows.map((r) => r.id), S.cursor);
        } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "i") {
          e.preventDefault();
          togglePane("detail");
        } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "b") {
          e.preventDefault();
          togglePane("sidebar");
        } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "r") {
          e.preventDefault();
          api.scan(null, false);
        }
    }
  }

  async function addFolder() {
    const picked = await api.pickRoot();
    if (picked) {
      await api.addRoot(picked);
      await refreshAll();
    }
  }

  /* ------------------------------------------------------------------ boot */
  async function boot() {
    // index.html carries icons as <span data-icon="name"> placeholders; the
    // path markup itself lives in api.js so there is exactly one copy.
    for (const s of document.querySelectorAll("[data-icon]")) s.innerHTML = icon(s.dataset.icon);
    if (localStorage.getItem("noSidebar") === "1") dom.app.querySelector(".body").classList.add("no-sidebar");
    if (localStorage.getItem("noDetail") === "1") dom.app.querySelector(".body").classList.add("no-detail");
    for (const k of Object.keys(PANE)) {
      const saved = parseInt(localStorage.getItem(PANE[k].key) || "", 10);
      if (saved > 0) setPaneWidth(k, saved);
      bindGrip($(PANE[k].grip), k);
    }
    $("btn-search-clear").hidden = true;

    const info = await api.appInfo();
    S.appInfo = info;
    document.title = info.product || "sfxer";

    const settings = await api.settings({});
    S.settings = settings || {};
    applyTheme(S.settings.theme || "system");

    // The bar outlives every selection, so it is built once, here.
    window.PlayerView.build($("playerbar"), {
      onStep: (d) => stepRow(d, false),
      onEnd: () => stepRow(1, true),
    });
    bind();
    await refreshAll();
    setSelection([], -1);

    T.event.listen("scan://progress", (e) => setProgress(e.payload));
    T.event.listen("scan://done", async (e) => {
      setProgress({ active: false });
      await refreshAll();
      if (e.payload && e.payload.errors) dom.statusCount.title = `${e.payload.errors} 个文件读取失败`;
    });

    if (T.core.invoke === undefined) dom.statusCount.textContent = "未连接到后端";

    // One anonymous request per launch, and silence on failure: a library
    // browser should not nag about a network that happens to be missing.
    if (S.settings.autoUpdate !== false) {
      api
        .checkUpdate()
        .then((r) => r.newer && flash(`sfxer ${r.version} 已发布 · 设置 → 更新`))
        .catch(() => {});
    }

    // A rejected command carries the backend's own message; show it instead of
    // swallowing it in the console.
    window.addEventListener("unhandledrejection", (e) => {
      const msg = typeof e.reason === "string" ? e.reason : (e.reason && e.reason.message) || "";
      if (msg && !/drag-drop/i.test(msg)) flash(msg);
      e.preventDefault();
    });

    // A throw inside a click handler is otherwise invisible, and an invisible
    // failure looks exactly like a control that does nothing.
    window.addEventListener("error", (e) => {
      if (e.message) flash(`界面错误：${e.message}`);
    });

    // player position poll: only while something is playing, at 8 Hz
    let playing = false;
    setInterval(async () => {
      try {
        const status = await api.status();
        const nowPlaying = status && (status.playing || status.paused);
        if (nowPlaying || playing) window.PlayerView.tick(status);
        playing = nowPlaying;
      } catch (_) {}
    }, 125);

    // drag & drop folders anywhere to add them
    try {
      const wv = T.webview ? T.webview.getCurrentWebview() : window.__TAURI__.window.getCurrentWindow();
      await wv.onDragDropEvent(async (e) => {
        if (e.payload.type !== "drop") return;
        const paths = e.payload.paths || [];
        let added = 0;
        let rejected = null;
        for (const p of paths) {
          try {
            await api.addRoot(p);
            added++;
          } catch (err) {
            rejected = String(err);
          }
        }
        await refreshAll();
        // Silence here reads as "the drop did nothing".
        if (added) flash(`已添加 ${added} 个文件夹${rejected ? `，${rejected}` : ""}`);
        else if (rejected) flash(rejected);
      });
    } catch (err) {
      console.warn("drag&drop unavailable:", err);
    }

    // keep tag counts fresh after external edits in the same process
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) refreshCounts();
    });
  }

  boot().catch((e) => {
    dom.listEmpty.hidden = false;
    dom.listEmpty.textContent = `启动失败：${e}`;
    console.error(e);
  });
})();
