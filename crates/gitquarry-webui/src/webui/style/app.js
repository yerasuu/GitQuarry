// Progressive enhancement only: every form this file touches still works
// as a plain full-page POST/GET without it (see e.g. `actions.rs`'s
// star/watch buttons, or `repo::handler_index`'s search form) -- this just
// intercepts the interaction and asks the server for a fragment instead of
// a whole new page. Two shapes, both delegated on `document` rather than
// bound per-element (so a form swapped in below is still covered without
// re-binding anything):
//   - `form[data-async]`: submit POSTs via fetch(), server's fragment
//     *replaces the form itself* (see `<form data-async>` in `actions.rs`/
//     `theme_pref.rs`) -- unless the form also has `data-async-target`
//     (a selector), in which case *that* element gets replaced instead
//     (see `merges/mod.rs`'s `#mr-actions`: closing an MR changes the status
//     line and which button shows next, not just the clicked button's own
//     label, so more than the form itself has to refresh) -- and/or
//     `data-async-extra-target` (another selector) for a *second*, disjoint
//     element the response's second root element replaces (see
//     `actions.rs`'s star/watch buttons: the button lives in the header,
//     the count it affects lives in the sidebar, too far apart for one
//     swap to reach both).
//   - `form[data-live-search="#target"]`: typing (debounced) *and* a
//     normal submit both fetch() a GET and drop the fragment into
//     `target`'s innerHTML instead -- the form itself (the search box)
//     stays put, only the results container gets replaced.
//
// CSRF: the token already sits in a `data-async` form's own hidden `csrf`
// input (`FormData(form)` picks it up like a normal submit would) -- this
// file never reads `document.cookie` (the session cookie is `HttpOnly`, it
// couldn't anyway). `data-live-search` never needs one -- listing repos
// has no side effect to protect.
//
// A third, unrelated shape: `[data-copy]` (see `page_chrome::copy_button`)
// -- click writes the attribute's value to the clipboard and briefly swaps
// the button's own text for `data-copied-label`. Nothing to progressively
// enhance here (there's no non-JS fallback for "copy to clipboard"), so
// this one's just additive: the value is already visible as plain text
// next to the button either way.
//
// A fourth: `[data-toggle-rows]` (`annotated_diff.rs`'s per-hunk collapse)
// -- click flips `hidden` on every `tr[data-rows-group]` sharing its value
// and its own `aria-expanded`. Also purely additive/opt-in, same reasoning
// as the copy button.
//
// A fifth: `form[data-confirm="message"]` -- a plain `confirm()` prompt in
// front of a one-click irreversible action that isn't already behind a
// type-the-name modal (mirror delete, comment delete, MR merge). Native
// `confirm()` rather than a `styler` modal: these are single table-row/
// inline buttons, not a whole section worth a modal's own markup.
//
// A sixth: `details.dropdown` (the account menu, the repo header's ref
// switcher) -- opening/closing on a click of its own `<summary>` is native,
// no JS needed there at all, but closing on a click *outside* it isn't a
// native `<details>` behavior, so left alone every dropdown would stay
// open until its own summary was clicked again. Purely additive on top of
// what already works with JS off: nothing here is needed to open or use
// one, only to auto-close it.

function fetchFragment(url, init, onHtml) {
  fetch(url, init)
    .then((resp) => (resp.ok ? resp.text() : Promise.reject(new Error(`request failed: ${resp.status}`))))
    .then(onHtml)
    .catch((err) => console.error("gitquarry: async request failed", err));
}

function runLiveSearch(form) {
  const target = document.querySelector(form.dataset.liveSearch);
  const field = form.querySelector("input[name]");
  if (!target || !field) {
    return;
  }
  const url = new URL(form.action, window.location.href);
  url.searchParams.set(field.name, field.value);
  fetchFragment(url, { headers: { "X-GQ-Async": "1" } }, (html) => {
    target.innerHTML = html;
  });
}

let liveSearchTimer = null;
document.addEventListener("input", (event) => {
  const form = event.target.closest("form[data-live-search]");
  if (event.target instanceof HTMLInputElement && form) {
    clearTimeout(liveSearchTimer);
    liveSearchTimer = setTimeout(() => runLiveSearch(form), 250);
  }
});

