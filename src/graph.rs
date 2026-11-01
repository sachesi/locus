use gettextrs::gettext;
use gtk::gio;
use gtk::gio::prelude::*;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use std::cell::Cell as StdCell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// What a graph can show: how far back it reaches, and over how long it averages readings
/// into one point. Sixty points each, or as many readings as there are in a minute.
const RANGES: [(Duration, Duration); 3] = [
    (Duration::from_secs(60), Duration::ZERO),
    (Duration::from_secs(120), Duration::from_secs(2)),
    (Duration::from_secs(300), Duration::from_secs(5)),
];

thread_local! {
    /// The range graphs show, chosen for all of them at once.
    static RANGE: StdCell<usize> = const { StdCell::new(0) };
}

fn range_from_name(name: &str) -> usize {
    match name {
        "2min" => 1,
        "5min" => 2,
        _ => 0,
    }
}

/// The readings of one series, at each range.
#[derive(Debug, Default)]
pub struct History {
    tiers: [Tier; 3],
}

#[derive(Debug, Default)]
struct Tier {
    points: VecDeque<(Instant, f64)>,
    /// The readings of the point being added up, and when it started.
    sum: f64,
    count: u32,
    started: Option<Instant>,
}

impl History {
    fn push(&mut self, at: Instant, value: f64) {
        for (tier, (reach, bucket)) in self.tiers.iter_mut().zip(RANGES) {
            if bucket.is_zero() {
                tier.points.push_back((at, value));
            } else {
                let started = *tier.started.get_or_insert(at);
                tier.sum += value;
                tier.count += 1;
                if at.duration_since(started) >= bucket {
                    tier.points
                        .push_back((at, tier.sum / f64::from(tier.count)));
                    tier.sum = 0.0;
                    tier.count = 0;
                    tier.started = Some(at);
                }
            }
            // One point past the edge, for the line to reach it.
            while tier
                .points
                .get(1)
                .is_some_and(|(time, _)| at.duration_since(*time) > reach)
            {
                tier.points.pop_front();
            }
        }
    }

