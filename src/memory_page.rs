use crate::format;
use crate::graph::Graph;
use crate::range_switch::RangeSwitch;
use crate::sampler::System;
use crate::segment_bar::{SegmentBar, Tone};
use crate::stat::Stat;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/locus/ui/memory_page.ui")]
    pub struct MemoryPage {
        #[template_child]
        pub title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub graph: TemplateChild<Graph>,
        #[template_child]
        pub memory_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub swap_legend: TemplateChild<gtk::Box>,
        #[template_child]
        pub swap_detail: TemplateChild<gtk::Label>,
        #[template_child]
        pub composition: TemplateChild<SegmentBar>,
        #[template_child]
        pub in_use: TemplateChild<Stat>,
        #[template_child]
        pub cache: TemplateChild<Stat>,
        #[template_child]
        pub buffers: TemplateChild<Stat>,
        #[template_child]
        pub free: TemplateChild<Stat>,
        #[template_child]
        pub available: TemplateChild<Stat>,
        #[template_child]
        pub committed: TemplateChild<Stat>,
        #[template_child]
        pub swap: TemplateChild<Stat>,
        #[template_child]
        pub compression: TemplateChild<Stat>,
        #[template_child]
        pub pressure: TemplateChild<Stat>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MemoryPage {
        const NAME: &'static str = "LocusMemoryPage";
        type Type = super::MemoryPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Graph::ensure_type();
            RangeSwitch::ensure_type();
            SegmentBar::ensure_type();
            Stat::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MemoryPage {}
    impl WidgetImpl for MemoryPage {}
    impl BinImpl for MemoryPage {}
}

glib::wrapper! {
    pub struct MemoryPage(ObjectSubclass<imp::MemoryPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MemoryPage {
    pub fn apply(&self, system: &System, shown: bool, unmeasured: bool) {
        let imp = self.imp();
        let memory = &system.memory;
        let share = format::share(memory.used, memory.total);
        if !unmeasured {
            if memory.swap_total > 0 {
                imp.graph
                    .push(&[share, format::share(memory.swap_used, memory.swap_total)]);
            } else {
                imp.graph.push(&[share]);
            }
        }
        if !shown {
            return;
        }
        imp.title.set_subtitle(&format::size(memory.total));
        imp.memory_detail
            .set_label(&format::used_of(memory.used, memory.total));
        imp.swap_legend.set_visible(memory.swap_total > 0);
        imp.swap.set_visible(memory.swap_total > 0);
        if memory.swap_total > 0 {
            let swap = format::used_of(memory.swap_used, memory.swap_total);
            imp.swap_detail.set_label(&swap);
            imp.swap.set_value(swap);
        }
        imp.composition.set_segments(vec![
            (memory.used as f64, Tone::Accent),
            (memory.cache as f64, Tone::SoftAccent),
            (memory.buffers as f64, Tone::Neutral),
            (memory.free as f64, Tone::Empty),
        ]);
        imp.in_use.set_value(format::size(memory.used));
        imp.cache.set_value(format::size(memory.cache));
        imp.buffers.set_value(format::size(memory.buffers));
        imp.free.set_value(format::size(memory.free));
        imp.available.set_value(format::size(memory.available));
        imp.committed.set_value(format::size(memory.committed));
        let Some(detail) = &system.memory_detail else {
            return;
        };
        imp.compression
            .show_value(detail.compressed_swap.and_then(|(stored, compressed)| {
                (compressed > 0).then(|| {
                    // Translators: how far swap is compressed, such as "3.1 to 1".
                    gettext("{ratio} to 1").replace(
                        "{ratio}",
                        &format::number(stored as f64 / compressed as f64, 1),
                    )
                })
            }));
        imp.pressure
            .show_value(detail.pressure.map(|share| format::percent(share, 1)));
    }
}