document.addEventListener("submit", (event) => {
  const form = event.target;
  if (!(form instanceof HTMLFormElement)) {
    return;
  }
  // `data-confirm="message"`: a native `confirm()` gate in front of an
  // otherwise one-click irreversible action (mirror delete, comment
  // delete, MR merge) -- checked first, ahead of the async/live-search
  // branches below, so declining stops the request before either of
  // those ever sees the submit.
  if (form.hasAttribute("data-confirm") && !window.confirm(form.dataset.confirm)) {
    event.preventDefault();
    return;
  }
  if (form.hasAttribute("data-live-search")) {
    event.preventDefault();
    clearTimeout(liveSearchTimer);
    runLiveSearch(form);
    return;
  }
  if (!form.hasAttribute("data-async")) {
    return;
  }
  event.preventDefault();
  // `URLSearchParams`, not a raw `FormData`: `fetch()` serializes a
  // `FormData` body as `multipart/form-data`, but every server handler on
  // the other end of a `data-async` form is `axum::extract::Form<T>`,
  // which only accepts `application/x-www-form-urlencoded` and 415s
  // anything else -- `fetch()` sets that content-type automatically for a
  // `URLSearchParams` body, so this needs no header of its own. Safe here
  // specifically because no `data-async` form has a file input (checked --
  // urlencoded can't carry one; a future one would need `Multipart` server
  // -side and a different client path, same as the plain-POST upload forms
  // already use).
  const body = new URLSearchParams(new FormData(form));
  // A form with more than one named submit button (the theme toggle: one
  // `<form>`, two buttons) needs the *clicked* one's name/value in the
  // body -- `new FormData(form)` alone doesn't reliably carry that across
  // browsers, `event.submitter` (the button that triggered this submit) is
  // the well-supported way to get it.
  if (event.submitter && event.submitter.name) {
    body.set(event.submitter.name, event.submitter.value);
  }
  // `data-html-attr` (theme/density toggles): flip `<html>`'s own attribute
  // the instant the button's clicked -- everything either one affects is
  // plain CSS keyed off that attribute (colors/spacing, and now syntax
  // highlighting too, see `highlight.rs`'s own doc comment on why it no
  // longer bakes a theme into rendered HTML), so this alone is the whole
  // repaint. The fetch below still persists the pick server-side (the
  // cookie, so it's still picked on the next full page load too) and
  // swaps the button pair same as any other data-async form.
  const htmlAttr = form.dataset.htmlAttr;
  const pickedValue = event.submitter && event.submitter.value;
  if (htmlAttr && pickedValue) {
    document.documentElement.setAttribute(htmlAttr, pickedValue);
  }
  // `data-class-target`/`data-class-value` (text-size, see `text_size.rs`):
  // same "paint it before the round trip" idea as `data-html-attr` above,
  // for a preference that's scoped to one element's class rather than an
  // attribute on `<html>` -- text-size only affects the summary tab's
  // body, not the whole page, so there's no single global attribute for it
  // to piggyback on. `!== undefined`, not a truthiness check: the "medium"
  // size's class is the empty string, which is a real, intentional value
  // here (it clears the class), not a missing one.
  const classTarget = form.dataset.classTarget && document.querySelector(form.dataset.classTarget);
  const classValue = form.dataset.classValue;
  if (classTarget && classValue !== undefined) {
    classTarget.className = classValue;
  }
  // `data-toggle-label`/`data-optimistic-target`/`data-optimistic-delta`
  // (star/watch, see `actions.rs`): same "paint it before the round trip"
  // idea as `data-html-attr` above, for a toggle whose repaint isn't pure
  // CSS -- the button's own label text and a count shown elsewhere on the
  // page. Flips the `.btn-label` text against its own stored opposite (so
  // a second click before the first response lands still toggles
  // correctly) and nudges the count target by `delta`, then negates
  // `delta` for next time. The real response a moment later still fully
  // replaces both (`data-async`/`data-async-extra-target`), so a wrong
  // guess here -- a lost CAS race, another tab -- self-corrects within the
  // same round trip instead of drifting.
  const toggleLabel = form.dataset.toggleLabel;
  const labelEl = form.querySelector(".btn-label");
  if (toggleLabel && labelEl) {
    form.dataset.toggleLabel = labelEl.textContent;
    labelEl.textContent = toggleLabel;
  }
  const optimisticDelta = Number(form.dataset.optimisticDelta);
  const optimisticEl = form.dataset.optimisticTarget && document.querySelector(form.dataset.optimisticTarget);
  if (optimisticEl && Number.isFinite(optimisticDelta)) {
    optimisticEl.textContent = String(Number(optimisticEl.textContent) + optimisticDelta);
    form.dataset.optimisticDelta = String(-optimisticDelta);
  }
  const targetSelector = form.dataset.asyncTarget;
  // `data-async-extra-target` (star/watch, see `actions.rs`): for a button
  // whose toggle also has to repaint a count shown somewhere else on the
  // page (the sidebar's star/watcher tally, nowhere near the button itself
  // in the DOM), the server's fragment response carries a *second* root
  // element after the primary one -- that second element replaces this
  // selector, independently of whatever the primary swap above targets.
  // Every other data-async form leaves this attribute unset, so its
  // response is still exactly one element and this branch never runs.
  const extraTargetSelector = form.dataset.asyncExtraTarget;
  fetchFragment(form.action, { method: form.method || "POST", body, headers: { "X-GQ-Async": "1" } }, (html) => {
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const replacement = template.content.firstElementChild;
    if (!replacement) {
      return;
    }
    const extraReplacement = extraTargetSelector ? replacement.nextElementSibling : null;
    const target = targetSelector ? document.querySelector(targetSelector) : form;
    if (target) {
      target.replaceWith(replacement);
    }
    const extraTarget = extraTargetSelector ? document.querySelector(extraTargetSelector) : null;
    if (extraTarget && extraReplacement) {
      extraTarget.replaceWith(extraReplacement);
    }
  });
});

