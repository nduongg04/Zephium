//! Website sign-in on a tab a person reads.
//!
//! The native request (WebKit's completion block, WebView2's deferral) stays
//! here, owned by the exact view that received it. Chrome sees only the
//! prompt; the typed credentials come back through the host once, go to the
//! engine, and are wiped. Agent, extension and privileged views never install
//! this broker, so their challenges keep the cancel path.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use url::Url;
use zephium_core::http_auth::{
    Admission, AuthChallenge, AuthOrigin, HttpAuthSession, HttpCredentials, MainFrame,
    HTTP_AUTH_TIMEOUT,
};
use zephium_core::ids::ItemId;
use zephium_core::ports::engine::EngineEvent;

use super::permits::{EventPermit, Sink};
use super::EngineHost;

#[cfg(target_os = "macos")]
pub(super) type NativeResponder = wry::HttpAuthResponder;
#[cfg(target_os = "windows")]
pub(super) type NativeResponder = crate::platform::imp::BasicAuthResponder;

pub(super) struct PendingNative {
    responder: Option<NativeResponder>,
    _timeout: Option<crate::platform::imp::ContentPolicyTimeout>,
}

impl PendingNative {
    fn respond(mut self, credentials: Option<&HttpCredentials>) {
        if let Some(responder) = self.responder.take() {
            responder.respond(credentials.map(|value| (value.username(), value.password())));
        }
    }
}

pub(super) struct HttpAuthBroker {
    item: Rc<Cell<ItemId>>,
    permit: EventPermit,
    sink: Sink,
    main_frame: RefCell<MainFrame>,
    session: RefCell<HttpAuthSession<PendingNative>>,
}

impl HttpAuthBroker {
    pub(super) fn new(item: Rc<Cell<ItemId>>, permit: EventPermit, sink: Sink) -> Rc<Self> {
        Rc::new(Self {
            item,
            permit,
            sink,
            main_frame: RefCell::default(),
            session: RefCell::default(),
        })
    }

    /// A challenge from the view's engine. Anything this cannot hold is
    /// cancelled by dropping `responder`.
    /// `main_frame_url` is the engine's own main-frame address at the
    /// challenge, where it has one that is current while a page-driven
    /// navigation is still provisional.
    pub(super) fn challenge(
        &self,
        challenge: AuthChallenge,
        main_frame_url: Option<&str>,
        responder: NativeResponder,
    ) {
        let native = PendingNative {
            responder: Some(responder),
            _timeout: None,
        };
        if self.permit.active_token().is_none() {
            return;
        }
        let (Ok(mut session), Ok(main_frame)) =
            (self.session.try_borrow_mut(), self.main_frame.try_borrow())
        else {
            return;
        };
        let main_frame = MainFrame {
            in_flight: main_frame_url
                .and_then(origin)
                .or_else(|| main_frame.in_flight.clone()),
            committed: main_frame.committed.clone(),
        };
        let (admission, rejected) = session.admit(&challenge, &main_frame, native);
        drop(main_frame);
        let item = self.item.get();
        let (prompt, replaced) = match admission {
            Admission::Prompt { prompt, replaced } => (Some(prompt), replaced),
            Admission::Cancel { replaced } => (None, replaced),
        };
        if let Some(prompt) = &prompt {
            let request = prompt.request;
            let timeout = schedule_timeout(move || {
                let _ = super::dispatch::try_with(move |host| host.expire_http_auth(item, request));
            });
            match (timeout, session.pending_native_mut()) {
                (Some(timeout), Some(pending)) => pending._timeout = Some(timeout),
                // Without its deadline a prompt could hold the load forever.
                _ => {
                    drop(session.expire(request));
                    drop(session);
                    drop(rejected);
                    self.end(replaced);
                    return;
                }
            }
        }
        drop(session);
        drop(rejected);
        self.end(replaced);
        if let Some(prompt) = prompt {
            self.permit.emit(
                &self.sink,
                EngineEvent::HttpAuthRequested { id: item, prompt },
            );
        }
    }

