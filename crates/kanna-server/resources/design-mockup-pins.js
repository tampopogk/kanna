// Kanna App Design: pins on an HTML mockup (docs/specs/app-design.md, section 5),
// after the design prototype's review room.
//
// The artifact preview listener adds this script to the HTML pages it serves
// under its pin path (`/p/<capability>/`). It runs inside the sandboxed
// mockup (an opaque origin with no network), so all it can do is talk to the
// Kanna window by postMessage:
//
// - Command-click (Ctrl-click off macOS) pins the element under it and
//   tells Kanna what it is (selector, visible text, tag, label, containing
//   landmark, HTML excerpt). A plain click is the mockup's own, so an
//   interactive mockup stays usable (Owner, 2026-09-27); the outline shows
//   only while the modifier is held.
// - Kanna sends the pins to draw (id, number, selector, text); a pin badge
//   click asks Kanna to focus its thread; pins whose element is gone are
//   reported so their threads can say so.
//
// The mockup's own scripts share this page, so Kanna treats what arrives as a
// description of what the person clicked, and nothing more.
(() => {
  "use strict";
  if (window.__kannaPins || window.top === window) return;
  window.__kannaPins = true;
  const KIND = "kanna-mockup";
  const kanna = window.top;
  const send = (message) => kanna.postMessage({ kind: KIND, ...message }, "*");
  // The page inside the mockup: the path after `/p/<capability>/`.
  const page = location.pathname.split("/").slice(3).map(decodeURIComponent).join("/");

  let pins = [];
  let layer = null;
  let hovered = null;

  const style = document.createElement("style");
  style.textContent =
    ".__kanna-pin{position:absolute;z-index:2147483647;background:#7b4ff0;color:#fff;" +
    "font:700 11px Inter,system-ui,sans-serif;border-radius:10px 10px 10px 2px;padding:2px 7px;" +
    "cursor:pointer;box-shadow:0 1px 4px rgba(0,0,0,.3)}" +
    ".__kanna-hover{outline:2px solid #7b4ff0!important;outline-offset:2px;cursor:crosshair!important}";
  (document.head || document.documentElement).appendChild(style);

  const ours = (node) => !!(node && node.nodeType === 1 && (node.classList.contains("__kanna-pin") || (layer && layer.contains(node))));
  const skipped = (node) => ours(node) || !!(node.closest && node.closest("[data-kanna-ui]"));

  function unhover() {
    if (!hovered) return;
    hovered.classList.remove("__kanna-hover");
    if (!hovered.classList.length) hovered.removeAttribute("class");
    hovered = null;
  }

  function selectorFor(element) {
    if (element.id) return "#" + CSS.escape(element.id);
    const parts = [];
    for (let node = element; node && node.nodeType === 1 && node !== document.body; node = node.parentElement) {
      let index = 1;
      for (let sibling = node.previousElementSibling; sibling; sibling = sibling.previousElementSibling) {
        if (sibling.tagName === node.tagName) index += 1;
      }
      parts.unshift(node.tagName.toLowerCase() + ":nth-of-type(" + index + ")");
    }
    return "body > " + parts.join(" > ");
  }

  // `tag#id.class.class`: how the element reads in a thread.
  const describe = (node) =>
    node.tagName.toLowerCase() +
    (node.id ? "#" + node.id : "") +
    Array.prototype.filter
      .call(node.classList, (name) => !name.startsWith("__"))
      .slice(0, 2)
      .map((name) => "." + name)
      .join("");

  function pinOf(element) {
    // innerText keeps the visual breaks between items; join them with " . "
    // so a container reads as its parts instead of one glued-together word.
    const raw = element.innerText || element.getAttribute("aria-label") || element.getAttribute("alt") || element.title || "";
    const text = raw.split(/\n+/).map((line) => line.trim()).filter(Boolean).join(" \u00b7 ").replace(/\s+/g, " ");
    const container = element.parentElement && element.parentElement.closest("[id], section, header, nav, aside, article, main, footer, form, dialog");
    const clone = element.cloneNode(true);
    for (const node of [clone, ...clone.querySelectorAll("[class]")]) {
      node.classList.remove("__kanna-hover");
      if (!node.classList.length) node.removeAttribute("class");
    }
    const rect = element.getBoundingClientRect();
    return {
      page,
      selector: selectorFor(element),
      excerpt: text.slice(0, 300) || describe(element),
      tag: element.tagName.toLowerCase(),
      label: describe(element),
      context: container ? describe(container) : "",
      html: clone.outerHTML.replace(/\s+/g, " ").slice(0, 400),
      rect: { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
    };
  }

  function find(pin) {
    try {
      const element = document.querySelector(pin.selector);
      if (element) return element;
    } catch (_) {
      /* a selector this page cannot parse: look by text instead */
    }
    if (pin.excerpt) {
      const start = pin.excerpt.slice(0, 30);
      for (const element of document.body.querySelectorAll("*")) {
        if (!element.children.length && element.textContent.trim().startsWith(start)) return element;
      }
    }
    return null;
  }

  function draw() {
    if (layer) layer.remove();
    layer = document.createElement("div");
    layer.setAttribute("data-kanna-pins", "");
    document.body.appendChild(layer);
    const gone = [];
    for (const pin of pins) {
      if (pin.page !== page) continue;
      const element = find(pin);
      if (!element) {
        gone.push(pin.id);
        continue;
      }
      const rect = element.getBoundingClientRect();
      const badge = document.createElement("div");
      badge.className = "__kanna-pin";
      badge.textContent = String(pin.n);
      badge.style.left = rect.right + window.scrollX - 12 + "px";
      badge.style.top = rect.top + window.scrollY - 10 + "px";
      badge.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        send({ type: "focus", id: pin.id });
      });
      layer.appendChild(badge);
    }
    send({ type: "detached", ids: gone });
  }

  // The pin modifier: Command on macOS (Owner: Command-click only there),
  // Control elsewhere, which has no Command key.
  const mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || "");
  const pinning = (event) => (mac ? event.metaKey : event.ctrlKey);

  function hover(target) {
    if (!target || target.nodeType !== 1 || skipped(target) || target === document.documentElement) {
      unhover();
      return;
    }
    if (target === hovered) return;
    unhover();
    hovered = target;
    hovered.classList.add("__kanna-hover");
  }
  let pointed = null;
  document.addEventListener(
    "mousemove",
    (event) => {
      pointed = event.target;
      if (pinning(event)) hover(event.target);
      else unhover();
    },
    true,
  );
  document.addEventListener("mouseout", (event) => {
    if (event.target === hovered) unhover();
  }, true);
  const modifierKey = mac ? "Meta" : "Control";
  window.addEventListener("keydown", (event) => {
    if (event.key === modifierKey) hover(pointed);
  });
  window.addEventListener("keyup", (event) => {
    if (event.key === modifierKey) unhover();
  });
  window.addEventListener("blur", unhover);

  // Command-click pins, and does nothing else in the mockup; a plain click
  // is the mockup's own.
  const swallow = (event) => {
    if (!pinning(event) || skipped(event.target)) return false;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    return true;
  };
  window.addEventListener("mousedown", swallow, true);
  window.addEventListener("mouseup", swallow, true);
  window.addEventListener("auxclick", swallow, true);
  const pick = (event) => {
    if (!swallow(event)) return;
    const element = event.target;
    if (!element || element.nodeType !== 1 || element === document.documentElement) return;
    unhover();
    send({ type: "pick", pin: pinOf(element) });
  };
  window.addEventListener("click", pick, true);

  let scheduled = false;
  const redraw = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      draw();
    });
  };
  window.addEventListener("resize", redraw);
  // An interactive mockup changes its own page: keep the badges on their elements.
  const isLayer = (node) => node.nodeType === 1 && node.hasAttribute("data-kanna-pins");
  const theirs = (record) => {
    if (layer && (layer.contains(record.target) || ours(record.target))) return false;
    const changed = [...record.addedNodes, ...record.removedNodes];
    return !(changed.length && changed.every(isLayer));
  };
  new MutationObserver((records) => {
    if (records.some(theirs)) redraw();
  }).observe(document.documentElement, { subtree: true, childList: true, characterData: true });

  window.addEventListener("message", (event) => {
    if (event.source !== kanna) return;
    const message = event.data;
    if (!message || message.kind !== KIND || message.type !== "pins" || !Array.isArray(message.pins)) return;
    pins = message.pins.filter(
      (pin) =>
        pin &&
        typeof pin.id === "string" &&
        typeof pin.n === "number" &&
        typeof pin.selector === "string" &&
        typeof pin.page === "string",
    );
    draw();
    if (typeof message.reveal === "string") {
      const pin = pins.find((candidate) => candidate.id === message.reveal && candidate.page === page);
      const element = pin && find(pin);
      if (element) element.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  });

  send({ type: "ready", page });
})();
