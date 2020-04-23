//! A widget that may present a modal.

use druid::kurbo::Vec2;
use druid::widget::prelude::*;
use druid::{Color, Command, Data, Rect, Selector, WidgetExt, WidgetPod};

/// A wrapper around a closure for constructing a widget.
struct ModalBuilder<T>(Box<dyn FnOnce() -> Box<dyn Widget<T>>>);

impl<T: Data> ModalBuilder<T> {
    /// Create a new `ModalBuilder
    fn new<W: Widget<T> + 'static>(f: impl FnOnce() -> W + 'static) -> Self {
        ModalBuilder(Box::new(|| f().boxed()))
    }

    fn build(self) -> Box<dyn Widget<T>> {
        (self.0)()
    }
}

/// A widget that has a child, and can optionally show a modal dialog
/// that obscures the child.
pub struct ModalHost<T, W> {
    child: WidgetPod<T, W>,
    modal: Option<WidgetPod<T, Box<dyn Widget<T>>>>,
}

// this impl block has concrete types or else we would need to specify the type
// parameters in order to access the consts.
impl ModalHost<(), ()> {
    /// Command to display a modal in this host.
    ///
    /// The argument **must** be a `ModalBuilder`.
    pub const SHOW_MODAL: Selector = Selector::new("runebender.show-modal-widget");

    /// Command to dismiss the modal.
    pub const DISMISS_MODAL: Selector = Selector::new("runebender.dismiss-modal-widget");

    /// A convenience for creating a command to send to this widget.
    ///
    /// This mostly just requires the user to import fewer types.
    pub fn make_modal_command<T, W>(f: impl FnOnce() -> W + 'static) -> Command
    where
        T: Data,
        W: Widget<T> + 'static,
    {
        Command::one_shot(ModalHost::SHOW_MODAL, ModalBuilder::new(f))
    }
}

impl<T, W: Widget<T>> ModalHost<T, W> {
    pub fn new(widget: W) -> Self {
        ModalHost {
            child: WidgetPod::new(widget),
            modal: None,
        }
    }
}

impl<T: Data, W: Widget<T>> Widget<T> for ModalHost<T, W> {
    fn event(&mut self, ctx: &mut EventCtx, event: &Event, data: &mut T, env: &Env) {
        match event {
            Event::Command(cmd) if cmd.selector == ModalHost::SHOW_MODAL => {
                if self.modal.is_none() {
                    let payload = cmd
                        .take_object::<ModalBuilder<T>>()
                        .expect("incorrect payload for SHOW_MODAL");
                    self.modal = Some(WidgetPod::new(payload.build()));
                    ctx.children_changed();
                } else {
                    log::warn!("cannot show modal; already showing modal");
                }
                ctx.set_handled();
            }
            Event::Command(cmd) if cmd.selector == ModalHost::DISMISS_MODAL => {
                if self.modal.is_some() {
                    self.modal = None;
                    ctx.children_changed();
                } else {
                    log::warn!("cannot dismiss modal; no modal shown");
                }
                ctx.set_handled();
            }

            // user input only gets delivered to modal, if modal is present
            e if is_user_input(e) => match self.modal.as_mut() {
                Some(modal) => modal.event(ctx, event, data, env),
                None => self.child.event(ctx, event, data, env),
            },
            // other events (timers, commands) are delivered to both widgets
            other => {
                if let Some(modal) = self.modal.as_mut() {
                    modal.event(ctx, other, data, env);
                }
                self.child.event(ctx, other, data, env);
            }
        }
    }

    fn lifecycle(&mut self, ctx: &mut LifeCycleCtx, event: &LifeCycle, data: &T, env: &Env) {
        if let Some(modal) = self.modal.as_mut() {
            modal.lifecycle(ctx, event, data, env);
        }
        self.child.lifecycle(ctx, event, data, env);
    }

    fn update(&mut self, ctx: &mut UpdateCtx, _old_data: &T, data: &T, env: &Env) {
        if let Some(modal) = self.modal.as_mut() {
            modal.update(ctx, data, env);
        }
        self.child.update(ctx, data, env);
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, data: &T, env: &Env) -> Size {
        let size = self.child.layout(ctx, bc, data, env);
        self.child.set_layout_rect(ctx, data, env, size.to_rect());
        if let Some(modal) = self.modal.as_mut() {
            let modal_constraints = BoxConstraints::new(Size::ZERO, size);
            let modal_size = modal.layout(ctx, &modal_constraints, data, env);
            let modal_orig = (size.to_vec2() - modal_size.to_vec2()) / 2.0;
            let modal_frame = Rect::from_origin_size(modal_orig.to_point(), modal_size);
            modal.set_layout_rect(ctx, data, env, modal_frame);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, data: &T, env: &Env) {
        self.child.paint(ctx, data, env);
        if let Some(modal) = self.modal.as_mut() {
            let frame = ctx.size().to_rect();
            ctx.fill(frame, &Color::BLACK.with_alpha(0.35));
            let modal_rect = modal.layout_rect() + Vec2::new(5.0, 5.0);
            let blur_color = Color::grey8(100);
            ctx.blurred_rect(modal_rect, 5.0, &blur_color);
            modal.paint_with_offset(ctx, data, env);
        }
    }
}

fn is_user_input(event: &Event) -> bool {
    match event {
        Event::MouseUp(_)
        | Event::MouseDown(_)
        | Event::MouseMove(_)
        | Event::KeyUp(_)
        | Event::KeyDown(_)
        | Event::Paste(_)
        | Event::Wheel(_)
        | Event::Zoom(_) => true,
        _ => false,
    }
}
