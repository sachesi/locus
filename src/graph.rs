use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use std::collections::VecDeque;

/// Readings kept per series, the width of the graph from edge to edge.
pub const HISTORY: usize = 60;

type ScaleLabel = Box<dyn Fn(f64) -> String>;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Graph)]
    pub struct Graph {
        /// The value at the top of the graph; 0 scales it to the largest value shown.
        #[property(get, set = Self::set_maximum, minimum = 0.0)]
        maximum: Cell<f64>,
        /// Draws the readings on a plain background, with a thinner line and no grid, for
        /// graphs too small for more.
        #[property(get, set)]
        plain: Cell<bool>,
        /// Up to two series of readings, oldest first. The first is drawn in the accent
        /// colour and filled beneath, the second as a line in the text colour.
        pub series: RefCell<[VecDeque<f64>; 2]>,
        pub scale_label: RefCell<Option<ScaleLabel>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Graph {
        const NAME: &'static str = "LocusGraph";
        type Type = super::Graph;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("graph");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Graph {
        fn constructed(&self) {
            self.parent_constructed();
            // The accent colour is not a CSS property of the widget, so a change to it
            // redraws nothing on its own.
            adw::StyleManager::default().connect_accent_color_notify(glib::clone!(
                #[weak(rename_to = graph)]
                self.obj(),
                move |_| graph.queue_draw()
            ));
        }
    }

    impl WidgetImpl for Graph {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let text = widget.color();
            let accent = adw::StyleManager::default().accent_color_rgba();
            let series = self.series.borrow();
            let maximum = self.maximum.get();
            let top = if maximum > 0.0 {
                maximum
            } else {
                scale_for(
                    series.iter().flatten().copied().fold(0.0, f64::max),
                    crate::format::is_binary(),
                )
            };

            // A graph scaled to its data names its top in a strip of its own above the
            // readings, where no peak draws over it.
            let label = self
                .scale_label
                .borrow()
                .as_ref()
                .filter(|_| maximum <= 0.0)
                .map(|label| widget.create_pango_layout(Some(&label(top))));
            let plot_top = label
                .as_ref()
                .map_or(0.0, |l| l.pixel_size().1 as f32 + 4.0);
            let plot_height = height - plot_top;

            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            if self.plain.get() {
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 4.0));
                snapshot.append_color(&text.with_alpha(0.05), &bounds);
            } else {
                snapshot.push_clip(&bounds);
            }
            let first_line = if label.is_some() { 0 } else { 1 };
            for quarter in (first_line..4).filter(|_| !self.plain.get()) {
                let y = (plot_top + plot_height * quarter as f32 / 4.0).round();
                snapshot.append_color(
                    &text.with_alpha(0.08),
                    &graphene::Rect::new(0.0, y, width, 1.0),
                );
            }

            let step = width / (HISTORY - 1) as f32;
            let point = |i: usize, count: usize, value: f64| {
                let x = width - step * (count - 1 - i) as f32;
                // A line at zero stays a line rather than falling off the bottom edge.
                let y = plot_top
                    + 1.0
                    + (plot_height - 2.0) * (1.0 - (value / top).clamp(0.0, 1.0) as f32);
                (x, y)
            };
            for (index, values) in series.iter().enumerate().rev() {
                if values.len() < 2 {
                    continue;
                }
                let line = gsk::PathBuilder::new();
                let area = gsk::PathBuilder::new();
                let left = point(0, values.len(), 0.0).0;
                area.move_to(left, height);
                for (i, &value) in values.iter().enumerate() {
                    let (x, y) = point(i, values.len(), value);
                    if i == 0 {
                        line.move_to(x, y);
                    } else {
                        line.line_to(x, y);
                    }
                    area.line_to(x, y);
                }
                area.line_to(width, height);
                area.close();
                let color = if index == 0 {
                    snapshot.append_fill(
                        &area.to_path(),
                        gsk::FillRule::Winding,
                        &accent.with_alpha(0.25),
                    );
                    accent
                } else {
                    text.with_alpha(0.6)
                };
                let stroke = gsk::Stroke::new(if self.plain.get() { 1.5 } else { 2.0 });
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&line.to_path(), &stroke, &color);
            }

            if let Some(layout) = label {
                let (label_width, _) = layout.pixel_size();
                snapshot.save();
                snapshot.translate(&graphene::Point::new(width - label_width as f32, 0.0));
                snapshot.append_layout(&layout, &text.with_alpha(0.6));
                snapshot.restore();
            }
            snapshot.pop();
        }
    }

    impl Graph {
        fn set_maximum(&self, maximum: f64) {
            self.maximum.set(maximum);
            self.obj().queue_draw();
        }
    }
}

glib::wrapper! {
    pub struct Graph(ObjectSubclass<imp::Graph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Graph {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Graph {
    /// Adds a reading to each series, dropping the oldest once the graph is full.
    pub fn push(&self, values: &[f64]) {
        let mut series = self.imp().series.borrow_mut();
        for (history, &value) in series.iter_mut().zip(values) {
            if history.len() == HISTORY {
                history.pop_front();
            }
            history.push_back(value);
        }
        drop(series);
        self.queue_draw();
    }

    /// How the top of a graph scaled to its data is labelled.
    pub fn set_scale_label(&self, label: impl Fn(f64) -> String + 'static) {
        self.imp().scale_label.replace(Some(Box::new(label)));
        self.queue_draw();
    }
}

/// The smallest round number that holds `largest`: 1, 2 or 5 times a power of ten, or,
/// counting in 1024s, that many KiB, MiB or GiB, so that the label reads as round too.
fn scale_for(largest: f64, binary: bool) -> f64 {
    let (base, steps): (f64, &[f64]) = if binary {
        (
            1024.0,
            &[1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1024.0],
        )
    } else {
        (10.0, &[1.0, 2.0, 5.0, 10.0])
    };
    // A graph of nothing larger is topped at a kilobyte, or a KiB.
    let largest = largest.max(if binary { 1024.0 } else { 1000.0 });
    let unit = base.powf(largest.log(base).floor());
    steps
        .iter()
        .map(|step| step * unit)
        .find(|&top| top >= largest)
        .unwrap_or(largest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rounds_up_to_a_round_number() {
        assert_eq!(scale_for(0.0, false), 1000.0);
        assert_eq!(scale_for(1000.0, false), 1000.0);
        assert_eq!(scale_for(1001.0, false), 2000.0);
        assert_eq!(scale_for(3_400_000.0, false), 5_000_000.0);
        assert_eq!(scale_for(7_000.0, false), 10_000.0);

        assert_eq!(scale_for(0.0, true), 1024.0);
        assert_eq!(scale_for(1024.0, true), 1024.0);
        assert_eq!(scale_for(3_000.0, true), 5.0 * 1024.0);
        assert_eq!(scale_for(600.0 * 1024.0, true), 1024.0 * 1024.0);
        assert_eq!(
            scale_for(3.4 * 1024.0 * 1024.0, true),
            5.0 * 1024.0 * 1024.0
        );
    }
}
