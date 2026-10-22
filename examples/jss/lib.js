(function () {
  // 不设短路守卫：库必须可覆盖重装，否则改完 lib.js 后页面里仍是旧版本
  // （页面级全局有粘性，__E 会一直存活到导航为止）。
  // 返回值里回 replaced=<旧版本>，让「改了库但页面还是旧实现」立刻可见。
  var prev = window.__E && window.__E.v;
  var E = { v: 3 };

  function norm(s) {
    return String(s == null ? "" : s)
      .replace(/[\u00a0\u200b\u200e\u200f]/g, " ")
      .replace(/[ \t]+/g, " ")
      .replace(/^ +| +$/g, "");
  }
  function cut(s, n) {
    s = norm(s);
    return s.length > n ? s.slice(0, n) + "..." : s;
  }
  function txt(el) {
    if (!el) return "";
    var t = el.innerText;
    if (t == null) t = el.textContent || "";
    return t;
  }
  function abs(u) {
    try { return new URL(u, location.href).href; } catch (e) { return String(u); }
  }
  function esc(q) { return String(q).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }

  E.meta = function () {
    var m = document.querySelector('meta[name="description"]');
    var can = document.querySelector('link[rel="canonical"]');
    var tm = document.querySelector('meta[property="article:published_time"], meta[name="date"], time[datetime]');
    var h1 = document.querySelector("h1");
    var body = txt(document.body);
    return [
      "title=" + norm(document.title),
      "url=" + location.href,
      "host=" + location.host,
      "canonical=" + (can ? abs(can.getAttribute("href")) : "-"),
      "desc=" + cut(m ? m.getAttribute("content") : "", 200),
      "time=" + (tm ? (tm.getAttribute("datetime") || tm.getAttribute("content") || norm(tm.textContent)) : "-"),
      "h1=" + cut(h1 ? txt(h1) : "", 140),
      "chars=" + body.length
    ].join("\n");
  };

  E.grep = function (q, ctx, max, scope) {
    ctx = ctx == null ? 1 : Number(ctx);
    max = max == null ? 40 : Number(max);
    if (!q) return "error=no_query";
    var re = null;
    try { re = new RegExp(String(q), "i"); } catch (e) { re = new RegExp(esc(q), "i"); }
    var root = scope ? document.querySelector(String(scope)) : document.body;
    var raw = txt(root);
    if (!raw) return "error=empty_page";
    var lines = raw.split("\n");
    var out = [];
    var hits = 0;
    var last = -1;
    for (var i = 0; i < lines.length; i++) {
      var L = norm(lines[i]);
      if (!L) continue;
      if (!re.test(L)) continue;
      hits++;
      if (out.length && i - last > ctx + 1) out.push("--");
      var from = Math.max(last + 1, i - ctx);
      for (var j = from; j < i; j++) {
        var C = norm(lines[j]);
        if (C) out.push(" " + (j + 1) + ": " + cut(C, 200));
      }
      out.push(">" + (i + 1) + ": " + cut(L, 400));
      var to = Math.min(i + ctx, lines.length - 1);
      for (var k = i + 1; k <= to; k++) {
        var D = norm(lines[k]);
        if (D) out.push(" " + (k + 1) + ": " + cut(D, 200));
      }
      last = to;
      if (out.length >= max) break;
    }
    if (!hits) return "hits=0 q=" + q;
    return out.join("\n") + "\nhits=" + hits + " shown=" + out.length + (out.length >= max ? " truncated=1" : "");
  };

  E.outline = function (max) {
    max = max == null ? 90 : Number(max);
    var hs = document.querySelectorAll("h1,h2,h3,h4");
    var out = [];
    for (var i = 0; i < hs.length && out.length < max; i++) {
      var t = cut(txt(hs[i]), 110);
      if (!t) continue;
      var lv = Number(hs[i].tagName.slice(1));
      out.push("  ".repeat(lv - 1) + "h" + lv + " " + t);
    }
    return out.length ? out.join("\n") + "\nheadings=" + out.length : "headings=0";
  };

  E.links = function (pat, max) {
    max = max == null ? 60 : Number(max);
    var re = null;
    if (pat) { try { re = new RegExp(String(pat), "i"); } catch (e) { re = new RegExp(esc(pat), "i"); } }
    var as = document.querySelectorAll("a[href]");
    var seen = {};
    var out = [];
    for (var i = 0; i < as.length && out.length < max; i++) {
      var href = abs(as[i].getAttribute("href"));
      if (!/^https?:/i.test(href)) continue;
      var t = cut(txt(as[i]), 90);
      if (re && !(re.test(t) || re.test(href))) continue;
      if (seen[href]) continue;
      seen[href] = 1;
      out.push("- " + (t || "(no text)") + " | " + href);
    }
    return "links=" + out.length + (out.length >= max ? " truncated=1" : "") + "\n" + out.join("\n");
  };

  function pickMain() {
    var sel = ["article", "main", "[role=main]", "#content", "#main", ".content", ".post", ".entry-content", ".article-body", ".post-content"];
    var cands = [];
    for (var i = 0; i < sel.length; i++) {
      var els = document.querySelectorAll(sel[i]);
      for (var j = 0; j < els.length && j < 6; j++) cands.push(els[j]);
    }
    cands.push(document.body);
    var best = document.body, bestScore = -1, name = "body";
    for (var k = 0; k < cands.length; k++) {
      var el = cands[k];
      if (!el) continue;
      var s = txt(el).length;
      if (s < 200) continue;
      var lk = el.querySelectorAll("a").length;
      var ps = el.querySelectorAll("p").length;
      var score = (s + ps * 150) / (1 + lk * 10);
      if (score > bestScore) {
        bestScore = score;
        best = el;
        var cls = typeof el.className === "string" ? norm(el.className).split(" ")[0] : "";
        name = el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + (cls ? "." + cls : "");
      }
    }
    var bodyLen = txt(document.body).length;
    if (best !== document.body && txt(best).length < bodyLen * 0.25) {
      best = document.body;
      name = "body(coverage_fallback)";
    }
    var lines = txt(best).split("\n");
    var out = [];
    var gap = 0;
    for (var m = 0; m < lines.length; m++) {
      var L = norm(lines[m]);
      if (!L) { gap++; continue; }
      if (gap > 0 && out.length) out.push("");
      gap = 0;
      out.push(L);
    }
    return { name: name, text: out.join("\n") };
  }

  E.main = function (max) {
    max = max == null ? 6000 : Number(max);
    var r = pickMain();
    return "main=" + r.name + " chars=" + r.text.length + (r.text.length > max ? " truncated=1" : "") + "\n" + r.text.slice(0, max);
  };

  E.digest = function (max) {
    max = max == null ? 900 : Number(max);
    var heads = document.querySelectorAll("h1,h2,h3");
    var hs = [];
    for (var i = 0; i < heads.length && hs.length < 6; i++) {
      var t = cut(txt(heads[i]), 70);
      if (t) hs.push(t);
    }
    var r = pickMain();
    return "title=" + norm(document.title) +
      "\nurl=" + location.href +
      "\nmain=" + r.name + " chars=" + r.text.length +
      "\nheads=" + (hs.length ? hs.join(" / ") : "-") +
      "\ntext=" + cut(r.text, max);
  };

  E.tables = function (max) {
    max = max == null ? 10 : Number(max);
    var ts = document.querySelectorAll("table");
    var out = [];
    for (var i = 0; i < ts.length && i < max; i++) {
      var rows = ts[i].querySelectorAll("tr");
      out.push("table#" + (i + 1) + " rows=" + rows.length);
      for (var j = 0; j < rows.length && j < 8; j++) {
        var cells = rows[j].querySelectorAll("th,td");
        var cs = [];
        for (var k = 0; k < cells.length; k++) cs.push(cut(txt(cells[k]), 40));
        out.push("  " + cs.join(" | "));
      }
    }
    return out.length ? out.join("\n") : "tables=0";
  };

  window.__E = E;
  var methods = Object.keys(E).filter(function (key) { return key !== "v"; });
  return "lib=installed v=" + E.v +
    (prev && prev !== E.v ? " replaced=" + prev : "") +
    " methods=" + methods.join(",");
})();
