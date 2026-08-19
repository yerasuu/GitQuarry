/* vendored from styler@v0.2.0 (unminified) - do not edit directly, see styler's own README */
var Styler = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/js/index.js
  var index_exports = {};
  __export(index_exports, {
    Accordion: () => Accordion,
    Carousel: () => Carousel,
    Modal: () => Modal,
    Offcanvas: () => Offcanvas,
    Sidebar: () => Sidebar,
    Tabs: () => Tabs,
    accent: () => accent,
    init: () => init,
    theme: () => theme,
    toast: () => toast
  });

  // src/js/dropdown.js
  function clippingAncestor(el) {
    for (let node = el.parentElement; node && node !== document.body; node = node.parentElement) {
      const { overflow, overflowX, overflowY } = getComputedStyle(node);
      if (/auto|scroll|hidden/.test(overflow + overflowX + overflowY)) return node;
    }
    return null;
  }
  function pin(dropdown) {
    const menu = dropdown.querySelector(".dropdown-menu");
    if (!menu) return;
    const triggerRect = dropdown.getBoundingClientRect();
    const menuRect = menu.getBoundingClientRect();
    const alignEnd = menu.classList.contains("right");
    let left = alignEnd ? triggerRect.right - menuRect.width : triggerRect.left;
    let top = triggerRect.bottom + 6;
    if (top + menuRect.height > window.innerHeight - 8) {
      top = Math.max(8, triggerRect.top - menuRect.height - 6);
    }
    left = Math.min(Math.max(8, left), window.innerWidth - menuRect.width - 8);
    menu.style.top = `${top}px`;
    menu.style.left = `${left}px`;
  }
  function unpin(dropdown) {
    const menu = dropdown.querySelector(".dropdown-menu");
    if (!menu) return;
    menu.classList.remove("pinned");
    menu.style.top = "";
    menu.style.left = "";
  }
  function closeAll(except) {
    document.querySelectorAll(".dropdown.open").forEach((el) => {
      if (el === except) return;
      el.classList.remove("open");
      unpin(el);
      el.querySelector("[data-dropdown-trigger]")?.setAttribute("aria-expanded", "false");
    });
  }
  function repositionOpen() {
    document.querySelectorAll(".dropdown.open").forEach((dropdown) => {
      if (dropdown.querySelector(".dropdown-menu.pinned")) pin(dropdown);
    });
  }
  function initDropdown() {
    document.addEventListener("click", (e) => {
      const trigger = e.target.closest("[data-dropdown-trigger]");
      if (trigger) {
        const dropdown = trigger.closest(".dropdown");
        if (!dropdown) return;
        const willOpen = !dropdown.classList.contains("open");
        closeAll(willOpen ? dropdown : null);
        dropdown.classList.toggle("open", willOpen);
        if (!trigger.hasAttribute("aria-haspopup")) trigger.setAttribute("aria-haspopup", "menu");
        trigger.setAttribute("aria-expanded", String(willOpen));
        if (willOpen && clippingAncestor(dropdown)) {
          const menu = dropdown.querySelector(".dropdown-menu");
          menu?.classList.add("pinned");
          pin(dropdown);
        } else if (!willOpen) {
          unpin(dropdown);
        }
        e.stopPropagation();
        return;
      }
      if (!e.target.closest(".dropdown-menu")) {
        closeAll();
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const open3 = document.querySelector(".dropdown.open");
      closeAll();
      open3?.querySelector("[data-dropdown-trigger]")?.focus();
    });
    document.addEventListener("scroll", repositionOpen, { passive: true, capture: true });
    window.addEventListener("resize", repositionOpen, { passive: true });
  }

  // src/js/tabs.js
  function tabsIn(list) {
    return Array.from(list.querySelectorAll(".tab[data-tab-target]"));
  }
  function activate(tab, { focus = false } = {}) {
    if (!tab || tab.classList.contains("disabled")) return;
    const list = tab.closest(".tab-list");
    const targetSelector = tab.getAttribute("data-tab-target");
    const target = document.querySelector(targetSelector);
    if (!list || !target) return;
    tabsIn(list).forEach((t) => {
      const isActive = t === tab;
      t.classList.toggle("active", isActive);
      t.setAttribute("aria-selected", String(isActive));
      t.setAttribute("tabindex", isActive ? "0" : "-1");
    });
    const panelGroup = target.parentElement;
    if (panelGroup) {
      panelGroup.querySelectorAll(":scope > .tab-panel").forEach((p) => {
        p.hidden = p !== target;
      });
    } else {
      target.hidden = false;
    }
    if (focus) tab.focus();
    tab.dispatchEvent(new CustomEvent("styler:tab-change", { bubbles: true, detail: { target: targetSelector } }));
  }
  function decorate(list) {
    const tabs = tabsIn(list);
    tabs.forEach((tab) => {
      tab.setAttribute("aria-selected", String(tab.classList.contains("active")));
      tab.setAttribute("tabindex", tab.classList.contains("active") ? "0" : "-1");
    });
  }
  function initTabs() {
    document.querySelectorAll(".tab-list").forEach(decorate);
    document.addEventListener("click", (e) => {
      const tab = e.target.closest(".tab[data-tab-target]");
      if (tab) activate(tab);
    });
    document.addEventListener("keydown", (e) => {
      const list = e.target.closest(".tab-list");
      if (!list) return;
      const tabs = tabsIn(list).filter((t) => !t.classList.contains("disabled"));
      if (!tabs.length) return;
      const currentIndex2 = tabs.indexOf(e.target.closest(".tab"));
      if (currentIndex2 === -1) return;
      let nextIndex = null;
      if (e.key === "ArrowRight") nextIndex = (currentIndex2 + 1) % tabs.length;
      else if (e.key === "ArrowLeft") nextIndex = (currentIndex2 - 1 + tabs.length) % tabs.length;
      else if (e.key === "Home") nextIndex = 0;
      else if (e.key === "End") nextIndex = tabs.length - 1;
      else return;
      e.preventDefault();
      activate(tabs[nextIndex], { focus: true });
    });
  }
  var Tabs = { activate };

  // src/js/focus-trap.js
  var FOCUSABLE_SELECTOR = [
    "a[href]",
    "button:not([disabled])",
    "input:not([disabled])",
    "select:not([disabled])",
    "textarea:not([disabled])",
    '[tabindex]:not([tabindex="-1"])'
  ].join(",");
  function getFocusable(container) {
    return Array.from(container.querySelectorAll(FOCUSABLE_SELECTOR)).filter(
      (el) => el.offsetParent !== null
    );
  }
  function trapFocus(container) {
    const previouslyFocused = document.activeElement;
    const focusables = getFocusable(container);
    const fallback = container.querySelector("[autofocus]") || focusables[0];
    if (!fallback && !container.hasAttribute("tabindex")) {
      container.setAttribute("tabindex", "-1");
    }
    (fallback || container).focus();
    function onKeydown(e) {
      if (e.key !== "Tab") return;
      const items = getFocusable(container);
      if (!items.length) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
    container.addEventListener("keydown", onKeydown);
    return function release() {
      container.removeEventListener("keydown", onKeydown);
      if (previouslyFocused && typeof previouslyFocused.focus === "function") {
        previouslyFocused.focus();
      }
    };
  }

  // src/js/modal.js
  var releaseFns = /* @__PURE__ */ new WeakMap();
  function open(id) {
    const backdrop = document.querySelector(id.startsWith("#") ? id : `#${id}`);
    const dialog = backdrop?.querySelector(".modal");
    if (!backdrop || !dialog) return;
    backdrop.hidden = false;
    document.body.style.overflow = "hidden";
    if (!dialog.hasAttribute("role")) dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    releaseFns.set(backdrop, trapFocus(dialog));
  }
  function close(backdropEl) {
    const backdrop = typeof backdropEl === "string" ? document.querySelector(backdropEl.startsWith("#") ? backdropEl : `#${backdropEl}`) : backdropEl;
    if (!backdrop) return;
    backdrop.hidden = true;
    releaseScrollLock();
    releaseFns.get(backdrop)?.();
    releaseFns.delete(backdrop);
  }
  function releaseScrollLock() {
    const anyOpen = document.querySelector(".modal-backdrop:not([hidden]), .offcanvas.open");
    if (!anyOpen) document.body.style.overflow = "";
  }
  function initModal() {
    document.addEventListener("click", (e) => {
      const opener = e.target.closest("[data-modal-open]");
      if (opener) {
        open(opener.getAttribute("data-modal-open"));
        return;
      }
      const closer = e.target.closest("[data-modal-close]");
      if (closer) {
        close(closer.closest(".modal-backdrop"));
        return;
      }
      if (e.target.classList.contains("modal-backdrop")) {
        close(e.target);
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const visible = document.querySelector(".modal-backdrop:not([hidden])");
      if (visible) close(visible);
    });
  }
  var Modal = { open, close };

  // src/js/toast.js
  var ICONS = {
    ok: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M13.5 4L6 11.5L2.5 8" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    error: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M4 4L12 12M12 4L4 12" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>',
    warn: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M8 6V9M8 11.2V11.21M2 13.5H14L8 2.5L2 13.5Z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/></svg>',
    info: '<svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M8 7V11.5M8 4.5V4.51" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/><circle cx="8" cy="8" r="6.25" stroke="currentColor" stroke-width="1.3"/></svg>'
  };
  function getContainer(position) {
    const id = `styler-toast-container-${position}`;
    let container = document.getElementById(id);
    if (!container) {
      container = document.createElement("div");
      container.id = id;
      container.className = "toast-container";
      if (position === "bottom-right") container.classList.add("bottom");
      if (position === "top-left") container.classList.add("left");
      if (position === "bottom-left") container.classList.add("bottom", "left");
      document.body.appendChild(container);
    }
    return container;
  }
  function toast({ title, message = "", type = "info", duration = 4e3, position = "top-right", action } = {}) {
    const container = getContainer(position);
    const el = document.createElement("div");
    el.className = `toast ${type}`;
    const icon = document.createElement("span");
    icon.className = "toast-icon";
    icon.innerHTML = ICONS[type] || ICONS.info;
    const body = document.createElement("div");
    body.className = "toast-body";
    if (title) {
      const titleEl = document.createElement("div");
      titleEl.className = "toast-title";
      titleEl.textContent = title;
      body.appendChild(titleEl);
    }
    if (message) {
      const messageEl = document.createElement("div");
      messageEl.className = "toast-message";
      messageEl.textContent = message;
      body.appendChild(messageEl);
    }
    const close4 = document.createElement("span");
    close4.className = "toast-close";
    close4.setAttribute("aria-label", "Close");
    close4.innerHTML = '<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M4 4L12 12M12 4L4 12" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>';
    const remove = () => {
      el.classList.remove("show");
      el.addEventListener("transitionend", () => el.remove(), { once: true });
    };
    if (action?.label) {
      const actionBtn = document.createElement("button");
      actionBtn.className = "toast-action";
      actionBtn.type = "button";
      actionBtn.textContent = action.label;
      actionBtn.addEventListener("click", () => {
        action.onClick?.();
        remove();
      });
      body.appendChild(actionBtn);
    }
    el.append(icon, body, close4);
    close4.addEventListener("click", remove);
    container.appendChild(el);
    requestAnimationFrame(() => el.classList.add("show"));
    if (duration > 0) {
      setTimeout(remove, duration);
    }
    return { close: remove };
  }

  // src/js/tooltip.js
  function initTooltip() {
    document.addEventListener("mouseover", (e) => {
      const el = e.target.closest("[data-tooltip-smart]");
      if (!el) return;
      const rect = el.getBoundingClientRect();
      if (rect.top < 48) {
        el.setAttribute("data-tooltip-pos", "bottom");
      } else {
        el.removeAttribute("data-tooltip-pos");
      }
    }, { passive: true });
  }

  // src/js/accordion.js
  function decorate2(header) {
    if (!header.hasAttribute("role")) header.setAttribute("role", "button");
    if (!header.hasAttribute("tabindex")) header.setAttribute("tabindex", "0");
    header.setAttribute("aria-expanded", String(header.closest(".accordion-item")?.classList.contains("open")));
  }
  function toggle(header) {
    const item = header.closest(".accordion-item");
    const body = item?.querySelector(":scope > .accordion-body");
    if (!item || !body) return;
    const willOpen = !item.classList.contains("open");
    const container = item.closest(".accordion");
    if (willOpen && container?.classList.contains("single-open")) {
      container.querySelectorAll(":scope > .accordion-item.open").forEach((sibling) => {
        if (sibling === item) return;
        sibling.classList.remove("open");
        const siblingBody = sibling.querySelector(":scope > .accordion-body");
        const siblingHeader = sibling.querySelector(":scope > .accordion-header");
        if (siblingBody) siblingBody.hidden = true;
        siblingHeader?.setAttribute("aria-expanded", "false");
      });
    }
    item.classList.toggle("open", willOpen);
    body.hidden = !willOpen;
    header.setAttribute("aria-expanded", String(willOpen));
  }
  function refresh() {
    document.querySelectorAll(".accordion-header").forEach(decorate2);
  }
  function initAccordion() {
    refresh();
    document.addEventListener("click", (e) => {
      const header = e.target.closest(".accordion-header");
      if (header) toggle(header);
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      const header = e.target.closest(".accordion-header");
      if (!header) return;
      e.preventDefault();
      toggle(header);
    });
  }
  var Accordion = { refresh };

  // src/js/popover.js
  function closeAll2(except) {
    document.querySelectorAll(".popover.open").forEach((el) => {
      if (el !== except) el.classList.remove("open");
    });
  }
  function initPopover() {
    document.addEventListener("click", (e) => {
      const trigger = e.target.closest("[data-popover-trigger]");
      if (trigger) {
        const popover = trigger.closest(".popover");
        if (!popover) return;
        const willOpen = !popover.classList.contains("open");
        closeAll2(willOpen ? popover : null);
        popover.classList.toggle("open", willOpen);
        e.stopPropagation();
        return;
      }
      if (!e.target.closest(".popover-content")) {
        closeAll2();
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeAll2();
    });
  }

  // src/js/theme.js
  var STORAGE_KEY = "styler-theme";
  function systemPrefersDark() {
    return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
  }
  function get() {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark") return stored;
    return systemPrefersDark() ? "dark" : "light";
  }
  function set(theme2) {
    document.documentElement.setAttribute("data-theme", theme2);
    localStorage.setItem(STORAGE_KEY, theme2);
  }
  function toggle2() {
    const next = get() === "dark" ? "light" : "dark";
    set(next);
    return next;
  }
  function applyStored() {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark") {
      document.documentElement.setAttribute("data-theme", stored);
    }
  }
  var theme = { get, set, toggle: toggle2, applyStored };

  // src/js/accent.js
  var STORAGE_KEY2 = "styler-accent";
  function get2() {
    return document.documentElement.getAttribute("data-accent") || localStorage.getItem(STORAGE_KEY2) || "indigo";
  }
  function set2(name) {
    document.documentElement.setAttribute("data-accent", name);
    localStorage.setItem(STORAGE_KEY2, name);
  }
  function applyStored2() {
    const stored = localStorage.getItem(STORAGE_KEY2);
    if (stored) document.documentElement.setAttribute("data-accent", stored);
  }
  var accent = { get: get2, set: set2, applyStored: applyStored2 };

  // src/js/offcanvas.js
  var releaseFns2 = /* @__PURE__ */ new WeakMap();
  function panelAndBackdrop(id) {
    const panel = document.querySelector(id.startsWith("#") ? id : `#${id}`);
    const backdrop = panel?.previousElementSibling?.classList.contains("offcanvas-backdrop") ? panel.previousElementSibling : panel?.parentElement?.querySelector(".offcanvas-backdrop");
    return { panel, backdrop };
  }
  function open2(id) {
    const { panel, backdrop } = panelAndBackdrop(id);
    if (!panel) return;
    panel.classList.add("open");
    backdrop?.classList.add("open");
    document.body.style.overflow = "hidden";
    if (!panel.hasAttribute("role")) panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    releaseFns2.set(panel, trapFocus(panel));
  }
  function close2(panelEl) {
    const panel = typeof panelEl === "string" ? panelAndBackdrop(panelEl).panel : panelEl;
    if (!panel) return;
    const backdrop = panel.previousElementSibling?.classList.contains("offcanvas-backdrop") ? panel.previousElementSibling : panel.parentElement?.querySelector(".offcanvas-backdrop");
    panel.classList.remove("open");
    backdrop?.classList.remove("open");
    releaseScrollLock();
    releaseFns2.get(panel)?.();
    releaseFns2.delete(panel);
  }
  function initOffcanvas() {
    document.addEventListener("click", (e) => {
      const opener = e.target.closest("[data-offcanvas-open]");
      if (opener) {
        open2(opener.getAttribute("data-offcanvas-open"));
        return;
      }
      const closer = e.target.closest("[data-offcanvas-close]");
      if (closer) {
        close2(closer.closest(".offcanvas"));
        return;
      }
      if (e.target.classList.contains("offcanvas-backdrop")) {
        close2(e.target.nextElementSibling);
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const openPanel = document.querySelector(".offcanvas.open");
      if (openPanel) close2(openPanel);
    });
  }
  var Offcanvas = { open: open2, close: close2 };

  // src/js/alert.js
  function initAlert() {
    document.addEventListener("click", (e) => {
      const dismiss = e.target.closest("[data-alert-dismiss]");
      if (!dismiss) return;
      dismiss.closest(".alert")?.remove();
    });
  }

  // src/js/collapse.js
  function initCollapse() {
    document.addEventListener("click", (e) => {
      const trigger = e.target.closest("[data-collapse-target]");
      if (!trigger) return;
      const selector = trigger.getAttribute("data-collapse-target");
      const target = document.querySelector(selector);
      if (!target) return;
      const willShow = !target.classList.contains("show");
      target.classList.toggle("show", willShow);
      trigger.classList.toggle("active", willShow);
      trigger.setAttribute("aria-expanded", String(willShow));
    });
  }

  // src/js/ripple.js
  function initRipple() {
    document.addEventListener("pointerdown", (e) => {
      const el = e.target.closest(".ripple");
      if (!el || el.hasAttribute("disabled") || el.classList.contains("disabled")) return;
      const rect = el.getBoundingClientRect();
      const size = Math.max(rect.width, rect.height);
      const span = document.createElement("span");
      span.className = "ripple-effect";
      span.style.width = `${size}px`;
      span.style.height = `${size}px`;
      span.style.left = `${e.clientX - rect.left - size / 2}px`;
      span.style.top = `${e.clientY - rect.top - size / 2}px`;
      el.appendChild(span);
      span.addEventListener("animationend", () => span.remove(), { once: true });
    });
  }

  // src/js/datepicker.js
  var state = /* @__PURE__ */ new WeakMap();
  function pad(n) {
    return String(n).padStart(2, "0");
  }
  function isoLocal(date) {
    return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  }
  function parseIsoLocal(iso) {
    const parts = iso.split("-").map(Number);
    if (parts.length !== 3 || parts.some(Number.isNaN)) return null;
    const [y, m, d] = parts;
    return new Date(y, m - 1, d);
  }
  function closeAll3(exceptRoot) {
    document.querySelectorAll(".datepicker-panel:not([hidden])").forEach((panel) => {
      const root = panel.closest(".datepicker");
      if (root !== exceptRoot) panel.hidden = true;
    });
  }
  function render(root) {
    const s = state.get(root);
    const panel = root.querySelector(".datepicker-panel");
    const { view, selected } = s;
    const year = view.getFullYear();
    const month = view.getMonth();
    const monthLabel = view.toLocaleDateString(void 0, { month: "long", year: "numeric" });
    const firstOfMonth = new Date(year, month, 1);
    const startOffset = firstOfMonth.getDay();
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const daysInPrevMonth = new Date(year, month, 0).getDate();
    const todayIso = isoLocal(/* @__PURE__ */ new Date());
    const selectedIso = selected ? isoLocal(selected) : null;
    const weekdayNames = Array.from(
      { length: 7 },
      (_, i) => new Date(2021, 7, i + 1).toLocaleDateString(void 0, { weekday: "narrow" })
    );
    let cells = "";
    for (let i = 0; i < startOffset; i++) {
      cells += `<span class="datepicker-day muted">${daysInPrevMonth - startOffset + i + 1}</span>`;
    }
    for (let day = 1; day <= daysInMonth; day++) {
      const iso = isoLocal(new Date(year, month, day));
      const classes = ["datepicker-day"];
      if (iso === todayIso) classes.push("today");
      if (iso === selectedIso) classes.push("selected");
      cells += `<span class="${classes.join(" ")}" data-date="${iso}">${day}</span>`;
    }
    const trailing = (7 - (startOffset + daysInMonth) % 7) % 7;
    for (let i = 1; i <= trailing; i++) {
      cells += `<span class="datepicker-day muted">${i}</span>`;
    }
    panel.innerHTML = `
    <div class="datepicker-header">
      <span class="datepicker-nav" data-nav="-1">&lsaquo;</span>
      <span class="label">${monthLabel}</span>
      <span class="datepicker-nav" data-nav="1">&rsaquo;</span>
    </div>
    <div class="datepicker-grid">
      ${weekdayNames.map((w) => `<span class="datepicker-weekday">${w}</span>`).join("")}
      ${cells}
    </div>
  `;
  }
  function ensureState(root, input) {
    if (state.has(root)) return;
    const parsed = input.value ? parseIsoLocal(input.value) : null;
    const initial = parsed || /* @__PURE__ */ new Date();
    state.set(root, {
      view: new Date(initial.getFullYear(), initial.getMonth(), 1),
      selected: parsed
    });
  }
  function initDatepicker() {
    document.addEventListener("click", (e) => {
      const input = e.target.closest("[data-datepicker]");
      if (input) {
        const root = input.closest(".datepicker");
        if (!root) return;
        ensureState(root, input);
        closeAll3(root);
        root.querySelector(".datepicker-panel").hidden = false;
        render(root);
        return;
      }
      const nav = e.target.closest("[data-nav]");
      if (nav) {
        const root = nav.closest(".datepicker");
        const s = state.get(root);
        if (!s) return;
        const dir = Number(nav.getAttribute("data-nav"));
        s.view = new Date(s.view.getFullYear(), s.view.getMonth() + dir, 1);
        render(root);
        return;
      }
      const day = e.target.closest(".datepicker-day[data-date]");
      if (day) {
        const root = day.closest(".datepicker");
        const s = state.get(root);
        if (!s) return;
        const iso = day.getAttribute("data-date");
        s.selected = parseIsoLocal(iso);
        const input2 = root.querySelector("[data-datepicker]");
        input2.value = iso;
        input2.dispatchEvent(new CustomEvent("styler:date-change", { bubbles: true, detail: { value: iso } }));
        root.querySelector(".datepicker-panel").hidden = true;
        return;
      }
      if (!e.target.closest(".datepicker")) {
        closeAll3();
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeAll3();
    });
  }

  // src/js/slider.js
  function updateBubble(root) {
    if (!root) return;
    const input = root.querySelector('input[type="range"]');
    const bubble = root.querySelector(".slider-value");
    if (!input || !bubble) return;
    const min = Number(input.min) || 0;
    const max = Number(input.max) || 100;
    const pct = max === min ? 0 : (Number(input.value) - min) / (max - min);
    bubble.textContent = input.value;
    bubble.style.left = `${pct * 100}%`;
  }
  function initSlider() {
    document.querySelectorAll(".slider").forEach(updateBubble);
    document.addEventListener("input", (e) => {
      const input = e.target.closest('.slider input[type="range"]');
      if (!input) return;
      updateBubble(input.closest(".slider"));
    });
  }

  // src/js/navbar-scroll.js
  function initNavbarScroll() {
    const navbars = document.querySelectorAll(".navbar[data-scroll-shrink]");
    if (!navbars.length) return;
    const onScroll = () => {
      const shrink = window.scrollY > 24;
      navbars.forEach((nav) => nav.classList.toggle("scrolled", shrink));
    };
    document.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  // src/js/table-sort.js
  function initTableSort() {
    document.addEventListener("click", (e) => {
      const th = e.target.closest("th.sortable");
      if (!th) return;
      const row = th.parentElement;
      const wasAsc = th.classList.contains("sorted-asc");
      const wasDesc = th.classList.contains("sorted-desc");
      row.querySelectorAll("th.sortable").forEach((sibling) => {
        sibling.classList.remove("sorted-asc", "sorted-desc");
      });
      let direction = null;
      if (!wasAsc && !wasDesc) direction = "asc";
      else if (wasAsc) direction = "desc";
      if (direction) th.classList.add(`sorted-${direction}`);
      th.dispatchEvent(new CustomEvent("styler:sort", {
        bubbles: true,
        detail: { column: th.getAttribute("data-sort-key") || th.textContent.trim(), direction }
      }));
    });
  }

  // src/js/scrollspy.js
  function initScrollspy() {
    document.querySelectorAll("[data-scrollspy]").forEach((nav) => {
      const links = Array.from(nav.querySelectorAll('a[href^="#"]'));
      const targets = links.map((link) => {
        const id = link.getAttribute("href").slice(1);
        return { link, section: id ? document.getElementById(id) : null };
      }).filter((entry) => entry.section);
      if (!targets.length) return;
      const setActive = (id) => {
        links.forEach((link) => {
          link.classList.toggle("active", link.getAttribute("href") === `#${id}`);
        });
      };
      const observer = new IntersectionObserver(
        (entries) => {
          const visible = entries.filter((entry) => entry.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
          if (visible.length) setActive(visible[0].target.id);
        },
        { rootMargin: "0px 0px -70% 0px", threshold: 0 }
      );
      targets.forEach(({ section }) => observer.observe(section));
    });
  }

  // src/js/carousel.js
  function goTo(root, index) {
    const track = root.querySelector(".carousel-track");
    const slides = root.querySelectorAll(".carousel-slide");
    if (!track || !slides.length) return;
    const clamped = (index + slides.length) % slides.length;
    track.style.transform = `translateX(-${clamped * 100}%)`;
    slides.forEach((s, i) => s.classList.toggle("active", i === clamped));
    root.querySelectorAll(".carousel-dot").forEach((d, i) => d.classList.toggle("active", i === clamped));
    root.dataset.carouselIndex = String(clamped);
    root.dispatchEvent(new CustomEvent("styler:carousel-change", { bubbles: true, detail: { index: clamped } }));
  }
  function currentIndex(root) {
    return Number(root.dataset.carouselIndex || 0);
  }
  function startAutoplay(root) {
    const interval = Number(root.getAttribute("data-carousel-interval") || 0);
    if (!interval) return;
    root._stylerCarouselTimer = setInterval(() => {
      goTo(root, currentIndex(root) + 1);
    }, interval);
  }
  function stopAutoplay(root) {
    clearInterval(root._stylerCarouselTimer);
  }
  function initCarousel() {
    document.querySelectorAll(".carousel").forEach((root) => {
      goTo(root, 0);
      startAutoplay(root);
      root.addEventListener("mouseenter", () => stopAutoplay(root));
      root.addEventListener("mouseleave", () => startAutoplay(root));
    });
    document.addEventListener("click", (e) => {
      const prev = e.target.closest(".carousel-prev");
      if (prev) {
        const root = prev.closest(".carousel");
        if (root) goTo(root, currentIndex(root) - 1);
        return;
      }
      const next = e.target.closest(".carousel-next");
      if (next) {
        const root = next.closest(".carousel");
        if (root) goTo(root, currentIndex(root) + 1);
        return;
      }
      const dot = e.target.closest("[data-carousel-to]");
      if (dot) {
        const root = dot.closest(".carousel");
        if (root) goTo(root, Number(dot.getAttribute("data-carousel-to")));
      }
    });
  }
  var Carousel = { goTo };

  // src/js/navbar-toggle.js
  function initNavbarToggle() {
    document.addEventListener("click", (e) => {
      const toggler = e.target.closest("[data-navbar-toggle]");
      if (toggler) {
        const nav = toggler.closest(".navbar")?.querySelector(".navbar-nav");
        nav?.classList.toggle("open");
        return;
      }
      const link = e.target.closest(".navbar-nav.open .navbar-link");
      if (link) {
        link.closest(".navbar-nav").classList.remove("open");
        return;
      }
      if (!e.target.closest(".navbar")) {
        document.querySelectorAll(".navbar-nav.open").forEach((nav) => nav.classList.remove("open"));
      }
    });
  }

  // src/js/sidebar.js
  function toggle3() {
    document.getElementById("sidebar")?.classList.toggle("open");
    document.querySelector(".sidebar-backdrop")?.classList.toggle("open");
  }
  function close3() {
    document.getElementById("sidebar")?.classList.remove("open");
    document.querySelector(".sidebar-backdrop")?.classList.remove("open");
  }
  function initSidebar() {
    document.addEventListener("click", (e) => {
      if (e.target.closest("[data-sidebar-toggle]")) {
        toggle3();
        return;
      }
      if (e.target.closest(".sidebar.open .sidebar-item")) close3();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") close3();
    });
  }
  var Sidebar = { toggle: toggle3, close: close3 };

  // src/js/index.js
  function init() {
    initDropdown();
    initTabs();
    initModal();
    initTooltip();
    initAccordion();
    initPopover();
    initOffcanvas();
    initAlert();
    initCollapse();
    initRipple();
    initDatepicker();
    initSlider();
    initNavbarScroll();
    initTableSort();
    initScrollspy();
    initCarousel();
    initNavbarToggle();
    initSidebar();
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
  return __toCommonJS(index_exports);
})();
