// Tag chips, the tag editor, and its autocomplete popup. Shared by the sidebar,
// list rows, the filter flyout and the detail panel.
(() => {
  const { el, ICONS } = window.DOMX;

  function colorDot(color) {
    const d = el("span", "dot");
    d.style.background = color || "var(--text-3)";
    return d;
  }

  function chip(tag, opts) {
    const c = el("span", "chip");
    if (opts && opts.on) c.classList.add("on");
    if (opts && opts.partial) c.classList.add("partial");
    if (tag.color) c.appendChild(colorDot(tag.color));
    c.appendChild(el("span", null, tag.name));
    if (opts && opts.count !== undefined && opts.count !== null) {
      const k = el("span", "count", String(opts.count));
      k.style.cssText = "color:var(--text-3);font:400 10px var(--font-mono)";
      c.appendChild(k);
    }
    if (opts && opts.removable) {
      const x = el("span", "x", "\u00d7");
      x.title = "移除";
      c.appendChild(x);
    }
    if (opts && opts.title) c.title = opts.title;
    if (opts && opts.onClick) c.addEventListener("click", (e) => opts.onClick(e, c));
    return c;
  }

  // Suggest popup anchored under an input; Esc/Enter/arrow keys handled here.
  function suggest(input, getSource, onPick) {
    let box = null;
    let items = [];
    let cur = -1;

    const close = () => {
      if (box) {
        box.remove();
        box = null;
      }
      cur = -1;
    };

    const draw = () => {
      if (!box) return;
      box.textContent = "";
      items.forEach((it, i) => {
        const b = el("button", "mi" + (i === cur ? " cur" : ""));
        b.type = "button";
        if (it.color) b.appendChild(colorDot(it.color));
        b.appendChild(el("span", null, it.name));
        if (it.count) {
          const s = el("span", "count", String(it.count));
          s.style.cssText = "margin-left:auto;color:var(--text-3);font:400 10px var(--font-mono)";
          b.appendChild(s);
        }
        b.addEventListener("mousedown", (e) => {
          e.preventDefault();
          close();
          onPick(it);
        });
        box.appendChild(b);
      });
    };

    const open = () => {
      const q = input.value.trim().toLowerCase();
      items = getSource(q).slice(0, 12);
      if (!items.length) {
        close();
        return;
      }
      if (!box) {
        box = el("div", "suggest");
        document.body.appendChild(box);
        const r = input.getBoundingClientRect();
        box.style.left = `${r.left}px`;
        box.style.top = `${r.bottom + 4}px`;
        box.style.minWidth = `${Math.max(r.width, 180)}px`;
      }
      cur = 0;
      draw();
    };

    input.addEventListener("input", open);
    input.addEventListener("focus", open);
    input.addEventListener("blur", () => setTimeout(close, 120));
    input.addEventListener("keydown", (e) => {
      if (!box) return;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        cur = Math.min(cur + 1, items.length - 1);
        draw();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        cur = Math.max(cur - 1, 0);
        draw();
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
      } else if (e.key === "Enter" && cur >= 0) {
        e.preventDefault();
        const it = items[cur];
        close();
        onPick(it);
      }
    });

    return { close, open };
  }

  // Tri-state chip editor used in the detail panel: `state` maps tagId ->
  // "on" | "partial" | "off". onToggle receives (tagId, nextActive).
  function editor(host, allTags, state, onToggle, onCreate) {
    host.textContent = "";
    const shown = new Set();
    const order = ["on", "partial", "off"];
    const sorted = [...allTags].sort((a, b) => {
      const sa = state[a.id] || "off";
      const sb = state[b.id] || "off";
      if (sa !== sb) return order.indexOf(sa) - order.indexOf(sb);
      return (b.useCount || 0) - (a.useCount || 0) || a.name.localeCompare(b.name);
    });

    for (const t of sorted) {
      const st = state[t.id] || "off";
      if (st === "off") continue;
      shown.add(t.id);
      host.appendChild(
        chip(t, {
          on: st === "on",
          partial: st === "partial",
          removable: true,
          title: st === "partial" ? "部分选中项带有该标签，点击为全部添加" : "点击移除",
          onClick: () => onToggle(t.id, st !== "on"),
        })
      );
    }

    const input = el("input");
    input.type = "text";
    input.placeholder = shown.size ? "添加标签…" : "输入标签名，回车新建";
    input.spellcheck = false;
    host.appendChild(input);

    host.addEventListener("click", (e) => {
      if (e.target === host) input.focus();
    });

    const lookup = (q) =>
      allTags.filter((t) => !shown.has(t.id) && (!q || t.name.toLowerCase().includes(q)));
    const sg = suggest(input, lookup, (t) => {
      input.value = "";
      onToggle(t.id, true);
    });

    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        const name = input.value.trim();
        if (!name) return;
        input.value = "";
        sg.close();
        onCreate(name);
      } else if (e.key === "Backspace" && !input.value) {
        const last = [...shown];
        if (last.length) onToggle(last[last.length - 1], false);
      }
    });

    input.addEventListener("paste", (e) => {
      const txt = (e.clipboardData || window.clipboardData).getData("text");
      if (!txt || !/[,，\n]/.test(txt)) return;
      e.preventDefault();
      const names = txt
        .split(/[,，\n]/)
        .map((s) => s.trim())
        .filter(Boolean);
      if (names.length) onCreate(names);
      input.value = "";
    });

    return input;
  }

  window.TagUI = { chip, suggest, editor, colorDot };
})();
