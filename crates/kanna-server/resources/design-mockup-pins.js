// Kanna App Design: pins on an HTML mockup (docs/specs/app-design.md §5).
//
// The artifact preview listener adds this script to the HTML pages it serves
// under its pin path (`/p/<capability>/`). It runs inside the sandboxed
// mockup (an opaque origin with no network), so all it can do is talk to the
// Kanna window by postMessage:
//
// - a click pins the element under it: the page tells Kanna what the element
//   is (tag, id, classes, container, visible text, an HTML excerpt and a
//   selector); Alt/Option-click uses the mockup instead. Marking up is always
//   on (Owner): there is no mode to enter.
// - Kanna sends back the pins to draw (number and selector), and which one
//   the person selected.
//
// The mockup's own scripts share this page, so nothing here is trusted by
// Kanna beyond being a description of what the person clicked.
(() => {
  "use strict";
  if (window.__kannaPins || window.top === window) return;
  window.__kannaPins = true;
  const KIND = "kanna-mockup";
  const kannaWindow = window.top;
  const send = (message) => kannaWindow.postMessage({ kind: KIND, ...message }, "*");
  const squash = (value, max) => String(value || "").replace(/\s+/g, " ").trim().slice(0, max);
  const escapeIdent = (value) =>
    window.CSS && CSS.escape ? CSS.escape(value) : String(value).replace(/[^\w-]/g, (c) => "\\" + c);
  // The page inside the mockup: the path after `/p/<capability>/`.
  const page = location.pathname.split("/").slice(3).map(decodeURIComponent).join("/");

  const layer = document.createElement("div");
  layer.setAttribute("data-kanna-pins", "");
  layer.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647;";
  const hover = document.createElement("div");
  hover.style.cssText =
    "position:fixed;display:none;pointer-events:none;border:2px solid #7b4ff0;" +
    "background:rgba(123,79,240,.08);border-radius:3px;box-sizing:border-box;";
  layer.appendChild(hover);
  const mount = () => (document.body || document.documentElement).appendChild(layer);
  if (document.body) mount();
  else document.addEventListener("DOMContentLoaded", mount);

  const ours = (node) => node && node.nodeType === 1 && layer.contains(node);

  function selectorOf(element) {
    const parts = [];
    for (let node = element; node && node.nodeType === 1 && node !== document.documentElement; node = node.parentElement) {
      if (node.id) {
        parts.unshift("#" + escapeIdent(node.id));
        break;
      }
      let part = node.localName;
      const parent = node.parentElement;
      if (parent) {
        const same = Array.prototype.filter.call(parent.children, (child) => child.localName === node.localName);
        if (same.length > 1) part += ":nth-of-type(" + (same.indexOf(node) + 1) + ")";
      }
      parts.unshift(part);
    }
    return parts.join(" > ");
  }

  function containerOf(element) {
    for (let node = element.parentElement; node && node !== document.body; node = node.parentElement) {
      const landmark = /^(section|article|main|nav|header|footer|aside|form|dialog|table|ul|ol)$/.test(node.localName);
      if (node.id || node.getAttribute("role") || node.getAttribute("aria-label") || landmark) {
        const heading = node.querySelector("h1,h2,h3,h4,h5,h6");
        const name = node.getAttribute("aria-label") || (heading ? heading.textContent : "");
        return squash([node.localName + (node.id ? "#" + node.id : ""), name].filter(Boolean).join(" "), 160);
      }
    }
    return "";
  }

  function describe(element) {
    const rect = element.getBoundingClientRect();
    return {
      page,
      selector: selectorOf(element),
      tag: element.localName,
      elementId: element.id || "",
      classes: squash(Array.prototype.slice.call(element.classList, 0, 12).join(" "), 300),
      container: containerOf(element),
      text: squash(element.innerText || element.textContent, 500),
      html: String(element.outerHTML || "").slice(0, 1000),
      rect: { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
    };
  }

  function outline(box, element) {
    const rect = element.getBoundingClientRect();
    box.style.display = "block";
    box.style.left = rect.left + "px";
    box.style.top = rect.top + "px";
    box.style.width = rect.width + "px";
    box.style.height = rect.height + "px";
  }

  window.addEventListener(
    "mousemove",
    (event) => {
      const target = event.target;
      if (event.altKey || !target || target.nodeType !== 1 || ours(target) || target === document.documentElement) {
        hover.style.display = "none";
        return;
      }
      outline(hover, target);
    },
    true,
  );
  document.addEventListener("mouseleave", () => (hover.style.display = "none"));

  // Capture before the mockup's own handlers: a plain click pins, and does
  // nothing else in the mockup.
  const swallow = (event) => {
    if (event.altKey || ours(event.target)) return false;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
    return true;
  };
  window.addEventListener("mousedown", swallow, true);
  window.addEventListener("mouseup", swallow, true);
  window.addEventListener("auxclick", swallow, true);
  window.addEventListener(
    "click",
    (event) => {
      if (!swallow(event)) return;
      const target = event.target;
      if (!target || target.nodeType !== 1 || target === document.documentElement) return;
      send({ type: "pin", pin: describe(target) });
    },
    true,
  );

  // Pins Kanna asks this page to show.
  let pins = [];
  let selected = null;
  const markers = [];
  function draw() {
    markers.splice(0).forEach((marker) => marker.remove());
    for (const pin of pins) {
      if (pin.page !== page) continue;
      let element = null;
      try {
        element = document.querySelector(pin.selector);
      } catch (_) {
        element = null;
      }
      if (!element) continue;
      const rect = element.getBoundingClientRect();
      const marker = document.createElement("button");
      marker.type = "button";
      marker.textContent = String(pin.number);
      const isSelected = pin.number === selected;
      marker.style.cssText =
        "position:fixed;pointer-events:auto;min-width:20px;height:20px;padding:0 5px;border-radius:10px;" +
        "font:600 11px/20px system-ui,sans-serif;text-align:center;cursor:pointer;border:2px solid #fff;" +
        "box-shadow:0 1px 3px rgba(0,0,0,.35);color:#fff;" +
        "background:" + (pin.resolved ? "#8a8a93" : "#7b4ff0") + ";" +
        "left:" + Math.max(0, rect.right - 10) + "px;top:" + Math.max(0, rect.top - 10) + "px;";
      marker.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        send({ type: "select", number: pin.number });
      });
      layer.appendChild(marker);
      markers.push(marker);
      if (isSelected) {
        const ring = document.createElement("div");
        ring.style.cssText =
          "position:fixed;pointer-events:none;border:2px solid #7b4ff0;border-radius:3px;box-sizing:border-box;";
        outline(ring, element);
        layer.appendChild(ring);
        markers.push(ring);
      }
    }
  }
  let scheduled = false;
  const redraw = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      draw();
    });
  };
  window.addEventListener("scroll", redraw, true);
  window.addEventListener("resize", redraw);
  new MutationObserver((records) => {
    if (records.some((record) => !ours(record.target))) redraw();
  }).observe(document.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });

  window.addEventListener("message", (event) => {
    if (event.source !== kannaWindow) return;
    const message = event.data;
    if (!message || message.kind !== KIND) return;
    if (message.type === "pins" && Array.isArray(message.pins)) {
      pins = message.pins.filter(
        (pin) => pin && typeof pin.selector === "string" && typeof pin.number === "number" && typeof pin.page === "string",
      );
      selected = typeof message.selected === "number" ? message.selected : null;
      draw();
      if (message.reveal && selected !== null) {
        const pin = pins.find((candidate) => candidate.number === selected && candidate.page === page);
        try {
          const element = pin && document.querySelector(pin.selector);
          if (element) element.scrollIntoView({ block: "center", behavior: "smooth" });
        } catch (_) {
          /* a selector this page cannot parse shows no marker */
        }
      }
    }
  });

  send({ type: "ready", page });
})();
