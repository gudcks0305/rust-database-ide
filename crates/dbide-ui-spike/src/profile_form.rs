use gpui_component::Sizable;

use dbide_profiles::{ConnectionProfile, ConnectionRequest, DatabaseEngine, TlsMode};
use dbide_ui_spike::workbench_model::{PortDraft, request_from_fields};
use gpui::prelude::*;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use std::{cell::RefCell, rc::Rc};

pub struct ProfileForm {
    pub original: ConnectionProfile,
    pub fields: Vec<Entity<InputState>>,
    pub tls: TlsMode,
    pub remember_password: bool,
    pub engine: DatabaseEngine,
    pub sqlite_create: bool,
    pub read_only: bool,
    port_draft: Rc<RefCell<PortDraft>>,
    _subscriptions: Vec<Subscription>,
}
impl ProfileForm {
    pub fn new(
        profile: ConnectionProfile,
        password: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let values = [
            profile.name.clone(),
            profile.host.clone(),
            profile.port.to_string(),
            profile.database.clone(),
            profile.username.clone(),
            password.into(),
            profile.connect_timeout_secs.to_string(),
            profile.root_cert_path.clone().unwrap_or_default(),
            profile.sqlite_path.clone().unwrap_or_default(),
        ];
        let fields: Vec<Entity<InputState>> = values
            .into_iter()
            .enumerate()
            .map(|(ix, value)| {
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx).masked(ix == 5);
                    state.set_value(value, window, cx);
                    state
                })
            })
            .collect();
        let tls = profile.tls_mode;
        let engine = profile.engine;
        let sqlite_create = profile.sqlite_create;
        let read_only = profile.read_only;
        let port_draft = Rc::new(RefCell::new(PortDraft::new(
            engine,
            &profile.port.to_string(),
        )));
        let tracker = port_draft.clone();
        let subscription = cx.subscribe(&fields[2], move |input, event, cx| {
            if matches!(event, InputEvent::Change) {
                tracker.borrow_mut().observe(&input.read(cx).value());
            }
        });
        Self {
            original: profile,
            fields,
            tls,
            remember_password: false,
            engine,
            sqlite_create,
            read_only,
            port_draft,
            _subscriptions: vec![subscription],
        }
    }
    pub fn request(&self, cx: &App) -> Result<ConnectionRequest, String> {
        let fields = self
            .fields
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect::<Vec<_>>();
        request_from_fields(
            &self.original,
            self.engine,
            &fields,
            self.tls,
            self.sqlite_create,
            self.read_only,
        )
    }
    pub fn set_engine(&mut self, engine: DatabaseEngine, window: &mut Window, cx: &mut App) {
        let current = self.fields[2].read(cx).value().to_string();
        let next = self.port_draft.borrow_mut().switch(engine, &current);
        if let Some(port) = next {
            self.fields[2].update(cx, |input, cx| input.set_value(port, window, cx));
        }
        self.engine = engine;
        if engine == DatabaseEngine::Sqlite {
            self.remember_password = false;
        }
    }
    pub fn field(&self, ix: usize, label: &'static str, disabled: bool) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x9ca7b9))
                    .child(label),
            )
            .child(
                Input::new(&self.fields[ix])
                    .aria_label(label)
                    .disabled(disabled)
                    .small(),
            )
    }
}
