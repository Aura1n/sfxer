/* sfxer — list column layout (width, order) and the sidebar splitter.
   Pure DOM + localStorage: app.js hands over the header element and a few
   hooks, and owns everything that touches library state. Layout is a per-machine
   preference, so it lives beside the other pane sizes rather than in the library. */
(() => {
  const read = (k, fallback) => {
    try {
      const v = JSON.parse(localStorage.getItem(k));
      return v === null || v === undefined ? fallback : v;
    } catch (e) {
      return fallback;
    }
  };
  const save = (k, v) => localStorage.setItem(k, JSON.stringify(v));
  const root = document.documentElement;

  // A column never shrinks below what its heading and a typical value need.
  const MIN = { name: 80, dur: 40, sr: 40, bits: 32, ch: 32, fmt: 40, size: 48, tags: 48 };
  const MAX = 640;

  let widths = read("colW", {}); // key -> px; absent means "use the default track"
  let order = read("colOrder", []); // movable keys, left to right

  const persist = () => {
    save("colW", widths);
    save("colOrder", order);
  };

  const track = (c) => {
    const px = widths[c.key];
    if (!px) return c.w;
    // The last, flexible column treats its width as a floor and keeps absorbing
    // spare room; the rest give way gracefully when the window narrows.
    if (c.key === "tags") return `minmax(${px}px, 1fr)`;
    return `minmax(${Math.min(px, MIN[c.key] || 32)}px, ${px}px)`;
  };

  // Leading fixed cells, then the movable columns in the saved order, then the
  // trailing action cell. Columns the saved order has never heard of keep their
  // default relative position at the end.
  function arrange(cols) {
    const rank = (k) => {
      const i = order.indexOf(k);
      return i < 0 ? 1000 : i;
    };
    const lead = cols.filter((c) => c.fixed && c.key !== "act");
    const tail = cols.filter((c) => c.key === "act");
    const movable = cols.filter((c) => !c.fixed).sort((a, b) => rank(a.key) - rank(b.key));
    return [...lead, ...movable, ...tail].map((c) => (c.fixed ? c : { ...c, w: track(c) }));
  }

  function reset() {
    widths = {};
    order = [];
    persist();
  }

  // Dressing a header button: it gets its key and a resize handle on its edge.
  function mark(button, col) {
    button.dataset.key = col.key;
    button.classList.add("hcell");
    const grip = document.createElement("span");
    grip.className = "col-grip";
    grip.setAttribute("role", "separator");
    grip.title = "拖动调整列宽 · 双击恢复默认";
    button.appendChild(grip);
  }

  // hooks: { relayout(), reorder(), overflow(), cols() }
  function bind(head, hooks) {
    let suppress = false;
    const swallowNextClick = () => {
      suppress = true;
      setTimeout(() => (suppress = false), 0);
    };
    // A drag ends with a click on the heading it started on; that must not sort.
    head.addEventListener(
      "click",
      (e) => {
        if (!suppress) return;
        e.stopPropagation();
        e.preventDefault();
      },
      true
    );

    head.addEventListener("dblclick", (e) => {
      const grip = e.target.closest(".col-grip");
      if (grip) {
        delete widths[grip.parentElement.dataset.key];
        persist();
        hooks.relayout();
        return;
      }
      // Empty header space resets the whole arrangement.
      if (e.target.closest("button, .head-check")) return;
      reset();
      hooks.reorder();
    });

    head.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      const cell = e.target.closest("button.hcell");
      if (!cell) return;
      if (e.target.closest(".col-grip")) startResize(e, cell);
      else startMove(e, cell);
    });

    const watch = (move, up) => {
      const stop = (ev) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", stop);
        window.removeEventListener("pointercancel", stop);
        up(ev);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", stop);
      window.addEventListener("pointercancel", stop);
    };

    function startResize(e, cell) {
      e.preventDefault();
      e.stopPropagation();
      const key = cell.dataset.key;
      const grip = e.target.closest(".col-grip");
      const index = [...head.children].indexOf(cell);
      const tracks = getComputedStyle(head).gridTemplateColumns.split(/\s+/).map(parseFloat);
      const startW = tracks[index] || cell.getBoundingClientRect().width;
      const startX = e.clientX;
      let good = Math.round(startW);
      grip.classList.add("on");
      root.classList.add("col-resizing");
      watch(
        (ev) => {
          const w = Math.round(Math.max(MIN[key] || 32, Math.min(MAX, startW + ev.clientX - startX)));
          widths[key] = w;
          hooks.relayout();
          // Past the pane edge the trailing cells would be pushed out of reach.
          if (hooks.overflow()) {
            widths[key] = good;
            hooks.relayout();
          } else {
            good = w;
          }
        },
        () => {
          grip.classList.remove("on");
          root.classList.remove("col-resizing");
          persist();
          swallowNextClick();
        }
      );
    }

    function startMove(e, cell) {
      const key = cell.dataset.key;
      const startX = e.clientX;
      const cells = () => [...head.querySelectorAll("button.hcell")];
      let active = false;
      let marker = null;
      let before = null; // the key the dragged column will land in front of
      watch(
        (ev) => {
          if (!active) {
            if (Math.abs(ev.clientX - startX) < 6) return;
            active = true;
            cell.classList.add("dragging");
            root.classList.add("col-moving");
            marker = document.createElement("div");
            marker.className = "col-drop";
            head.appendChild(marker);
          }
          const box = head.getBoundingClientRect();
          const others = cells().filter((c) => c !== cell);
          const hit = others.find((c) => {
            const r = c.getBoundingClientRect();
            return ev.clientX < r.left + r.width / 2;
          });
          before = hit ? hit.dataset.key : null;
          const x = hit
            ? hit.getBoundingClientRect().left - box.left - 3
            : others.length
              ? others[others.length - 1].getBoundingClientRect().right - box.left + 3
              : 0;
          marker.style.left = `${Math.round(x)}px`;
        },
        () => {
          if (!active) return;
          cell.classList.remove("dragging");
          root.classList.remove("col-moving");
          if (marker) marker.remove();
          swallowNextClick();
          const seen = cells()
            .map((c) => c.dataset.key)
            .filter((k) => k !== key);
          const at = before === null ? seen.length : Math.max(0, seen.indexOf(before));
          seen.splice(at, 0, key);
          order = seen.concat(order.filter((k) => !seen.includes(k)));
          persist();
          hooks.reorder();
        }
      );
    }
  }

  /* ----------------------------------------------------- sidebar splitter */
  // Between the scope list and the tag list. Without it the scope list is only
  // as tall as the window leaves it, and the entries at its foot cannot be reached.
  function bindSideSplit() {
    const grip = document.getElementById("grip-side");
    const side = document.getElementById("sidebar");
    if (!grip || !side) return;
    const block = side.querySelector(".side-block");
    const KEY = "paneScopeH";
    const limits = () => ({ lo: 72, hi: Math.max(96, side.clientHeight - 150) });
    const apply = (px) => {
      const { lo, hi } = limits();
      const v = Math.round(Math.max(lo, Math.min(hi, px)));
      block.style.flex = `0 0 ${v}px`;
      return v;
    };
    const saved = parseInt(localStorage.getItem(KEY) || "", 10);
    if (saved > 0) apply(saved);

    grip.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const startY = e.clientY;
      const startH = block.getBoundingClientRect().height;
      let last = startH;
      grip.classList.add("on");
      root.classList.add("row-resizing");
      const move = (ev) => {
        last = apply(startH + ev.clientY - startY);
      };
      const up = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        window.removeEventListener("pointercancel", up);
        grip.classList.remove("on");
        root.classList.remove("row-resizing");
        localStorage.setItem(KEY, String(last));
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
      window.addEventListener("pointercancel", up);
    });
    grip.addEventListener("dblclick", () => {
      localStorage.removeItem(KEY);
      block.style.flex = "";
    });
    // A shorter window must not leave the tag list with no room at all.
    window.addEventListener("resize", () => {
      if (block.style.flex) apply(block.getBoundingClientRect().height);
    });
  }

  window.ColLayout = { arrange, mark, bind, reset, bindSideSplit };
})();