    fn points(&self, range: usize) -> &VecDeque<(Instant, f64)> {
        &self.tiers[range].points
    }
}

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
        /// Up to two series of readings. The first is drawn in the accent colour and filled
        /// beneath, the second as a line in the text colour.
        pub series: RefCell<[super::History; 2]>,
        /// When the last reading was added, which the right edge stands for.
        pub last: Cell<Option<Instant>>,
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
            super::watch_range(&self.obj());
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
            let plain = self.plain.get();
            // A small graph has no room for the choice of range, and stays on the shortest.
            let range = if plain { 0 } else { RANGE.get() };
            let (reach, _) = RANGES[range];
            let maximum = self.maximum.get();
            let top = if maximum > 0.0 {
                maximum
            } else {
                scale_for(
                    series
                        .iter()
                        .flat_map(|history| history.points(range))
                        .map(|point| point.1)
                        .fold(0.0, f64::max),
                    crate::format::is_binary(),
                )
            };

            // Other than a small graph, a graph names how far back it reaches and its top in
            // a strip of its own above the readings, where no peak draws over them.
            let (reach_label, label) = if plain {
                (None, None)
            } else {
                let scale = self.scale_label.borrow();
                let top_text = match scale.as_ref().filter(|_| maximum <= 0.0) {
                    Some(label) => label(top),
                    None => crate::format::percent(top, 0),
                };
                (
                    Some(widget.create_pango_layout(Some(&range_caption(range)))),
                    Some(widget.create_pango_layout(Some(&top_text))),
                )
            };
            let plot_top = label
                .as_ref()
                .map_or(0.0, |l| l.pixel_size().1 as f32 + 4.0);
            let plot_height = height - plot_top;

            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            if plain {
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 4.0));
                snapshot.append_color(&text.with_alpha(0.05), &bounds);
            } else {
                snapshot.push_clip(&bounds);
            }
            for quarter in (0..4).filter(|_| !plain) {
                let y = (plot_top + plot_height * quarter as f32 / 4.0).round();
                snapshot.append_color(
                    &text.with_alpha(0.08),
                    &graphene::Rect::new(0.0, y, width, 1.0),
                );
            }

            let Some(last) = self.last.get() else {
                snapshot.pop();
                return;
            };
            let point = |at: Instant, value: f64| {
                let age = last.duration_since(at).as_secs_f32() / reach.as_secs_f32();
                let x = width * (1.0 - age);
                // A line at zero stays a line rather than falling off the bottom edge.
                let y = plot_top
                    + 1.0
                    + (plot_height - 2.0) * (1.0 - (value / top).clamp(0.0, 1.0) as f32);
                (x, y)
            };
            for (index, history) in series.iter().enumerate().rev() {
                let values = history.points(range);
                if values.len() < 2 {
                    continue;
                }
                let line = gsk::PathBuilder::new();
                let area = gsk::PathBuilder::new();
                let left = point(values[0].0, 0.0).0;
                let right = point(values[values.len() - 1].0, 0.0).0;
                area.move_to(left, height);
                for (i, &(at, value)) in values.iter().enumerate() {
                    let (x, y) = point(at, value);
                    if i == 0 {
                        line.move_to(x, y);
                    } else {
                        line.line_to(x, y);
                    }
                    area.line_to(x, y);
                }
                area.line_to(right, height);
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
                let stroke = gsk::Stroke::new(if plain { 1.5 } else { 2.0 });
                stroke.set_line_join(gsk::LineJoin::Round);
                snapshot.append_stroke(&line.to_path(), &stroke, &color);
            }

            if let Some(layout) = reach_label {
                snapshot.append_layout(&layout, &text.with_alpha(0.6));
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
    /// Adds a reading to each series, dropping what has passed out of reach.
    pub fn push(&self, values: &[f64]) {
        let now = Instant::now();
        let imp = self.imp();
        let mut series = imp.series.borrow_mut();
        for (history, &value) in series.iter_mut().zip(values) {
            history.push(now, value);
        }
        drop(series);
        imp.last.set(Some(now));
        self.queue_draw();
    }

    /// Forgets the readings, for a graph that is to show something else.
    pub fn clear(&self) {
        self.imp().series.replace(Default::default());
        self.queue_draw();
    }

    /// How the top of a graph scaled to its data is labelled.
    pub fn set_scale_label(&self, label: impl Fn(f64) -> String + 'static) {
        self.imp().scale_label.replace(Some(Box::new(label)));
        self.queue_draw();
    }
}

fn range_caption(range: usize) -> String {
    match range {
        1 => gettext("Last 2 Minutes"),
        2 => gettext("Last 5 Minutes"),
        _ => gettext("Last 60 Seconds"),
    }
}

/// Has `graph` follow the range chosen, through the one object watching the setting.
fn watch_range(graph: &Graph) {
    thread_local! {
        static SETTINGS: gio::Settings = {
            let settings = gio::Settings::new(crate::config::APP_ID);
            RANGE.set(range_from_name(&settings.string("graph-range")));
            settings
        };
    }
    SETTINGS.with(|settings| {
        settings.connect_changed(
            Some("graph-range"),
            glib::clone!(
                #[weak]
                graph,
                move |settings, key| {
                    RANGE.set(range_from_name(&settings.string(key)));
                    graph.queue_draw();
                }
            ),
        );
    });
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
    fn readings_are_kept_as_far_back_as_each_range_reaches() {
        let start = Instant::now();
        let mut history = History::default();
        for second in 0..=120 {
            history.push(start + Duration::from_secs(second), second as f64);
        }
        let minute = history.points(0);
        assert_eq!(minute.back().map(|point| point.1), Some(120.0));
        assert_eq!(
            minute.len(),
            62,
            "a minute of points, and one past the edge"
        );
        // Each point of the longer ranges averages what came in over its time.
        let two_minutes = history.points(1);
        assert_eq!(two_minutes.front().map(|point| point.1), Some(1.0));
        assert_eq!(two_minutes.len(), 60);
        let five_minutes = history.points(2);
        assert_eq!(five_minutes.front().map(|point| point.1), Some(2.5));
        assert_eq!(five_minutes.len(), 24);
    }

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
