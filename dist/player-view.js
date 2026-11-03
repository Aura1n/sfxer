// The transport bar pinned under the list. One player for the whole app, so it
// is built once at boot instead of being rebuilt inside the detail panel.
// Owns presentation and the loop mode; the queue itself belongs to app.js,
// because only the list knows what "next" means.
(() => {
  const { el, icon } = window.DOMX;
  const api = window.API;

  const MODES = ["off", "all", "one"];
  const MODE_TEXT = { off: "只播一次", all: "列表循环", one: "单曲循环" };
  const MODE_ICON = { off: "repeatAll", all: "repeatAll", one: "repeatOne" };

  let root = null;
  let canvas = null;
  let ctx = null;
  let btnPrev = null;
  let btnPlay = null;
  let btnNext = null;
  let btnStop = null;
  let btnLoop = null;
  let titleEl = null;
  let subEl = null;
  let timeEl = null;
  let durEl = null;
  let volEl = null;
  let hintEl = null;
  let hooks = { onStep() {}, onEnd() {} };

  const st = {
    track: null,
    peaks: null,
    ratio: 0,
    hover: null,
    durationMs: 0,
    playing: false,
    paused: false,
    decodeOk: false,
    mode: MODES.includes(localStorage.getItem("loopMode")) ? localStorage.getItem("loopMode") : "off",
    playGlyph: "playFill",
    timeText: "",
    noWaveText: "无波形数据",
  };

  function build(host, h) {
    hooks = Object.assign(hooks, h || {});
    root = host;
    root.classList.add("playerbar");

    const info = el("div", "pb-info");
    const art = el("span", "pb-art");
    art.innerHTML = icon("wave");
    const names = el("div", "pb-names");
    titleEl = el("div", "pb-title", "无音频");
    subEl = el("div", "pb-sub");
    names.append(titleEl, subEl);
    info.append(art, names);

    const mid = el("div", "pb-mid");
    btnPrev = tbtn("skipBack", "上一首 (Alt+←)");
    btnPlay = tbtn("playFill", "播放 / 暂停 (空格)", "main");
    btnNext = tbtn("skipForward", "下一首 (Alt+→)");
    btnStop = tbtn("stopFill", "停止");
    timeEl = el("span", "time");
    st.timeText = "";
    setTime(null);
    durEl = el("span", "time", "—");
    canvas = document.createElement("canvas");
    canvas.className = "wave pb-wave";
    canvas.title = "拖动定位 · 双击回到开头";
    hintEl = el("span", "pb-hint");
    mid.append(btnPrev, btnPlay, btnNext, btnStop, timeEl, canvas, durEl, hintEl);

    const right = el("div", "pb-right");
    btnLoop = tbtn(MODE_ICON[st.mode], `播放方式：${MODE_TEXT[st.mode]} (L)`);
    const vico = el("span", "pb-vico");
    vico.innerHTML = icon("volume");
    volEl = document.createElement("input");
    volEl.type = "range";
    volEl.min = "0";
    volEl.max = "1";
    volEl.step = "0.01";
    volEl.value = String(localStorage.getItem("volume") ?? 0.7);
    volEl.title = "音量";
    right.append(btnLoop, vico, volEl);

    root.append(info, mid, right);
    paintMeta();
    paintMode();
    bind();
    draw();
  }

  function tbtn(name, title, extra) {
    const b = el("button", "tbtn" + (extra ? " " + extra : ""));
    b.type = "button";
    b.title = title;
    b.innerHTML = icon(name);
    return b;
  }

  function bind() {
    btnPlay.addEventListener("click", () => toggle());
    btnStop.addEventListener("click", () => stop());
    btnPrev.addEventListener("click", () => hooks.onStep(-1));
    btnNext.addEventListener("click", () => hooks.onStep(1));
    btnLoop.addEventListener("click", () => cycleMode());
    volEl.addEventListener("input", () => {
      localStorage.setItem("volume", volEl.value);
      api.setVolume(parseFloat(volEl.value));
    });

    const ratioFrom = (ev) => {
      const r = canvas.getBoundingClientRect();
      return Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
    };
    canvas.addEventListener("pointermove", (ev) => {
      st.hover = ratioFrom(ev);
      draw();
    });
    canvas.addEventListener("pointerleave", () => {
      st.hover = null;
      draw();
    });
    canvas.addEventListener("pointerdown", (ev) => {
      canvas.setPointerCapture(ev.pointerId);
      st.hover = ratioFrom(ev);
      seekTo(st.hover);
    });
    canvas.addEventListener("pointerup", (ev) => {
      try {
        canvas.releasePointerCapture(ev.pointerId);
      } catch (_) {}
      st.hover = null;
      draw();
    });
    canvas.addEventListener("dblclick", () => seekTo(0));
  }

  function seekTo(ratio) {
    if (!st.track || !st.decodeOk) return;
    api.seek(Math.round(ratio * st.durationMs)).then(() => {
      st.ratio = ratio;
      setTime(ratio * st.durationMs);
      draw();
    });
  }

  function toggle() {
    if (!st.track) return;
    if (st.playing || st.paused) api.toggle();
    else play();
  }

  // The caller names the track it wants, so a click can never play whatever was
  // loaded before the list finished repainting.
  async function play(id) {
    const target = id || (st.track && st.track.id);
    if (!target) return null;
    const r = await api.play(target, 0, st.mode === "one");
    if (r && r.error) hintEl.textContent = r.error;
    return r;
  }

  function stop() {
    api.stop();
    // Once playback has ended the poller stops ticking, so nothing would clear
    // the readout later — reset it here instead of waiting for a status frame.
    st.ratio = 0;
    st.playing = false;
    st.paused = false;
    setTime(0);
    paintGlyph();
    draw();
  }

  function setMode(mode) {
    if (!MODES.includes(mode)) return;
    st.mode = mode;
    localStorage.setItem("loopMode", mode);
    api.setLoop(mode === "one");
    paintMode();
  }

  function cycleMode() {
    setMode(MODES[(MODES.indexOf(st.mode) + 1) % MODES.length]);
  }

  function paintMode() {
    btnLoop.innerHTML = icon(MODE_ICON[st.mode]);
    btnLoop.title = `播放方式：${MODE_TEXT[st.mode]} (L)`;
    btnLoop.classList.toggle("on", st.mode !== "off");
  }

  // List rows already carry id/durationMs/decodeable, so a click can arm
  // playback right away instead of waiting for a detail round trip.
  function prepare(row) {
    if (!row || (st.track && st.track.id === row.id)) return;
    if (st.playing || st.paused) api.stop();
    st.track = row;
    st.peaks = null;
    st.durationMs = row.durationMs || 0;
    st.decodeOk = false;
    st.ratio = 0;
    st.playing = false;
    st.paused = false;
    // Asking the backend for the envelope of a file it cannot decode would only
    // cost a round trip, so those rows keep the explanatory text instead.
    st.noWaveText = row.decodeable ? "波形尚未生成" : "该容器/编码无法在本机解码";
    if (row.decodeable) loadPeaks(row.id);
    paintMeta();
    setTime(0);
    loadPeaks(row.id);
    draw();
  }

  async function loadPeaks(id) {
    let values = null;
    try {
      const r = await api.peaks(id);
      values = r && r.values && r.values.length ? r.values : null;
    } catch (_) {
      values = null;
    }
    // A newer arm may have landed first; painting this one afterwards would put
    // the previous track's envelope under the current title.
    if (!st.track || st.track.id !== id) return;
    st.peaks = values;
    st.decodeOk = !!st.track.decodeable && values !== null;
    st.noWaveText = st.track.decodeable ? "波形尚未生成" : "该容器/编码无法在本机解码";
    paintMeta();
    draw();
  }

  function paintMeta() {
    const t = st.track;
    titleEl.textContent = t ? t.alias || t.name : "无音频";
    titleEl.title = t ? `${t.alias || t.name}\n${t.path}` : "";
    subEl.textContent = t ? (t.alias ? t.name : "") : "";
    subEl.hidden = !t || !t.alias;
    durEl.textContent = t && st.durationMs > 0 ? window.FMT.time(st.durationMs) : "—";
    hintEl.textContent = t && !t.decodeable ? "不可预览" : "";
    btnPlay.disabled = !t || !t.decodeable;
    btnPlay.style.opacity = t && t.decodeable ? "" : "0.4";
    btnPrev.disabled = !t;
    btnNext.disabled = !t;
    btnStop.disabled = !t;
  }

  function resize() {
    const dpr = window.devicePixelRatio || 1;
    const r = canvas.getBoundingClientRect();
    const w = Math.max(40, Math.floor(r.width * dpr));
    const h = Math.max(12, Math.floor(r.height * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    ctx = canvas.getContext("2d");
  }

  function draw() {
    if (!canvas) return;
    resize();
    const w = canvas.width;
    const h = canvas.height;
    if (!ctx) return;
    const cs = getComputedStyle(document.documentElement);
    const colWave = cs.getPropertyValue("--wave").trim() || "rgba(0,103,192,.75)";
    const colPlayed = cs.getPropertyValue("--wave-played").trim() || "rgba(0,103,192,1)";
    const colTrack = cs.getPropertyValue("--wave-track").trim() || "rgba(0,0,0,.06)";
    const colStroke = cs.getPropertyValue("--stroke").trim() || "rgba(0,0,0,.08)";
    const colDanger = cs.getPropertyValue("--danger").trim() || "#c42b1c";

    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = colTrack;
    ctx.fillRect(0, 0, w, h);

    const mid = h / 2;
    const p = st.peaks;
    if (!p || !p.length) {
      ctx.strokeStyle = colStroke;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, mid + 0.5);
      ctx.lineTo(w, mid + 0.5);
      ctx.stroke();
      if (st.track) {
        ctx.fillStyle = cs.getPropertyValue("--text-3").trim() || "#8c8c8c";
        // Canvas font strings cannot carry a CSS var(), so read the token here.
        ctx.font = `${Math.round(10.5 * (window.devicePixelRatio || 1))}px ${
          cs.getPropertyValue("--font-mono").trim() || "monospace"
        }`;
        ctx.textAlign = "center";
        ctx.fillText(st.noWaveText, w / 2, mid + 3.5 * (window.devicePixelRatio || 1));
      }
      return;
    }

    const buckets = p.length / 2;
    const bw = w / buckets;
    const cut = st.ratio * w;
    const scale = mid - 1;

    for (let i = 0; i < buckets; i++) {
      const mn = p[i * 2];
      const mx = p[i * 2 + 1];
      const x = i * bw;
      const y0 = mid - mx * scale;
      const y1 = mid - mn * scale;
      ctx.fillStyle = x < cut ? colPlayed : colWave;
      const wid = Math.max(1, bw - (buckets > w / 2 ? 0 : 1));
      ctx.fillRect(x, y0, wid, Math.max(1, y1 - y0));
      if (mx >= 0.999) {
        ctx.fillStyle = colDanger;
        ctx.fillRect(x, Math.max(0, y0 - 2 * (window.devicePixelRatio || 1)), wid, 2 * (window.devicePixelRatio || 1));
      }
    }

    // playhead
    ctx.fillStyle = cs.getPropertyValue("--text-1").trim() || "#1b1b1b";
    ctx.fillRect(Math.min(w - 1, cut), 0, 1.5 * (window.devicePixelRatio || 1), h);

    if (st.hover !== null) {
      ctx.fillStyle = colStroke;
      ctx.fillRect(st.hover * w, 0, 1, h);
    }
  }

  function paintGlyph() {
    const glyph = st.playing ? "pauseFill" : "playFill";
    if (glyph !== st.playGlyph) {
      st.playGlyph = glyph;
      btnPlay.innerHTML = icon(glyph);
    }
  }

  // tick() runs at 8 Hz; skip the DOM write when the readout already says this.
  function setTime(ms) {
    const t = ms === null ? "—" : window.FMT.time(ms);
    if (t !== st.timeText) {
      st.timeText = t;
      timeEl.textContent = t;
    }
  }

  function tick(status) {
    if (!status) return;
    const wasPlaying = st.playing;
    st.playing = !!status.playing && !status.paused;
    st.paused = !!status.paused;
    const sameTrack = status.trackId && st.track && status.trackId === st.track.id;
    if (!sameTrack && !status.playing) {
      // A stopped player reports no track, so the readout belongs at the start
      // too; otherwise the playhead resets while the label keeps the old time.
      st.ratio = 0;
      setTime(st.track ? 0 : null);
    } else if (sameTrack && st.durationMs > 0) {
      st.ratio = Math.min(1, (status.positionMs || 0) / st.durationMs);
      setTime(status.positionMs || 0);
    }
    // The last frame of a finished track is the only one the poller sees, so the
    // queue has to move on here rather than on a timer of its own.
    if (wasPlaying && sameTrack && !status.playing && !status.paused) hooks.onEnd();
    if (st.mode === "one" && status.loop === false) api.setLoop(true);
    paintGlyph();
    draw();
  }

  window.PlayerView = {
    build,
    prepare,
    draw,
    tick,
    toggle,
    play,
    stop,
    setMode,
    cycleMode,
    loopMode: () => st.mode,
    currentTrack: () => st.track,
    isPlaying: () => st.playing,
  };
})();
