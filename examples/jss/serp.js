(function () {
  function norm(s) {
    return String(s == null ? "" : s).replace(/[\u00a0]/g, " ").replace(/[ \t]+/g, " ").replace(/^ +| +$/g, "");
  }
  function cut(s, n) {
    s = norm(s);
    return s.length > n ? s.slice(0, n) + "..." : s;
  }
  function deRef(u) {
    try {
      var x = new URL(u, location.href);
      var d = x.searchParams.get("uddg") || x.searchParams.get("q") || x.searchParams.get("url");
      if (/duckduckgo/.test(x.host) && d && /^https?:/.test(d)) return d;
      return x.href;
    } catch (e) { return String(u); }
  }
  function inner(el, sel) {
    if (!el) return null;
    return el.querySelector(sel);
  }

  var host = location.host;
  var body = (document.body && document.body.innerText) || "";
  var rows = [];
  var engine = "unknown";

  if (/bing\./i.test(host)) {
    engine = "bing";
    var ns = document.querySelectorAll("li.b_algo");
    for (var i = 0; i < ns.length; i++) {
      var a = inner(ns[i], "h2 a") || inner(ns[i], "a");
      if (!a) continue;
      var sn = inner(ns[i], ".b_caption p") || inner(ns[i], ".b_lineclamp2") || inner(ns[i], "p");
      rows.push([norm(a.innerText), deRef(a.getAttribute("href")), cut(sn ? sn.innerText : "", 220)]);
    }
  } else if (/duckduckgo/i.test(host)) {
    engine = "ddg";
    var ns2 = document.querySelectorAll(".result, .web-result");
    for (var j = 0; j < ns2.length; j++) {
      var a2 = inner(ns2[j], "a.result__a") || inner(ns2[j], "h2 a");
      if (!a2) continue;
      var sn2 = inner(ns2[j], ".result__snippet") || inner(ns2[j], "a.result__snippet");
      rows.push([norm(a2.innerText), deRef(a2.getAttribute("href")), cut(sn2 ? sn2.innerText : "", 220)]);
    }
  } else if (/google\./i.test(host)) {
    engine = "google";
    var ns3 = document.querySelectorAll("#rso div.g, #rso div[data-hveid]");
    for (var k = 0; k < ns3.length; k++) {
      var h3 = inner(ns3[k], "h3");
      if (!h3) continue;
      var a3 = h3.closest("a") || inner(ns3[k], "a");
      if (!a3) continue;
      var sn3 = inner(ns3[k], ".VwiC3b") || inner(ns3[k], "div[data-sncf]");
      rows.push([norm(h3.innerText), deRef(a3.getAttribute("href")), cut(sn3 ? sn3.innerText : "", 220)]);
    }
  } else if (/search\.brave|startpage|mojeek|ecosia/i.test(host)) {
    engine = "alt";
    var ns4 = document.querySelectorAll("div.snippet, .result, article");
    for (var m = 0; m < ns4.length; m++) {
      var a4 = ns4[m].querySelector("a[href^='http']");
      if (!a4) continue;
      rows.push([norm(a4.innerText), deRef(a4.getAttribute("href")), cut(ns4[m].innerText, 200)]);
    }
  }

  if (!rows.length) {
    if (/unusual traffic|not a robot|verify you are human|captcha|bots use|complete the following challenge|select all squares/i.test(body)) {
      return "serp=challenge engine=" + engine + " hint=bot_wall_use_another_engine";
    }
    if (/enable javascript|consent|before you continue/i.test(body) && body.length < 4000) return "serp=interstitial engine=" + engine + " hint=consent_or_js_wall";
    return "serp=empty engine=" + engine + " chars=" + body.length + " hint=check_selector_or_page_state";
  }

  // 先按 URL 去重收集，再按字符预算封顶输出：results= 只数真正写进正文的行，
  // 避免 CLI 侧 --budget 截断后「头部计数虚高、尾部其实没发出来」。
  var MAX_ROWS = 20;
  var MAX_CHARS = 5000;
  var clean = [];
  var seen = {};
  for (var p = 0; p < rows.length && clean.length < MAX_ROWS; p++) {
    var t = rows[p][0], u = rows[p][1], s = rows[p][2];
    if (!t || !u || !/^https?:/i.test(u)) continue;
    if (seen[u]) continue;
    seen[u] = 1;
    clean.push([cut(t, 100), u, s]);
  }
  var out = [];
  var chars = 0;
  for (var q = 0; q < clean.length; q++) {
    var line = (q + 1) + "|" + clean[q][0] + "|" + clean[q][1] + "|" + clean[q][2];
    // 至少输出一行：单条超长结果仍要可见。
    if (out.length && chars + line.length > MAX_CHARS) break;
    chars += line.length + 1;
    out.push(line);
  }
  var tail = "serp=ok engine=" + engine +
    " results=" + out.length + " total=" + clean.length + " chars=" + chars +
    (out.length < clean.length ? " capped=1" : "");
  return tail + "\n" + out.join("\n");
})();
