// Passkey (WebAuthn) ceremonies for `auth::web::passkey`. Loaded only by
// the pages that render a passkey control, and bound by data attributes
// alone -- `script-src 'self'` allows no inline handler.
//
// Every control is rendered `hidden` and shown here only once
// `window.PublicKeyCredential` exists: without WebAuthn (or without JS)
// there's nothing it could do, so it's better never seen at all.
//
// The server speaks base64url for every binary value (challenge, user id,
// credential ids, the authenticator's responses); `navigator.credentials`
// wants ArrayBuffers, so each ceremony converts both ways. Every request is
// JSON with the page's CSRF token in its `csrf` field.
//
// Shapes:
//   - `form[data-passkey="register"]` (`/_account/security`): begin with
//     the form's current-password field and the pressed button's `kind`
//     (`security-key` or `device`), `navigator.credentials.create()`,
//     finish with the label; the finish response is the refreshed passkeys
//     section, which replaces `data-target`.
//   - `[data-passkey="sign-in"]` (`/_login`, `/_login_totp`): its
//     `[data-passkey-start]` button runs begin, `navigator.credentials.get()`
//     and finish against `data-begin-url`/`data-finish-url` with
//     `data-csrf`; finish answers `{ redirect }` once the session cookie is
//     set, and the browser goes there.

(function () {
  "use strict";

  if (!window.PublicKeyCredential || !navigator.credentials) {
    return;
  }

  function reveal() {
    document.querySelectorAll("[data-passkey][hidden]").forEach((el) => {
      el.hidden = false;
    });
  }

  function toBuffer(value) {
    const b64 = value.replace(/-/g, "+").replace(/_/g, "/");
    const padded = b64 + "===".slice((b64.length + 3) % 4);
    const binary = atob(padded);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes.buffer;
  }

  function toBase64url(buffer) {
    const bytes = new Uint8Array(buffer);
    let binary = "";
    for (let i = 0; i < bytes.length; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }

  function descriptors(list) {
    return (list || []).map((c) => ({ type: c.type, id: toBuffer(c.id) }));
  }

  class PasskeyError extends Error {}

  // A refusal comes back as `AppError`'s HTML page; its message is the
  // same `.flash-err span` app.js's `extractFlashMessage` reads.
  function postJson(url, payload) {
    return fetch(url, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }).then((resp) => {
      if (resp.ok) {
        return resp;
      }
      return resp.text().then((body) => {
        const template = document.createElement("template");
        template.innerHTML = body;
        const span = template.content.querySelector(".flash-err span");
        throw new PasskeyError(span ? span.textContent.trim() : "");
      });
    });
  }

  function showError(scope, err) {
    const flash = scope.querySelector("[data-passkey-flash]");
    if (!flash) {
      return;
    }
    // A cancelled or timed-out browser prompt is a DOMException whose
    // message is the browser's own; a server refusal carries ours.
    const message = err && err.message ? err.message : "";
    flash.textContent = message || document.body.dataset.errorGeneric || "something went wrong, try again";
    flash.hidden = false;
  }

  function clearError(scope) {
    const flash = scope.querySelector("[data-passkey-flash]");
    if (flash) {
      flash.hidden = true;
      flash.textContent = "";
    }
  }

  function fieldValue(form, name) {
    const field = form.querySelector(`[name="${name}"]`);
    return field ? field.value : "";
  }

  function register(form, kind) {
    clearError(form);
    const csrf = fieldValue(form, "csrf");
    const label = fieldValue(form, "label");
    const begin = { csrf, reauth: fieldValue(form, "reauth") };
    if (kind) {
      begin.kind = kind;
    }
    postJson(form.dataset.beginUrl, begin)
      .then((resp) => resp.json())
      .then((options) => {
        options.challenge = toBuffer(options.challenge);
        options.user.id = toBuffer(options.user.id);
        options.excludeCredentials = descriptors(options.excludeCredentials);
        return navigator.credentials.create({ publicKey: options });
      })
      .then((credential) =>
        postJson(form.dataset.finishUrl, {
          csrf,
          label,
          clientDataJSON: toBase64url(credential.response.clientDataJSON),
          attestationObject: toBase64url(credential.response.attestationObject),
        }),
      )
      .then((resp) => resp.text())
      .then((html) => {
        const target = document.querySelector(form.dataset.target);
        const template = document.createElement("template");
        template.innerHTML = html.trim();
        const replacement = template.content.firstElementChild;
        if (target && replacement) {
          target.replaceWith(replacement);
          reveal();
        }
      })
      .catch((err) => showError(form, err));
  }

  function signIn(scope) {
    clearError(scope);
    const csrf = scope.dataset.csrf;
    postJson(scope.dataset.beginUrl, { csrf })
      .then((resp) => resp.json())
      .then((options) => {
        options.challenge = toBuffer(options.challenge);
        options.allowCredentials = descriptors(options.allowCredentials);
        return navigator.credentials.get({ publicKey: options });
      })
      .then((credential) =>
        postJson(scope.dataset.finishUrl, {
          csrf,
          return_to: scope.dataset.returnTo || "",
          id: toBase64url(credential.rawId),
          clientDataJSON: toBase64url(credential.response.clientDataJSON),
          authenticatorData: toBase64url(credential.response.authenticatorData),
          signature: toBase64url(credential.response.signature),
          userHandle: credential.response.userHandle ? toBase64url(credential.response.userHandle) : null,
        }),
      )
      .then((resp) => resp.json())
      .then((body) => {
        window.location.assign(body.redirect);
      })
      .catch((err) => showError(scope, err));
  }

  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-passkey-start]");
    const scope = button && button.closest("[data-passkey='sign-in']");
    if (scope) {
      event.preventDefault();
      signIn(scope);
    }
  });

  document.addEventListener("submit", (event) => {
    const form = event.target.closest("form[data-passkey='register']");
    if (form) {
      event.preventDefault();
      // Empty only where `submitter` isn't supported; the server then
      // leaves the browser's own chooser alone.
      register(form, event.submitter ? event.submitter.value : "");
    }
  });

  // app.js swaps the passkeys section in after an async remove, arriving
  // `hidden` like any server render; this keeps it visible.
  new MutationObserver(reveal).observe(document.body, { childList: true, subtree: true });
  reveal();
})();
