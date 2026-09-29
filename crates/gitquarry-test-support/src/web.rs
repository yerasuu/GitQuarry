use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use std::net::SocketAddr;
use tower::ServiceExt;

/// A response read to the end, so assertions can look at all of it.
pub struct Page {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Page {
    /// `name=value` of the `Set-Cookie` planting `name`, ready to send back.
    pub fn cookie(&self, name: &str) -> String {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(&format!("{name}=")))
            .and_then(|c| c.split(';').next())
            .unwrap_or_else(|| panic!("no {name} cookie in {:?}", self.headers))
            .to_string()
    }

    pub fn location(&self) -> &str {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }

    /// The first `csrf` hidden field's value, read out of the markup the
    /// way a browser submitting the form would.
    pub fn csrf(&self) -> String {
        let marker = r#"name="csrf" value=""#;
        let start = self.body.find(marker).expect("page carries no csrf field") + marker.len();
        self.body[start..].split('"').next().unwrap_or_default().to_string()
    }
}

/// One `request` through `app`, as `run_server` would hand it over: with
/// the peer's socket address attached, which the git-smart-HTTP layer
/// needs before it routes anything.
pub async fn send(app: &axum::Router, mut request: Request<Body>) -> Page {
    if request.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))));
    }
    let response = app.clone().oneshot(request).await.expect("the router is infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .expect("reading the response body");
    Page {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// What a logged-in browser carries: the session cookie, and the CSRF token
/// its pages embed for it.
pub struct Session {
    pub cookie: String,
    pub csrf: String,
}

impl Session {
    /// A real password login for a user created with [`crate::user`]. The
    /// token is read from an account page every logged-in user can open.
    pub async fn login(app: &axum::Router, username: &str) -> Session {
        let form = Session::get_as(app, "/_login", None).await;
        let anchor = form.cookie("login-csrf");
        let csrf = form.csrf();
        let fields = [
            ("csrf", csrf.as_str()),
            ("username", username),
            ("password", crate::PASSWORD),
            ("return_to", "/"),
        ];
        let done = post_form(app, "/_login", Some(&anchor), &fields).await;
        assert_eq!(done.status, StatusCode::SEE_OTHER, "login failed: {}", done.body);
        let cookie = done.cookie("web-session");
        let account = Session::get_as(app, "/_account/appearance", Some(&cookie)).await;
        assert_eq!(account.status, StatusCode::OK, "logged-in account page");
        Session {
            csrf: account.csrf(),
            cookie,
        }
    }

    pub async fn get(&self, app: &axum::Router, path: &str) -> Page {
        Session::get_as(app, path, Some(&self.cookie)).await
    }

    /// Submits `fields` plus this session's own CSRF token.
    pub async fn post(&self, app: &axum::Router, path: &str, fields: &[(&str, &str)]) -> Page {
        let mut all = Vec::with_capacity(fields.len() + 1);
        all.push(("csrf", self.csrf.as_str()));
        all.extend_from_slice(fields);
        post_form(app, path, Some(&self.cookie), &all).await
    }

    /// A GET as `cookie`, or anonymous with `None`.
    pub async fn get_as(app: &axum::Router, path: &str, cookie: Option<&str>) -> Page {
        let mut builder = Request::get(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        send(app, builder.body(Body::empty()).expect("building the request")).await
    }
}

/// A form POST exactly as given — no CSRF token added — for tests about what
/// happens when the token is missing, forged, or taken from somewhere else.
/// [`Session::post`] is the one that adds the session's own.
pub async fn post_form(app: &axum::Router, path: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Page {
    let mut builder = Request::post(path).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    send(app, builder.body(Body::from(body)).expect("building the request")).await
}
