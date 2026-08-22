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

// Renders in-place via styler's own toast (top-right, auto-dismissing)
// instead of a native `alert()` -- a native dialog blocks the main thread
// and reads as "another window" popping up, exactly what a `data-async`
// form is meant to avoid by not doing a real page navigation on submit.
// `message` defaults to `page_chrome::layout`'s `data-error-generic`
// (`gitquarry_i18n::web::chrome::async_request_failed`, the same idiom as
// `data-confirm`'s messages) -- only used when the caller has no more
// specific text of its own, e.g. a network failure with no response body
// to read one from at all. Whenever a response body *is* available (see
// `extractFlashMessage` below), that's the one message this app already
// writes for "your last action failed" (`webui::error::AppError`'s own
// `render_flash(Flash::Err(...))`), so the toast says the same thing the
// inline `.flash-err` banner would have on a plain, non-async submit of
// the same failure -- one authored message, two delivery mechanisms,
// instead of a second, generic one invented just for the async path.
function showErrorToast(message) {
  const text = message || document.body.dataset.errorGeneric || "something went wrong, try again";
  if (window.Styler && typeof window.Styler.toast === "function") {
    window.Styler.toast({ message: text, type: "error" });
  } else {
    console.error("gitquarry:", text);
  }
}

// Pulls the message back out of `flash::render_flash(Flash::Err(...))`'s
// own markup (`<p class="... flash-err"><span>{message}</span>...`) -- see
// `AppError::into_response` (`webui/error.rs`), which always renders this
// on a failed request's body, `data-async` or not. Returns `null` (not the
// generic fallback) when the body isn't that shape at all, so the caller
// can tell "no message here" apart from "the message was empty".
function extractFlashMessage(html) {
  const template = document.createElement("template");
  template.innerHTML = html;
  const span = template.content.querySelector(".flash-err span");
  return span ? span.textContent.trim() || null : null;
}

class AsyncRequestError extends Error {
  constructor(status, body) {
    super(`request failed: ${status}`);
    this.body = body;
  }
}

function fetchFragment(url, init, onHtml, onError) {
  fetch(url, init)
    .then((resp) =>
      resp.ok
        ? resp.text()
        : resp.text().then((body) => Promise.reject(new AsyncRequestError(resp.status, body))),
    )
    .then((html) => {
      try {
        onHtml(html);
      } catch (err) {
        // The request itself succeeded -- the server already applied
        // whatever it was asked to -- so a bug in *this* callback (an
        // unexpected fragment shape, a selector that no longer matches)
        // must not fall into the `catch` below: that one means the request
        // failed and is what triggers `onError`'s optimistic-UI rollback,
        // which would be wrong here since nothing needs undoing.
        console.error("gitquarry: fragment handler failed", err);
      }
    })
    .catch((err) => {
      console.error("gitquarry: async request failed", err);
      if (onError) {
        // A network failure (offline, DNS, CORS) never reached the server
        // at all, so there's no `AppError`-authored body to read a message
        // from -- `onError` gets `null` and its caller falls back to the
        // generic string.
        onError(err instanceof AsyncRequestError ? extractFlashMessage(err.body) : null);
      }
    });
}

