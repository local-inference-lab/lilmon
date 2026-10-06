// Copyright 2026 Local Inference Lab, Inc.
// SPDX-License-Identifier: Apache-2.0
//
// A small standalone renderer for lilmon's design-component page (index.html). It supports the
// subset the page uses: {{dotted.path}} holes in text and attributes, <sc-for list as>, <sc-if value>,
// on* event attributes bound to functions, <helmet> (moved into <head>), data-props defaults (also
// settable from the URL, e.g. ?theme=lil-light&window=1h), setState and componentDidMount.
// Each render is morphed into the live DOM so focus and hover survive the once-a-second updates.
(function () {
  'use strict';

  class DCLogic {
    constructor(props) {
      this.props = props || {};
      this.state = {};
    }
    setState(patch) {
      const next = typeof patch === 'function' ? patch(this.state, this.props) : patch;
      this.state = Object.assign({}, this.state, next);
      if (this.__schedule) this.__schedule();
    }
    forceUpdate() {
      if (this.__schedule) this.__schedule();
    }
  }
  window.DCLogic = DCLogic;

  const HOLE = /\{\{\s*([^}]+?)\s*\}\}/g;
  const WHOLE = /^\s*\{\{\s*([^}]+?)\s*\}\}\s*$/;

  function lookup(path, scopes) {
    if (path === 'true') return true;
    if (path === 'false') return false;
    if (path === 'null') return null;
    if (/^-?\d+(\.\d+)?$/.test(path)) return Number(path);
    const parts = path.split('.');
    for (let i = scopes.length - 1; i >= 0; i--) {
      const s = scopes[i];
      if (s != null && Object.prototype.hasOwnProperty.call(s, parts[0])) {
        let v = s;
        for (const p of parts) {
          if (v == null) return undefined;
          v = v[p];
        }
        return v;
      }
    }
    return undefined;
  }

  function interp(str, scopes) {
    return str.replace(HOLE, (_, p) => {
      const v = lookup(p, scopes);
      return v == null ? '' : String(v);
    });
  }

  // Delegating listeners: one per element and event type, reading the current handler from el.__h,
  // so a morph can swap handlers without re-binding.
  function bind(el, type) {
    el.__t = el.__t || new Set();
    if (el.__t.has(type)) return;
    el.__t.add(type);
    el.addEventListener(type, (e) => {
      const h = el.__h && el.__h[type];
      if (typeof h === 'function') h(e);
    });
  }

  function render(node, scopes, out) {
    if (node.nodeType === 3) {
      out.appendChild(document.createTextNode(interp(node.nodeValue, scopes)));
      return;
    }
    if (node.nodeType !== 1) return;
    const tag = node.localName;
    if (tag === 'sc-for') {
      const m = WHOLE.exec(node.getAttribute('list') || '');
      const list = (m && lookup(m[1], scopes)) || [];
      const as = node.getAttribute('as') || 'item';
      list.forEach((item, i) => {
        const scope = { $index: i };
        scope[as] = item;
        const sc = scopes.concat([scope]);
        for (const c of node.childNodes) render(c, sc, out);
      });
      return;
    }
    if (tag === 'sc-if') {
      const m = WHOLE.exec(node.getAttribute('value') || '');
      if (m && lookup(m[1], scopes)) for (const c of node.childNodes) render(c, scopes, out);
      return;
    }
    const el = node.namespaceURI === 'http://www.w3.org/1999/xhtml'
      ? document.createElement(tag)
      : document.createElementNS(node.namespaceURI, node.localName);
    for (const a of node.attributes) {
      if (a.name.startsWith('hint-')) continue;
      if (/^on[a-z]+$/.test(a.name)) {
        const m = WHOLE.exec(a.value);
        if (m) {
          const type = a.name.slice(2);
          el.__h = el.__h || {};
          el.__h[type] = lookup(m[1], scopes);
          bind(el, type);
        }
        continue;
      }
      el.setAttribute(a.name, interp(a.value, scopes));
    }
    for (const c of node.childNodes) render(c, scopes, el);
    out.appendChild(el);
  }

  function sameKind(a, b) {
    return a.nodeType === b.nodeType && (a.nodeType !== 1 || (a.namespaceURI === b.namespaceURI && a.localName === b.localName));
  }

  function morphChildren(from, to) {
    const fc = Array.from(from.childNodes);
    const tc = Array.from(to.childNodes);
    tc.forEach((t, i) => {
      const f = fc[i];
      if (!f) from.appendChild(t);
      else if (!sameKind(f, t)) from.replaceChild(t, f);
      else if (f.nodeType === 3) { if (f.nodeValue !== t.nodeValue) f.nodeValue = t.nodeValue; }
      else if (f.nodeType === 1) morph(f, t);
    });
    for (let i = tc.length; i < fc.length; i++) from.removeChild(fc[i]);
  }

  function morph(from, to) {
    for (const a of Array.from(from.attributes)) if (!to.hasAttribute(a.name)) from.removeAttribute(a.name);
    for (const a of to.attributes) if (from.getAttribute(a.name) !== a.value) from.setAttribute(a.name, a.value);
    from.__h = to.__h;
    if (to.__h) for (const type of Object.keys(to.__h)) bind(from, type);
    morphChildren(from, to);
  }

  function boot() {
    const xdc = document.querySelector('x-dc');
    const script = document.querySelector('script[data-dc-script]');
    if (!xdc || !script) return;
    const helmet = xdc.querySelector('helmet');
    if (helmet) {
      for (const n of Array.from(helmet.childNodes)) if (n.nodeType === 1) document.head.appendChild(n);
      helmet.remove();
    }
    const tmpl = document.createDocumentFragment();
    while (xdc.firstChild) tmpl.appendChild(xdc.firstChild);
    const root = document.createElement('div');
    root.id = 'dc-root';
    xdc.replaceWith(root);

    const meta = JSON.parse(script.getAttribute('data-props') || '{}');
    const props = {};
    for (const [k, v] of Object.entries(meta)) if (k[0] !== '$' && v && 'default' in v) props[k] = v.default;
    new URLSearchParams(location.search).forEach((v, k) => {
      props[k] = v === 'true' ? true : v === 'false' ? false : v;
    });

    const Component = new Function('DCLogic', script.textContent + '\n;return Component;')(DCLogic);
    const c = new Component(props);
    c.props = props;
    c.state = c.state || {};
    let queued = false;
    const paint = () => {
      const vals = c.renderVals();
      const next = document.createElement('div');
      for (const n of tmpl.childNodes) render(n, [vals], next);
      morphChildren(root, next);
    };
    c.__schedule = () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        paint();
      });
    };
    paint();
    if (typeof c.componentDidMount === 'function') c.componentDidMount();
    window.addEventListener('pagehide', () => {
      if (typeof c.componentWillUnmount === 'function') c.componentWillUnmount();
    });
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot);
  else boot();
})();