    pub(super) fn answer(&self, request: u32, credentials: Option<HttpCredentials>) -> bool {
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return false;
        };
        let native = session.answer(request, credentials.is_some());
        drop(session);
        let Some(native) = native else {
            return false;
        };
        native.respond(credentials.as_ref());
        true
    }

    pub(super) fn expire(&self, request: u32) {
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return;
        };
        let native = session.expire(request);
        drop(session);
        if native.is_some() {
            self.end(Some((request, native)));
        }
    }

    pub(super) fn navigation_started(&self, url: &str) {
        self.set_frame(|frame| frame.in_flight = origin(url));
        self.reset();
    }

    pub(super) fn navigation_redirected(&self, url: &str) {
        self.set_frame(|frame| frame.in_flight = origin(url));
    }

    pub(super) fn navigation_committed(&self, url: &str) {
        self.set_frame(|frame| {
            frame.in_flight = None;
            frame.committed = origin(url);
        });
        self.reset();
    }

    pub(super) fn navigation_failed(&self) {
        self.set_frame(|frame| frame.in_flight = None);
        self.reset();
    }

    /// The view is closing or being discarded.
    pub(super) fn cancel(&self) {
        if let Ok(mut session) = self.session.try_borrow_mut() {
            let pending = session.reset();
            drop(session);
            drop(pending);
        }
    }

    fn set_frame(&self, update: impl FnOnce(&mut MainFrame)) {
        if let Ok(mut frame) = self.main_frame.try_borrow_mut() {
            update(&mut frame);
        }
    }

    fn reset(&self) {
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return;
        };
        let pending = session.reset();
        drop(session);
        self.end(pending.map(|(request, native)| (request, Some(native))));
    }

    /// Cancels a native request the person can no longer answer, and takes
    /// its prompt down.
    fn end<T>(&self, ended: Option<(u32, T)>) {
        let Some((request, native)) = ended else {
            return;
        };
        drop(native);
        self.permit.emit(
            &self.sink,
            EngineEvent::HttpAuthEnded {
                id: self.item.get(),
                request,
            },
        );
    }
}

fn origin(url: &str) -> Option<AuthOrigin> {
    AuthOrigin::of_url(&Url::parse(url).ok()?)
}

#[cfg(target_os = "macos")]
fn schedule_timeout(
    callback: impl FnOnce() + Send + 'static,
) -> Option<crate::platform::imp::ContentPolicyTimeout> {
    crate::platform::imp::schedule_content_policy_timeout(HTTP_AUTH_TIMEOUT, callback)
}

#[cfg(target_os = "windows")]
fn schedule_timeout(
    callback: impl FnOnce() + 'static,
) -> Option<crate::platform::imp::ContentPolicyTimeout> {
    crate::platform::imp::schedule_browser_timeout(HTTP_AUTH_TIMEOUT, callback)
}

/// WebKit's challenge as the shared rules read it. A proxy speaking
/// anything but http or https (SOCKS) is not asked about.
#[cfg(target_os = "macos")]
pub(super) fn native_challenge(challenge: &wry::HttpAuthChallenge) -> Option<AuthChallenge> {
    let origin = AuthOrigin::new(&challenge.scheme, &challenge.host, challenge.port)?;
    Some(AuthChallenge {
        insecure: origin.scheme() != "https",
        origin,
        proxy: challenge.proxy,
        previous_failures: Some(challenge.previous_failures),
    })
}

/// WebView2 names only the address that asked; it keeps no failure count,
/// so a repeat is recognised by the shared rules.
#[cfg(target_os = "windows")]
pub(super) fn native_challenge(uri: &str) -> Option<AuthChallenge> {
    let origin = origin(uri)?;
    Some(AuthChallenge {
        insecure: origin.scheme() != "https",
        origin,
        proxy: false,
        previous_failures: None,
    })
}

impl EngineHost {
    pub(crate) fn answer_http_auth(
        &mut self,
        item: ItemId,
        request: u32,
        credentials: Option<HttpCredentials>,
    ) {
        if let Some(view) = self.views.get(&item) {
            view.http_auth.answer(request, credentials);
        }
    }

    fn expire_http_auth(&mut self, item: ItemId, request: u32) {
        if let Some(view) = self.views.get(&item) {
            view.http_auth.expire(request);
        }
    }
}