function runLiveSearch(form) {
  const target = document.querySelector(form.dataset.liveSearch);
  const field = form.querySelector("input[name]");
  if (!target || !field) {
    return;
  }
  const url = new URL(form.action, window.location.href);
  url.searchParams.set(field.name, field.value);
  // Typing fast enough sends overlapping GETs; nothing guarantees they
  // resolve in the order they were sent. `_gqSearchSeq` lives on `target`
  // itself (not `form`) because that's the node a stale response would
  // otherwise stomp -- a slower response for an earlier keystroke arriving
  // after a faster one for a later keystroke is simply dropped.
  const searchSeq = (target._gqSearchSeq = (target._gqSearchSeq || 0) + 1);
  fetchFragment(
    url,
    { headers: { "X-GQ-Async": "1" } },
    (html) => {
      if (target._gqSearchSeq !== searchSeq) {
        return;
      }
      target.innerHTML = html;
    },
    (message) => showErrorToast(message),
  );
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
  const htmlAttr = form.dataset.htmlAttr;
  const pickedValue = event.submitter && event.submitter.value;
  const classTarget = form.dataset.classTarget && document.querySelector(form.dataset.classTarget);
  const classValue = form.dataset.classValue;
  const toggleLabel = form.dataset.toggleLabel;
  const labelEl = form.querySelector(".btn-label");
  const optimisticDeltaAttr = form.dataset.optimisticDelta;
  const optimisticDelta = Number(optimisticDeltaAttr);
  const optimisticEl = form.dataset.optimisticTarget && document.querySelector(form.dataset.optimisticTarget);
  const targetSelector = form.dataset.asyncTarget;
  const extraTargetSelector = form.dataset.asyncExtraTarget;
  const target = targetSelector ? document.querySelector(targetSelector) : form;
  const extraTarget = extraTargetSelector ? document.querySelector(extraTargetSelector) : null;
  // Every node this submit will optimistically paint or eventually swap,
  // deduplicated. Locked for the round trip so a *different* form reaching
  // the same node (star + watch both repaint `#community-stats`, the three
  // text-size buttons all repaint `#about-body`, two MR-action forms both
  // repaint `#mr-actions`) can't race this one -- and so a double-click on
  // *this* form's own button can't either, since `form` is always in the
  // set. Replaces the old per-form `_gqRequestSeq` counter, which only
  // caught a form racing itself and only guarded the failure path, not
  // success: a locked node simply refuses a second submit outright, so
  // there's no "which of two in-flight responses wins" question left to
  // get half-answered.
  const lockedNodes = Array.from(new Set([form, target, extraTarget, classTarget, optimisticEl].filter(Boolean)));
  if (lockedNodes.some((node) => node._gqLocked)) {
    return;
  }
  lockedNodes.forEach((node) => {
    node._gqLocked = true;
  });
  const unlock = () => {
    lockedNodes.forEach((node) => {
      node._gqLocked = false;
    });
  };
  // `data-html-attr` (theme/density toggles): flip `<html>`'s own attribute
  // the instant the button's clicked -- everything either one affects is
  // plain CSS keyed off that attribute (colors/spacing, and now syntax
  // highlighting too, see `highlight.rs`'s own doc comment on why it no
  // longer bakes a theme into rendered HTML), so this alone is the whole
  // repaint. The fetch below still persists the pick server-side (the
  // cookie, so it's still picked on the next full page load too) and
  // swaps the button pair same as any other data-async form.
  const originalHtmlAttrValue = htmlAttr ? document.documentElement.getAttribute(htmlAttr) : null;
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
  const originalClassName = classTarget ? classTarget.className : null;
  if (classTarget && classValue !== undefined) {
    classTarget.className = classValue;
  }
  // `data-toggle-label`/`data-optimistic-target`/`data-optimistic-delta`
  // (star/watch, see `actions.rs`): same "paint it before the round trip"
  // idea as `data-html-attr` above, for a toggle whose repaint isn't pure
  // CSS -- the button's own label text and a count shown elsewhere on the
  // page. Flips the `.btn-label` text against its own stored opposite and
  // nudges the count target by `delta`. The lock above already rules out a
  // second submit touching these same nodes before this one settles, so
  // there's no concurrent guess to defend against here anymore.
  const originalLabelText = labelEl ? labelEl.textContent : null;
  if (toggleLabel && labelEl) {
    form.dataset.toggleLabel = originalLabelText;
    labelEl.textContent = toggleLabel;
  }
  const originalOptimisticText = optimisticEl ? optimisticEl.textContent : null;
  if (optimisticEl && Number.isFinite(optimisticDelta)) {
    optimisticEl.textContent = String(Number(originalOptimisticText) + optimisticDelta);
    form.dataset.optimisticDelta = String(-optimisticDelta);
  }
  // `data-async-extra-target` (star/watch, see `actions.rs`): for a button
  // whose toggle also has to repaint a count shown somewhere else on the
  // page (the sidebar's star/watcher tally, nowhere near the button itself
  // in the DOM), the server's fragment response carries a *second* root
  // element after the primary one -- that second element replaces this
  // selector, independently of whatever the primary swap above targets.
  // Every other data-async form leaves this attribute unset, so its
  // response is still exactly one element and this branch never runs.
  fetchFragment(
    form.action,
    { method: form.method || "POST", body, headers: { "X-GQ-Async": "1" } },
    (html) => {
      unlock();
      const template = document.createElement("template");
      template.innerHTML = html.trim();
      const replacement = template.content.firstElementChild;
      if (!replacement) {
        return;
      }
      const extraReplacement = extraTargetSelector ? replacement.nextElementSibling : null;
      if (target) {
        target.replaceWith(replacement);
      }
      if (extraTarget && extraReplacement) {
        extraTarget.replaceWith(extraReplacement);
      }
    },
    // A failed request otherwise leaves every optimistic paint above
    // (label, count, html/class attribute) stuck in the guessed state with
    // no visible error. Unlike the drag-and-drop handler below (whose
    // failure fallback is a full reload -- a moved DOM card has no cheap
    // undo), every optimistic branch here recorded its own prior value, so
    // the honest fix is to put each one back and surface the failure in
    // place, not to bounce the user off the page they were just on.
    (message) => {
      unlock();
      if (htmlAttr) {
        if (originalHtmlAttrValue === null) {
          document.documentElement.removeAttribute(htmlAttr);
        } else {
          document.documentElement.setAttribute(htmlAttr, originalHtmlAttrValue);
        }
      }
      if (classTarget) {
        classTarget.className = originalClassName;
      }
      if (labelEl && originalLabelText !== null) {
        labelEl.textContent = originalLabelText;
        // `toggleLabel` is `undefined` when the form never had
        // `data-toggle-label` set in the first place -- `dataset` coerces
        // `undefined` to the literal string `"undefined"`, so an absent
        // original is a `delete`, not an assignment.
        if (toggleLabel === undefined) {
          delete form.dataset.toggleLabel;
        } else {
          form.dataset.toggleLabel = toggleLabel;
        }
      }
      if (optimisticEl && originalOptimisticText !== null) {
        optimisticEl.textContent = originalOptimisticText;
        if (optimisticDeltaAttr === undefined) {
          delete form.dataset.optimisticDelta;
        } else {
          form.dataset.optimisticDelta = optimisticDeltaAttr;
        }
      }
      showErrorToast(message);
    },
  );
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
  navigator.clipboard
    .writeText(btn.dataset.copy)
    .then(() => {
      const original = btn.textContent;
      btn.textContent = btn.dataset.copiedLabel || original;
      btn.disabled = true;
      setTimeout(() => {
        btn.textContent = original;
        btn.disabled = false;
      }, 1200);
    })
    .catch((err) => {
      console.error("gitquarry: clipboard write failed", err);
      showErrorToast();
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