document.addEventListener("click", (event) => {
  document.querySelectorAll("details.dropdown[open]").forEach((details) => {
    // A click anywhere *inside* the dropdown (its own summary, or a link
    // in the menu) is left alone -- clicking its own summary is the native
    // toggle's job, not this handler's, and this only ever closes a
    // dropdown from a click that landed outside it entirely.
    if (!details.contains(event.target)) {
      details.removeAttribute("open");
    }
  });
});

document.addEventListener("click", (event) => {
  const btn = event.target.closest("[data-copy]");
  if (!btn) {
    return;
  }
  navigator.clipboard.writeText(btn.dataset.copy).then(() => {
    const original = btn.textContent;
    btn.textContent = btn.dataset.copiedLabel || original;
    btn.disabled = true;
    setTimeout(() => {
      btn.textContent = original;
      btn.disabled = false;
    }, 1200);
  });
});

// Per-hunk diff collapse (`annotated_diff.rs`'s `.hunk-toggle`). Not
// `styler`'s `collapse.js`: that wraps content in a `<div class="collapse">`,
// which can't legally sit inside a `<table>` around `<tr>`s. Plain `hidden`
// attribute instead -- same mechanism the phase-2b modal backdrop and
// `styler`'s own `[hidden]`-driven tab panels already rely on.
document.addEventListener("click", (event) => {
  const btn = event.target.closest("[data-toggle-rows]");
  if (!btn) {
    return;
  }
  const expanded = btn.getAttribute("aria-expanded") !== "false";
  document.querySelectorAll(`tr[data-rows-group="${CSS.escape(btn.dataset.toggleRows)}"]`).forEach((row) => {
    row.hidden = expanded;
  });
  btn.setAttribute("aria-expanded", String(!expanded));
});

// Kanban cross-column drag (`board/mod.rs`'s `[draggable]`/`[data-column-index]`).
// Within-column reordering stays button-driven (up/down) -- the backend has
// no "move to this exact position" call to drive anything richer, only
// move-to-column. A drop moves the card in the DOM immediately (always to
// the top of the target column) and fires the same `/_board/move_to_column`
// form every card already carries -- via fetch() instead of a real submit --
// to persist it; a failed request just reloads the page to resync instead
// of trying to hand-roll an undo of the optimistic move.
let draggedCard = null;

document.addEventListener("dragstart", (event) => {
  const card = event.target.closest('[draggable="true"]');
  if (!card) {
    return;
  }
  draggedCard = card;
  card.classList.add("dragging");
});

document.addEventListener("dragend", () => {
  if (draggedCard) {
    draggedCard.classList.remove("dragging");
  }
  draggedCard = null;
});

document.addEventListener("dragover", (event) => {
  if (draggedCard && event.target.closest("[data-column-index]")) {
    event.preventDefault(); // required for `drop` to fire at all
  }
});

document.addEventListener("drop", (event) => {
  const column = event.target.closest("[data-column-index]");
  const card = draggedCard;
  if (!column || !card) {
    return;
  }
  event.preventDefault();
  const sourceColumn = card.closest("[data-column-index]");
  if (sourceColumn === column) {
    return;
  }
  const form = card.querySelector('form[action$="/_board/move_to_column"]');
  if (!form) {
    return; // single-column board: no such form exists at all
  }
  const heading = column.querySelector("h4");
  if (heading) {
    heading.after(card);
  } else {
    column.prepend(card);
  }
  const body = new FormData(form);
  body.set("to_column", column.dataset.columnIndex);
  fetch(form.action, { method: "POST", body, headers: { "X-GQ-Async": "1" } }).then((resp) => {
    if (!resp.ok) {
      window.location.reload();
    }
  }).catch(() => window.location.reload());
});
