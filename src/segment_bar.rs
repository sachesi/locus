use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use std::cell::RefCell;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SegmentBar {
        pub segments: RefCell<Vec<(f64, super::Tone)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SegmentBar {
        const NAME: &'static str = "LocusSegmentBar";
        type Type = super::SegmentBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("segmentbar");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for SegmentBar {}

    impl WidgetImpl for SegmentBar {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            let text = widget.color();
            let accent = adw::StyleManager::default().accent_color_rgba();
            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 6.0));
            snapshot.append_color(&text.with_alpha(0.06), &bounds);
            let segments = self.segments.borrow();
            let total: f64 = segments.iter().map(|segment| segment.0).sum();
            let mut x = 0.0;
            for (amount, tone) in &*segments {
                let piece = if total > 0.0 {
                    width * (*amount / total) as f32
                } else {
                    0.0
                };
                let color = match tone {
                    super::Tone::Accent => accent,
                    super::Tone::SoftAccent => accent.with_alpha(0.5),
                    super::Tone::Neutral => text.with_alpha(0.55),
                    super::Tone::SoftNeutral => text.with_alpha(0.3),
                    super::Tone::Empty => text.with_alpha(0.0),
                };
                snapshot.append_color(
                    &color,
                    &graphene::Rect::new(x, 0.0, (piece - 2.0).max(0.0), height),
                );
                x += piece;
            }
            snapshot.pop();
        }
    }
}

/// The colour of a part of a [`SegmentBar`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Accent,
    SoftAccent,
    Neutral,
    SoftNeutral,
    Empty,
}

glib::wrapper! {
    /// A whole split into parts, side by side in one bar.
    pub struct SegmentBar(ObjectSubclass<imp::SegmentBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SegmentBar {
    pub fn set_segments(&self, segments: Vec<(f64, Tone)>) {
        self.imp().segments.replace(segments);
        self.queue_draw();
    }
}
