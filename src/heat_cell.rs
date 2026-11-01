use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};
use std::cell::{Cell, OnceCell};

/// How strongly each level tints the cell with the accent colour.
const TINTS: [f32; 6] = [0.0, 0.11, 0.22, 0.34, 0.47, 0.62];

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::HeatCell)]
    pub struct HeatCell {
        /// How much the value weighs on the machine, from 0, no tint, to 5.
        #[property(get, set = Self::set_level, maximum = 5)]
        level: Cell<u32>,
        /// Rounds the corners, for a cell standing on its own rather than in a table.
        #[property(get, set)]
        rounded: Cell<bool>,
        pub text: OnceCell<gtk::Inscription>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HeatCell {
        const NAME: &'static str = "LocusHeatCell";
        type Type = super::HeatCell;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("heatcell");
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for HeatCell {
        fn constructed(&self) {
            self.parent_constructed();
            let text = gtk::Inscription::builder()
                .xalign(1.0)
                .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
                .css_classes(["numeric"])
                .build();
            text.set_parent(&*self.obj());
            let _ = self.text.set(text);
        }

        fn dispose(&self) {
            if let Some(text) = self.text.get() {
                text.unparent();
            }
        }
    }

    impl WidgetImpl for HeatCell {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let level = self.level.get() as usize;
            if level > 0 {
                let accent = adw::StyleManager::default().accent_color_rgba();
                let bounds =
                    graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32);
                let color = accent.with_alpha(TINTS[level.min(5)]);
                if self.rounded.get() {
                    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 6.0));
                    snapshot.append_color(&color, &bounds);
                    snapshot.pop();
                } else {
                    snapshot.append_color(&color, &bounds);
                }
            }
            self.parent_snapshot(snapshot);
        }
    }

    impl HeatCell {
        fn set_level(&self, level: u32) {
            // A CSS class would restyle the cell each time; only the tint is drawn again,
            // and only when the level has changed.
            if self.level.replace(level) != level {
                self.obj().queue_draw();
            }
        }
    }
}

glib::wrapper! {
    /// A value in a cell tinted by how much it weighs on the machine.
    pub struct HeatCell(ObjectSubclass<imp::HeatCell>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for HeatCell {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl HeatCell {
    pub fn text(&self) -> &gtk::Inscription {
        self.imp().text.get().expect("made when constructed")
    }
}

fn level(value: f64, steps: [f64; 5]) -> u32 {
    steps.iter().map(|step| u32::from(value >= *step)).sum()
}

/// `share` of the machine's processor time, from 0 to 100.
pub fn cpu_level(share: f64) -> u32 {
    level(share, [0.5, 3.0, 8.0, 15.0, 30.0])
}

pub fn memory_level(bytes: u64) -> u32 {
    let mib = (bytes / (1024 * 1024)) as f64;
    level(mib, [200.0, 500.0, 1000.0, 2000.0, 4000.0])
}

/// Bytes per second; negative where it cannot be read.
pub fn disk_level(rate: i64) -> u32 {
    let kib = rate as f64 / 1024.0;
    level(kib, [1.0, 200.0, 1000.0, 3000.0, 10000.0])
}

/// `share` of the busiest engine of a card, from 0 to 100; negative where it cannot be read.
pub fn gpu_level(share: f64) -> u32 {
    level(share, [0.5, 5.0, 15.0, 30.0, 60.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_rise_with_the_value() {
        assert_eq!(cpu_level(0.0), 0);
        assert_eq!(cpu_level(0.5), 1);
        assert_eq!(cpu_level(22.1), 4);
        assert_eq!(cpu_level(100.0), 5);
        assert_eq!(memory_level(3482 * 1024 * 1024), 4);
        assert_eq!(memory_level(10 * 1024 * 1024), 0);
    }

    #[test]
    fn what_cannot_be_read_has_no_tint() {
        assert_eq!(disk_level(-1), 0);
        assert_eq!(gpu_level(-1.0), 0);
    }
}
