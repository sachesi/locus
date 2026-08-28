use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use std::collections::VecDeque;

/// Readings kept per series, the width of the graph from edge to edge.
pub const HISTORY: usize = 60;

/// Top of a graph scaled to its data when it holds nothing larger.
const SMALLEST_SCALE: f64 = 1000.0;

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
                scale_for(series.iter().flatten().copied().fold(0.0, f64::max))
            };

            snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, width, height));
            for quarter in 1..4 {
                let y = (height * quarter as f32 / 4.0).round();
                snapshot.append_color(
                    &text.with_alpha(0.08),
                    &graphene::Rect::new(0.0, y, width, 1.0),
                );
            }

            let step = width / (HISTORY - 1) as f32;
            let point = |i: usize, count: usize, value: f64| {
                let x = width - step * (count - 1 - i) as f32;
                // A line at zero stays a line rather than falling off the bottom edge.
                let y = 1.0 + (height - 2.0) * (1.0 - (value / top).clamp(0.0, 1.0) as f32);
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
                let stroke = gsk::Stroke::new(2.0);
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&line.to_path(), &stroke, &color);
            }

            if maximum <= 0.0
                && let Some(label) = &*self.scale_label.borrow()
            {
                let layout = widget.create_pango_layout(Some(&label(top)));
                let (label_width, _) = layout.pixel_size();
                snapshot.save();
                snapshot.translate(&graphene::Point::new(width - label_width as f32 - 6.0, 4.0));
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

/// The smallest of 1, 2 and 5 times a power of ten that holds `largest`.
fn scale_for(largest: f64) -> f64 {
    let largest = largest.max(SMALLEST_SCALE);
    let magnitude = 10f64.powf(largest.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * magnitude)
        .find(|&top| top >= largest)
        .unwrap_or(largest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rounds_up_to_a_round_number() {
        assert_eq!(scale_for(0.0), 1000.0);
        assert_eq!(scale_for(1000.0), 1000.0);
        assert_eq!(scale_for(1001.0), 2000.0);
        assert_eq!(scale_for(3_400_000.0), 5_000_000.0);
        assert_eq!(scale_for(7_000.0), 10_000.0);
    }
}
